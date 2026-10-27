//! Video encoders for screen recordings: MP4 (H.264, or H.265 where the PC has an encoder for it,
//! via Media Foundation, built into Windows) and animated GIF. Frames are BGRA with the time since the recording started.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::time::Duration;

use windows::core::{Interface, HSTRING};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

use crate::capture::Image;

pub trait Encoder {
    fn frame(&mut self, img: &Image, at: Duration) -> Result<(), String>;
    /// Interleaved 48 kHz stereo samples starting `at` into the recording. Only MP4s made with
    /// sound take them; for the rest it does nothing.
    fn audio(&mut self, _pcm: &[i16], _at: Duration) -> Result<(), String> {
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<(), String>;
}

/// The sound sent to `Encoder::audio`: 48 kHz, stereo, 16-bit.
pub const AUDIO_RATE: u32 = 48_000;
pub const AUDIO_CHANNELS: u32 = 2;

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
    /// The sound's stream, if this MP4 has any, and where its last sample ended.
    audio: Option<u32>,
    audio_end: i64,
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
    /// Without sound, as tests make them.
    #[cfg(test)]
    pub fn new(path: &Path, width: i32, height: i32, fps: u32, hevc: bool) -> Result<Mp4, String> {
        Mp4::with_sound(path, width, height, fps, hevc, false)
    }

    /// As `new`, with a stream for the sound given to `audio` if `sound`.
    pub fn with_sound(path: &Path, width: i32, height: i32, fps: u32, hevc: bool, sound: bool) -> Result<Mp4, String> {
        unsafe { Mp4::create(path, width, height, fps, hevc, sound) }.map_err(|e| format!("Couldn't start the video encoder: {}", e.message()))
    }

    unsafe fn create(path: &Path, width: i32, height: i32, fps: u32, hevc: bool, sound: bool) -> windows::core::Result<Mp4> {
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
        // A keyframe every second, so trimming and seeking land within a second of where asked
        // (see mp4.rs). Not every encoder takes it; those keep their own spacing.
        let mut codec: *mut std::ffi::c_void = std::ptr::null_mut();
        if writer.GetServiceForStream(stream, &windows::core::GUID::zeroed(), &ICodecAPI::IID, &mut codec).is_ok() && !codec.is_null() {
            let codec = ICodecAPI::from_raw(codec);
            let _ = codec.SetValue(&CODECAPI_AVEncMPVGOPSize, &windows::Win32::System::Variant::VARIANT::from(fps));
        }
        let audio = if sound { Some(Mp4::add_audio(&writer)?) } else { None };
        writer.BeginWriting()?;
        Ok(Mp4 { writer, stream, width, height, frame_time: 10_000_000 / fps as i64, last: None, audio, audio_end: 0 })
    }

