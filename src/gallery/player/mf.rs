//! Decoding recordings with Media Foundation's source reader, which seeks, and converts frames to
//! 32-bit RGB at the size they're shown at itself: its video processor does that many times faster
//! than converting and shrinking them here, which a 1440p recording at 60 frames a second didn't
//! keep up with. Their sound is decoded the same way and played on the default output (see
//! `audio::Speaker`).

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{Color32, ColorImage};
use windows::core::{GUID, HSTRING};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

use super::{fit, Frame, Sink, Sound};
use crate::audio::Speaker;

pub fn decode(path: &Path, from: Duration, max: (u32, u32), sink: &Sink) -> Result<(), String> {
    unsafe { read(path, from, max, sink) }.map_err(|e| format!("Couldn't play it: {}", e.message()))
}

unsafe fn read(path: &Path, from: Duration, max: (u32, u32), sink: &Sink) -> windows::core::Result<()> {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
    let mut attrs = None;
    MFCreateAttributes(&mut attrs, 2)?;
    let attrs = attrs.unwrap();
    // Converts whatever the file holds to the RGB, and the size, asked for below.
    attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
    // Each frame out as soon as it's decoded: otherwise the decoder keeps many in hand (about
    // 450 MB of them for a 1440p recording, against 170 MB). Not for one with B-frames, which have
    // to wait for frames decoded after them (RSnip's own have none).
    if crate::mp4::Movie::open(path).is_ok_and(|movie| !movie.reorders()) {
        attrs.SetUINT32(&MF_LOW_LATENCY, 1)?;
    }
    let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), &attrs)?;
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let native = reader.GetNativeMediaType(stream, 0)?.GetUINT64(&MF_MT_FRAME_SIZE)?;
    let (out_w, out_h) = fit(((native >> 32) as u32, (native & 0xFFFF_FFFF) as u32), max);
    let wanted = MFCreateMediaType()?;
    wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    wanted.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
    wanted.SetUINT64(&MF_MT_FRAME_SIZE, (out_w as u64) << 32 | out_h as u64)?;
    reader.SetCurrentMediaType(stream, None, &wanted)?;
    let current = reader.GetCurrentMediaType(stream)?;
    let size = current.GetUINT64(&MF_MT_FRAME_SIZE)?;
    let (width, height) = ((size >> 32) as usize, (size & 0xFFFF_FFFF) as usize);
    // Rows run top-down for a positive stride, bottom-up for a negative one.
    let stride = current.GetUINT32(&MF_MT_DEFAULT_STRIDE).map_or(width as i32 * 4, |s| s as i32);
    if !from.is_zero() {
        // In 100 ns units; the reader goes to the keyframe before, and frames up to `from` are skipped.
        let at = PROPVARIANT::from(from.as_nanos() as i64 / 100);
        reader.SetCurrentPosition(&GUID::zeroed(), &at)?;
    }
    loop {
        if sink.stopped() {
            return Ok(());
        }
        let (mut flags, mut time) = (0u32, 0i64);
        let mut sample = None;
        reader.ReadSample(stream, 0, None, Some(&mut flags), Some(&mut time), Some(&mut sample))?;
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            return Ok(());
        }
        let Some(sample) = sample else { continue };
        let at = Duration::from_nanos(time.max(0) as u64 * 100);
        let duration = sample.GetSampleDuration().map_or(Duration::ZERO, |d| Duration::from_nanos(d.max(0) as u64 * 100));
        if at + duration <= from {
            continue;
        }
        let buffer = sample.ConvertToContiguousBuffer()?;
        let mut data = std::ptr::null_mut();
        let mut len = 0u32;
        buffer.Lock(&mut data, None, Some(&mut len))?;
        let image = picture(std::slice::from_raw_parts(data, len as usize), width, height, stride);
        let _ = buffer.Unlock();
        // In case the reader couldn't make it the size asked for.
        let image = if width as u32 > out_w || height as u32 > out_h { shrink(&image, out_w, out_h) } else { image };
        if !sink.send(Frame { at, image }) {
            return Ok(());
        }
    }
}

/// A `width` × `height` BGRX picture (rows `stride` bytes apart, bottom-up if it's negative), as
/// egui's. Rows the buffer is too short for are black.
fn picture(bytes: &[u8], width: usize, height: usize, stride: i32) -> ColorImage {
    let mut pixels = vec![Color32::BLACK; width * height];
    let pitch = stride.unsigned_abs() as usize;
    for (y, row) in pixels.chunks_exact_mut(width).enumerate() {
        let from = if stride < 0 { height - 1 - y } else { y } * pitch;
        let Some(src) = bytes.get(from..from + width * 4) else { break };
        for (pixel, bgrx) in row.iter_mut().zip(src.chunks_exact(4)) {
            *pixel = Color32::from_rgb(bgrx[2], bgrx[1], bgrx[0]);
        }
    }
    ColorImage::new([width, height], pixels)
}

/// `image` made `width` × `height`.
fn shrink(image: &ColorImage, width: u32, height: u32) -> ColorImage {
    let full = image::RgbaImage::from_raw(image.size[0] as u32, image.size[1] as u32, image.as_raw().to_vec()).expect("the frame's size");
    let small = image::imageops::thumbnail(&full, width, height);
    ColorImage::from_rgba_premultiplied([width as usize, height as usize], small.as_raw())
}

