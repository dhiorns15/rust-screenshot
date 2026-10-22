//! Preferences persisted under HKCU\Software\RSnip.

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::*;

use crate::hook::Hotkey;
use crate::util::{wide, wide_os};
use crate::APP_NAME;

const KEY: &str = "Software\\RSnip";
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

#[derive(Clone, Copy)]
pub struct Settings {
    pub hotkey: Hotkey,
    pub save_to_folder: bool,
    pub notify: bool,
    pub win_shift_s: bool,
    pub open_editor: bool,
    pub open_library_on_launch: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            hotkey: Hotkey::DEFAULT,
            save_to_folder: true,
            notify: true,
            win_shift_s: true,
            open_editor: false,
            open_library_on_launch: false,
        }
    }
}

impl Settings {
    pub fn load() -> Settings {
        let d = Settings::default();
        let flag = |name, default| read_dword(KEY, name).map_or(default, |v| v != 0);
        let hotkey = read_dword(KEY, "Hotkey").map(Hotkey::unpack).filter(|h| h.vk != 0);
        Settings {
            hotkey: hotkey.unwrap_or(d.hotkey),
            save_to_folder: flag("SaveToFolder", d.save_to_folder),
            notify: flag("Notify", d.notify),
            win_shift_s: flag("WinShiftS", d.win_shift_s),
            open_editor: flag("OpenEditor", d.open_editor),
            open_library_on_launch: flag("OpenLibraryOnLaunch", d.open_library_on_launch),
        }
    }

    pub fn store(&self) {
        write_dword("Hotkey", self.hotkey.pack());
        write_dword("SaveToFolder", self.save_to_folder as u32);
        write_dword("Notify", self.notify as u32);
        write_dword("WinShiftS", self.win_shift_s as u32);
        write_dword("OpenEditor", self.open_editor as u32);
        write_dword("OpenLibraryOnLaunch", self.open_library_on_launch as u32);
    }
}

fn read_dword(subkey: &str, name: &str) -> Option<u32> {
    let (subkey, name) = (wide(subkey), wide(name));
    let mut value = 0u32;
    let mut size = 4u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
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
