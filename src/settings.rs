//! Preferences persisted under HKCU\Software\RSnip.

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{ERROR_SUCCESS, RECT};
use windows_sys::Win32::System::Registry::*;

use crate::options::{self, Flag};
use crate::shortcuts::{self, Action};
use crate::util::{wide, wide_os};
use crate::APP_NAME;

const KEY: &str = "Software\\RSnip";
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

#[derive(Clone, Copy)]
pub struct Settings {
    /// Each capture action's shortcut (see `shortcuts`).
    pub shortcuts: shortcuts::Table,
    pub save_to_folder: bool,
    pub notify: bool,
    pub win_shift_s: bool,
    pub open_editor: bool,
    pub show_magnifier: bool,
    pub hide_while_capturing: bool,
    pub open_library_on_launch: bool,
    pub record_gif: bool,
    pub record_hevc: bool,
    pub record_fps: u32,
    pub check_updates: bool,
    /// Install updates without asking (installed copies only).
    pub auto_update: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            shortcuts: shortcuts::defaults(),
            save_to_folder: options::SAVE_TO_FOLDER.default,
            notify: options::NOTIFY.default,
            win_shift_s: options::WIN_SHIFT_S.default,
            open_editor: options::OPEN_EDITOR.default,
            show_magnifier: options::SHOW_MAGNIFIER.default,
            hide_while_capturing: options::HIDE_WHILE_CAPTURING.default,
            open_library_on_launch: options::OPEN_LIBRARY_ON_LAUNCH.default,
            record_gif: options::RECORD_GIF.default,
            record_hevc: options::RECORD_HEVC.default,
            record_fps: options::RECORD_FPS.default,
            check_updates: options::CHECK_UPDATES.default,
            auto_update: options::AUTO_UPDATE.default,
        }
    }
}

impl Settings {
    pub fn load() -> Settings {
        let flag = |f: Flag| f.read(|name| read_dword(KEY, name));
        Settings {
            shortcuts: shortcuts::load(|name| read_dword(KEY, name)),
            save_to_folder: flag(options::SAVE_TO_FOLDER),
            notify: flag(options::NOTIFY),
            win_shift_s: flag(options::WIN_SHIFT_S),
            open_editor: flag(options::OPEN_EDITOR),
            show_magnifier: flag(options::SHOW_MAGNIFIER),
            hide_while_capturing: flag(options::HIDE_WHILE_CAPTURING),
            open_library_on_launch: flag(options::OPEN_LIBRARY_ON_LAUNCH),
            record_gif: flag(options::RECORD_GIF),
            record_hevc: flag(options::RECORD_HEVC),
            record_fps: options::RECORD_FPS.read(|name| read_dword(KEY, name)),
            check_updates: flag(options::CHECK_UPDATES),
            auto_update: flag(options::AUTO_UPDATE),
        }
    }

    pub fn store(&self) {
        for action in Action::ALL {
            write_dword(action.setting(), shortcuts::get(&self.shortcuts, action));
        }
        let flags = [
            (options::SAVE_TO_FOLDER, self.save_to_folder),
            (options::NOTIFY, self.notify),
            (options::WIN_SHIFT_S, self.win_shift_s),
            (options::OPEN_EDITOR, self.open_editor),
            (options::SHOW_MAGNIFIER, self.show_magnifier),
            (options::HIDE_WHILE_CAPTURING, self.hide_while_capturing),
            (options::OPEN_LIBRARY_ON_LAUNCH, self.open_library_on_launch),
            (options::RECORD_GIF, self.record_gif),
            (options::RECORD_HEVC, self.record_hevc),
            (options::CHECK_UPDATES, self.check_updates),
            (options::AUTO_UPDATE, self.auto_update),
        ];
        for (flag, on) in flags {
            write_dword(flag.name, on as u32);
        }
        write_dword(options::RECORD_FPS.name, self.record_fps);
    }
}

/// Reads one RSnip value. For settings owned by the library window, which mustn't go through
/// `Settings` (the tray's copy would overwrite them on its next `store`).
pub fn get(name: &str) -> Option<u32> {
    read_dword(KEY, name)
}

