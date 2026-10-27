//! Sound for screen recordings (see record.rs): what the computer is playing, via a WASAPI
//! loopback on the default output, and/or the microphone. Each is read on a thread of its own as
//! 48 kHz stereo floats, placed on the recording's timeline by the time WASAPI stamped each packet
//! with, mixed, and sent on as 16-bit chunks for the encoder.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use windows::core::{HRESULT, HSTRING};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::E_ACCESSDENIED;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

use crate::encode::{AUDIO_CHANNELS, AUDIO_RATE};

const RATE: i64 = AUDIO_RATE as i64;
/// Packets are read this often; the buffers WASAPI keeps are a second long, so there's plenty of room.
const POLL: Duration = Duration::from_millis(10);
/// Packet times wobble a little; only a gap or overlap bigger than this moves the sound on the
/// timeline (10 ms), so the sound stays continuous otherwise.
const TOLERANCE: i64 = RATE / 100;
/// How long after its time a packet may still turn up before the mix moves past it (200 ms).
const SLACK: i64 = RATE / 5;
/// WAVEFORMATEX's tag for 32-bit floats.
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
/// "Element not found": there's no such device, or none at all.
const NOT_FOUND: HRESULT = HRESULT(0x80070490_u32 as i32);

/// A moment on the performance counter, in 100 ns units like the times WASAPI stamps packets
/// with. The recording's start, which the sound's timeline counts from.
#[derive(Clone, Copy)]
pub struct Clock(i64);

impl Clock {
    pub fn now() -> Clock {
        Clock(counter())
    }

    /// Frames of sound since this moment.
    fn frames(self) -> i64 {
        frames(counter() - self.0)
    }
}

fn counter() -> i64 {
    let (mut count, mut frequency) = (0, 0);
    unsafe {
        let _ = QueryPerformanceCounter(&mut count);
        let _ = QueryPerformanceFrequency(&mut frequency);
    }
    (count as i128 * 10_000_000 / frequency.max(1) as i128) as i64
}

/// 100 ns units as frames of sound.
fn frames(t: i64) -> i64 {
    (t as i128 * RATE as i128 / 10_000_000) as i64
}

/// What to record: the computer's sound, and the microphone (None for none, Some(None) for the
/// default one, Some(Some(id)) for that device).
#[derive(Clone, Debug)]
pub struct Sources {
    pub computer: bool,
    pub microphone: Option<Option<String>>,
}

impl Sources {
    pub fn any(&self) -> bool {
        self.computer || self.microphone.is_some()
    }
}

/// Sound ready for the encoder: interleaved stereo, starting `at` into the recording.
pub struct Chunk {
    pub at: Duration,
    pub pcm: Vec<i16>,
}

/// Sound being recorded on its own thread. The chunks it makes follow on from each other, from
/// the recording's start to the moment it's stopped.
pub struct Capture {
    stop: Arc<AtomicBool>,
    chunks: Receiver<Chunk>,
    thread: Option<JoinHandle<Vec<String>>>,
}

impl Capture {
    /// Starts recording `sources` from `clock`, which says when the recording began. Any that
    /// can't be used are left out, with a note on that when it's stopped. If none can, the error
    /// says why and that the recording will have no sound.
    pub fn start(sources: &Sources, clock: Clock) -> Result<Capture, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let (chunks_tx, chunks) = channel();
        let (ready_tx, ready_rx) = channel();
        let thread = {
            let (sources, stop) = (sources.clone(), stop.clone());
            std::thread::spawn(move || run(&sources, clock, &stop, ready_tx, chunks_tx))
        };
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Capture { stop, chunks, thread: Some(thread) }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("Couldn't start recording sound, so this recording has none.".into()),
        }
    }

    /// The sound made since the last call.
    pub fn ready(&self) -> Vec<Chunk> {
        self.chunks.try_iter().collect()
    }

    /// Stops, and returns the sound not taken yet, and notes on anything that didn't work.
    pub fn stop(mut self) -> (Vec<Chunk>, Vec<String>) {
        let notes = self.finish();
        (self.chunks.try_iter().collect(), notes)
    }

    fn finish(&mut self) -> Vec<String> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().map(|t| t.join().unwrap_or_default()).unwrap_or_default()
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Puts the sources' sound on one timeline, measured in frames from the recording's start, and
/// mixes it. Sources that have nothing for a while (a loopback is silent while nothing plays)
/// count as silence there.
pub struct Mixer {
    /// What each source has for the frames from `emitted` on, interleaved.
    lanes: Vec<VecDeque<f32>>,
    /// Frames already mixed.
    emitted: i64,
}

