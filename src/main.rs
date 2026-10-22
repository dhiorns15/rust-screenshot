//! RSnip: a small open-source Snagit-style screenshot tool for Windows.
//!
//! `rsnip`            runs in the tray and takes over Print Screen (and optionally Win+Shift+S).
//! `rsnip --region`   starts a region capture (in the running instance if there is one).
//! `rsnip --full`     captures every monitor.
//! `rsnip --delay N`  starts a region capture in N seconds.
//! `rsnip --text`     selects a region and copies the text in it.
//! `rsnip --last`     captures the same area as last time.
//! `rsnip --gallery [file]` opens the screenshot library / editor.
//! `rsnip --pin <file>` shows an image pinned on top of other windows.

#![windows_subsystem = "windows"]

mod capture;
mod gallery;
mod hook;
mod icon;
mod icon_art;
mod ocr;
mod output;
mod overlay;
mod pin;
mod prompt;
mod settings;
mod tray;
mod util;

use std::cell::{Cell, RefCell};
use std::mem::{size_of, zeroed};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::atomic::Ordering::Relaxed;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use capture::Image;
use hook::Hotkey;
use settings::Settings;
use util::{wide, wide_os};

pub const APP_NAME: &str = "RSnip";
const MAIN_CLASS: &str = "RSnip.Main";

pub const WM_APP_TRAY: u32 = WM_APP + 1;
pub const WM_APP_CAPTURE: u32 = WM_APP + 2;
pub const WM_APP_SAVED: u32 = WM_APP + 3;
pub const WM_APP_ALREADY_RUNNING: u32 = WM_APP + 4;
pub const WM_APP_OVERLAY_KEY: u32 = WM_APP + 5;
pub const WM_APP_HOTKEY_RECORDED: u32 = WM_APP + 6;
/// Start a region capture in `wparam` seconds.
pub const WM_APP_DELAYED: u32 = WM_APP + 7;
/// Text recognition finished; `lparam` is a `Box<Result<String, String>>`.
pub const WM_APP_OCR: u32 = WM_APP + 8;

const TIMER_REGION: usize = 1;
const TIMER_FULL: usize = 2;
const TIMER_COUNTDOWN: usize = 3;
const TIMER_TEXT: usize = 4;
const TIMER_LAST: usize = 5;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Region = 0,
    Full = 1,
    /// Select a region and copy the text in it.
    Text = 2,
    /// The same area as the last capture.
    Last = 3,
}

impl Mode {
    fn from_wparam(w: usize) -> Mode {
        match w {
            1 => Mode::Full,
            2 => Mode::Text,
            3 => Mode::Last,
            _ => Mode::Region,
        }
    }
}

struct SaveResult {
    path: PathBuf,
    error: Option<String>,
    copied: bool,
}

thread_local! {
    static SETTINGS: Cell<Settings> = Cell::new(Settings::default());
    static ICON: Cell<HICON> = const { Cell::new(null_mut()) };
    static ICON_LARGE: Cell<HICON> = const { Cell::new(null_mut()) };
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
    static CAPTURING: Cell<bool> = const { Cell::new(false) };
    static LAST_SAVED: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    /// Seconds left before a delayed capture.
    static COUNTDOWN: Cell<u32> = const { Cell::new(0) };
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--gallery") => return gallery::run(args.get(1).map(PathBuf::from)),
        Some("--pin") => return args.get(1).map_or((), |file| pin::run(PathBuf::from(file))),
        _ => {}
    }
    let mode = args.iter().find_map(|a| match a.as_str() {
        "--region" | "--capture" => Some(Mode::Region),
        "--full" => Some(Mode::Full),
        "--text" => Some(Mode::Text),
        "--last" => Some(Mode::Last),
        _ => None,
    });
    let delay = args.iter().position(|a| a == "--delay").and_then(|i| args.get(i + 1)?.parse().ok());
    unsafe { run_tray(mode, delay) }
}

fn tip(hotkey: Hotkey) -> String {
    format!("{APP_NAME} — press {} to capture", hotkey.name())
}

