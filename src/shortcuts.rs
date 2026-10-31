//! The capture actions that can have a keyboard shortcut, and how their shortcuts are stored.
//!
//! A shortcut is a key and modifiers packed into a number as `hook::Hotkey::pack` does (virtual key
//! in the low 16 bits, Ctrl/Shift/Alt/Win as bits 16–19), and 0 means none. Each is stored as its
//! own setting, e.g. `ShortcutRegion`.

// Only the Windows app has global shortcuts.
#![cfg_attr(not(windows), allow(dead_code))]

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Region,
    Full,
    Last,
    Scroll,
    Record,
    Text,
}

pub const COUNT: usize = 6;
pub type Table = [u32; COUNT];

/// Print Screen, unmodified.
pub const PRINT_SCREEN: u32 = 0x2C;
/// Win+Shift+S, which "Also take over Win+Shift+S" maps to a region capture.
const WIN_SHIFT_S: u32 = 0x53 | (2 | 8) << 16;
/// The single hotkey of RSnip 0.3 and earlier, which always meant a region capture.
const LEGACY_SETTING: &str = "Hotkey";

impl Action {
    pub const ALL: [Action; COUNT] = [Action::Region, Action::Full, Action::Last, Action::Scroll, Action::Record, Action::Text];

    pub fn label(self) -> &'static str {
        match self {
            Action::Region => "Capture region",
            Action::Full => "Capture all screens",
            Action::Last => "Capture last region",
            Action::Scroll => "Scrolling capture",
            Action::Record => "Record screen area",
            Action::Text => "Capture text (OCR)",
        }
    }

    pub fn setting(self) -> &'static str {
        match self {
            Action::Region => "ShortcutRegion",
            Action::Full => "ShortcutFull",
            Action::Last => "ShortcutLast",
            Action::Scroll => "ShortcutScroll",
            Action::Record => "ShortcutRecord",
            Action::Text => "ShortcutText",
        }
    }

    pub fn index(self) -> usize {
        Action::ALL.iter().position(|&a| a == self).unwrap()
    }

    pub fn from_index(i: usize) -> Option<Action> {
        Action::ALL.get(i).copied()
    }
}

pub fn defaults() -> Table {
    [PRINT_SCREEN, 0, 0, 0, 0, 0]
}

/// The stored shortcuts. Before any were stored, the old single hotkey (if set) is the region
/// capture's, and otherwise the defaults apply.
pub fn load(get: impl Fn(&str) -> Option<u32>) -> Table {
    let stored = Action::ALL.map(|a| get(a.setting()));
    if stored.iter().any(Option::is_some) {
        return stored.map(|v| v.map_or(0, valid));
    }
    match get(LEGACY_SETTING).map(valid) {
        Some(key) if key != 0 => [key, 0, 0, 0, 0, 0],
        _ => defaults(),
    }
}

/// A stored value as a shortcut: junk (no key, stray bits) means none.
fn valid(v: u32) -> u32 {
    if v & 0xFFFF == 0 || v >> 20 != 0 {
        0
    } else {
        v
    }
}

/// Whether a key can be a global shortcut: with a modifier, or one of the keys that do nothing on
/// their own (Print Screen, Pause, the F-keys). A bare letter, Space or Enter would be taken from
/// every app.
pub fn usable(key: u32) -> bool {
    let (vk, mods) = (key & 0xFFFF, key >> 16);
    mods != 0 || matches!(vk, PRINT_SCREEN | 0x13 | 0x70..=0x87)
}

/// Gives `key` to `action`, taking it from whichever action had it (returned).
pub fn assign(table: &mut Table, action: Action, key: u32) -> Option<Action> {
    let taken = (key != 0).then(|| table.iter().position(|&k| k == key)).flatten().filter(|&i| i != action.index());
    if let Some(i) = taken {
        table[i] = 0;
    }
    table[action.index()] = key;
    taken.and_then(Action::from_index)
}

pub fn get(table: &Table, action: Action) -> u32 {
    table[action.index()]
}

/// Actions sharing a shortcut (only possible if the settings were edited by hand). The keys may be
/// Windows' (`Table`) or GNOME's (see `gallery::platform::gnome_keys`), with the default as none.
pub fn conflicts<K: PartialEq + Default>(table: &[K; COUNT]) -> Vec<(Action, Action)> {
    let mut out = Vec::new();
    for i in 0..COUNT {
        for j in i + 1..COUNT {
            if table[i] != K::default() && table[i] == table[j] {
                out.push((Action::ALL[i], Action::ALL[j]));
            }
        }
    }
    out
}

/// A shortcut's name without asking Windows (which names keys in the keyboard's language): for
/// systems without the tray app, and tests.
#[cfg_attr(windows, allow(dead_code))]
pub fn fallback_name(key: u32) -> String {
    let (vk, mods) = (key & 0xFFFF, key >> 16);
    let mut parts: Vec<String> = [(1, "Ctrl"), (4, "Alt"), (2, "Shift"), (8, "Win")]
        .into_iter()
        .filter(|&(bit, _)| mods & bit != 0)
        .map(|(_, name)| name.to_string())
        .collect();
    parts.push(match vk {
        PRINT_SCREEN => "PrtScn".into(),
        0x13 => "Pause".into(),
        0x30..=0x39 | 0x41..=0x5A => char::from(vk as u8).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        _ => format!("Key 0x{vk:02X}"),
    });
    parts.join("+")
}

