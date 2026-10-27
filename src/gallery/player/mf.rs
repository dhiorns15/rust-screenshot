//! Decoding recordings with Media Foundation's source reader, which seeks, and converts frames to
//! 32-bit RGB itself. Their sound is decoded the same way and played on the default output (see
//! `audio::Speaker`).

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::ColorImage;
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
    MFCreateAttributes(&mut attrs, 1)?;
    let attrs = attrs.unwrap();
    // Converts whatever the file holds to the RGB asked for below.
    attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
    let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), &attrs)?;
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let wanted = MFCreateMediaType()?;
    wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    wanted.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
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
    let (out_w, out_h) = fit((width as u32, height as u32), max);
    let mut rgba = vec![0u8; width * height * 4];
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
        let bytes = std::slice::from_raw_parts(data, len as usize);
        let row = width * 4;
        for y in 0..height {
            let src_row = if stride < 0 { height - 1 - y } else { y };
            let Some(src) = bytes.get(src_row * stride.unsigned_abs() as usize..).and_then(|b| b.get(..row)) else { break };
            for (d, s) in rgba[y * row..(y + 1) * row].chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                // BGRX to RGBA.
                d.copy_from_slice(&[s[2], s[1], s[0], 255]);
            }
        }
        let _ = buffer.Unlock();
        let image = if (out_w as usize, out_h as usize) == (width, height) {
            ColorImage::from_rgba_unmultiplied([width, height], &rgba)
        } else {
            let full = image::RgbaImage::from_raw(width as u32, height as u32, rgba.clone()).expect("the frame's size");
            let small = image::imageops::thumbnail(&full, out_w, out_h);
            ColorImage::from_rgba_unmultiplied([out_w as usize, out_h as usize], small.as_raw())
        };
        if !sink.send(Frame { at, image }) {
            return Ok(());
        }
    }
}

/// Starts `path`'s sound playing from `from`, on a thread of its own.
pub fn sound(path: &Path, from: Duration) -> Option<Sound> {
    let (stop, ready) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let (path, thread_stop, thread_ready) = (path.to_owned(), stop.clone(), ready.clone());
    std::thread::spawn(move || {
        let _ = unsafe { play(&path, from, &thread_stop, &thread_ready) };
        // Played or not, there's nothing more to wait for.
        thread_ready.store(true, Ordering::Relaxed);
    });
    Some(Sound::new(ready, Stop(stop)))
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
/// set; sets `ready` once it's playing. Returns how much sound was played.
pub(super) unsafe fn play(path: &Path, from: Duration, stop: &AtomicBool, ready: &AtomicBool) -> windows::core::Result<Duration> {
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
    let from_units = from.as_nanos() as i64 / 100;
    if !from.is_zero() {
        // The reader goes to somewhere before; the sound before `from` is dropped below.
        reader.SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from(from_units))?;
    }

    let mut speaker = Speaker::open(rate, channels as u32)?;
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
