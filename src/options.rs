//! The on/off options shared by the tray app and the Settings window: how each is stored, and its
//! default. Each is a setting holding 0 or 1.

// Most of these only exist in the Windows app.
#![cfg_attr(not(windows), allow(dead_code))]

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Flag {
    pub name: &'static str,
    pub default: bool,
}

impl Flag {
    /// Its value among `get`'s settings: anything but 0 is on, and unset means the default.
    pub fn read(self, get: impl Fn(&str) -> Option<u32>) -> bool {
        get(self.name).map_or(self.default, |v| v != 0)
    }
}

pub const SAVE_TO_FOLDER: Flag = Flag { name: "SaveToFolder", default: true };
pub const NOTIFY: Flag = Flag { name: "Notify", default: true };
pub const WIN_SHIFT_S: Flag = Flag { name: "WinShiftS", default: true };
pub const OPEN_EDITOR: Flag = Flag { name: "OpenEditor", default: false };
pub const OPEN_LIBRARY_ON_LAUNCH: Flag = Flag { name: "OpenLibraryOnLaunch", default: false };
pub const RECORD_GIF: Flag = Flag { name: "RecordGif", default: false };
pub const CHECK_UPDATES: Flag = Flag { name: "CheckUpdates", default: true };
pub const AUTO_UPDATE: Flag = Flag { name: "AutoUpdate", default: false };

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_means_default_and_anything_else_is_on() {
        assert!(NOTIFY.read(|_| None));
        assert!(!OPEN_EDITOR.read(|_| None));
        assert!(OPEN_EDITOR.read(|_| Some(7)));
        assert!(!NOTIFY.read(|_| Some(0)));
    }
}