    /// Adds the sound's stream: 16-bit PCM in, AAC at 160 kbit/s out.
    unsafe fn add_audio(writer: &IMFSinkWriter) -> windows::core::Result<u32> {
        let common = |t: &IMFMediaType, subtype: &windows::core::GUID, bytes_per_second: u32, block: u32| -> windows::core::Result<()> {
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            t.SetGUID(&MF_MT_SUBTYPE, subtype)?;
            t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, AUDIO_RATE)?;
            t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, AUDIO_CHANNELS)?;
            t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, bytes_per_second)?;
            t.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, block)
        };
        let output = MFCreateMediaType()?;
        common(&output, &MFAudioFormat_AAC, 20_000, 1)?;
        // Raw AAC frames, as MP4 holds them, at the profile and level (AAC-LC) players all take.
        output.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?;
        output.SetUINT32(&MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, 0x29)?;
        let stream = writer.AddStream(&output)?;
        let input = MFCreateMediaType()?;
        common(&input, &MFAudioFormat_PCM, AUDIO_RATE * AUDIO_CHANNELS * 2, AUDIO_CHANNELS * 2)?;
        writer.SetInputMediaType(stream, &input, None)?;
        Ok(stream)
    }

    unsafe fn write_audio(&mut self, stream: u32, pcm: &[i16], at: Duration) -> windows::core::Result<()> {
        let len = std::mem::size_of_val(pcm);
        let buffer = MFCreateMemoryBuffer(len as u32)?;
        let mut dst = std::ptr::null_mut();
        buffer.Lock(&mut dst, None, None)?;
        std::ptr::copy_nonoverlapping(pcm.as_ptr() as *const u8, dst, len);
        buffer.Unlock()?;
        buffer.SetCurrentLength(len as u32)?;
        let sample = MFCreateSample()?;
        sample.AddBuffer(&buffer)?;
        // Chunks are meant to follow one another; keep the times from overlapping if they don't.
        let time = ticks(at).max(self.audio_end);
        let duration = (pcm.len() / AUDIO_CHANNELS as usize) as i64 * 10_000_000 / AUDIO_RATE as i64;
        sample.SetSampleTime(time)?;
        sample.SetSampleDuration(duration)?;
        self.audio_end = time + duration;
        self.writer.WriteSample(stream, &sample)
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

    fn audio(&mut self, pcm: &[i16], at: Duration) -> Result<(), String> {
        match self.audio {
            Some(stream) if !pcm.is_empty() => unsafe { self.write_audio(stream, pcm, at) }.map_err(|e| format!("Couldn't encode the sound: {}", e.message())),
            _ => Ok(()),
        }
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

/// The tracks in an MP4 ("vide", "soun"), in order, from the handler boxes in its header.
#[cfg(test)]
pub fn handlers(mp4: &[u8]) -> Vec<String> {
    // A handler box is "hdlr", 4 bytes of version and flags, 4 of pre-defined, then the kind.
    let kinds = mp4.windows(4).enumerate().filter(|(_, w)| w == b"hdlr").filter_map(|(i, _)| mp4.get(i + 12..i + 16));
    // (The metadata box has one too, "mdir".)
    kinds.filter(|k| *k == b"vide" || *k == b"soun").map(|k| String::from_utf8_lossy(k).into_owned()).collect()
}

/// Reads the first video (or sound) track of an MP4 through Media Foundation, as a player would:
/// how long it runs, and for sound the loudest sample once decoded.
#[cfg(test)]
pub fn read_stream(path: &Path, sound: bool) -> (Duration, i16) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_FULL).unwrap();
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None).unwrap();
        let stream = if sound { MF_SOURCE_READER_FIRST_AUDIO_STREAM } else { MF_SOURCE_READER_FIRST_VIDEO_STREAM }.0 as u32;
        if sound {
            let wanted = MFCreateMediaType().unwrap();
            wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).unwrap();
            wanted.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM).unwrap();
            wanted.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).unwrap();
            reader.SetCurrentMediaType(stream, None, &wanted).unwrap();
        }
        let (mut end, mut peak) = (0i64, 0i16);
        loop {
            let (mut flags, mut time, mut sample) = (0, 0, None);
            reader.ReadSample(stream, 0, None, Some(&mut flags), Some(&mut time), Some(&mut sample)).unwrap();
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
            let Some(sample) = sample else { continue };
            end = end.max(time + sample.GetSampleDuration().unwrap());
            if sound {
                let buffer = sample.ConvertToContiguousBuffer().unwrap();
                let (mut data, mut len) = (std::ptr::null_mut(), 0);
                buffer.Lock(&mut data, None, Some(&mut len)).unwrap();
                let pcm = std::slice::from_raw_parts(data as *const i16, len as usize / 2);
                peak = pcm.iter().fold(peak, |peak, s| peak.max(s.saturating_abs()));
                buffer.Unlock().unwrap();
            }
        }
        drop(reader);
        let _ = MFShutdown();
        (Duration::from_nanos(end as u64 * 100), peak)
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

    /// Two seconds of video with a 440 Hz tone, sent in 10 ms pieces as a recording would.
    #[test]
    fn mp4_with_sound_has_a_video_and_a_sound_track_of_the_same_length() {
        let path = std::env::temp_dir().join(format!("rsnip-test-sound-{}.mp4", std::process::id()));
        let mut mp4: Box<dyn Encoder> = match Mp4::with_sound(&path, 64, 48, 30, false, true) {
            Ok(mp4) => Box::new(mp4),
            Err(e) => return eprintln!("skipped: {e}"),
        };
        let piece = AUDIO_RATE as usize / 100;
        let tone = |n: usize| (0.5 * (n as f32 * 440.0 * std::f32::consts::TAU / AUDIO_RATE as f32).sin() * i16::MAX as f32) as i16;
        let mut written = 0;
        for (i, img) in frames(60, 64, 48).iter().enumerate() {
            let at = Duration::from_millis(i as u64 * 1000 / 30);
            mp4.frame(img, at).unwrap();
            // The sound up to this frame's time.
            while written < at.as_millis() as usize * AUDIO_RATE as usize / 1000 {
                let pcm: Vec<i16> = (written..written + piece).flat_map(|n| [tone(n), tone(n)]).collect();
                mp4.audio(&pcm, Duration::from_nanos(written as u64 * 1_000_000_000 / AUDIO_RATE as u64)).unwrap();
                written += piece;
            }
        }
        mp4.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(handlers(&bytes), ["vide", "soun"]);
        let (video, _) = read_stream(&path, false);
        let (sound, peak) = read_stream(&path, true);
        assert!(video.abs_diff(Duration::from_secs(2)) < Duration::from_millis(100), "{video:?}");
        assert!(sound.abs_diff(video) < Duration::from_millis(150), "video {video:?}, sound {sound:?}");
        assert!(peak > 10_000, "the tone survives encoding and decoding: {peak}");

        // Trimmed (as the library does), it keeps the sound of the part kept.
        let cut = path.with_extension("cut.mp4");
        let (from, to) = crate::mp4::Movie::open(&path).unwrap().cut(&path, &cut, Duration::from_millis(1100), Duration::from_millis(1800)).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(handlers(&std::fs::read(&cut).unwrap()), ["vide", "soun"]);
        let (video, _) = read_stream(&cut, false);
        let (sound, peak) = read_stream(&cut, true);
        std::fs::remove_file(&cut).unwrap();
        assert!(video.abs_diff(to - from) < Duration::from_millis(100), "{from:?}..{to:?}: {video:?}");
        assert!(sound.abs_diff(video) < Duration::from_millis(150), "trimmed: video {video:?}, sound {sound:?}");
        assert!(peak > 10_000, "the trimmed copy's tone: {peak}");
    }

    #[test]
    fn mp4_without_sound_has_only_video_and_ignores_audio() {
        let path = std::env::temp_dir().join(format!("rsnip-test-silent-{}.mp4", std::process::id()));
        let mut mp4: Box<dyn Encoder> = match Mp4::new(&path, 64, 48, 30, false) {
            Ok(mp4) => Box::new(mp4),
            Err(e) => return eprintln!("skipped: {e}"),
        };
        mp4.frame(&frames(1, 64, 48)[0], Duration::ZERO).unwrap();
        mp4.audio(&[1, 2, 3, 4], Duration::ZERO).unwrap();
        mp4.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(handlers(&bytes), ["vide"]);
    }

    #[test]
    fn hevc_writes_a_playable_file_where_theres_an_encoder() {
        if !hevc_available() {
            return eprintln!("skipped: no H.265 encoder");
        }
        writes_a_playable_mp4(true, 30);
    }
}
