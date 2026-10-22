//! Notification-area icon, its menu and balloon notifications.

use std::mem::{size_of, zeroed};
use std::ptr::null;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::settings::Settings;
use crate::util::{fill, wide};
use crate::WM_APP_TRAY;

const TRAY_ID: u32 = 1;

// Tray callback events (low word of lparam with NOTIFYICON_VERSION_4).
pub const NIN_SELECT: u32 = WM_USER;
pub const NIN_KEYSELECT: u32 = WM_USER + 1;
pub const NIN_BALLOONUSERCLICK: u32 = WM_USER + 5;

pub const CMD_REGION: u32 = 1;
pub const CMD_FULL: u32 = 2;
pub const CMD_GALLERY: u32 = 3;
pub const CMD_OPEN_FOLDER: u32 = 4;
pub const CMD_HOTKEY: u32 = 5;
pub const CMD_SAVE: u32 = 6;
pub const CMD_NOTIFY: u32 = 7;
pub const CMD_WIN_SHIFT_S: u32 = 8;
pub const CMD_STARTUP: u32 = 9;
pub const CMD_EXIT: u32 = 10;
pub const CMD_OPEN_EDITOR: u32 = 11;
pub const CMD_LIBRARY_ON_LAUNCH: u32 = 12;
pub const CMD_CANCEL_DELAY: u32 = 13;
pub const CMD_TEXT: u32 = 14;
pub const CMD_LAST: u32 = 15;
/// `CMD_DELAY + n` starts a capture in `n` seconds.
pub const CMD_DELAY: u32 = 100;
pub const DELAYS: [u32; 3] = [3, 5, 10];

unsafe fn base(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = zeroed();
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = TRAY_ID;
    nid
}

pub unsafe fn add(hwnd: HWND, icon: HICON, tip: &str) {
    let mut nid = base(hwnd);
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    nid.uCallbackMessage = WM_APP_TRAY;
    nid.hIcon = icon;
    fill(&mut nid.szTip, tip);
    Shell_NotifyIconW(NIM_ADD, &nid);
    nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    Shell_NotifyIconW(NIM_SETVERSION, &nid);
}

pub unsafe fn set_tip(hwnd: HWND, tip: &str) {
    let mut nid = base(hwnd);
    nid.uFlags = NIF_TIP | NIF_SHOWTIP;
    fill(&mut nid.szTip, tip);
    Shell_NotifyIconW(NIM_MODIFY, &nid);
}

pub unsafe fn remove(hwnd: HWND) {
    let nid = base(hwnd);
    Shell_NotifyIconW(NIM_DELETE, &nid);
}

pub unsafe fn notify(hwnd: HWND, icon: HICON, title: &str, text: &str) {
    let mut nid = base(hwnd);
    nid.uFlags = NIF_INFO;
    fill(&mut nid.szInfoTitle, title);
    fill(&mut nid.szInfo, text);
    nid.dwInfoFlags = NIIF_USER | NIIF_LARGE_ICON | NIIF_NOSOUND;
    nid.hBalloonIcon = icon;
    Shell_NotifyIconW(NIM_MODIFY, &nid);
}

/// Shows the context menu at the cursor and returns the chosen command (0 if dismissed).
/// `countdown` is the seconds left on a delayed capture, or 0.
pub unsafe fn show_menu(hwnd: HWND, settings: &Settings, startup: bool, countdown: u32) -> u32 {
    let menu = CreatePopupMenu();
    let item = |id: u32, text: &str, checked: bool| {
        let text = wide(text);
        let flags = MF_STRING | if checked { MF_CHECKED } else { MF_UNCHECKED };
        AppendMenuW(menu, flags, id as usize, text.as_ptr());
    };
    let separator = || {
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
    };

    item(CMD_REGION, &format!("Capture region\t{}", settings.hotkey.name()), false);
    item(CMD_FULL, "Capture all screens", false);
    item(CMD_LAST, "Capture last region", false);
    item(CMD_TEXT, "Capture text (OCR)", false);
    if countdown > 0 {
        item(CMD_CANCEL_DELAY, &format!("Cancel delayed capture ({countdown} s)"), false);
    } else {
        let delays = CreatePopupMenu();
        for secs in DELAYS {
            let text = wide(&format!("In {secs} seconds"));
            AppendMenuW(delays, MF_STRING, (CMD_DELAY + secs) as usize, text.as_ptr());
        }
        let text = wide("Delayed capture");
        AppendMenuW(menu, MF_POPUP, delays as usize, text.as_ptr());
    }
    separator();
    item(CMD_GALLERY, "Open library && editor", false);
    item(CMD_OPEN_FOLDER, "Open screenshots folder", false);
    separator();
    item(CMD_HOTKEY, &format!("Change hotkey…\t{}", settings.hotkey.name()), false);
    item(CMD_WIN_SHIFT_S, "Also take over Win+Shift+S", settings.win_shift_s);
    item(CMD_OPEN_EDITOR, "Open editor after capture", settings.open_editor);
    item(CMD_SAVE, "Save screenshots to folder", settings.save_to_folder);
    item(CMD_NOTIFY, "Show notifications", settings.notify);
    item(CMD_STARTUP, "Start with Windows", startup);
    item(CMD_LIBRARY_ON_LAUNCH, "Open library on launch", settings.open_library_on_launch);
    separator();
    item(CMD_EXIT, "Exit", false);
    SetMenuDefaultItem(menu, CMD_REGION, 0);

    let mut pt = zeroed();
    GetCursorPos(&mut pt);
    // Required so the menu closes when the user clicks elsewhere.
    SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, null());
    PostMessageW(hwnd, WM_NULL, 0, 0);
    DestroyMenu(menu);
    cmd as u32
}
