//! RSnip: a small open-source Snagit-style screenshot tool.
//!
//! On Windows it's the full app (see `win_main.rs`). Elsewhere it has the screenshot library and
//! editor, `rsnip [file]`, and takes screenshots and recordings from the background icon (see
//! `gallery::background`), but not scrolling captures or pins.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod crash;
mod describe;
mod gallery;
mod http;
mod icon_art;
mod mp4;
mod options;
mod push;
mod secrets;
mod shortcuts;
mod update;

#[cfg(windows)]
include!("win_main.rs");

/// What `rsnip` was asked to do.
#[cfg(not(windows))]
#[derive(Debug, PartialEq, Eq)]
enum Action {
    Settings,
    CapturePanel,
    Tray,
    Record,
    RecordScreen,
    /// A frame of the screen, and what the area is for (see `gallery::pick_area`).
    PickArea(String, Option<String>),
    SystemInfo,
    CheckRecording,
    TrayQuit,
    WriteIcon(String),
    Update,
    Capture(gallery::Capture),
    /// The library, with a file selected if there is one.
    Library(Option<std::path::PathBuf>),
}

/// What `args` (without the program's name) ask for. Flags can come in any order, and ones RSnip
/// doesn't know are ignored. As on Windows, `--delay N` is a region capture in N seconds whichever
/// capture was named, and the first capture named wins.
#[cfg(not(windows))]
fn action(args: &[String]) -> Action {
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let after = |flag: &str, n: usize| args.iter().skip_while(|a| *a != flag).nth(n).cloned();
    for (flag, action) in [
        ("--settings", Action::Settings),
        ("--capture-panel", Action::CapturePanel),
        ("--tray", Action::Tray),
        ("--record", Action::Record),
        ("--record-screen", Action::RecordScreen),
    ] {
        if has(flag) {
            return action;
        }
    }
    if let Some(frame) = after("--pick-area", 1) {
        return Action::PickArea(frame, after("--pick-area", 2));
    }
    for (flag, action) in [("--system-info", Action::SystemInfo), ("--check-recording", Action::CheckRecording), ("--tray-quit", Action::TrayQuit)] {
        if has(flag) {
            return action;
        }
    }
    if let Some(path) = after("--write-icon", 1) {
        return Action::WriteIcon(path);
    }
    if has("--update") {
        return Action::Update;
    }
    let delay = after("--delay", 1).and_then(|secs| secs.parse::<u32>().ok());
    let mode = args.iter().find_map(|a| match a.as_str() {
        "--region" | "--capture" => Some(gallery::CaptureMode::Region),
        "--full" => Some(gallery::CaptureMode::Full),
        "--text" => Some(gallery::CaptureMode::Text),
        "--last" => Some(gallery::CaptureMode::Last),
        _ => None,
    });
    match (delay, mode) {
        (Some(secs), _) => return Action::Capture(gallery::Capture { mode: gallery::CaptureMode::Region, delay: secs.clamp(1, 60) }),
        (None, Some(mode)) => return Action::Capture(gallery::Capture { mode, delay: 0 }),
        (None, None) => {}
    }
    // `--gallery [file]` is the same.
    Action::Library(args.iter().find(|a| !a.starts_with("--")).map(std::path::PathBuf::from))
}

#[cfg(not(windows))]
fn main() {
    crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match action(&args) {
        Action::Settings => gallery::run_settings(),
        Action::CapturePanel => gallery::run_capture_panel(),
        Action::Tray => gallery::run_background(None),
        Action::Record => gallery::record(true),
        Action::RecordScreen => gallery::record(false),
        Action::PickArea(frame, purpose) => gallery::pick_area(std::path::Path::new(&frame), purpose.as_deref()),
        Action::SystemInfo => gallery::print_system_info(),
        Action::CheckRecording => gallery::check_recording(),
        Action::TrayQuit => gallery::quit_background(),
        Action::WriteIcon(path) => {
            if let Err(e) = gallery::write_icon(std::path::Path::new(&path)) {
                eprintln!("Couldn't write the icon: {e}");
                std::process::exit(1);
            }
        }
        Action::Update => gallery::run_update(),
        Action::Capture(capture) => gallery::capture(capture),
        Action::Library(file) => gallery::run(file),
    }
}

