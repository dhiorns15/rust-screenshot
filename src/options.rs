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
/// Keep an icon in the top bar or panel when the library closes (Linux).
pub const RUN_IN_BACKGROUND: Flag = Flag { name: "RunInBackground", default: true };
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
/// The folder screenshots and recordings are saved to, chosen in Settings (a text setting).
pub const SAVE_FOLDER: &str = "SaveFolder";

/// Where screenshots and recordings are saved: the folder chosen in Settings, else `RSnip` in the
/// home folder. Before there was a choice, RSnip saved to the Pictures folder's `Screenshots`
/// (`legacy`); a copy that did keeps that folder, and `remember` makes it the chosen one so the
/// library stays as it was.
pub fn save_folder(chosen: Option<String>, legacy: std::path::PathBuf, home: &std::path::Path, remember: impl FnOnce(&std::path::Path)) -> std::path::PathBuf {
    if let Some(chosen) = chosen.filter(|c| !c.trim().is_empty()) {
        return chosen.into();
    }
    let used_before = std::fs::read_dir(&legacy).is_ok_and(|mut files| files.any(|f| f.is_ok_and(|f| made_by_rsnip(&f.file_name().to_string_lossy()))));
    if used_before {
        remember(&legacy);
        return legacy;
    }
    home.join("RSnip")
}

/// The names RSnip gives what it saves: "Screenshot 2026-10-24 091500.png", "Recording ….mp4".
/// Windows' and GNOME's own screenshots ("Screenshot (3).png", "Screenshot From …") aren't.
fn made_by_rsnip(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("Screenshot ").or_else(|| name.strip_prefix("Recording ")) else { return false };
    let b = rest.as_bytes();
    let digits = |r: std::ops::Range<usize>| b.get(r).is_some_and(|d| d.iter().all(u8::is_ascii_digit));
    digits(0..4) && b.get(4) == Some(&b'-') && digits(5..7) && b.get(7) == Some(&b'-') && digits(8..10) && b.get(10) == Some(&b' ') && digits(11..17)
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

    #[test]
    fn saves_to_the_chosen_folder_else_where_it_did_else_rsnip() {
        let root = std::env::temp_dir().join(format!("rsnip-save-folder-{}", std::process::id()));
        let (home, legacy) = (root.join("home"), root.join("Pictures/Screenshots"));
        std::fs::create_dir_all(&legacy).unwrap();
        let folder = |chosen: Option<&str>| {
            let mut remembered = None;
            let dir = save_folder(chosen.map(String::from), legacy.clone(), &home, |d| remembered = Some(d.to_owned()));
            (dir, remembered)
        };
        // A new user, or one whose folder only has the system's own screenshots.
        std::fs::write(legacy.join("Screenshot (3).png"), b"").unwrap();
        std::fs::write(legacy.join("Screenshot From 2026-10-24 09-15-00.png"), b"").unwrap();
        assert_eq!(folder(None), (home.join("RSnip"), None));
        assert_eq!(folder(Some("/elsewhere")), ("/elsewhere".into(), None));
        // Someone who's been saving there with RSnip keeps it, from now on as their choice.
        std::fs::write(legacy.join("Recording 2026-10-23 220500.mp4"), b"").unwrap();
        assert_eq!(folder(None), (legacy.clone(), Some(legacy.clone())));
        assert_eq!(folder(Some(" ")), (legacy.clone(), Some(legacy.clone())), "blank is unset");
        let _ = std::fs::remove_dir_all(&root);
        assert!(made_by_rsnip("Screenshot 2026-10-24 091500 (2).png"));
        assert!(!made_by_rsnip("Screenshot 2026-10-24.png") && !made_by_rsnip("Recording.mp4"));
    }
}
