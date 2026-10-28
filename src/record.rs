//! Recording an area of the screen to MP4 (or GIF). While recording, a red frame marks the area
//! and a small bar shows the time with a Stop button; both are hidden from the recording itself.
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
use crate::encode::{even, Encoder, Gif, Mp4};
use crate::gallery::camera::{Bubble, Launch};
use windows_sys::core::PCWSTR;

use crate::util::{inflate, rect, wide};
use crate::WM_APP_STOP_RECORDING;

const BAR_CLASS: &str = "RSnip.RecordBar";
const FRAME_CLASS: &str = "RSnip.RecordFrame";
const RED: COLORREF = 0x003030E0;
const BAR_BG: COLORREF = 0x00202020;
const WHITE: COLORREF = 0x00FFFFFF;
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
/// Stop button is clicked.
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
    // The camera's bubble says how far behind it is, and the microphone is held back that much.
    let microphone_delay = Arc::default();
    // The encoder lives on the recording thread (Media Foundation objects stay on the thread that
    // made them); it reports when it started, or why it didn't, so a failure shows straight away.
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let thread = {
        let (stop, microphone_delay) = (stop.clone(), Arc::clone(&microphone_delay));
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
            match encoder.and_then(|encoder| run(encoder, area, fps, started, &stop, sound)) {
                Ok(later) => {
                    notes.extend(later);
                    let note = Some(notes.join(" ")).filter(|n| !n.is_empty());
                    crate::gallery::finish_recording(&working, &path).map(|()| Recorded { path, note })
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&working);
                    Err(e)
                }
            }
        })
    };
    let started = ready_rx.recv().map_err(|_| "The recording didn't start".to_string())??;
    let windows = unsafe { [frame_window(area), bar_window(main, area, started)] };
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

/// Gives the encoder the sound made so far.
fn give_sound(encoder: &mut dyn Encoder, chunks: Vec<audio::Chunk>) -> Result<(), String> {
    chunks.iter().try_for_each(|chunk| encoder.audio(&chunk.pcm, chunk.at))
}

/// Captures frames until `stop` (or the longest recording), writing the sound along with them.
/// Returns what to tell the user about the sound, if anything went wrong with it.
fn run(mut encoder: Box<dyn Encoder>, area: RECT, fps: u32, started: Instant, stop: &AtomicBool, mut sound: Option<audio::Capture>) -> Result<Vec<String>, String> {
    let interval = Duration::from_secs(1) / fps;
    let mut next = started;
    let mut notes = Vec::new();
    // The sound is the thing that can go wrong without spoiling the video: carry on without it.
    let lost = |e: String| format!("{}. The rest of the recording has no sound.", e.trim_end_matches('.'));
    while !stop.load(Ordering::Relaxed) && started.elapsed() < MAX_LENGTH {
        if let Some(capture) = &sound {
            if let Err(e) = give_sound(&mut *encoder, capture.ready()) {
                notes.push(lost(e));
                sound = None;
            }
        }
        let now = Instant::now();
        if now < next {
            std::thread::sleep(next - now);
            continue;
        }
        if let Some(frame) = capture_rect_with_cursor(area) {
            encoder.frame(&frame, now - started)?;
        }
        // If encoding falls behind, skip ahead rather than queueing frames.
        next = (next + interval).max(Instant::now());
    }
    if let Some(capture) = sound {
        let (rest, problems) = capture.stop();
        notes.extend(problems);
        if let Err(e) = give_sound(&mut *encoder, rest) {
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
}

/// The Stop bar, centred under the area (or above it, or inside it, where there's room).
unsafe fn bar_window(main: HWND, area: RECT, started: Instant) -> HWND {
    register(BAR_CLASS, Some(bar_proc), BAR_BG, IDC_HAND);
    let scale = GetDpiForSystem() as f32 / 96.0;
    let px = |v: f32| (v * scale).round() as i32;
    let (w, h, gap) = (px(150.0), px(34.0), px(10.0));
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
    let bar = Box::into_raw(Box::new(Bar { main, started, font }));
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
            InvalidateRect(hwnd, null(), 1);
            0
        }
        WM_LBUTTONUP => {
            PostMessageW((*bar).main, WM_APP_STOP_RECORDING, 0, 0);
            0
        }
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut r: RECT = zeroed();
            GetClientRect(hwnd, &mut r);
            let h = r.bottom - r.top;
            // Red "recording" square, then the time and the action.
            let dot = h / 3;
            let brush = CreateSolidBrush(RED);
            let mark = rect(h / 2 - dot / 2, h / 2 - dot / 2, h / 2 + dot / 2, h / 2 + dot / 2);
            FillRect(dc, &mark, brush);
            DeleteObject(brush);
            SelectObject(dc, (*bar).font);
            SetBkMode(dc, TRANSPARENT as i32);
            SetTextColor(dc, WHITE);
            let text: Vec<u16> = format!("{}   Stop", clock((*bar).started.elapsed())).encode_utf16().collect();
            let mut tr = rect(h, 0, r.right, h);
            DrawTextW(dc, text.as_ptr(), text.len() as i32, &mut tr, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
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
            run(encoder, rect(0, 0, 64, 48), 30, started, &stop, None).unwrap();
        });
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp");
        assert_eq!(crate::encode::handlers(&bytes), ["vide"], "no sound unless asked for");
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
            let notes = run(encoder, rect(0, 0, 64, 48), 30, started, &stop, Some(sound)).unwrap();
            assert!(notes.is_empty(), "{notes:?}");
        });
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(crate::encode::handlers(&bytes), ["vide", "soun"]);
        let (video, _) = crate::encode::read_stream(&path, false);
        let (sound, _) = crate::encode::read_stream(&path, true);
        std::fs::remove_file(&path).unwrap();
        assert!(sound.abs_diff(video) < Duration::from_millis(250), "video {video:?}, sound {sound:?}");
    }

    #[test]
    fn clock_formats_elapsed_time() {
        assert_eq!(super::clock(Duration::from_secs(7)), "0:07");
        assert_eq!(super::clock(Duration::from_secs(754)), "12:34");
        assert_eq!(super::clock(Duration::from_secs(3723)), "1:02:03");
    }
}