impl Mixer {
    pub fn new(sources: usize) -> Mixer {
        Mixer { lanes: vec![VecDeque::new(); sources], emitted: 0 }
    }

    /// Adds `samples` (interleaved stereo) to `source`. `at` is the frame they begin at, if the
    /// packet had a usable time. Without one, or when it's within a few milliseconds of where
    /// the last packet ended, they follow on directly; otherwise silence fills the gap, or the
    /// part that overlaps what's there is left out. `discontinuity` means WASAPI lost some sound
    /// just before, so the time is taken exactly.
    pub fn push(&mut self, source: usize, at: Option<i64>, samples: &[f32], discontinuity: bool) {
        let lane = &mut self.lanes[source];
        let end = self.emitted + (lane.len() / AUDIO_CHANNELS as usize) as i64;
        let mut samples = samples;
        if let Some(at) = at {
            let tolerance = if discontinuity { 0 } else { TOLERANCE };
            if at > end + tolerance {
                lane.extend(std::iter::repeat_n(0.0, (at - end) as usize * AUDIO_CHANNELS as usize));
            } else if at + tolerance < end {
                let skip = ((end - at) as usize * AUDIO_CHANNELS as usize).min(samples.len());
                samples = &samples[skip..];
            }
        }
        lane.extend(samples);
    }

    /// Mixes what's settled when it's `now` (in frames), as 16-bit stereo and the frame it begins
    /// at; None if there's nothing new yet.
    pub fn take(&mut self, now: i64) -> Option<(i64, Vec<i16>)> {
        self.mix_to(now - SLACK)
    }

    /// Mixes everything up to `now`, the end of the recording, and drops any later sound.
    pub fn finish(&mut self, now: i64) -> Option<(i64, Vec<i16>)> {
        let mixed = self.mix_to(now);
        self.lanes.iter_mut().for_each(VecDeque::clear);
        mixed
    }

    fn mix_to(&mut self, until: i64) -> Option<(i64, Vec<i16>)> {
        if until <= self.emitted {
            return None;
        }
        let start = self.emitted;
        let count = (until - start) as usize * AUDIO_CHANNELS as usize;
        let out = (0..count)
            .map(|_| {
                let sum: f32 = self.lanes.iter_mut().map(|lane| lane.pop_front().unwrap_or(0.0)).sum();
                (sum.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
            })
            .collect();
        self.emitted = until;
        Some((start, out))
    }
}

fn chunk(mixed: Option<(i64, Vec<i16>)>) -> Option<Chunk> {
    mixed.map(|(start, pcm)| Chunk { at: Duration::from_nanos((start * 1_000_000_000 / RATE) as u64), pcm })
}

/// One of the devices being read.
struct Source {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    /// Its place in the mixer.
    lane: usize,
    /// What the notes call it.
    what: &'static str,
}

const COMPUTER: &str = "computer's sound";
const MICROPHONE: &str = "microphone";

impl Source {
    /// Opens `device` (else the default output, for the computer's sound, or the default
    /// microphone) and starts it.
    unsafe fn open(enumerator: &IMMDeviceEnumerator, what: &'static str, device: Option<&str>, lane: usize) -> windows::core::Result<Source> {
        let loopback = what == COMPUTER;
        let device = match device {
            Some(id) => enumerator.GetDevice(&HSTRING::from(id))?,
            // The default for the console role is what Windows' Sound settings call the default
            // device. The "communications" one is for calls, and is only different when someone
            // has set it to be.
            None => enumerator.GetDefaultAudioEndpoint(if loopback { eRender } else { eCapture }, eConsole)?,
        };
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
            nChannels: AUDIO_CHANNELS as u16,
            nSamplesPerSec: AUDIO_RATE,
            nAvgBytesPerSec: AUDIO_RATE * AUDIO_CHANNELS * 4,
            nBlockAlign: AUDIO_CHANNELS as u16 * 4,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        // Whatever the device uses, Windows converts to this, so both sources come in alike.
        let flags = AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY | if loopback { AUDCLNT_STREAMFLAGS_LOOPBACK } else { 0 };
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 10_000_000, 0, &format, None)?;
        let capture: IAudioCaptureClient = client.GetService()?;
        client.Start()?;
        Ok(Source { client, capture, lane, what })
    }

