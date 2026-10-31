//! Recording an area of the screen to MP4 (or GIF). While recording, a red frame marks the area
//! and a small bar shows the time with Pause and Stop buttons; both are hidden from the recording
//! itself. What happens while it's paused is left out of the video and its sound.
//! The camera's bubble, if it's on, is not: it's recorded wherever it's put in the area.

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::audio;
use crate::capture::capture_rect_with_cursor;
use crate::encode::{even, Encoder, Gif, Mp4, AUDIO_CHANNELS, AUDIO_RATE};
use crate::gallery::camera::{Bubble, Launch};
use windows_sys::core::PCWSTR;

use crate::util::{inflate, rect, wide};
use crate::WM_APP_STOP_RECORDING;

const BAR_CLASS: &str = "RSnip.RecordBar";
const FRAME_CLASS: &str = "RSnip.RecordFrame";
const RED: COLORREF = 0x003030E0;
const BAR_BG: COLORREF = 0x00202020;
const WHITE: COLORREF = 0x00FFFFFF;
/// The line between the bar's two buttons.
const DIVIDER: COLORREF = 0x00606060;
/// Recordings stop on their own after this long.
const MAX_LENGTH: Duration = Duration::from_secs(30 * 60);

pub struct Recording {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<Result<Recorded, String>>,
    windows: [HWND; 2],
    /// The camera's bubble, if it's shown. It's a window of its own process, left in the recording.
    camera: Option<Bubble>,
}

/// What recordings are saved as (see Settings → Recording).
#[derive(Clone, Debug)]
pub struct Format {
    pub gif: bool,
    /// H.265 rather than H.264, for MP4s.
    pub hevc: bool,
    /// Frames per second, for MP4s.
    pub fps: u32,
    /// Record what the computer plays: from the default output, or the one with this id (MP4s only).
    pub sound: Option<Option<String>>,
    /// Record a microphone too: the default one, or the one with this id (MP4s only).
    pub microphone: Option<Option<String>>,
    /// Show the camera on top while recording (see gallery's `camera`), so it's in the recording:
    /// the first one, or the one with this id.
    pub camera: Option<Option<String>>,
}

impl Format {
    /// GIFs are bigger and slower to encode per frame, so they keep to 12.
    fn fps(&self) -> u32 {
        if self.gif {
            12
        } else {
            self.fps.clamp(1, 60)
        }
    }
}

/// A finished recording, and anything worth saying about how it was made.
pub struct Recorded {
    pub path: PathBuf,
    pub note: Option<String>,
}