/// Starts `path`'s sound playing from `from`, in step with a clock that's there at `due` (see
/// `StartSound`), on a thread of its own.
pub fn sound(path: &Path, from: Duration, due: Instant) -> Option<Sound> {
    let (stop, ready, failed) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)), Arc::new(Mutex::new(None)));
    let (path, thread_stop, thread_ready, failing) = (path.to_owned(), stop.clone(), ready.clone(), failed.clone());
    std::thread::spawn(move || {
        if let Err(e) = unsafe { play(&path, from, due, &thread_stop, &thread_ready) } {
            *failing.lock().unwrap() = Some(format!("Couldn't play the sound: {}", e.message()));
        }
        // Played or not, there's nothing more to wait for.
        thread_ready.store(true, Ordering::Relaxed);
    });
    Some(Sound::new(ready, Stop(stop)).failing(failed))
}

/// Stops the sound's thread when dropped (which it notices within a few milliseconds).
struct Stop(Arc<AtomicBool>);

impl Drop for Stop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// How often the speaker's buffer is topped up.
const POLL: Duration = Duration::from_millis(10);

/// Decodes the first sound track of `path` from `from` and plays it, until it ends or `stop` is
/// set; sets `ready` once it's playing. What's late for `due` is left out. Returns how much sound
/// was played.
pub(super) unsafe fn play(path: &Path, from: Duration, due: Instant, stop: &AtomicBool, ready: &AtomicBool) -> windows::core::Result<Duration> {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
    let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None)?;
    let stream = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
    // Whatever the track is (AAC, for RSnip's), as 16-bit PCM. The decoder keeps the track's rate
    // and channels, and the speaker converts if the device wants others.
    let wanted = MFCreateMediaType()?;
    wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
    wanted.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
    wanted.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
    reader.SetCurrentMediaType(stream, None, &wanted)?;
    let current = reader.GetCurrentMediaType(stream)?;
    let rate = current.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)?;
    let channels = current.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)?.max(1) as usize;
    // In 100 ns units.
    let units = |d: Duration| d.as_nanos() as i64 / 100;
    if !from.is_zero() {
        // The reader goes to somewhere before; the sound before `from` is dropped below.
        reader.SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from(units(from)))?;
    }

    let mut speaker = Speaker::open(rate, channels as u32)?;
    // As is what's late, if it's taken longer than it had to get this far.
    let from_units = units(from + Instant::now().saturating_duration_since(due));
    // Decoded, and waiting for room in the speaker.
    let mut pending: VecDeque<i16> = VecDeque::new();
    let (mut started, mut ended) = (false, false);
    while !stop.load(Ordering::Relaxed) {
        let room = speaker.room()? as usize;
        while pending.len() < room * channels && !ended {
            let (mut flags, mut time) = (0u32, 0i64);
            let mut sample = None;
            reader.ReadSample(stream, 0, None, Some(&mut flags), Some(&mut time), Some(&mut sample))?;
            ended = flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0;
            let Some(sample) = sample else { continue };
            let buffer = sample.ConvertToContiguousBuffer()?;
            let (mut data, mut len) = (std::ptr::null_mut(), 0u32);
            buffer.Lock(&mut data, None, Some(&mut len))?;
            let pcm = std::slice::from_raw_parts(data as *const i16, len as usize / 2);
            // A sample can begin before `from`: that part isn't played.
            let skip = (from_units - time).max(0) as u128 * rate as u128 / 10_000_000;
            pending.extend(pcm.iter().skip(skip as usize * channels));
            let _ = buffer.Unlock();
        }
        let frames = (pending.len() / channels).min(room);
        if frames > 0 {
            let pcm: Vec<i16> = pending.drain(..frames * channels).collect();
            speaker.write(&pcm)?;
        }
        // It starts once the buffer is as full as it'll get, so there are no gaps to begin with.
        if !started {
            speaker.start()?;
            started = true;
            ready.store(true, Ordering::Relaxed);
        }
        if ended && pending.is_empty() && speaker.queued()? == 0 {
            break;
        }
        std::thread::sleep(POLL);
    }
    Ok(Duration::from_nanos(speaker.played() * 1_000_000_000 / rate as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_come_out_as_egui_pictures_either_way_up() {
        // 2 × 2 BGRX, each pixel blue x, green y, red 200; rows padded to 12 bytes.
        let bytes: Vec<u8> = (0..2u8).flat_map(|y| (0..2u8).flat_map(move |x| [x, y, 200, 0]).chain([9; 4])).collect();
        let top_down = picture(&bytes, 2, 2, 12);
        assert_eq!(top_down.size, [2, 2]);
        assert_eq!(top_down.pixels[1], Color32::from_rgb(200, 0, 1), "red from the R, its x in blue");
        assert_eq!(top_down.pixels[2], Color32::from_rgb(200, 1, 0), "the next row past the padding");
        let bottom_up = picture(&bytes, 2, 2, -12);
        assert_eq!(bottom_up.pixels[0], Color32::from_rgb(200, 1, 0), "the last row in memory first");
        // A short buffer leaves black rather than reading past its end.
        assert_eq!(picture(&bytes[..12], 2, 2, 12).pixels[3], Color32::BLACK);
        assert_eq!(shrink(&top_down, 1, 1).size, [1, 1]);
    }
}
