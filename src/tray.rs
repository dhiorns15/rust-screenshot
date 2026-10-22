//! Notification-area icon, its menu and balloon notifications.

use std::mem::{size_of, zeroed};
use std::ptr::null;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::hook::Hotkey;
use crate::settings::Settings;
use crate::shortcuts::{self, Action};
use crate::update::Version;
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
pub const CMD_SCROLL: u32 = 16;
pub const CMD_RECORD: u32 = 17;
pub const CMD_STOP_RECORD: u32 = 18;
pub const CMD_RECORD_GIF: u32 = 19;
pub const CMD_UPDATE: u32 = 20;
pub const CMD_CHECK_UPDATES: u32 = 21;
pub const CMD_AUTO_CHECK: u32 = 22;
pub const CMD_AUTO_UPDATE: u32 = 23;
pub const CMD_SETTINGS: u32 = 24;
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

/// What the menu shows besides the settings.
pub struct MenuState {
    pub startup: bool,
    /// Seconds left on a delayed capture, or 0.
    pub countdown: u32,
    pub recording: bool,
    /// A newer version that is available.
    pub update: Option<Version>,
    /// Whether this copy was installed (and so can update itself).
    pub installed: bool,
}

/// Shows the context menu at the cursor and returns the chosen command (0 if dismissed).
pub unsafe fn show_menu(hwnd: HWND, settings: &Settings, state: &MenuState) -> u32 {
    let menu = CreatePopupMenu();
    let item = |id: u32, text: &str, checked: bool| {
        let text = wide(text);
        let flags = MF_STRING | if checked { MF_CHECKED } else { MF_UNCHECKED };
        AppendMenuW(menu, flags, id as usize, text.as_ptr());
    };
    let separator = || {
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
    };

    // Each action with its shortcut, if it has one.
    let action = |id: u32, action: Action| {
        let key = shortcuts::get(&settings.shortcuts, action);
        let text = if key == 0 { action.label().to_string() } else { format!("{}\t{}", action.label(), Hotkey::unpack(key).name()) };
        item(id, &text, false);
    };
    action(CMD_REGION, Action::Region);
    action(CMD_FULL, Action::Full);
    action(CMD_LAST, Action::Last);
    action(CMD_SCROLL, Action::Scroll);
    if state.recording {
        item(CMD_STOP_RECORD, "Stop recording", false);
    } else {
        action(CMD_RECORD, Action::Record);
    }
    action(CMD_TEXT, Action::Text);
    if state.countdown > 0 {
        item(CMD_CANCEL_DELAY, &format!("Cancel delayed capture ({} s)", state.countdown), false);
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
    item(CMD_SETTINGS, "Settings…", false);
    separator();
    let region = shortcuts::get(&settings.shortcuts, Action::Region);
    let region = if region == 0 { "none".into() } else { Hotkey::unpack(region).name() };
    item(CMD_HOTKEY, &format!("Change region shortcut…\t{region}"), false);
    item(CMD_WIN_SHIFT_S, "Also take over Win+Shift+S", settings.win_shift_s);
    item(CMD_OPEN_EDITOR, "Open editor after capture", settings.open_editor);
    item(CMD_SAVE, "Save screenshots to folder", settings.save_to_folder);
    item(CMD_RECORD_GIF, "Record as GIF instead of MP4", settings.record_gif);
    item(CMD_NOTIFY, "Show notifications", settings.notify);
    item(CMD_STARTUP, "Start with Windows", state.startup);
    item(CMD_LIBRARY_ON_LAUNCH, "Open library on launch", settings.open_library_on_launch);
    let updates = CreatePopupMenu();
    let sub = |id: u32, text: &str, flags: MENU_ITEM_FLAGS| {
        let text = wide(text);
        AppendMenuW(updates, MF_STRING | flags, id as usize, text.as_ptr());
    };
    let checked = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
    sub(CMD_CHECK_UPDATES, "Check now", 0);
    sub(CMD_AUTO_CHECK, "Check automatically", checked(settings.check_updates));
    let auto_update = checked(settings.auto_update) | if state.installed { 0 } else { MF_GRAYED };
    sub(CMD_AUTO_UPDATE, "Install updates automatically", auto_update);
    let text = wide("Updates");
    AppendMenuW(menu, MF_POPUP, updates as usize, text.as_ptr());
    separator();
    if let Some(version) = state.update {
        let verb = if state.installed { "Update to" } else { "Download" };
        item(CMD_UPDATE, &format!("{verb} RSnip {version}…"), false);
    }
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
