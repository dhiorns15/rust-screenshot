//! Low-level keyboard hook. Runs before the shell sees keys, so it can take over
//! Print Screen and Win+Shift+S from the built-in Snipping Tool.

use std::mem::size_of;
use std::ptr::null_mut;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::shortcuts;
use crate::{Mode, WM_APP_CAPTURE, WM_APP_HOTKEY_RECORDED, WM_APP_OVERLAY_KEY};

pub const MOD_CTRL: u32 = 1;
pub const MOD_SHIFT: u32 = 2;
pub const MOD_ALT: u32 = 4;
pub const MOD_WIN: u32 = 8;

/// Keys the capture overlay handles (see `overlay::State::on_key`). They are forwarded to it from
/// the hook so they work even if the overlay didn't get keyboard focus.
const OVERLAY_KEYS: [u32; 12] = [
    VK_ESCAPE as u32,
    VK_RETURN as u32,
    VK_LEFT as u32,
    VK_RIGHT as u32,
    VK_UP as u32,
    VK_DOWN as u32,
    b'A' as u32,
    b'C' as u32,
    b'F' as u32,
    b'M' as u32,
    b'P' as u32,
    b'R' as u32,
];
/// Unassigned virtual key, sent to stop a lone Win/Alt release from opening Start or a menu bar.
const VK_MASK: u16 = 0xE8;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub vk: u32,
    pub mods: u32,
}

impl Hotkey {
    pub fn pack(self) -> u32 {
        self.vk & 0xFFFF | self.mods << 16
    }

    pub fn unpack(v: u32) -> Hotkey {
        Hotkey { vk: v & 0xFFFF, mods: v >> 16 & 0xF }
    }

    pub fn name(self) -> String {
        let mut parts = Vec::new();
        for (bit, label) in [(MOD_CTRL, "Ctrl"), (MOD_ALT, "Alt"), (MOD_SHIFT, "Shift"), (MOD_WIN, "Win")] {
            if self.mods & bit != 0 {
                parts.push(label.to_string());
            }
        }
        parts.push(key_name(self.vk));
        parts.join("+")
    }
}

fn key_name(vk: u32) -> String {
    match vk as u16 {
        VK_SNAPSHOT => return "PrtScn".into(),
        VK_PAUSE => return "Pause".into(),
        VK_CANCEL => return "Break".into(),
        _ => {}
    }
    let extended = matches!(
        vk as u16,
        VK_INSERT | VK_DELETE | VK_HOME | VK_END | VK_PRIOR | VK_NEXT | VK_LEFT | VK_RIGHT | VK_UP
            | VK_DOWN | VK_DIVIDE | VK_NUMLOCK | VK_APPS
    );
    unsafe {
        let scan = MapVirtualKeyW(vk, MAPVK_VK_TO_VSC);
        let lparam = (scan << 16 | if extended { 1 << 24 } else { 0 }) as i32;
        let mut buf = [0u16; 64];
        let len = GetKeyNameTextW(lparam, buf.as_mut_ptr(), buf.len() as i32);
        if len > 0 {
            return String::from_utf16_lossy(&buf[..len as usize]);
        }
    }
    format!("Key 0x{vk:02X}")
}

pub static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
/// Non-zero while the selection overlay is showing.
pub static OVERLAY_HWND: AtomicIsize = AtomicIsize::new(0);
static HOOK: AtomicIsize = AtomicIsize::new(0);
/// Each capture action's shortcut (`shortcuts::Table`), 0 for none.
static SHORTCUTS: [AtomicU32; shortcuts::COUNT] = [const { AtomicU32::new(0) }; shortcuts::COUNT];
static WIN_SHIFT_S: AtomicBool = AtomicBool::new(true);
static RECORDING: AtomicBool = AtomicBool::new(false);
/// Key we acted on whose auto-repeats and release must also be swallowed.
static HELD: AtomicU32 = AtomicU32::new(0);

pub fn set_shortcuts(table: &shortcuts::Table) {
    for (slot, &key) in SHORTCUTS.iter().zip(table) {
        slot.store(key, Relaxed);
    }
}

pub fn set_win_shift_s(enabled: bool) {
    WIN_SHIFT_S.store(enabled, Relaxed);
}

/// While recording, the next key combination is posted to the main window as
/// `WM_APP_HOTKEY_RECORDED` (wparam = packed hotkey, or 0 if cancelled with Esc).
pub fn set_recording(recording: bool) {
    RECORDING.store(recording, Relaxed);
}

pub unsafe fn install(hinstance: HINSTANCE) -> bool {
    let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), hinstance, 0);
    HOOK.store(hook as isize, Relaxed);
    !hook.is_null()
}