    /// Moves what has arrived into the mixer.
    unsafe fn read(&self, mixer: &mut Mixer, clock: Clock) -> windows::core::Result<()> {
        while self.capture.GetNextPacketSize()? > 0 {
            let (mut data, mut count, mut flags, mut position) = (std::ptr::null_mut(), 0, 0, 0u64);
            self.capture.GetBuffer(&mut data, &mut count, &mut flags, None, Some(&mut position))?;
            let len = count as usize * AUDIO_CHANNELS as usize;
            let mut samples = vec![0f32; len];
            // While nothing sounds, the buffer's contents are not to be trusted: it's silence.
            if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 == 0 && !data.is_null() {
                std::ptr::copy_nonoverlapping(data, samples.as_mut_ptr() as *mut u8, len * 4);
            }
            self.capture.ReleaseBuffer(count)?;
            // In 100 ns units on the performance counter, like the clock.
            let at = (flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 == 0).then(|| frames(position as i64 - clock.0));
            mixer.push(self.lane, at, &samples, flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0);
        }
        Ok(())
    }
}

/// Why something couldn't be used, in a few words.
fn reason(e: &windows::core::Error) -> String {
    if e.code() == NOT_FOUND {
        "it wasn't found".into()
    } else {
        e.message().trim().trim_end_matches('.').to_lowercase()
    }
}

/// What to tell the user about a source that couldn't be used.
fn failure(what: &str, e: &windows::core::Error) -> String {
    if what == MICROPHONE && e.code() == E_ACCESSDENIED {
        "Windows didn't let RSnip use the microphone (Settings → Privacy & security → Microphone), so it was recorded without it.".into()
    } else if what == MICROPHONE {
        format!("Couldn't use the microphone ({}), so it was recorded without it.", reason(e))
    } else {
        format!("Couldn't record the computer's sound ({}), so it was recorded without it.", reason(e))
    }
}

/// The capture thread: opens the sources, tells `ready` how that went, then reads and mixes
/// until stopped. Returns the notes to give the user.
fn run(sources: &Sources, clock: Clock, stop: &AtomicBool, ready: Sender<Result<(), String>>, chunks: Sender<Chunk>) -> Vec<String> {
    let mut notes = Vec::new();
    let mut open = Vec::new();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        match CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) {
            Ok(enumerator) => {
                let wanted = [(COMPUTER, sources.computer.then_some(None)), (MICROPHONE, sources.microphone.clone())];
                for (what, device) in wanted.into_iter().filter_map(|(what, device)| Some((what, device?))) {
                    match Source::open(&enumerator, what, device.as_deref(), open.len()) {
                        Ok(source) => open.push(source),
                        Err(e) => notes.push(failure(what, &e)),
                    }
                }
            }
            Err(e) => notes.push(format!("Couldn't record sound ({}), so this recording has none.", reason(&e))),
        }
    }
    if open.is_empty() {
        // A note per source that failed, then what that means.
        let silent = if notes.is_empty() { "" } else { " There's no sound in this recording." };
        let _ = ready.send(Err(format!("{}{silent}", notes.join(" "))));
        return Vec::new();
    }
    let _ = ready.send(Ok(()));

    let mut mixer = Mixer::new(open.len());
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(POLL);
        read_all(&mut open, &mut mixer, clock, &mut notes);
        if let Some(chunk) = chunk(mixer.take(clock.frames())) {
            let _ = chunks.send(chunk);
        }
    }
    read_all(&mut open, &mut mixer, clock, &mut notes);
    if let Some(chunk) = chunk(mixer.finish(clock.frames())) {
        let _ = chunks.send(chunk);
    }
    for source in &open {
        let _ = unsafe { source.client.Stop() };
    }
    notes
}

