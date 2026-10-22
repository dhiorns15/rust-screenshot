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

    fn index(self) -> usize {
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