pub unsafe fn uninstall() {
    let hook = HOOK.swap(0, Relaxed);
    if hook != 0 {
        UnhookWindowsHookEx(hook as HHOOK);
    }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let kb = &*(lparam as *const KBDLLHOOKSTRUCT);
        let down = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        if kb.flags & LLKHF_INJECTED == 0 && handle(kb.vkCode, down) {
            return 1;
        }
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

fn is_down(vk: u16) -> bool {
    unsafe { GetAsyncKeyState(vk as i32) < 0 }
}

fn current_mods() -> u32 {
    let mut mods = 0;
    if is_down(VK_CONTROL) {
        mods |= MOD_CTRL;
    }
    if is_down(VK_SHIFT) {
        mods |= MOD_SHIFT;
    }
    if is_down(VK_MENU) {
        mods |= MOD_ALT;
    }
    if is_down(VK_LWIN) || is_down(VK_RWIN) {
        mods |= MOD_WIN;
    }
    mods
}

fn is_modifier(vk: u32) -> bool {
    matches!(
        vk as u16,
        VK_SHIFT | VK_LSHIFT | VK_RSHIFT | VK_CONTROL | VK_LCONTROL | VK_RCONTROL | VK_MENU | VK_LMENU
            | VK_RMENU | VK_LWIN | VK_RWIN
    )
}

unsafe fn post(hwnd: isize, msg: u32, wparam: usize) {
    if hwnd != 0 {
        PostMessageW(hwnd as HWND, msg, wparam, 0);
    }
}

unsafe fn mask_menu_key() {
    let mut inputs: [INPUT; 2] = std::mem::zeroed();
    for (input, flags) in inputs.iter_mut().zip([0, KEYEVENTF_KEYUP]) {
        input.r#type = INPUT_KEYBOARD;
        input.Anonymous.ki.wVk = VK_MASK;
        input.Anonymous.ki.dwFlags = flags;
    }
    SendInput(2, inputs.as_ptr(), size_of::<INPUT>() as i32);
}

/// Returns true when the key event should be swallowed.
unsafe fn handle(vk: u32, down: bool) -> bool {
    let held = HELD.load(Relaxed);
    if held != 0 && vk == held {
        if !down {
            HELD.store(0, Relaxed);
        }
        return true;
    }
    if is_modifier(vk) {
        return false;
    }
    let mods = current_mods();
    let main = MAIN_HWND.load(Relaxed);

    // Print Screen sometimes only reports its release, so treat that as a press too.
    let pressed = down || vk == VK_SNAPSHOT as u32;

    if RECORDING.load(Relaxed) {
        if pressed {
            if down {
                HELD.store(vk, Relaxed);
            }
            if mods & (MOD_ALT | MOD_WIN) != 0 {
                mask_menu_key();
            }
            let packed = Hotkey { vk, mods }.pack();
            if vk == VK_ESCAPE as u32 && mods == 0 {
                post(main, WM_APP_HOTKEY_RECORDED, 0);
            } else if shortcuts::usable(packed) {
                post(main, WM_APP_HOTKEY_RECORDED, packed as usize);
            }
            // Otherwise a bare key, which would be taken from every app: wait for a combination.
        }
        return true;
    }

    let table: shortcuts::Table = std::array::from_fn(|i| SHORTCUTS[i].load(Relaxed));

    let overlay = OVERLAY_HWND.load(Relaxed);
    if overlay != 0 {
        if OVERLAY_KEYS.contains(&vk) {
            if down {
                post(overlay, WM_APP_OVERLAY_KEY, vk as usize);
            }
            return true;
        }
        // Keep shortcut keys from reaching other apps while the overlay is up.
        return table.iter().any(|&k| k != 0 && Hotkey::unpack(k).vk == vk);
    }

    let action = shortcuts::action_for(Hotkey { vk, mods }.pack(), &table, WIN_SHIFT_S.load(Relaxed));
    if let (Some(action), true) = (action, pressed) {
        if down {
            HELD.store(vk, Relaxed);
        }
        if mods & (MOD_ALT | MOD_WIN) != 0 {
            mask_menu_key();
        }
        post(main, WM_APP_CAPTURE, Mode::from(action) as usize);
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    proptest::proptest! {
        #[test]
        fn any_stored_hotkey_is_usable(v in proptest::prelude::any::<u32>()) {
            let hotkey = Hotkey::unpack(v);
            proptest::prop_assert!(!hotkey.name().is_empty());
            proptest::prop_assert_eq!(Hotkey::unpack(hotkey.pack()).pack(), hotkey.pack());
        }
    }

    #[test]
    fn hotkey_round_trips() {
        let hk = Hotkey { vk: 0x58, mods: MOD_CTRL | MOD_SHIFT };
        assert!(Hotkey::unpack(hk.pack()) == hk);
        assert_eq!(hk.name(), "Ctrl+Shift+X");
        assert_eq!(Hotkey::unpack(shortcuts::PRINT_SCREEN).name(), "PrtScn");
    }
}
