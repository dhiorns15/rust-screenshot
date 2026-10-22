// The Windows app: tray icon, keyboard hook, capture overlay and the rest. Included by main.rs.
//
// `rsnip`            runs in the tray and takes over Print Screen (and optionally Win+Shift+S).
// `rsnip --region`   starts a region capture (in the running instance if there is one).
// `rsnip --full`     captures every monitor.
// `rsnip --delay N`  starts a region capture in N seconds.
// `rsnip --text`     selects a region and copies the text in it.
// `rsnip --last`     captures the same area as last time.
// `rsnip --scroll`   picks a window, scrolls it and captures the whole length.
// `rsnip --record`   picks an area and records it to video; run again (or the hotkey) to stop.
// `rsnip --gallery [file]` opens the screenshot library / editor.
// `rsnip --pin <file>` shows an image pinned on top of other windows.
// `rsnip --update`   checks for a newer version and installs it.

mod capture;
mod encode;
mod hook;
mod icon;
mod ocr;
mod output;
mod overlay;
mod pin;
mod record;
mod prompt;
mod scroll;
mod settings;
mod stitch;
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
/// The recording's Stop button was clicked.
pub const WM_APP_STOP_RECORDING: u32 = WM_APP + 9;
/// A recording finished saving; `lparam` is a `Box<Result<PathBuf, String>>`.
pub const WM_APP_RECORDED: u32 = WM_APP + 10;
/// An update check finished; `wparam` is the `Check`, `lparam` a `Box<Result<Option<Release>, String>>`.
const WM_APP_UPDATE_CHECKED: u32 = WM_APP + 11;
/// The update's installer was downloaded; `lparam` is a `Box<Result<PathBuf, String>>`.
const WM_APP_UPDATE_READY: u32 = WM_APP + 12;
/// `rsnip --update` was run again: check for an update and install it.
const WM_APP_UPDATE_NOW: u32 = WM_APP + 13;

const TIMER_REGION: usize = 1;
const TIMER_FULL: usize = 2;
const TIMER_COUNTDOWN: usize = 3;
const TIMER_TEXT: usize = 4;
const TIMER_LAST: usize = 5;
const TIMER_SCROLL: usize = 6;
const TIMER_RECORD: usize = 7;
const TIMER_UPDATE: usize = 8;

/// The first update check waits a little so it doesn't slow down signing in, then repeats daily.
const UPDATE_FIRST_CHECK_MS: u32 = 15_000;
const UPDATE_CHECK_EVERY_MS: u32 = 24 * 60 * 60 * 1000;

/// Why an update check runs, which decides what happens with the result.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Check {
    /// The daily check: says once per new version that it's available.
    Daily = 0,
    /// "Check now" in the menu: always reports the outcome.
    Manual = 1,
    /// `rsnip --update`: installs a newer version straight away.
    Install = 2,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Region = 0,
    Full = 1,
    /// Select a region and copy the text in it.
    Text = 2,
    /// The same area as the last capture.
    Last = 3,
    /// Scroll the picked window and stitch it into one tall image.
    Scroll = 4,
    /// Record the picked area to a video.
    Record = 5,
}

impl Mode {
    fn from_wparam(w: usize) -> Mode {
        match w {
            1 => Mode::Full,
            2 => Mode::Text,
            3 => Mode::Last,
            4 => Mode::Scroll,
            5 => Mode::Record,
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
    static RECORDING: RefCell<Option<record::Recording>> = const { RefCell::new(None) };
    /// A newer release, once a check has found one.
    static AVAILABLE: RefCell<Option<update::Release>> = const { RefCell::new(None) };
    /// An update is downloading.
    static UPDATING: Cell<bool> = const { Cell::new(false) };
    /// The last notification was about an available update, so clicking it installs that.
    static UPDATE_BALLOON: Cell<bool> = const { Cell::new(false) };
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
        "--scroll" => Some(Mode::Scroll),
        "--record" => Some(Mode::Record),
        _ => None,
    });
    let delay = args.iter().position(|a| a == "--delay").and_then(|i| args.get(i + 1)?.parse().ok());
    let update_now = args.iter().any(|a| a == "--update");
    unsafe { run_tray(mode, delay, update_now) }
}

fn tip(hotkey: Hotkey) -> String {
    format!("{APP_NAME} — press {} to capture", hotkey.name())
}

