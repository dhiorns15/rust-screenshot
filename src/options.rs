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
/// Move new screenshots from the desktop's screenshot tool into RSnip's folder (Linux; see
/// `gallery::bring_in`), and since when (seconds since 1970).
pub const BRING_IN_SCREENSHOTS: Flag = Flag { name: "BringInScreenshots", default: true };
pub const BRING_IN_SINCE: &str = "BringInSince";
pub const OPEN_LIBRARY_ON_LAUNCH: Flag = Flag { name: "OpenLibraryOnLaunch", default: false };
pub const RECORD_GIF: Flag = Flag { name: "RecordGif", default: false };
/// Record MP4s as H.265 (HEVC) rather than H.264.
pub const RECORD_HEVC: Flag = Flag { name: "RecordHevc", default: false };
/// Frames per second for MP4 recordings.
pub const RECORD_FPS: Choice = Choice { name: "RecordFps", default: 30, values: &[15, 24, 30, 60] };
/// Record what the computer plays (MP4s; GIFs have no sound).
pub const RECORD_SOUND: Flag = Flag { name: "RecordSound", default: true };
/// Record the microphone too, mixed in with the computer's sound.
pub const RECORD_MICROPHONE: Flag = Flag { name: "RecordMicrophone", default: false };
/// Which microphone, by the id its platform gives it (a text setting); empty means the default one.
pub const RECORD_MICROPHONE_DEVICE: &str = "RecordMicrophoneDevice";
/// Which output the computer's sound is recorded from, by its id (a text setting); empty means the
/// default one. With a mixer such as SteelSeries Sonar, the real headphones have every app's sound.
pub const RECORD_SOUND_DEVICE: &str = "RecordSoundDevice";
/// Show the camera in a round window on top while recording (see `gallery::camera`), so it's in
/// the recording wherever it's put.
pub const RECORD_CAMERA: Flag = Flag { name: "RecordCamera", default: false };
/// Which camera, by the id its platform gives it (a text setting); empty means the first one.
pub const RECORD_CAMERA_DEVICE: &str = "RecordCameraDevice";
/// How big the camera's bubble is: small, medium or large, as an index (the bubble remembers it).
pub const RECORD_CAMERA_SIZE: Choice = Choice { name: "RecordCameraSize", default: 1, values: &[0, 1, 2] };
/// What the capture panel (by the tray icon) shows and does, as the index of each choice (see
/// `gallery::capture_panel`): its tab, and what the Capture button captures or records.
pub const CAPTURE_PANEL_TAB: Choice = Choice { name: "CapturePanelTab", default: 0, values: &[0, 1] };
pub const CAPTURE_PANEL_IMAGE: Choice = Choice { name: "CapturePanelImage", default: 0, values: &[0, 1, 2, 3, 4] };
pub const CAPTURE_PANEL_VIDEO: Choice = Choice { name: "CapturePanelVideo", default: 0, values: &[0, 1] };
/// The capture panel stays open when another window is clicked (its × or Esc closes it).
pub const CAPTURE_PANEL_KEEP_OPEN: Flag = Flag { name: "CapturePanelKeepOpen", default: false };
/// Where the capture panel was last moved to: its top left in pixels, as "x,y" (a text setting).
pub const CAPTURE_PANEL_PLACE: &str = "CapturePanelPlace";
/// The action Print Screen runs when Settings → Shortcuts takes it from GNOME's screenshot tool
/// (Linux), as its `shortcuts::Action::index`; Capture region by default.
pub const PRINT_SCREEN_ACTION: Choice = Choice { name: "PrintScreenAction", default: 0, values: &[0, 1, 2, 3, 4, 5] };
/// GNOME's screenshot tool's keys from before RSnip took Print Screen, to give back when it stops
/// (a text setting, as GVariant text like `['Print']`); empty when there's nothing to give back.
pub const GNOME_SCREENSHOT_KEYS: &str = "GnomeScreenshotKeys";
/// The library's player is muted.
pub const PLAYER_MUTED: Flag = Flag { name: "PlayerMuted", default: false };