/// The action a key press (packed like a shortcut) triggers; the first one if several share it.
pub fn action_for(key: u32, table: &Table, win_shift_s: bool) -> Option<Action> {
    if key == 0 {
        return None;
    }
    if let Some(i) = table.iter().position(|&k| k == key) {
        return Action::from_index(i);
    }
    (win_shift_s && key == WIN_SHIFT_S).then_some(Action::Region)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHIFT_PRTSCN: u32 = PRINT_SCREEN | 2 << 16;

    #[test]
    fn keys_trigger_their_action_only_with_the_exact_modifiers() {
        let mut table = defaults();
        assign(&mut table, Action::Record, SHIFT_PRTSCN);
        assert_eq!(action_for(PRINT_SCREEN, &table, false), Some(Action::Region));
        assert_eq!(action_for(SHIFT_PRTSCN, &table, false), Some(Action::Record));
        assert_eq!(action_for(PRINT_SCREEN | 1 << 16, &table, false), None, "Ctrl+PrtScn isn't assigned");
        assert_eq!(action_for(0x41, &table, true), None);
        assert_eq!(action_for(0, &table, true), None);
    }

    #[test]
    fn win_shift_s_is_a_region_capture_when_taken_over() {
        let table = [0; COUNT];
        assert_eq!(action_for(WIN_SHIFT_S, &table, true), Some(Action::Region));
        assert_eq!(action_for(WIN_SHIFT_S, &table, false), None);
        // An explicit shortcut wins.
        let mut table = table;
        assign(&mut table, Action::Text, WIN_SHIFT_S);
        assert_eq!(action_for(WIN_SHIFT_S, &table, true), Some(Action::Text));
    }

    #[test]
    fn assigning_a_used_key_moves_it() {
        let mut table = defaults();
        assert_eq!(assign(&mut table, Action::Full, PRINT_SCREEN), Some(Action::Region));
        assert_eq!(get(&table, Action::Full), PRINT_SCREEN);
        assert_eq!(get(&table, Action::Region), 0);
        assert_eq!(assign(&mut table, Action::Full, PRINT_SCREEN), None, "already its own");
        assert_eq!(assign(&mut table, Action::Full, 0), None, "clearing takes nothing");
        assert_eq!(table, [0; COUNT]);
        assert!(conflicts(&table).is_empty());
    }

    #[test]
    fn a_bare_key_is_no_shortcut_unless_it_does_nothing_alone() {
        assert!(usable(PRINT_SCREEN));
        assert!(usable(SHIFT_PRTSCN));
        assert!(usable(0x13), "Pause");
        assert!(usable(0x7B), "F12");
        assert!(usable(0x41 | 1 << 16), "Ctrl+A");
        assert!(usable(0x20 | 8 << 16), "Win+Space");
        assert!(!usable(0x41), "A");
        assert!(!usable(0x20), "Space");
        assert!(!usable(0x0D), "Enter");
        assert!(!usable(0), "none");
    }

    #[test]
    fn conflicts_are_reported() {
        let table = [PRINT_SCREEN, 0, PRINT_SCREEN, 0, SHIFT_PRTSCN, SHIFT_PRTSCN];
        assert_eq!(conflicts(&table), [(Action::Region, Action::Last), (Action::Record, Action::Text)]);
    }

    #[test]
    fn fallback_names() {
        assert_eq!(fallback_name(PRINT_SCREEN), "PrtScn");
        assert_eq!(fallback_name(SHIFT_PRTSCN), "Shift+PrtScn");
        assert_eq!(fallback_name(0x58 | (1 | 2) << 16), "Ctrl+Shift+X");
        assert_eq!(fallback_name(0x7B | 4 << 16), "Alt+F12");
        assert_eq!(fallback_name(0xBA), "Key 0xBA");
    }

    /// Settings holding just `pairs`.
    fn settings<'a>(pairs: &'a [(&'a str, u32)]) -> impl Fn(&str) -> Option<u32> + 'a {
        move |name| pairs.iter().find(|p| p.0 == name).map(|p| p.1)
    }

    #[test]
    fn loads_migrate_the_old_hotkey_and_ignore_junk() {
        assert_eq!(load(settings(&[])), defaults());
        let ctrl_x = 0x58 | 1 << 16;
        assert_eq!(load(settings(&[("Hotkey", ctrl_x)])), [ctrl_x, 0, 0, 0, 0, 0], "the old hotkey was a region capture");
        assert_eq!(load(settings(&[("Hotkey", 0)])), defaults());
        let stored = load(settings(&[("Hotkey", ctrl_x), ("ShortcutRecord", SHIFT_PRTSCN)]));
        assert_eq!(stored, [0, 0, 0, 0, SHIFT_PRTSCN, 0], "once shortcuts are stored, the old hotkey is ignored");
        let junk = load(settings(&[("ShortcutRegion", 0x00F0_0000), ("ShortcutFull", 7 << 20 | 0x41)]));
        assert_eq!(junk, [0; COUNT]);
    }
}