/// Reads every source, dropping any that stops working (a device being unplugged, say): the
/// mixer carries on with silence in its place.
fn read_all(open: &mut Vec<Source>, mixer: &mut Mixer, clock: Clock, notes: &mut Vec<String>) {
    open.retain(|source| match unsafe { source.read(mixer, clock) } {
        Ok(()) => true,
        Err(e) => {
            notes.push(format!("The {} stopped working part way through ({}), so the rest was recorded without it.", source.what, reason(&e)));
            false
        }
    });
}

/// The active microphones as (id, name), for the pickers in Settings and the quick settings.
pub fn microphones() -> Vec<(String, String)> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(enumerator) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return Vec::new() };
        let Ok(devices) = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) else { return Vec::new() };
        let count = devices.GetCount().unwrap_or(0);
        (0..count)
            .filter_map(|i| {
                let device = devices.Item(i).ok()?;
                let id = device.GetId().ok()?;
                let id_text = id.to_string().ok();
                CoTaskMemFree(Some(id.as_ptr() as *const _));
                Some((id_text?, friendly_name(&device)?))
            })
            .collect()
    }
}

unsafe fn friendly_name(device: &IMMDevice) -> Option<String> {
    let store = device.OpenPropertyStore(STGM_READ).ok()?;
    let mut value = store.GetValue(&PKEY_Device_FriendlyName).ok()?;
    let text = PropVariantToStringAlloc(&value).ok();
    let name = text.and_then(|t| {
        let name = t.to_string().ok();
        CoTaskMemFree(Some(t.as_ptr() as *const _));
        name
    });
    let _ = PropVariantClear(&mut value);
    name.filter(|n| !n.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn stereo(frames: usize, level: f32) -> Vec<f32> {
        vec![level; frames * 2]
    }

    #[test]
    fn a_gap_is_filled_with_silence() {
        let mut mixer = Mixer::new(1);
        mixer.push(0, Some(0), &stereo(100, 0.5), false);
        // Nothing for 1,900 frames (as a loopback does while nothing plays), then more.
        mixer.push(0, Some(2000), &stereo(100, 0.25), false);
        let (start, pcm) = mixer.finish(2100).unwrap();
        assert_eq!((start, pcm.len()), (0, 4200));
        assert!(pcm[..200].iter().all(|&s| s == 16384));
        assert!(pcm[200..4000].iter().all(|&s| s == 0));
        assert!(pcm[4000..].iter().all(|&s| s == 8192));
    }

    #[test]
    fn a_source_with_nothing_is_silent_and_the_end_is_padded() {
        let mut mixer = Mixer::new(2);
        mixer.push(1, Some(0), &stereo(10, 0.5), false);
        let (_, pcm) = mixer.finish(50).unwrap();
        assert_eq!(pcm.len(), 100);
        assert!(pcm[..20].iter().all(|&s| s == 16384) && pcm[20..].iter().all(|&s| s == 0));
        assert!(mixer.finish(50).is_none(), "nothing more once it's finished");
    }

    #[test]
    fn sources_are_summed_and_clamped() {
        let mut mixer = Mixer::new(2);
        mixer.push(0, Some(0), &[0.25, -0.25, 0.9, -0.9], false);
        mixer.push(1, Some(0), &[0.25, -0.5, 0.9, -0.9], false);
        let (_, pcm) = mixer.finish(2).unwrap();
        assert_eq!(pcm, [16384, -24575, i16::MAX, -i16::MAX]);
    }

    #[test]
    fn samples_become_16_bit() {
        let mut mixer = Mixer::new(1);
        mixer.push(0, Some(0), &[1.0, -1.0, 0.0, f32::NAN, 3.0, -3.0], false);
        let (_, pcm) = mixer.finish(3).unwrap();
        assert_eq!(pcm, [i16::MAX, -i16::MAX, 0, 0, i16::MAX, -i16::MAX]);
    }

    #[test]
    fn mixing_waits_for_late_packets() {
        let mut mixer = Mixer::new(1);
        mixer.push(0, Some(0), &stereo(1000, 0.1), false);
        assert!(mixer.take(1000).is_none(), "too soon to know nothing else comes");
        let (start, pcm) = mixer.take(SLACK + 500).unwrap();
        assert_eq!((start, pcm.len()), (0, 1000));
        let (start, pcm) = mixer.take(SLACK + 700).unwrap();
        assert_eq!((start, pcm.len()), (500, 400), "carries on from there");
    }

    #[test]
    fn small_time_errors_are_ignored_but_big_ones_and_losses_are_not() {
        let at = |mixer: &Mixer| mixer.lanes[0].len() / 2;
        let mut mixer = Mixer::new(1);
        mixer.push(0, Some(0), &stereo(1000, 0.1), false);
        // A packet 5 ms late, or early, follows on directly.
        mixer.push(0, Some(1000 + 240), &stereo(100, 0.1), false);
        mixer.push(0, Some(1100 - 240), &stereo(100, 0.1), false);
        assert_eq!(at(&mixer), 1200);
        // One 50 ms early overlaps, so what overlaps is dropped.
        mixer.push(0, Some(1200 - 2400 + 50), &stereo(2400, 0.1), false);
        assert_eq!(at(&mixer), 1200 + 50);
        // Lost sound is made up for exactly, however short; with no time, it follows on.
        mixer.push(0, Some(1250 + 100), &stereo(10, 0.1), true);
        assert_eq!(at(&mixer), 1250 + 110);
        mixer.push(0, None, &stereo(10, 0.1), false);
        assert_eq!(at(&mixer), 1250 + 120);
        // A packet from before the recording began loses its start.
        let mut mixer = Mixer::new(1);
        mixer.push(0, Some(-1000), &stereo(1500, 0.1), false);
        assert_eq!(at(&mixer), 500);
    }

    #[test]
    fn chunks_carry_their_start_time() {
        let chunk = chunk(Some((48_000, vec![0; 4]))).unwrap();
        assert_eq!(chunk.at, Duration::from_secs(1));
    }

    #[test]
    fn records_the_computers_sound_without_gaps() {
        let sources = Sources { computer: true, microphone: None };
        let begun = Instant::now();
        let capture = match Capture::start(&sources, Clock::now()) {
            Ok(capture) => capture,
            // CI's machines may have no sound device.
            Err(e) => return eprintln!("skipped: {e}"),
        };
        std::thread::sleep(Duration::from_millis(300));
        let (chunks, notes) = capture.stop();
        let elapsed = begun.elapsed();
        assert!(notes.is_empty(), "{notes:?}");
        // Whether or not anything was playing, the sound runs from the start to the stop.
        let mut next = 0;
        for chunk in &chunks {
            assert_eq!(chunk.at, Duration::from_nanos((next * 1_000_000_000 / RATE) as u64), "chunks follow on");
            next += chunk.pcm.len() as i64 / 2;
        }
        let length = Duration::from_nanos((next * 1_000_000_000 / RATE) as u64);
        assert!(length >= Duration::from_millis(300) && length <= elapsed, "{length:?} of {elapsed:?}");
    }

    #[test]
    fn lists_microphones_without_trouble() {
        for (id, name) in microphones() {
            assert!(!id.is_empty() && !name.is_empty());
        }
    }
}