/// Move screenshots and recordings that haven't changed for `CLEAN_UP_DAYS` days to the Recycle
/// Bin (the Trash on Linux), unless an automatic push still has to get them (see
/// `gallery::clean_up`). Owned by the library window, so not in the tray's `Settings`.
pub const CLEAN_UP: Flag = Flag { name: "CleanUp", default: false };
pub const CLEAN_UP_DAYS: Choice = Choice { name: "CleanUpDays", default: 30, values: &[7, 30, 90, 365] };
/// When that last moved any, and how many: seconds since 1970, a space, the count (a text setting).
pub const CLEAN_UP_LAST: &str = "CleanUpLast";

/// Describe each new screenshot with an AI model, with the user's own key, to search by what's in
/// it (see `describe` and `gallery::describer`). Off unless asked for.
pub const DESCRIBE: Flag = Flag { name: "Describe", default: false };
/// Which provider: 0 Anthropic, 1 OpenAI (see `describe::Provider`).
pub const DESCRIBE_PROVIDER: Choice = Choice { name: "DescribeProvider", default: 0, values: &[0, 1] };
/// Which of its models, by id (a text setting); empty means the provider's default one.
pub const DESCRIBE_MODEL: &str = "DescribeModel";
/// When Describe was turned on (seconds since 1970), so only screenshots taken since are described
/// by themselves.
pub const DESCRIBE_SINCE: &str = "DescribeSince";
/// What stopped the last description, if something did (a text setting): the process that
/// describes writes it, and Settings shows it. Empty when all is well.
pub const DESCRIBE_STATUS: &str = "DescribeStatus";

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
/// The folder they were saved to before it was last changed in Settings, so what's still there
/// can be moved to the new one; the new one again once that's done or declined.
pub const PREVIOUS_SAVE_FOLDER: &str = "PreviousSaveFolder";
/// The places to push screenshots and recordings to, as a JSON list (a text setting); see `push`.
/// Their secrets are in the system's credential store, never here.
pub const DESTINATIONS: &str = "Destinations";

/// Where screenshots and recordings are saved: the folder chosen in Settings, else `RSnip` in the
/// home folder. Before there was a choice, RSnip saved to the Pictures folder's `Screenshots`
/// (`legacy`); a copy that did keeps that folder, and `remember` makes it the chosen one so the
/// library stays as it was.
pub fn save_folder(chosen: Option<String>, legacy: std::path::PathBuf, home: &std::path::Path, remember: impl FnOnce(&std::path::Path)) -> std::path::PathBuf {
    if let Some(chosen) = chosen.filter(|c| !c.trim().is_empty()) {
        return chosen.into();
    }
    if saved_to_by_rsnip(&legacy) {
        remember(&legacy);
        return legacy;
    }
    home.join("RSnip")
}

/// Whether RSnip has saved screenshots or recordings in `dir`.
pub fn saved_to_by_rsnip(dir: &std::path::Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut files| files.any(|f| f.is_ok_and(|f| made_by_rsnip(&f.file_name().to_string_lossy()))))
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
/// Offer release candidates too: versions to try before they're released (see `update::check`).
pub const RELEASE_CANDIDATES: Flag = Flag { name: "ReleaseCandidates", default: false };
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
        assert!(!CLEAN_UP.read(|_| None), "nothing is deleted unless asked for");
    }

    #[test]
    fn a_choice_falls_back_to_its_default() {
        assert_eq!(RECORD_FPS.read(|_| None), 30);
        assert_eq!(RECORD_FPS.read(|_| Some(60)), 60);
        assert_eq!(RECORD_FPS.read(|_| Some(1000)), 30, "not one of the choices");
        assert_eq!(CLEAN_UP_DAYS.read(|_| None), 30);
        assert_eq!(CLEAN_UP_DAYS.read(|_| Some(90)), 90);
        assert_eq!(CLEAN_UP_DAYS.read(|_| Some(0)), 30, "not one of the choices");
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