/// Starts recording `area` (screen coordinates). `main` gets `WM_APP_STOP_RECORDING` when the
/// Stop button is clicked; the Pause button pauses it there and then.
pub fn start(main: HWND, area: RECT, format: Format) -> Result<Recording, String> {
    let area = rect(area.left, area.top, area.left + even(area.right - area.left), area.top + even(area.bottom - area.top));
    let (w, h) = (area.right - area.left, area.bottom - area.top);
    if w < 16 || h < 16 {
        return Err("The area is too small to record".into());
    }
    let ext = if format.gif { "gif" } else { "mp4" };
    let path = crate::output::new_path("Recording", ext)?;
    let working = crate::gallery::working_path(&path)?;
    let fps = format.fps();
    let stop = Arc::new(AtomicBool::new(false));
    let paused = Arc::new(AtomicBool::new(false));
    // The camera's bubble says how far behind it is, and the microphone is held back that much.
    let microphone_delay = Arc::default();
    // The encoder lives on the recording thread (Media Foundation objects stay on the thread that
    // made them); it reports when it started, or why it didn't, so a failure shows straight away.
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let thread = {
        let (stop, paused, microphone_delay, main) = (stop.clone(), paused.clone(), Arc::clone(&microphone_delay), main as isize);
        std::thread::spawn(move || {
            let mut notes = Vec::new();
            let mut hevc = format.hevc;
            let open = |sound: bool, hevc: &mut bool, notes: &mut Vec<String>| -> Result<Box<dyn Encoder>, String> {
                if format.gif {
                    return Gif::new(&working, w, h, fps).map(|e| Box::new(e) as _);
                }
                Mp4::with_sound(&working, w, h, fps, *hevc, sound)
                    .or_else(|e| {
                        // Not every PC has an H.265 encoder; H.264 is always there.
                        if !*hevc {
                            return Err(e);
                        }
                        *hevc = false;
                        notes.push("This PC has no H.265 encoder, so it was recorded as H.264.".to_string());
                        Mp4::with_sound(&working, w, h, fps, false, sound)
                    })
                    .map(|e| Box::new(e) as _)
            };
            let sources = audio::Sources { computer: format.sound.clone(), microphone: format.microphone.clone(), microphone_delay };
            let with_sound = !format.gif && sources.any();
            let mut encoder = open(with_sound, &mut hevc, &mut notes);
            // Time zero is when the encoder is ready (it can take a while), so neither the video
            // nor the sound opens on a stretch the other doesn't have.
            let mut started = Instant::now();
            let mut sound = None;
            if with_sound && encoder.is_ok() {
                match audio::Capture::start(&sources, audio::Clock::now()) {
                    Ok(capture) => sound = Some(capture),
                    Err(note) => {
                        // None of the sources worked: the MP4 gets no stream for sound.
                        notes.push(note);
                        drop(encoder);
                        encoder = open(false, &mut hevc, &mut notes);
                        started = Instant::now();
                    }
                }
            }
            let _ = ready_tx.send(encoder.as_ref().map(|_| started).map_err(Clone::clone));
            let recorded = encoder.and_then(|encoder| {
                let recorded = run(encoder, area, fps, started, &stop, &paused, sound);
                if !stop.load(Ordering::Relaxed) {
                    // It ended by itself (the longest recording, or the encoder failed): the
                    // main window stops it as if Stop were clicked, taking down the frame and
                    // the bar.
                    unsafe { PostMessageW(main as HWND, WM_APP_STOP_RECORDING, 0, 0) };
                }
                recorded
            });
            match recorded {
                Ok(later) => {
                    notes.extend(later);
                    let note = Some(notes.join(" ")).filter(|n| !n.is_empty());
                    crate::gallery::move_into_place(&working, &path).map(|()| Recorded { path, note })
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&working);
                    Err(e)
                }
            }
        })
    };
    let started = ready_rx.recv().map_err(|_| "The recording didn't start".to_string())??;
    let windows = unsafe { [frame_window(area), bar_window(main, area, started, paused)] };
    // By the area's bottom right, in it.
    let camera = format.camera.map(|device| Bubble::start_for_sound(&Launch::in_area(device, [area.left, area.top, area.right, area.bottom]), microphone_delay));
    Ok(Recording { stop, thread, windows, camera })
}

impl Recording {
    /// Stops and returns the thread finishing the file.
    pub fn stop(self) -> JoinHandle<Result<Recorded, String>> {
        self.stop.store(true, Ordering::Relaxed);
        // Closed once the recording has stopped, so it's there to the end.
        let camera = self.camera.and_then(Bubble::stop);
        for hwnd in self.windows {
            unsafe { DestroyWindow(hwnd) };
        }
        let Some(camera) = camera else { return self.thread };
        // What became of the camera goes with the rest of what's said about the recording.
        let thread = self.thread;
        std::thread::spawn(move || {
            let recorded = thread.join().unwrap_or_else(|_| Err("The recording stopped unexpectedly".into()))?;
            let note = Some(recorded.note.map_or(camera.clone(), |n| format!("{n} {camera}")));
            Ok(Recorded { note, ..recorded })
        })
    }
}

/// Gives the encoder the sound made so far, less what was made while paused.
fn give_sound(encoder: &mut dyn Encoder, chunks: Vec<audio::Chunk>, pauses: &Pauses) -> Result<(), String> {
    chunks.into_iter().flat_map(|chunk| pauses.trim(chunk)).try_for_each(|chunk| encoder.audio(&chunk.pcm, chunk.at))
}

/// The stretches of a recording that were paused, in time since it started, so the video and the
/// sound can both leave them out and close up around them.
#[derive(Default)]
struct Pauses {
    /// Finished pauses, from when each began to when it ended, in order.
    cut: Vec<(Duration, Duration)>,
    /// When the pause under way began.
    since: Option<Duration>,
}