unsafe fn run_tray(mode: Option<Mode>, delay: Option<u32>) {
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

    let class = wide(MAIN_CLASS);
    let mutex_name = wide("Local\\RSnip.Tray");
    let _mutex = CreateMutexW(null(), 0, mutex_name.as_ptr());
    if GetLastError() == ERROR_ALREADY_EXISTS {
        let other = FindWindowW(class.as_ptr(), null());
        if !other.is_null() {
            match (delay, mode) {
                (Some(secs), _) => PostMessageW(other, WM_APP_DELAYED, secs as usize, 0),
                (None, Some(m)) => PostMessageW(other, WM_APP_CAPTURE, m as usize, 0),
                (None, None) => PostMessageW(other, WM_APP_ALREADY_RUNNING, 0, 0),
            };
        }
        return;
    }

    let hinstance = GetModuleHandleW(null());
    let dpi = GetDpiForSystem();
    let icon = icon::create_icon(GetSystemMetricsForDpi(SM_CXSMICON, dpi));
    let icon_large = icon::create_icon(GetSystemMetricsForDpi(SM_CXICON, dpi));
    ICON.set(icon);
    ICON_LARGE.set(icon_large);

    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(main_wndproc),
        hInstance: hinstance,
        hIcon: icon_large,
        hIconSm: icon,
        lpszClassName: class.as_ptr(),
        ..zeroed()
    };
    RegisterClassExW(&wc);
    overlay::register_class(hinstance);
    prompt::register_class(hinstance);

    // A hidden top-level window (not message-only) so it receives the TaskbarCreated broadcast.
    let title = wide(APP_NAME);
    let hwnd = CreateWindowExW(
        0,
        class.as_ptr(),
        title.as_ptr(),
        WS_OVERLAPPED,
        0,
        0,
        0,
        0,
        null_mut(),
        null_mut(),
        hinstance,
        null(),
    );
    if hwnd.is_null() {
        return;
    }
    hook::MAIN_HWND.store(hwnd as isize, Relaxed);

    let settings = Settings::load();
    SETTINGS.set(settings);
    hook::set_hotkey(settings.hotkey);
    hook::set_win_shift_s(settings.win_shift_s);
    settings::refresh_startup_path();

    TASKBAR_CREATED.set(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()));
    tray::add(hwnd, icon, &tip(settings.hotkey));
    if !hook::install(hinstance) {
        notify(hwnd, "Keyboard hook failed", "Print Screen won't be captured; use the tray icon instead.");
    }

    match (delay, mode) {
        (Some(secs), _) => start_countdown(hwnd, secs),
        (None, Some(m)) => {
            PostMessageW(hwnd, WM_APP_CAPTURE, m as usize, 0);
        }
        (None, None) if settings.open_library_on_launch => open_gallery(None),
        (None, None) => {}
    }

    let mut msg: MSG = zeroed();
    while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }

    hook::uninstall();
    tray::remove(hwnd);
    DestroyIcon(icon);
    DestroyIcon(icon_large);
}

/// Starts (or restarts) a delayed region capture. The countdown shows only in the tray tooltip, so
/// nothing extra pops up on screen to end up in the capture.
unsafe fn start_countdown(hwnd: HWND, secs: u32) {
    let secs = secs.clamp(1, 60);
    COUNTDOWN.set(secs);
    tray::set_tip(hwnd, &format!("{APP_NAME} — capturing in {secs} s"));
    SetTimer(hwnd, TIMER_COUNTDOWN, 1000, None);
}

unsafe fn cancel_countdown(hwnd: HWND) {
    KillTimer(hwnd, TIMER_COUNTDOWN);
    COUNTDOWN.set(0);
    tray::set_tip(hwnd, &tip(SETTINGS.get().hotkey));
}

unsafe fn notify(hwnd: HWND, title: &str, text: &str) {
    tray::notify(hwnd, ICON_LARGE.get(), title, text);
}

fn update_settings(f: impl FnOnce(&mut Settings)) -> Settings {
    let mut s = SETTINGS.get();
    f(&mut s);
    s.store();
    SETTINGS.set(s);
    s
}

pub fn shell_open(path: &Path) {
    let verb = wide("open");
    let file = wide_os(path.as_os_str());
    unsafe { ShellExecuteW(null_mut(), verb.as_ptr(), file.as_ptr(), null(), null(), SW_SHOWNORMAL) };
}

/// Opens the library window, optionally with `file` selected.
fn open_gallery(file: Option<&Path>) {
    let Ok(exe) = std::env::current_exe() else { return };
    // Let the library window come to the front even though it is a different process.
    unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--gallery");
    if let Some(file) = file {
        cmd.arg(file);
    }
    let _ = cmd.spawn();
}

