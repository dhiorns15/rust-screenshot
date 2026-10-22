//! RSnip: a small open-source Snagit-style screenshot tool for Windows.
//!
//! `rsnip`            runs in the tray and takes over Print Screen (and optionally Win+Shift+S).
//! `rsnip --region`   starts a region capture (in the running instance if there is one).
//! `rsnip --full`     captures every monitor.
//! `rsnip --gallery [file]` opens the screenshot library / editor.

#![windows_subsystem = "windows"]

mod capture;
mod gallery;
mod hook;
mod icon;
mod output;
mod overlay;
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

const TIMER_REGION: usize = 1;
const TIMER_FULL: usize = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Region = 0,
    Full = 1,
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
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--gallery") {
        gallery::run(args.get(1).map(PathBuf::from));
        return;
    }
    let mode = args.iter().find_map(|a| match a.as_str() {
        "--region" | "--capture" => Some(Mode::Region),
        "--full" => Some(Mode::Full),
        _ => None,
    });
    unsafe { run_tray(mode) }
}

fn tip(hotkey: Hotkey) -> String {
    format!("{APP_NAME} — press {} to capture", hotkey.name())
}

unsafe fn run_tray(mode: Option<Mode>) {
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

    let class = wide(MAIN_CLASS);
    let mutex_name = wide("Local\\RSnip.Tray");
    let _mutex = CreateMutexW(null(), 0, mutex_name.as_ptr());
    if GetLastError() == ERROR_ALREADY_EXISTS {
        let other = FindWindowW(class.as_ptr(), null());
        if !other.is_null() {
            match mode {
                Some(m) => PostMessageW(other, WM_APP_CAPTURE, m as usize, 0),
                None => PostMessageW(other, WM_APP_ALREADY_RUNNING, 0, 0),
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

    match mode {
        Some(m) => {
            PostMessageW(hwnd, WM_APP_CAPTURE, m as usize, 0);
        }
        None if settings.open_library_on_launch => open_gallery(None),
        None => {}
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

pub unsafe fn shell_open(path: &Path) {
    let verb = wide("open");
    let file = wide_os(path.as_os_str());
    ShellExecuteW(null_mut(), verb.as_ptr(), file.as_ptr(), null(), null(), SW_SHOWNORMAL);
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
        WM_TIMER => {
            KillTimer(hwnd, wparam);
            match wparam {
                TIMER_REGION => capture(hwnd, Mode::Region),
                TIMER_FULL => capture(hwnd, Mode::Full),
                _ => {}
            }
            0
        }
        WM_APP_CAPTURE => {
            capture(hwnd, if wparam == Mode::Full as usize { Mode::Full } else { Mode::Region });
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
    match tray::show_menu(hwnd, &s, startup) {
        // Give the menu time to fade out so it isn't in the capture.
        tray::CMD_REGION => {
            SetTimer(hwnd, TIMER_REGION, 250, None);
        }
        tray::CMD_FULL => {
            SetTimer(hwnd, TIMER_FULL, 250, None);
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
    if let Some(image) = image {
        deliver(hwnd, image);
    }
}

fn take(mode: Mode) -> Option<Image> {
    let shot = capture::capture_screen()?;
    match mode {
        Mode::Full => Some(shot.image),
        Mode::Region => {
            let targets = capture::window_targets(&shot);
            match overlay::select(&shot, targets)? {
                overlay::Selection::Area(r) | overlay::Selection::Window { rect: r, occluded: false, .. } => {
                    Some(shot.image.crop(r))
                }
                // Something was on top of the window: raise it and capture it again cleanly.
                overlay::Selection::Window { hwnd, rect, occluded: true } => {
                    capture::capture_window_in_front(hwnd).or_else(|| Some(shot.image.crop(rect)))
                }
            }
        }
    }
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