impl Pauses {
    /// Pauses at `at`, unless it's paused already.
    fn pause(&mut self, at: Duration) {
        self.since = self.since.or(Some(at));
    }

    /// Carries on at `at`, if it's paused.
    fn resume(&mut self, at: Duration) {
        if let Some(since) = self.since.take() {
            self.cut.push((since, at.max(since)));
        }
    }

    /// Where `at` ends up once the pauses are taken out; None if it was paused then.
    fn place(&self, at: Duration) -> Option<Duration> {
        let mut removed = Duration::ZERO;
        for &(from, to) in &self.cut {
            if at < from {
                break;
            }
            if at < to {
                return None;
            }
            removed += to - from;
        }
        if self.since.is_some_and(|since| at >= since) {
            return None;
        }
        Some(at - removed)
    }

    /// How long it's been paused for by `now`, the pause under way included.
    fn removed(&self, now: Duration) -> Duration {
        let open = self.since.map_or(Duration::ZERO, |since| now.saturating_sub(since));
        self.cut.iter().map(|&(from, to)| to - from).sum::<Duration>() + open
    }

    /// The parts of `chunk` from outside the pauses, moved to where they end up (see `place`).
    /// It's cut at the sample nearest each pause's start and end, so chunks that followed on from
    /// each other still do once trimmed.
    fn trim(&self, chunk: audio::Chunk) -> Vec<audio::Chunk> {
        if self.cut.is_empty() && self.since.is_none() {
            return vec![chunk];
        }
        let channels = AUDIO_CHANNELS as usize;
        let start = to_frames(chunk.at);
        let end = start + (chunk.pcm.len() / channels) as i64;
        let pauses = self.cut.iter().map(|&(from, to)| (to_frames(from), to_frames(to)));
        // The pause under way lasts until further notice.
        let pauses = pauses.chain(self.since.map(|since| (to_frames(since), i64::MAX)));
        // Each part to keep, as its first and last frame and the frames taken out before it.
        let mut keep = Vec::new();
        let (mut at, mut removed) = (start, 0i64);
        for (from, to) in pauses {
            if at >= end {
                break;
            }
            if from > at {
                keep.push((at, from.min(end), removed));
            }
            at = at.max(to);
            removed = removed.saturating_add(to - from);
        }
        if at < end {
            keep.push((at, end, removed));
        }
        let samples = |frame: i64| (frame - start) as usize * channels;
        keep.into_iter().map(|(from, to, removed)| audio::Chunk { at: from_frames(from - removed), pcm: chunk.pcm[samples(from)..samples(to)].to_vec() }).collect()
    }
}

/// A time on the recording's timeline as the frame of sound nearest it.
fn to_frames(d: Duration) -> i64 {
    ((d.as_nanos() * AUDIO_RATE as u128 + 500_000_000) / 1_000_000_000) as i64
}

/// When frame `frame` of the sound begins, worked out as `audio` does for its chunks.
fn from_frames(frame: i64) -> Duration {
    Duration::from_nanos((frame.max(0) as u128 * 1_000_000_000 / AUDIO_RATE as u128) as u64)
}

