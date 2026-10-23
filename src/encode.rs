//! Video encoders for screen recordings: MP4 (H.264, or H.265 where the PC has an encoder for it,
//! via Media Foundation, built into Windows) and animated GIF. Frames are BGRA with the time since the recording started.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::time::Duration;

use windows::core::HSTRING;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

use crate::capture::Image;

pub trait Encoder {
    fn frame(&mut self, img: &Image, at: Duration) -> Result<(), String>;
    fn finish(self: Box<Self>) -> Result<(), String>;
}

/// H.264 needs even dimensions.
pub fn even(n: i32) -> i32 {
    n & !1
}

pub struct Mp4 {
    writer: IMFSinkWriter,
    stream: u32,
    width: i32,
    height: i32,
    frame_time: i64,
    last: Option<i64>,
}

/// 100 ns units, as Media Foundation counts time.
fn ticks(d: Duration) -> i64 {
    (d.as_nanos() / 100) as i64
}

/// Whether this PC has an H.265 encoder: Windows has none of its own, but most graphics drivers
/// add one, and so do the HEVC Video Extensions from the Microsoft Store.
pub fn hevc_available() -> bool {
    let output = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_HEVC };
    let flags = MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER;
    let mut found = std::ptr::null_mut();
    let mut count = 0;
    unsafe {
        if MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER, flags, None, Some(&output), &mut found, &mut count).is_err() || found.is_null() {
            return false;
        }
        // Release each one, then the list.
        for i in 0..count as usize {
            std::ptr::drop_in_place(found.add(i));
        }
        windows::Win32::System::Com::CoTaskMemFree(Some(found as *const _));
    }
    count > 0
}

impl Mp4 {
    pub fn new(path: &Path, width: i32, height: i32, fps: u32, hevc: bool) -> Result<Mp4, String> {
        unsafe { Mp4::create(path, width, height, fps, hevc) }.map_err(|e| format!("Couldn't start the video encoder: {}", e.message()))
    }

    unsafe fn create(path: &Path, width: i32, height: i32, fps: u32, hevc: bool) -> windows::core::Result<Mp4> {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 2)?;
        let attrs = attrs.unwrap();
        attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
        // Frames come at their own pace; don't block waiting for the encoder.
        attrs.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)?;
        let writer = MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, &attrs)?;

        let size = ((width as u64) << 32) | height as u64;
        let rate = ((fps as u64) << 32) | 1;
        let common = |t: &IMFMediaType| -> windows::core::Result<()> {
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            t.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
            t.SetUINT64(&MF_MT_FRAME_RATE, rate)?;
            t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1 << 32) | 1)
        };
        let output = MFCreateMediaType()?;
        common(&output)?;
        output.SetGUID(&MF_MT_SUBTYPE, if hevc { &MFVideoFormat_HEVC } else { &MFVideoFormat_H264 })?;
        // About 0.1 bit per pixel per frame looks clean for screen content in H.264; H.265 needs
        // about two thirds of that for the same quality.
        let bits = width as u64 * height as u64 * fps as u64 / 10;
        let bitrate = if hevc { bits * 2 / 3 } else { bits }.clamp(1_000_000, 20_000_000);
        output.SetUINT32(&MF_MT_AVG_BITRATE, bitrate as u32)?;
        let stream = writer.AddStream(&output)?;

        let input = MFCreateMediaType()?;
        common(&input)?;
        input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        // A positive stride means top-down rows, as captured.
        input.SetUINT32(&MF_MT_DEFAULT_STRIDE, (width * 4) as u32)?;
        writer.SetInputMediaType(stream, &input, None)?;
        writer.BeginWriting()?;
        Ok(Mp4 { writer, stream, width, height, frame_time: 10_000_000 / fps as i64, last: None })
    }

    unsafe fn write(&mut self, img: &Image, at: Duration) -> windows::core::Result<()> {
        let row = self.width as usize * 4;
        let len = row * self.height as usize;
        let buffer = MFCreateMemoryBuffer(len as u32)?;
        let mut dst = std::ptr::null_mut();
        buffer.Lock(&mut dst, None, None)?;
        // The capture may be a pixel wider/taller than the (even) video; copy row by row.
        let src_row = img.width as usize * 4;
        for y in 0..self.height as usize {
            std::ptr::copy_nonoverlapping(img.data[y * src_row..].as_ptr(), dst.add(y * row), row);
        }
        buffer.Unlock()?;
        buffer.SetCurrentLength(len as u32)?;
        let sample = MFCreateSample()?;
        sample.AddBuffer(&buffer)?;
        let time = ticks(at).max(self.last.map_or(0, |t| t + 1));
        sample.SetSampleTime(time)?;
        sample.SetSampleDuration(self.frame_time)?;
        self.last = Some(time);
        self.writer.WriteSample(self.stream, &sample)
    }
}