#[cfg(all(test, not(windows)))]
mod tests {
    use std::path::PathBuf;

    use super::gallery::{Capture, CaptureMode};
    use super::*;

    fn action(args: &str) -> Action {
        super::action(&args.split_whitespace().map(String::from).collect::<Vec<_>>())
    }

    fn capture(mode: CaptureMode, delay: u32) -> Action {
        Action::Capture(Capture { mode, delay })
    }

    #[test]
    fn opens_the_library_by_default() {
        assert_eq!(action(""), Action::Library(None));
        assert_eq!(action("shot.png"), Action::Library(Some("shot.png".into())));
        assert_eq!(action("--gallery"), Action::Library(None));
        assert_eq!(action("--gallery /home/a/RSnip/shot.png"), Action::Library(Some(PathBuf::from("/home/a/RSnip/shot.png"))));
        assert_eq!(action("--scroll"), Action::Library(None), "unknown flags are ignored");
    }

    #[test]
    fn takes_screenshots() {
        assert_eq!(action("--full"), capture(CaptureMode::Full, 0));
        assert_eq!(action("--region"), capture(CaptureMode::Region, 0));
        assert_eq!(action("--capture"), capture(CaptureMode::Region, 0), "as on Windows");
        assert_eq!(action("--text"), capture(CaptureMode::Text, 0));
        assert_eq!(action("--last"), capture(CaptureMode::Last, 0));
        assert_eq!(action("--text --full"), capture(CaptureMode::Text, 0), "the first one named");
    }

    #[test]
    fn a_delay_means_a_region_in_that_many_seconds() {
        assert_eq!(action("--delay 5"), capture(CaptureMode::Region, 5));
        assert_eq!(action("--full --delay 5"), capture(CaptureMode::Region, 5), "as on Windows");
        assert_eq!(action("--delay 0"), capture(CaptureMode::Region, 1));
        assert_eq!(action("--delay 600"), capture(CaptureMode::Region, 60));
        // Without a number it's not a delay.
        assert_eq!(action("--delay --text"), capture(CaptureMode::Text, 0));
        assert_eq!(action("--delay"), Action::Library(None));
    }

    #[test]
    fn keeps_to_what_it_did_before() {
        assert_eq!(action("--settings"), Action::Settings);
        assert_eq!(action("--settings --system-info"), Action::Settings, "Settings, on System info");
        assert_eq!(action("--settings --recording"), Action::Settings, "Settings, on Recording");
        assert_eq!(action("--settings --updates"), Action::Settings, "Settings, on Updates");
        assert_eq!(action("--capture-panel"), Action::CapturePanel);
        assert_eq!(action("--tray"), Action::Tray);
        assert_eq!(action("--record"), Action::Record);
        assert_eq!(action("--record-screen"), Action::RecordScreen);
        assert_eq!(action("--pick-area /tmp/frame.png"), Action::PickArea("/tmp/frame.png".into(), None));
        assert_eq!(action("--pick-area /tmp/frame.png capture"), Action::PickArea("/tmp/frame.png".into(), Some("capture".into())));
        assert_eq!(action("--pick-area"), Action::Library(None), "no frame");
        assert_eq!(action("--system-info"), Action::SystemInfo);
        assert_eq!(action("--check-recording"), Action::CheckRecording);
        assert_eq!(action("--tray-quit"), Action::TrayQuit);
        assert_eq!(action("--write-icon /tmp/rsnip.png"), Action::WriteIcon("/tmp/rsnip.png".into()));
        assert_eq!(action("--record --full"), Action::Record, "recording was first");
    }

    #[test]
    fn updates() {
        assert_eq!(action("--update"), Action::Update);
        assert_eq!(action("--update --region"), Action::Update, "as on Windows");
    }
}
