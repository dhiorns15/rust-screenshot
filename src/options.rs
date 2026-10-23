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
/// Whether the capture overlay starts with its magnifier showing (M toggles it).
pub const SHOW_MAGNIFIER: Flag = Flag { name: "ShowMagnifier", default: true };
/// Hide the library and Settings windows during a capture or recording.
pub const HIDE_WHILE_CAPTURING: Flag = Flag { name: "HideWhileCapturing", default: true };
pub const OPEN_LIBRARY_ON_LAUNCH: Flag = Flag { name: "OpenLibraryOnLaunch", default: false };
pub const RECORD_GIF: Flag = Flag { name: "RecordGif", default: false };
/// Record MP4s as H.265 (HEVC) rather than H.264.
pub const RECORD_HEVC: Flag = Flag { name: "RecordHevc", default: false };
/// Frames per second for MP4 recordings.
pub const RECORD_FPS: Choice = Choice { name: "RecordFps", default: 30, values: &[15, 24, 30, 60] };

/// A setting that holds one of a few numbers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Choice {
    pub name: &'static str,
    pub default: u32,
    pub values: &'static [u32],
}

impl Choice {
    /// Its value among `get`'s settings: the default when unset or not one of `values`.
    pub fn read(self, get: impl Fn(&str) -> Option<u32>) -> u32 {
        get(self.name).filter(|v| self.values.contains(v)).unwrap_or(self.default)
    }
}
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
        assert!(SHOW_MAGNIFIER.read(|_| None), "the magnifier shows unless turned off");
    }

    #[test]
    fn a_choice_falls_back_to_its_default() {
        assert_eq!(RECORD_FPS.read(|_| None), 30);
        assert_eq!(RECORD_FPS.read(|_| Some(60)), 60);
        assert_eq!(RECORD_FPS.read(|_| Some(1000)), 30, "not one of the choices");
    }
}