pub fn set(name: &str, value: u32) {
    write_dword(name, value);
}

const LAST_REGION: [&str; 4] = ["LastRegionLeft", "LastRegionTop", "LastRegionRight", "LastRegionBottom"];

/// The last captured area, in virtual-screen coordinates (which can be negative).
pub fn last_region() -> Option<RECT> {
    let [left, top, right, bottom] = LAST_REGION.map(|name| get(name).map(|v| v as i32));
    let r = RECT { left: left?, top: top?, right: right?, bottom: bottom? };
    (r.right > r.left && r.bottom > r.top).then_some(r)
}

pub fn set_last_region(r: RECT) {
    for (name, v) in LAST_REGION.iter().zip([r.left, r.top, r.right, r.bottom]) {
        set(name, v as u32);
    }
}

pub fn read_dword(subkey: &str, name: &str) -> Option<u32> {
    dword(HKEY_CURRENT_USER, subkey, name)
}

/// A number under HKEY_LOCAL_MACHINE, e.g. about Windows itself.
pub fn read_machine_dword(subkey: &str, name: &str) -> Option<u32> {
    dword(HKEY_LOCAL_MACHINE, subkey, name)
}

/// A text value under HKEY_LOCAL_MACHINE, trimmed; None if it's missing or empty.
pub fn read_machine_string(subkey: &str, name: &str) -> Option<String> {
    let (subkey, name) = (wide(subkey), wide(name));
    let mut buf = [0u16; 256];
    let mut size = std::mem::size_of_val(&buf) as u32;
    let status = unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, subkey.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, null_mut(), buf.as_mut_ptr() as *mut _, &mut size)
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]).trim().to_string()).filter(|s| !s.is_empty())
}

fn dword(root: HKEY, subkey: &str, name: &str) -> Option<u32> {
    let (subkey, name) = (wide(subkey), wide(name));
    let mut value = 0u32;
    let mut size = 4u32;
    let status = unsafe {
        RegGetValueW(
            root,
            subkey.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            &mut value as *mut u32 as *mut _,
            &mut size,
        )
    };
    (status == ERROR_SUCCESS).then_some(value)
}

fn write_value(subkey: &str, name: &str, kind: REG_VALUE_TYPE, data: &[u8]) -> bool {
    let (subkey, name) = (wide(subkey), wide(name));
    unsafe {
        let mut key: HKEY = null_mut();
        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            null(),
            &mut key,
            null_mut(),
        );
        if status != ERROR_SUCCESS {
            return false;
        }
        let ok = RegSetValueExW(key, name.as_ptr(), 0, kind, data.as_ptr(), data.len() as u32) == ERROR_SUCCESS;
        RegCloseKey(key);
        ok
    }
}

fn write_dword(name: &str, value: u32) {
    write_value(KEY, name, REG_DWORD, &value.to_le_bytes());
}

pub fn startup_enabled() -> bool {
    let (subkey, name) = (wide(RUN_KEY), wide(APP_NAME));
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, subkey.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, null_mut(), null_mut(), null_mut())
            == ERROR_SUCCESS
    }
}

pub fn set_startup(enable: bool) -> bool {
    if !enable {
        let (subkey, name) = (wide(RUN_KEY), wide(APP_NAME));
        return unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, subkey.as_ptr(), name.as_ptr()) } == ERROR_SUCCESS;
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    let mut command = std::ffi::OsString::from("\"");
    command.push(exe.as_os_str());
    command.push("\"");
    let command = wide_os(&command);
    let bytes = unsafe { std::slice::from_raw_parts(command.as_ptr() as *const u8, command.len() * 2) };
    write_value(RUN_KEY, APP_NAME, REG_SZ, bytes)
}

/// Keeps the Run entry pointing at this exe if it has been moved or rebuilt elsewhere.
pub fn refresh_startup_path() {
    if startup_enabled() {
        set_startup(true);
    }
}