/// Captures frames until `stop` (or the longest recording), writing the sound along with them.
/// While `paused` nothing is captured and the sound made then is dropped, so the recording runs
/// on as though there'd been no gap. Returns what to tell the user about the sound, if anything
/// went wrong with it.
fn run(mut encoder: Box<dyn Encoder>, area: RECT, fps: u32, started: Instant, stop: &AtomicBool, paused: &AtomicBool, mut sound: Option<audio::Capture>) -> Result<Vec<String>, String> {
    let interval = Duration::from_secs(1) / fps;
    let mut next = started;
    let mut notes = Vec::new();
    let mut pauses = Pauses::default();
    // The sound is the thing that can go wrong without spoiling the video: carry on without it.
    let lost = |e: String| format!("{}. The rest of the recording has no sound.", e.trim_end_matches('.'));
    while !stop.load(Ordering::Relaxed) {
        let elapsed = started.elapsed();
        // The longest recording counts what's recorded, not the time spent paused.
        if elapsed.saturating_sub(pauses.removed(elapsed)) >= MAX_LENGTH {
            break;
        }
        let pause = paused.load(Ordering::Relaxed);
        if pause != pauses.since.is_some() {
            if pause {
                pauses.pause(elapsed);
            } else {
                pauses.resume(elapsed);
                next = Instant::now();
            }
        }
        // Sound keeps coming while paused; taking it (and dropping it) keeps it from piling up.
        if let Some(capture) = &sound {
            if let Err(e) = give_sound(&mut *encoder, capture.ready(), &pauses) {
                notes.push(lost(e));
                sound = None;
            }
        }
        if pause {
            std::thread::sleep(interval);
            continue;
        }
        let now = Instant::now();
        if now < next {
            std::thread::sleep(next - now);
            continue;
        }
        if let Some(frame) = capture_rect_with_cursor(area) {
            if let Some(at) = pauses.place(now - started) {
                encoder.frame(&frame, at)?;
            }
        }
        // If encoding falls behind, skip ahead rather than queueing frames.
        next = (next + interval).max(Instant::now());
    }
    if let Some(capture) = sound {
        let (rest, problems) = capture.stop();
        notes.extend(problems);
        if let Err(e) = give_sound(&mut *encoder, rest, &pauses) {
            notes.push(lost(e));
        }
    }
    encoder.finish()?;
    Ok(notes)
}

unsafe fn register(class: &str, wndproc: WNDPROC, background: COLORREF, cursor: PCWSTR) {
    let name = wide(class);
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: wndproc,
        hInstance: GetModuleHandleW(null()),
        hCursor: LoadCursorW(null_mut(), cursor),
        hbrBackground: CreateSolidBrush(background),
        lpszClassName: name.as_ptr(),
        ..zeroed()
    };
    // Fails harmlessly when already registered by an earlier recording.
    RegisterClassExW(&wc);
}

unsafe fn popup(class: &str, ex_style: WINDOW_EX_STYLE, r: RECT, param: *const c_void) -> HWND {
    let name = wide(class);
    let hwnd = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | ex_style,
        name.as_ptr(),
        null(),
        WS_POPUP,
        r.left,
        r.top,
        r.right - r.left,
        r.bottom - r.top,
        null_mut(),
        null_mut(),
        GetModuleHandleW(null()),
        param,
    );
    // Keep it out of the recording (and any other capture).
    SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    hwnd
}

/// A red outline just outside the recorded area; clicks pass through it.
unsafe fn frame_window(area: RECT) -> HWND {
    register(FRAME_CLASS, Some(DefWindowProcW), RED, IDC_ARROW);
    let (outer, inner) = (inflate(&area, 4), inflate(&area, 1));
    let hwnd = popup(FRAME_CLASS, WS_EX_LAYERED | WS_EX_TRANSPARENT, outer, null());
    SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA);
    let (w, h) = (outer.right - outer.left, outer.bottom - outer.top);
    let ring = CreateRectRgn(0, 0, w, h);
    let hole = CreateRectRgn(inner.left - outer.left, inner.top - outer.top, inner.right - outer.left, inner.bottom - outer.top);
    CombineRgn(ring, ring, hole, RGN_DIFF);
    DeleteObject(hole);
    // The window owns the region from here on.
    SetWindowRgn(hwnd, ring, 1);
    hwnd
}

struct Bar {
    main: HWND,
    started: Instant,
    font: HFONT,
    /// The recording's paused flag. The tray menu can change it too, so the bar follows the flag
    /// rather than its own clicks.
    paused: Arc<AtomicBool>,
    /// When the pause under way began, as far as the bar knows.
    paused_at: Option<Instant>,
    /// How long the pauses before that lasted.
    paused_total: Duration,
}

impl Bar {
    /// Catches up with the paused flag, whoever changed it.
    fn follow(&mut self) {
        match (self.paused.load(Ordering::Relaxed), self.paused_at) {
            (true, None) => self.paused_at = Some(Instant::now()),
            (false, Some(at)) => {
                self.paused_total += at.elapsed();
                self.paused_at = None;
            }
            _ => {}
        }
    }

    /// How much has been recorded so far, the pauses left out.
    fn recorded(&self) -> Duration {
        let paused = self.paused_total + self.paused_at.map_or(Duration::ZERO, |at| at.elapsed());
        self.started.elapsed().saturating_sub(paused)
    }
}