unsafe fn run_tray(mode: Option<Mode>, delay: Option<u32>, update_now: bool) {
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

    let class = wide(MAIN_CLASS);
    let mutex_name = wide("Local\\RSnip.Tray");
    let _mutex = CreateMutexW(null(), 0, mutex_name.as_ptr());
    if GetLastError() == ERROR_ALREADY_EXISTS {
        let other = FindWindowW(class.as_ptr(), null());
        if !other.is_null() {
            match (delay, mode) {
                _ if update_now => PostMessageW(other, WM_APP_UPDATE_NOW, 0, 0),
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

    SetTimer(hwnd, TIMER_UPDATE, UPDATE_FIRST_CHECK_MS, None);
    match (delay, mode) {
        _ if update_now => start_check(hwnd, Check::Install),
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
    UPDATE_BALLOON.set(false);
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
                tray::NIN_BALLOONUSERCLICK if UPDATE_BALLOON.get() => start_update(hwnd),
                tray::NIN_BALLOONUSERCLICK => {
                    let last = LAST_SAVED.with(|l| l.borrow().clone());
                    match last {
                        Some(video) if is_video(&video) => shell_open(&video),
                        last => open_gallery(last.as_deref()),
                    }
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
        WM_APP_STOP_RECORDING => {
            stop_recording(hwnd);
            0
        }
        WM_APP_RECORDED => {
            on_recorded(hwnd, *Box::from_raw(lparam as *mut Result<PathBuf, String>));
            0
        }
        WM_APP_OCR => {
            on_text(hwnd, *Box::from_raw(lparam as *mut Result<String, String>));
            0
        }
        WM_APP_UPDATE_CHECKED => {
            let result = *Box::from_raw(lparam as *mut Result<Option<update::Release>, String>);
            let check = match wparam {
                1 => Check::Manual,
                2 => Check::Install,
                _ => Check::Daily,
            };
            on_update_checked(hwnd, check, result);
            0
        }
        WM_APP_UPDATE_READY => {
            on_update_ready(hwnd, *Box::from_raw(lparam as *mut Result<PathBuf, String>));
            0
        }
        WM_APP_UPDATE_NOW => {
            start_check(hwnd, Check::Install);
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
                TIMER_SCROLL => capture(hwnd, Mode::Scroll),
                TIMER_RECORD => capture(hwnd, Mode::Record),
                TIMER_UPDATE => {
                    if SETTINGS.get().check_updates {
                        start_check(hwnd, Check::Daily);
                    }
                    SetTimer(hwnd, TIMER_UPDATE, UPDATE_CHECK_EVERY_MS, None);
                }
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
    let state = tray::MenuState {
        startup,
        countdown: COUNTDOWN.get(),
        recording: RECORDING.with(|r| r.borrow().is_some()),
        update: AVAILABLE.with(|a| a.borrow().as_ref().map(|r| r.version)),
        installed: update::installed(),
    };
    match tray::show_menu(hwnd, &s, &state) {
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
        tray::CMD_SCROLL => {
            SetTimer(hwnd, TIMER_SCROLL, 250, None);
        }
        tray::CMD_RECORD => {
            SetTimer(hwnd, TIMER_RECORD, 250, None);
        }
        tray::CMD_STOP_RECORD => stop_recording(hwnd),
        tray::CMD_RECORD_GIF => {
            update_settings(|s| s.record_gif = !s.record_gif);
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
        tray::CMD_UPDATE => start_update(hwnd),
        tray::CMD_CHECK_UPDATES => start_check(hwnd, Check::Manual),
        tray::CMD_AUTO_CHECK => {
            update_settings(|s| s.check_updates = !s.check_updates);
        }
        tray::CMD_AUTO_UPDATE => {
            update_settings(|s| s.auto_update = !s.auto_update);
        }
        tray::CMD_EXIT => {
            DestroyWindow(hwnd);
        }
        _ => {}
    }
}

/// Asks GitHub for the latest release on a worker thread; the answer comes back as
/// `WM_APP_UPDATE_CHECKED`.
fn start_check(hwnd: HWND, check: Check) {
    let main = hwnd as isize;
    std::thread::spawn(move || {
        let result = Box::new(update::check());
        unsafe { PostMessageW(main as HWND, WM_APP_UPDATE_CHECKED, check as usize, Box::into_raw(result) as isize) };
    });
}

unsafe fn on_update_checked(hwnd: HWND, check: Check, result: Result<Option<update::Release>, String>) {
    let release = match result {
        Ok(Some(release)) => release,
        Ok(None) => {
            AVAILABLE.with(|a| a.borrow_mut().take());
            if check != Check::Daily {
                let version = update::Version::current();
                notify(hwnd, "RSnip is up to date", &format!("You have the latest version, {version}."));
            }
            return;
        }
        Err(e) => {
            if check != Check::Daily {
                notify(hwnd, "Couldn't check for updates", &e);
            }
            return;
        }
    };
    let version = release.version;
    AVAILABLE.with(|a| *a.borrow_mut() = Some(release));
    let installed = update::installed();
    if check == Check::Install || (check == Check::Daily && SETTINGS.get().auto_update && installed && idle()) {
        return start_update(hwnd);
    }
    // The daily check mentions each new version once; the menu offers it from then on.
    if check == Check::Daily && settings::get("NotifiedVersion") == Some(version.pack()) {
        return;
    }
    settings::set("NotifiedVersion", version.pack());
    let action = if installed { "Click here to install it." } else { "Click here to download it." };
    notify(hwnd, &format!("RSnip {version} is available"), action);
    UPDATE_BALLOON.set(true);
}

/// Nothing is in progress that an automatic update would interrupt.
unsafe fn idle() -> bool {
    let (library, pin) = (wide(gallery::TITLE), wide("RSnip.Pin"));
    !CAPTURING.get()
        && COUNTDOWN.get() == 0
        && RECORDING.with(|r| r.borrow().is_none())
        && FindWindowW(null(), library.as_ptr()).is_null()
        && FindWindowW(pin.as_ptr(), null()).is_null()
}

/// Installs the available update, or opens its download page if this copy wasn't installed.
unsafe fn start_update(hwnd: HWND) {
    let Some(release) = AVAILABLE.with(|a| a.borrow().clone()) else { return };
    if !update::installed() || release.installer.is_none() {
        return shell_open(Path::new(&release.page));
    }
    if UPDATING.replace(true) {
        return;
    }
    notify(hwnd, &format!("Updating to RSnip {}", release.version), "RSnip will restart when the update is installed.");
    let main = hwnd as isize;
    std::thread::spawn(move || {
        let result = update::download(&release).and_then(|path| {
            // Give the library a chance to save its edits before the installer closes it.
            let closed = update::close_library(std::time::Duration::from_secs(120));
            if closed { Ok(path) } else { Err("Close the RSnip library, then try again.".to_owned()) }
        });
        let result = Box::new(result);
        unsafe { PostMessageW(main as HWND, WM_APP_UPDATE_READY, 0, Box::into_raw(result) as isize) };
    });
}

/// Runs the downloaded installer, which closes this instance and starts the new version.
unsafe fn on_update_ready(hwnd: HWND, result: Result<PathBuf, String>) {
    UPDATING.set(false);
    if let Err(e) = result.and_then(|path| update::run_installer(&path)) {
        notify(hwnd, "Couldn't update RSnip", &e);
    }
}

unsafe fn capture(hwnd: HWND, mode: Mode) {
    // While recording, the capture hotkey (or running `--record` again) stops it.
    if RECORDING.with(|r| r.borrow().is_some()) {
        return stop_recording(hwnd);
    }
    if CAPTURING.replace(true) {
        return;
    }
    if mode == Mode::Record {
        let area = pick_area();
        CAPTURING.set(false);
        if let Some(area) = area {
            start_recording(hwnd, area);
        }
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

/// Lets the user pick an area with the overlay, in screen coordinates.
fn pick_area() -> Option<RECT> {
    let shot = capture::capture_screen()?;
    let last = settings::last_region().and_then(|r| shot.to_image_rect(r));
    let (selection, _) = overlay::select(&shot, capture::window_targets(&shot), last)?;
    let (overlay::Selection::Area(r) | overlay::Selection::Window { rect: r, .. }) = selection;
    Some(shot.to_screen_rect(r))
}

unsafe fn start_recording(hwnd: HWND, area: RECT) {
    match record::start(hwnd, area, SETTINGS.get().record_gif) {
        Ok(recording) => {
            RECORDING.with(|r| *r.borrow_mut() = Some(recording));
            let hotkey = SETTINGS.get().hotkey.name();
            tray::set_tip(hwnd, &format!("{APP_NAME} — recording. Press {hotkey} or Stop to finish"));
        }
        Err(e) => notify(hwnd, "Couldn't start recording", &e),
    }
}

/// Stops recording; the file is finished on a worker thread and reported as `WM_APP_RECORDED`.
unsafe fn stop_recording(hwnd: HWND) {
    let Some(recording) = RECORDING.with(|r| r.borrow_mut().take()) else { return };
    tray::set_tip(hwnd, &tip(SETTINGS.get().hotkey));
    let finishing = recording.stop();
    let target = hwnd as isize;
    std::thread::spawn(move || {
        let result = finishing.join().unwrap_or_else(|_| Err("The recording stopped unexpectedly".into()));
        let result = Box::into_raw(Box::new(result));
        if unsafe { PostMessageW(target as HWND, WM_APP_RECORDED, 0, result as LPARAM) } == 0 {
            drop(unsafe { Box::from_raw(result) });
        }
    });
}

unsafe fn on_recorded(hwnd: HWND, result: Result<PathBuf, String>) {
    let path = match result {
        Ok(path) => path,
        Err(e) => return notify(hwnd, "Couldn't save the recording", &e),
    };
    // Copied as a file, so it pastes straight into a chat or email.
    let copied = output::copy_files_to_clipboard(hwnd, std::slice::from_ref(&path)).is_ok();
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    LAST_SAVED.with(|l| *l.borrow_mut() = Some(path));
    let detail = if copied { "copied as a file" } else { "saved" };
    notify(hwnd, "Recording saved", &format!("{name} — {detail}. Click to play it."));
}

fn is_video(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(), Some("mp4" | "gif"))
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
    if mode == Mode::Scroll {
        let (area, window) = match selection {
            overlay::Selection::Area(r) => (r, None),
            overlay::Selection::Window { hwnd, rect, .. } => (rect, Some(hwnd)),
        };
        return scroll::capture(shot.to_screen_rect(area), window).map(|image| (image, false));
    }
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