impl Encoder for Mp4 {
    fn frame(&mut self, img: &Image, at: Duration) -> Result<(), String> {
        if img.width < self.width || img.height < self.height {
            return Err("Frame is smaller than the video".into());
        }
        unsafe { self.write(img, at) }.map_err(|e| format!("Couldn't encode the video: {}", e.message()))
    }

    fn finish(self: Box<Self>) -> Result<(), String> {
        let result = unsafe { self.writer.Finalize() }.map_err(|e| format!("Couldn't finish the video: {}", e.message()));
        drop(self);
        unsafe {
            let _ = MFShutdown();
        }
        result
    }
}

pub struct Gif {
    encoder: gif::Encoder<BufWriter<File>>,
    /// The previous frame waits until the next arrives, which tells us how long to show it.
    pending: Option<(gif::Frame<'static>, Duration)>,
    frame_time: Duration,
}

/// GIF delays are in hundredths of a second; browsers treat anything under 2 as 10.
pub fn delay_cs(d: Duration) -> u16 {
    ((d.as_millis() + 5) / 10).clamp(2, u16::MAX as u128) as u16
}

impl Gif {
    pub fn new(path: &Path, width: i32, height: i32, fps: u32) -> Result<Gif, String> {
        let file = File::create(path).map_err(|e| e.to_string())?;
        let mut encoder = gif::Encoder::new(BufWriter::new(file), width as u16, height as u16, &[]).map_err(|e| e.to_string())?;
        encoder.set_repeat(gif::Repeat::Infinite).map_err(|e| e.to_string())?;
        Ok(Gif { encoder, pending: None, frame_time: Duration::from_secs(1) / fps })
    }

    fn flush(&mut self, shown_for: Duration) -> Result<(), String> {
        if let Some((mut frame, _)) = self.pending.take() {
            frame.delay = delay_cs(shown_for);
            self.encoder.write_frame(&frame).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

impl Encoder for Gif {
    fn frame(&mut self, img: &Image, at: Duration) -> Result<(), String> {
        if let Some((_, prev)) = self.pending {
            self.flush(at.saturating_sub(prev))?;
        }
        let mut rgba: Vec<u8> = img.data.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], 255]).collect();
        // Speed 10 of 30: much faster quantising than the default, still good for screen content.
        let frame = gif::Frame::from_rgba_speed(img.width as u16, img.height as u16, &mut rgba, 10);
        self.pending = Some((frame, at));
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<(), String> {
        let last = self.frame_time;
        self.flush(last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(n: usize, w: i32, h: i32) -> Vec<Image> {
        (0..n)
            .map(|i| Image { width: w, height: h, data: (0..w * h).flat_map(|p| [(p as usize * 3 + i * 40) as u8, (i as u8).wrapping_mul(20), 90, 255]).collect() })
            .collect()
    }

    #[test]
    fn gif_keeps_real_timing() {
        assert_eq!(delay_cs(Duration::from_millis(66)), 7);
        assert_eq!(delay_cs(Duration::from_millis(5)), 2);
        let path = std::env::temp_dir().join(format!("rsnip-test-{}.gif", std::process::id()));
        let mut gif: Box<dyn Encoder> = Box::new(Gif::new(&path, 32, 24, 15).unwrap());
        let times = [0, 70, 300];
        for (img, ms) in frames(3, 32, 24).iter().zip(times) {
            gif.frame(img, Duration::from_millis(ms)).unwrap();
        }
        gif.finish().unwrap();

        let mut options = gif::DecodeOptions::new();
        options.set_color_output(gif::ColorOutput::RGBA);
        let mut decoder = options.read_info(File::open(&path).unwrap()).unwrap();
        let mut delays = Vec::new();
        while let Some(frame) = decoder.read_next_frame().unwrap() {
            delays.push(frame.delay);
        }
        std::fs::remove_file(&path).unwrap();
        assert_eq!(delays, [7, 23, 7], "each frame lasts until the next; the last gets one frame time");
    }

    fn writes_a_playable_mp4(hevc: bool, fps: u32) {
        let path = std::env::temp_dir().join(format!("rsnip-test-{}-{hevc}-{fps}.mp4", std::process::id()));
        let mut mp4: Box<dyn Encoder> = match Mp4::new(&path, 64, 48, fps, hevc) {
            Ok(mp4) => Box::new(mp4),
            // Server installs (like CI's) may not include Media Foundation.
            Err(e) => return eprintln!("skipped: {e}"),
        };
        for (i, img) in frames(15, 64, 48).iter().enumerate() {
            mp4.frame(img, Duration::from_millis(i as u64 * 1000 / fps as u64)).unwrap();
        }
        mp4.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp", "an MP4 container");
        assert!(bytes.len() > 1000);
    }

    #[test]
    fn mp4_writes_a_playable_file() {
        writes_a_playable_mp4(false, 30);
        writes_a_playable_mp4(false, 60);
    }

    #[test]
    fn hevc_writes_a_playable_file_where_theres_an_encoder() {
        if !hevc_available() {
            return eprintln!("skipped: no H.265 encoder");
        }
        writes_a_playable_mp4(true, 30);
    }
}