/// Where the parts of a bar `w` by `h` go, left to right: the clock (after the red mark, which is
/// `h` square), the Pause button and the Stop button.
fn bar_layout(w: i32, h: i32) -> [RECT; 3] {
    let stop = w - h * 3 / 2;
    let pause = stop - h * 2;
    [rect(h, 0, pause, h), rect(pause, 0, stop, h), rect(stop, 0, w, h)]
}

/// The bar with the time and the Pause and Stop buttons, centred under the area (or above it, or
/// inside it, where there's room).
unsafe fn bar_window(main: HWND, area: RECT, started: Instant, paused: Arc<AtomicBool>) -> HWND {
    register(BAR_CLASS, Some(bar_proc), BAR_BG, IDC_HAND);
    let scale = GetDpiForSystem() as f32 / 96.0;
    let px = |v: f32| (v * scale).round() as i32;
    let (w, h, gap) = (px(215.0), px(34.0), px(10.0));
    let screen_bottom = GetSystemMetrics(SM_YVIRTUALSCREEN) + GetSystemMetrics(SM_CYVIRTUALSCREEN);
    let x = (area.left + area.right - w) / 2;
    let y = if area.bottom + gap + h <= screen_bottom {
        area.bottom + gap
    } else if area.top - gap - h >= GetSystemMetrics(SM_YVIRTUALSCREEN) {
        area.top - gap - h
    } else {
        area.bottom - gap - h
    };
    let face = wide("Segoe UI");
    let font = CreateFontW(-px(14.0), 0, 0, 0, FW_SEMIBOLD as i32, 0, 0, 0, DEFAULT_CHARSET as u32, 0, 0, CLEARTYPE_QUALITY as u32, 0, face.as_ptr());
    let bar = Box::into_raw(Box::new(Bar { main, started, font, paused, paused_at: None, paused_total: Duration::ZERO }));
    let hwnd = popup(BAR_CLASS, 0, rect(x, y, x + w, y + h), bar as *const c_void);
    SetTimer(hwnd, 1, 500, None);
    hwnd
}

/// "m:ss", or "h:mm:ss" past an hour.
pub fn clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