unsafe extern "system" fn main_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_APP_TRAY => {
            match (lparam & 0xFFFF) as u32 {
                tray::NIN_SELECT | tray::NIN_KEYSELECT => {
                    SetTimer(hwnd, TIMER_REGION, 150, None);
                }
                WM_CONTEXTMENU => on_menu(hwnd),
                tray::NIN_BALLOONUSERCLICK => {
                    let last = LAST_SAVED.with(|l| l.borrow().clone());
                    open_gallery(last.as_deref());
                }
                _ => {}
            }
            0
        }
        WM_TIMER if wparam == TIMER_COUNTDOWN => {
            let left = COUNTDOWN.get().saturating_sub(1);
            if left == 0 {
                cancel_countdown(hwnd);
                capture(hwnd, Mode::Region);
            } else {
                COUNTDOWN.set(left);
                tray::set_tip(hwnd, &format!("{APP_NAME} — capturing in {left} s"));
            }
            0
        }
        WM_APP_OCR => {
            on_text(hwnd, *Box::from_raw(lparam as *mut Result<String, String>));
            0
        }
        WM_APP_DELAYED => {
            start_countdown(hwnd, wparam as u32);
            0
        }
        WM_TIMER => {
            KillTimer(hwnd, wparam);
            match wparam {
                TIMER_REGION => capture(hwnd, Mode::Region),
                TIMER_FULL => capture(hwnd, Mode::Full),
                TIMER_TEXT => capture(hwnd, Mode::Text),
                TIMER_LAST => capture(hwnd, Mode::Last),
                _ => {}
            }
            0
        }
        WM_APP_CAPTURE => {
            capture(hwnd, Mode::from_wparam(wparam));
            0
        }
        WM_APP_SAVED => {
            on_saved(hwnd, *Box::from_raw(lparam as *mut SaveResult));
            0
        }
        WM_APP_HOTKEY_RECORDED => {
            hook::set_recording(false);
            prompt::close();
            if wparam != 0 {
                let hotkey = Hotkey::unpack(wparam as u32);
                update_settings(|s| s.hotkey = hotkey);
                hook::set_hotkey(hotkey);
                tray::set_tip(hwnd, &tip(hotkey));
                notify(hwnd, "Hotkey changed", &format!("Press {} to take a screenshot.", hotkey.name()));
            }
            0
        }
        WM_APP_ALREADY_RUNNING => {
            let hotkey = SETTINGS.get().hotkey.name();
            notify(hwnd, "RSnip is already running", &format!("Press {hotkey} to take a screenshot."));
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ if msg != 0 && msg == TASKBAR_CREATED.get() => {
            tray::add(hwnd, ICON.get(), &tip(SETTINGS.get().hotkey));
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn on_menu(hwnd: HWND) {
    let s = SETTINGS.get();
    let startup = settings::startup_enabled();
    match tray::show_menu(hwnd, &s, startup, COUNTDOWN.get()) {
        cmd if cmd > tray::CMD_DELAY && tray::DELAYS.contains(&(cmd - tray::CMD_DELAY)) => {
            start_countdown(hwnd, cmd - tray::CMD_DELAY);
        }
        tray::CMD_CANCEL_DELAY => cancel_countdown(hwnd),
        // Give the menu time to fade out so it isn't in the capture.
        tray::CMD_REGION => {
            SetTimer(hwnd, TIMER_REGION, 250, None);
        }
        tray::CMD_FULL => {
            SetTimer(hwnd, TIMER_FULL, 250, None);
        }
        tray::CMD_TEXT => {
            SetTimer(hwnd, TIMER_TEXT, 250, None);
        }
        tray::CMD_LAST => {
            SetTimer(hwnd, TIMER_LAST, 250, None);
        }
        tray::CMD_GALLERY => open_gallery(None),
        tray::CMD_OPEN_FOLDER => shell_open(&output::screenshots_dir()),
        tray::CMD_HOTKEY => {
            hook::set_recording(true);
            prompt::show(hwnd, &s.hotkey.name());
        }
        tray::CMD_SAVE => {
            update_settings(|s| s.save_to_folder = !s.save_to_folder);
        }
        tray::CMD_OPEN_EDITOR => {
            update_settings(|s| s.open_editor = !s.open_editor);
        }
        tray::CMD_LIBRARY_ON_LAUNCH => {
            update_settings(|s| s.open_library_on_launch = !s.open_library_on_launch);
        }
        tray::CMD_NOTIFY => {
            update_settings(|s| s.notify = !s.notify);
        }
        tray::CMD_WIN_SHIFT_S => {
            let s = update_settings(|s| s.win_shift_s = !s.win_shift_s);
            hook::set_win_shift_s(s.win_shift_s);
        }
        tray::CMD_STARTUP => {
            if !settings::set_startup(!startup) {
                notify(hwnd, "Couldn't change startup setting", "Writing to the registry failed.");
            }
        }
        tray::CMD_EXIT => {
            DestroyWindow(hwnd);
        }
        _ => {}
    }
}

unsafe fn capture(hwnd: HWND, mode: Mode) {
    if CAPTURING.replace(true) {
        return;
    }
    let image = take(mode);
    CAPTURING.set(false);
    match (image, mode) {
        (Some((image, true)), _) => {
            // Saving a big capture takes a moment; keep the hook's thread responsive.
            std::thread::spawn(move || pin::pin_image(&image));
        }
        (Some((image, false)), Mode::Text) => read_text(hwnd, image),
        (Some((image, false)), _) => deliver(hwnd, image),
        (None, _) => {}
    }
}

/// Recognises the text in a capture on a worker thread; the result comes back as `WM_APP_OCR`.
fn read_text(hwnd: HWND, img: Image) {
    let target = hwnd as isize;
    std::thread::spawn(move || {
        let result = Box::into_raw(Box::new(ocr::recognize(&img)));
        if unsafe { PostMessageW(target as HWND, WM_APP_OCR, 0, result as LPARAM) } == 0 {
            drop(unsafe { Box::from_raw(result) });
        }
    });
}

unsafe fn on_text(hwnd: HWND, result: Result<String, String>) {
    let text = match result {
        Ok(text) if text.trim().is_empty() => return notify(hwnd, "No text found", "Try selecting the text more closely."),
        Ok(text) => text,
        Err(e) => return notify(hwnd, "Couldn't read the text", &e),
    };
    if let Err(e) = output::copy_text_to_clipboard(hwnd, &text) {
        return notify(hwnd, "Couldn't copy the text", &e);
    }
    let lines = text.lines().count();
    let first: String = text.lines().next().unwrap_or_default().chars().take(60).collect();
    let title = if lines == 1 { "Copied 1 line of text".to_string() } else { format!("Copied {lines} lines of text") };
    notify(hwnd, &title, &first);
}

/// The captured image, and whether the user asked to pin it (P in the overlay).
fn take(mode: Mode) -> Option<(Image, bool)> {
    let shot = capture::capture_screen()?;
    let last = settings::last_region().and_then(|r| shot.to_image_rect(r));
    match (mode, last) {
        (Mode::Full, _) => return Some((shot.image, false)),
        (Mode::Last, Some(r)) => return Some((shot.image.crop(r), false)),
        // No last region (or it's off screen now): pick one as usual.
        _ => {}
    }
    let targets = capture::window_targets(&shot);
    let (selection, pin) = overlay::select(&shot, targets, last)?;
    let (image, area) = match selection {
        overlay::Selection::Area(r) | overlay::Selection::Window { rect: r, occluded: false, .. } => (shot.image.crop(r), r),
        // Something was on top of the window: raise it and capture it again cleanly.
        overlay::Selection::Window { hwnd, rect, occluded: true } => {
            (capture::capture_window_in_front(hwnd).unwrap_or_else(|| shot.image.crop(rect)), rect)
        }
    };
    settings::set_last_region(shot.to_screen_rect(area));
    Some((image, pin))
}

unsafe fn deliver(hwnd: HWND, img: Image) {
    let s = SETTINGS.get();
    let copied = match output::copy_to_clipboard(hwnd, &img) {
        Ok(()) => true,
        Err(e) => {
            notify(hwnd, "Couldn't copy screenshot", &e);
            false
        }
    };
    // The editor works on files, so opening it implies saving.
    if !s.save_to_folder && !s.open_editor {
        LAST_SAVED.with(|l| *l.borrow_mut() = None);
        if copied && s.notify {
            notify(hwnd, "Screenshot copied", "Paste it anywhere with Ctrl+V.");
        }
        return;
    }
    let path = match output::next_path() {
        Ok(p) => p,
        Err(e) => return notify(hwnd, "Couldn't save screenshot", &e),
    };
    // PNG encoding of a large capture can take a moment; keep the hook's thread responsive.
    let target = hwnd as isize;
    std::thread::spawn(move || {
        let error = output::save_png(&img, &path).err();
        let result = Box::into_raw(Box::new(SaveResult { path, error, copied }));
        if unsafe { PostMessageW(target as HWND, WM_APP_SAVED, 0, result as LPARAM) } == 0 {
            drop(unsafe { Box::from_raw(result) });
        }
    });
}

unsafe fn on_saved(hwnd: HWND, result: SaveResult) {
    if let Some(e) = result.error {
        return notify(hwnd, "Couldn't save screenshot", &e);
    }
    let name = result.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if SETTINGS.get().open_editor {
        open_gallery(Some(&result.path));
        LAST_SAVED.with(|l| *l.borrow_mut() = Some(result.path));
        return;
    }
    LAST_SAVED.with(|l| *l.borrow_mut() = Some(result.path));
    if SETTINGS.get().notify {
        let title = if result.copied { "Screenshot copied to clipboard" } else { "Screenshot saved" };
        notify(hwnd, title, &format!("Saved as {name}. Click to annotate it in the library."));
    }
}