unsafe extern "system" fn bar_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(lparam as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let bar = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Bar;
    if bar.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    match msg {
        WM_TIMER => {
            (*bar).follow();
            InvalidateRect(hwnd, null(), 1);
            0
        }
        WM_LBUTTONUP => {
            let x = (lparam & 0xFFFF) as i16 as i32;
            let mut r: RECT = zeroed();
            GetClientRect(hwnd, &mut r);
            let [_, pause, stop] = bar_layout(r.right, r.bottom);
            if x >= stop.left {
                PostMessageW((*bar).main, WM_APP_STOP_RECORDING, 0, 0);
            } else if x >= pause.left {
                (*bar).paused.fetch_xor(true, Ordering::Relaxed);
                (*bar).follow();
                InvalidateRect(hwnd, null(), 1);
            }
            0
        }
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut r: RECT = zeroed();
            GetClientRect(hwnd, &mut r);
            let h = r.bottom - r.top;
            (*bar).follow();
            let paused = (*bar).paused_at.is_some();
            // Red "recording" square (two bars while paused), then the time and the two buttons.
            let dot = h / 3;
            let brush = CreateSolidBrush(RED);
            let mark = rect(h / 2 - dot / 2, h / 2 - dot / 2, h / 2 + dot / 2, h / 2 + dot / 2);
            if paused {
                let line = (dot / 3).max(1);
                FillRect(dc, &rect(mark.left, mark.top, mark.left + line, mark.bottom), brush);
                FillRect(dc, &rect(mark.right - line, mark.top, mark.right, mark.bottom), brush);
            } else {
                FillRect(dc, &mark, brush);
            }
            DeleteObject(brush);
            let [time, pause, stop] = bar_layout(r.right, h);
            // A line between the buttons, so they read as two.
            let brush = CreateSolidBrush(DIVIDER);
            FillRect(dc, &rect(stop.left, h / 4, stop.left + (h / 34).max(1), h - h / 4), brush);
            DeleteObject(brush);
            SelectObject(dc, (*bar).font);
            SetBkMode(dc, TRANSPARENT as i32);
            SetTextColor(dc, WHITE);
            let action = if paused { "Resume" } else { "Pause" };
            for (text, mut place, align) in [(clock((*bar).recorded()), time, DT_LEFT), (action.into(), pause, DT_CENTER), ("Stop".into(), stop, DT_CENTER)] {
                let text: Vec<u16> = text.encode_utf16().collect();
                DrawTextW(dc, text.as_ptr(), text.len() as i32, &mut place, align | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
            }
            EndPaint(hwnd, &ps);
            0
        }
        WM_DESTROY => {
            let bar = Box::from_raw(bar);
            DeleteObject(bar.font);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_the_screen() {
        let path = std::env::temp_dir().join(format!("rsnip-rec-{}.mp4", std::process::id()));
        let encoder = match Mp4::new(&path, 64, 48, 30, false) {
            Ok(mp4) => Box::new(mp4),
            Err(e) => return eprintln!("skipped: {e}"),
        };
        let stop = AtomicBool::new(false);
        let started = Instant::now();
        std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_millis(300));
                stop.store(true, Ordering::Relaxed);
            });
            run(encoder, rect(0, 0, 64, 48), 30, started, &stop, &AtomicBool::new(false), None).unwrap();
        });
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp");
        assert_eq!(crate::encode::handlers(&bytes), ["vide"], "no sound unless asked for");
    }

    /// Records for 300 ms from `started`, pauses for 400 ms, records 300 ms more and stops.
    fn pause_for_a_moment(started: Instant, paused: &AtomicBool, stop: &AtomicBool) {
        let until = |ms| std::thread::sleep((started + Duration::from_millis(ms)).saturating_duration_since(Instant::now()));
        until(300);
        paused.store(true, Ordering::Relaxed);
        until(700);
        paused.store(false, Ordering::Relaxed);
        until(1000);
        stop.store(true, Ordering::Relaxed);
    }

    /// What happens while paused isn't in the video: 300 ms, a 400 ms pause, then 300 ms more
    /// make a video about 600 ms long.
    #[test]
    fn leaves_a_pause_out_of_the_video() {
        let path = std::env::temp_dir().join(format!("rsnip-rec-pause-{}.mp4", std::process::id()));
        let encoder = match Mp4::new(&path, 64, 48, 30, false) {
            Ok(mp4) => Box::new(mp4),
            Err(e) => return eprintln!("skipped: {e}"),
        };
        let (stop, paused) = (AtomicBool::new(false), AtomicBool::new(false));
        let started = Instant::now();
        std::thread::scope(|s| {
            s.spawn(|| pause_for_a_moment(started, &paused, &stop));
            run(encoder, rect(0, 0, 64, 48), 30, started, &stop, &paused, None).unwrap();
        });
        let (video, _) = crate::encode::read_stream(&path, false);
        std::fs::remove_file(&path).unwrap();
        assert!(video.abs_diff(Duration::from_millis(600)) <= Duration::from_millis(150), "{video:?}");
    }

    /// The sound made while paused is left out too, so the sound and the video stay together.
    #[test]
    fn leaves_a_pause_out_of_the_sound() {
        let path = std::env::temp_dir().join(format!("rsnip-rec-pause-sound-{}.mp4", std::process::id()));
        let encoder = match Mp4::with_sound(&path, 64, 48, 30, false, true) {
            Ok(mp4) => Box::new(mp4),
            Err(e) => return eprintln!("skipped: {e}"),
        };
        let started = Instant::now();
        let sources = audio::Sources { computer: Some(None), ..Default::default() };
        let sound = match audio::Capture::start(&sources, audio::Clock::now()) {
            Ok(sound) => sound,
            // CI's machines may have no sound device.
            Err(e) => {
                drop(encoder);
                let _ = std::fs::remove_file(&path);
                return eprintln!("skipped: {e}");
            }
        };
        let (stop, paused) = (AtomicBool::new(false), AtomicBool::new(false));
        std::thread::scope(|s| {
            s.spawn(|| pause_for_a_moment(started, &paused, &stop));
            let notes = run(encoder, rect(0, 0, 64, 48), 30, started, &stop, &paused, Some(sound)).unwrap();
            assert!(notes.is_empty(), "{notes:?}");
        });
        let (video, _) = crate::encode::read_stream(&path, false);
        let (sound, _) = crate::encode::read_stream(&path, true);
        std::fs::remove_file(&path).unwrap();
        let near = |d: Duration| d.abs_diff(Duration::from_millis(600)) <= Duration::from_millis(150);
        assert!(near(video) && near(sound) && sound.abs_diff(video) < Duration::from_millis(250), "video {video:?}, sound {sound:?}");
    }

    const MS: i64 = AUDIO_RATE as i64 / 1000;

    /// Stereo sound starting `from` ms in and lasting `ms`, each frame holding its own number.
    fn sound(from: i64, ms: i64) -> audio::Chunk {
        let pcm = (from * MS..(from + ms) * MS).flat_map(|f| [f as i16; 2]).collect();
        audio::Chunk { at: Duration::from_millis(from as u64), pcm }
    }

    /// Pauses for each of `spans`, from and to, in ms.
    fn with_pauses(spans: &[(u64, u64)]) -> Pauses {
        let mut pauses = Pauses::default();
        for &(from, to) in spans {
            pauses.pause(Duration::from_millis(from));
            pauses.resume(Duration::from_millis(to));
        }
        pauses
    }

    /// Each piece as where it starts (ms), its first frame's number, and how many frames it has.
    fn pieces(chunks: Vec<audio::Chunk>) -> Vec<(u128, i16, usize)> {
        chunks.iter().map(|c| (c.at.as_millis(), c.pcm[0], c.pcm.len() / 2)).collect()
    }

    #[test]
    fn places_times_around_the_pauses() {
        let mut pauses = with_pauses(&[(100, 300)]);
        let place = |pauses: &Pauses, ms| pauses.place(Duration::from_millis(ms)).map(|d| d.as_millis());
        assert_eq!(place(&pauses, 50), Some(50), "before");
        assert_eq!(place(&pauses, 100), None, "inside");
        assert_eq!(place(&pauses, 299), None, "inside");
        assert_eq!(place(&pauses, 300), Some(100), "after");
        assert_eq!(place(&pauses, 400), Some(200), "after");
        pauses.pause(Duration::from_millis(500));
        assert_eq!(place(&pauses, 499), Some(299));
        assert_eq!(place(&pauses, 600), None, "in the pause under way");
        assert_eq!(pauses.removed(Duration::from_millis(700)), Duration::from_millis(400));
        pauses.pause(Duration::from_millis(650));
        pauses.resume(Duration::from_millis(800));
        assert_eq!(place(&pauses, 900), Some(400), "the pause began when it was first paused");
        assert_eq!(pauses.removed(Duration::from_secs(1)), Duration::from_millis(500));
    }

    #[test]
    fn sound_from_inside_a_pause_is_dropped() {
        assert!(with_pauses(&[(100, 300)]).trim(sound(150, 100)).is_empty());
        let mut pauses = with_pauses(&[]);
        pauses.pause(Duration::from_millis(100));
        assert!(pauses.trim(sound(150, 100)).is_empty(), "in the pause under way");
    }

    #[test]
    fn sound_across_a_pause_is_cut_where_it_begins_and_ends() {
        let pauses = with_pauses(&[(150, 300)]);
        // Across its start: only the part before it.
        assert_eq!(pieces(pauses.trim(sound(100, 100))), [(100, 100 * MS as i16, 50 * MS as usize)]);
        // Across its end: only the part after, moved back by the pause's 150 ms.
        assert_eq!(pieces(pauses.trim(sound(250, 100))), [(150, 300 * MS as i16, 50 * MS as usize)]);
        // Before and after it: untouched but for the move.
        assert_eq!(pieces(pauses.trim(sound(50, 50))), [(50, 50 * MS as i16, 50 * MS as usize)]);
        assert_eq!(pieces(pauses.trim(sound(400, 10))), [(250, 400 * MS as i16, 10 * MS as usize)]);
        // Across the start of the pause under way: the part before it.
        let mut pauses = pauses;
        pauses.pause(Duration::from_millis(420));
        assert_eq!(pieces(pauses.trim(sound(400, 100))), [(250, 400 * MS as i16, 20 * MS as usize)]);
    }

    #[test]
    fn sound_around_a_whole_pause_is_two_pieces_that_follow_on() {
        let trimmed = with_pauses(&[(150, 170)]).trim(sound(100, 100));
        assert_eq!(pieces(trimmed), [(100, 100 * MS as i16, 50 * MS as usize), (150, 170 * MS as i16, 30 * MS as usize)]);
        assert!(with_pauses(&[]).trim(sound(100, 100)).len() == 1, "without pauses it's as it was");
    }

    /// Sound in short chunks, as the mixer sends it, still follows on from one chunk to the next
    /// once the pauses are taken out, however the chunks and pauses line up.
    #[test]
    fn trimmed_sound_follows_on() {
        let pauses = with_pauses(&[(33, 47), (60, 61), (95, 140)]);
        let trimmed: Vec<_> = (0..20).flat_map(|i| pauses.trim(sound(i * 10, 10))).collect();
        let mut next = 0;
        for chunk in &trimmed {
            assert_eq!(chunk.at, from_frames(next), "follows on");
            next += chunk.pcm.len() as i64 / 2;
        }
        assert_eq!(next, (200 - 14 - 1 - 45) * MS);
    }

    /// Records the screen and the computer's sound (silence, if nothing plays) for a moment, and
    /// the MP4 has both, about as long as each other.
    #[test]
    fn records_the_screen_with_sound() {
        let path = std::env::temp_dir().join(format!("rsnip-rec-sound-{}.mp4", std::process::id()));
        let started = Instant::now();
        let sources = audio::Sources { computer: Some(None), ..Default::default() };
        let sound = match audio::Capture::start(&sources, audio::Clock::now()) {
            Ok(sound) => sound,
            // CI's machines may have no sound device, or no Media Foundation.
            Err(e) => return eprintln!("skipped: {e}"),
        };
        let encoder = match Mp4::with_sound(&path, 64, 48, 30, false, true) {
            Ok(mp4) => Box::new(mp4),
            Err(e) => return eprintln!("skipped: {e}"),
        };
        let stop = AtomicBool::new(false);
        std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_millis(1500));
                stop.store(true, Ordering::Relaxed);
            });
            let notes = run(encoder, rect(0, 0, 64, 48), 30, started, &stop, &AtomicBool::new(false), Some(sound)).unwrap();
            assert!(notes.is_empty(), "{notes:?}");
        });
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(crate::encode::handlers(&bytes), ["vide", "soun"]);
        let (video, _) = crate::encode::read_stream(&path, false);
        let (sound, _) = crate::encode::read_stream(&path, true);
        std::fs::remove_file(&path).unwrap();
        assert!(sound.abs_diff(video) < Duration::from_millis(250), "video {video:?}, sound {sound:?}");
    }

    /// The longest recording ends by itself, with its file finished.
    #[test]
    fn stops_at_the_longest_recording() {
        let path = std::env::temp_dir().join(format!("rsnip-rec-long-{}.mp4", std::process::id()));
        let encoder = match Mp4::new(&path, 64, 48, 30, false) {
            Ok(mp4) => Box::new(mp4),
            Err(e) => return eprintln!("skipped: {e}"),
        };
        let begun = Instant::now();
        let stop = AtomicBool::new(false);
        let notes = run(encoder, rect(0, 0, 64, 48), 30, begun - MAX_LENGTH + Duration::from_millis(200), &stop, &AtomicBool::new(false), None).unwrap();
        assert!(notes.is_empty() && begun.elapsed() < Duration::from_secs(5), "{notes:?} after {:?}", begun.elapsed());
        assert!(!stop.load(Ordering::Relaxed), "it wasn't stopped");
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp");
    }

    #[test]
    fn clock_formats_elapsed_time() {
        assert_eq!(super::clock(Duration::from_secs(7)), "0:07");
        assert_eq!(super::clock(Duration::from_secs(754)), "12:34");
        assert_eq!(super::clock(Duration::from_secs(3723)), "1:02:03");
    }
}
