//! The Settings window driven headlessly (egui_kittest), with sandboxed settings and a pretend
//! tray app, plus pixel snapshots of its sections on Linux.

use std::path::Path;
use std::time::Duration;

use eframe::egui::Vec2;
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::Harness;

use super::describer::Event;
use super::env::{Env, TrayRequest};
use super::platform::SoundDevice;
use super::settings_window::{Section, SettingsApp};
use super::theme::Appearance;
use super::{tests, Dock};
use crate::crash;
use crate::describe::{Model, Provider};
use crate::options;
use crate::push::{self, Destination, Kind};
use crate::shortcuts::{self, Action};

pub const SIZE: Vec2 = Vec2::new(800.0, 580.0);

/// The window with all sections (as on Windows), after `setup` has prepared the sandbox.
pub fn settings(name: &str, setup: impl FnOnce(&mut Env)) -> Harness<'static, SettingsApp> {
    let mut env = Env::sandbox(tests::sandbox_dir(name));
    env.tray_app = true;
    setup(&mut env);
    let mut h = Harness::builder().with_size(SIZE).build_eframe(move |cc| SettingsApp::new(&cc.egui_ctx, env));
    settle(&mut h);
    h
}

pub fn settle(h: &mut Harness<SettingsApp>) {
    for _ in 0..4 {
        h.step();
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn open(h: &mut Harness<SettingsApp>, section: Section) {
    h.get_by_label_contains(section.title()).click();
    settle(h);
    assert_eq!(h.state().section, section);
}

fn click(h: &mut Harness<SettingsApp>, label: &str) {
    h.get_by_label(label).click();
    settle(h);
}

/// Flips the switch named `label` (the row's text has the same name).
fn toggle(h: &mut Harness<SettingsApp>, label: &str) {
    h.get_by_role_and_label(eframe::egui::accesskit::Role::CheckBox, label).click();
    settle(h);
}

fn is_on(h: &Harness<SettingsApp>, label: &str) -> bool {
    h.get_by_role_and_label(eframe::egui::accesskit::Role::CheckBox, label).accesskit_node().toggled() == Some(eframe::egui::accesskit::Toggled::True)
}

fn requests(h: &Harness<SettingsApp>) -> Vec<TrayRequest> {
    h.state().env.sandboxed().requests.clone()
}

#[test]
fn shows_the_tray_app_sections_only_where_there_is_one() {
    let h = settings("settings-sections", |_| {});
    assert_eq!(h.state().sections().len(), 9);
    let h = settings("settings-sections-linux", |env| env.tray_app = false);
    assert_eq!(h.state().sections(), [Section::General, Section::Capture, Section::Recording, Section::Library, Section::Destinations, Section::Updates, Section::SystemInfo, Section::About]);
    assert!(h.query_by_label("Start with Windows").is_none(), "only the theme in General");
    assert!(h.query_by_label_contains("Shortcuts").is_none());
}

#[test]
fn linux_capture_settings_are_saving_and_the_editor() {
    let mut h = settings("settings-capture-linux", |env| env.tray_app = false);
    open(&mut h, Section::Capture);
    assert!(is_on(&h, "Save screenshots to the folder"));
    assert!(!is_on(&h, "Open the editor after capture"));
    assert!(h.query_by_label("Show the magnifier").is_none() && h.query_by_label("Hide RSnip while capturing").is_none());
}

#[test]
fn switches_save_the_option_and_tell_the_tray_app() {
    let mut h = settings("settings-switches", |_| {});
    assert!(is_on(&h, "Show notifications"), "on by default");
    toggle(&mut h, "Show notifications");
    assert!(!is_on(&h, "Show notifications"));
    assert_eq!(h.state().env.setting(options::NOTIFY.name), Some(0));
    assert_eq!(requests(&h), [TrayRequest::SettingsChanged]);

    toggle(&mut h, "Start with Windows");
    assert!(h.state().env.sandboxed().start_with_windows);

    open(&mut h, Section::Capture);
    toggle(&mut h, "Open the editor after capture");
    assert_eq!(h.state().env.setting(options::OPEN_EDITOR.name), Some(1));
    assert!(is_on(&h, "Show the magnifier"), "on by default");
    toggle(&mut h, "Show the magnifier");
    assert_eq!(h.state().env.setting(options::SHOW_MAGNIFIER.name), Some(0));
    assert!(is_on(&h, "Hide RSnip while capturing"), "on by default");
    toggle(&mut h, "Hide RSnip while capturing");
    assert_eq!(h.state().env.setting(options::HIDE_WHILE_CAPTURING.name), Some(0));
}

#[test]
fn linux_keeps_rsnip_in_the_background_and_starts_it_at_sign_in() {
    let mut h = settings("settings-background", |env| env.tray_app = false);
    assert!(is_on(&h, "Keep RSnip in the background"), "on by default");
    assert!(h.query_by_label_contains("The icon isn't showing").is_none(), "the sandbox's icon is running");
    toggle(&mut h, "Start when you sign in");
    assert!(h.state().env.sandboxed().start_with_windows);

    toggle(&mut h, "Keep RSnip in the background");
    assert_eq!(h.state().env.setting(options::RUN_IN_BACKGROUND.name), Some(0));
    assert_eq!(requests(&h), [TrayRequest::Quit]);
    assert!(h.get_by_role_and_label(eframe::egui::accesskit::Role::CheckBox, "Start when you sign in").accesskit_node().is_disabled());
    toggle(&mut h, "Keep RSnip in the background");
    assert_eq!(h.state().env.sandboxed().trays_started, 1);

    h.state().env.sandboxed().tray_running = false;
    settle(&mut h);
    assert!(h.query_by_label_contains("The icon isn't showing").is_some());
}

#[test]
fn theme_choice_is_saved_and_shown_at_once() {
    let mut h = settings("settings-theme", |_| {});
    click(&mut h, "Light");
    assert_eq!(h.state().env.setting(Appearance::SETTING), Some(Appearance::Light.to_setting()));
    assert_eq!(h.ctx.theme(), eframe::egui::Theme::Light);
    click(&mut h, "Dark");
    assert_eq!(h.ctx.theme(), eframe::egui::Theme::Dark);
}

#[test]
fn shortcuts_are_changed_by_the_tray_app_and_cleared_here() {
    let mut h = settings("settings-shortcuts", |_| {});
    open(&mut h, Section::Shortcuts);
    // One Change button per action, in order.
    h.get_all_by_label("Change").nth(Action::Record.index()).unwrap().click();
    settle(&mut h);
    assert_eq!(requests(&h), [TrayRequest::RecordShortcut(Action::Record)]);

    // Region has Print Screen by default; clearing it stores every action, so it stays cleared.
    h.get_all_by_label("Clear").next().unwrap().click();
    settle(&mut h);
    let table = shortcuts::load(|name| h.state().env.setting(name));
    assert_eq!(table, [0; shortcuts::COUNT]);
    assert_eq!(requests(&h).last(), Some(&TrayRequest::SettingsChanged));
}

#[test]
fn shortcut_problems_are_flagged() {
    let mut h = settings("settings-shortcut-problems", |env| {
        env.set_setting(Action::Region.setting(), shortcuts::PRINT_SCREEN);
        env.set_setting(Action::Full.setting(), shortcuts::PRINT_SCREEN);
        let mut sandbox = env.sandboxed();
        sandbox.snipping_key = true;
        sandbox.tray_running = false;
    });
    open(&mut h, Section::Shortcuts);
    assert!(h.query_by_label_contains("is set for both").is_some(), "the conflict");
    assert!(h.query_by_label_contains("Windows also opens its own screen capture").is_some());
    assert!(h.query_by_label_contains("RSnip isn't running").is_some());
    assert!(h.get_all_by_label("Change").all(|b| b.accesskit_node().is_disabled()), "nothing can record a key");
    h.get_by_label_contains("Start RSnip").click();
    settle(&mut h);
    assert_eq!(h.state().env.sandboxed().trays_started, 1);
}

#[test]
fn recording_format_and_library_position() {
    let mut h = settings("settings-format", |_| {});
    open(&mut h, Section::Recording);
    click(&mut h, "GIF");
    assert_eq!(h.state().env.setting(options::RECORD_GIF.name), Some(1));

    open(&mut h, Section::Library);
    click(&mut h, "Bottom");
    assert_eq!(h.state().env.setting(Dock::SETTING), Some(Dock::Bottom.to_setting()));
    toggle(&mut h, "Drop shadow");
    assert_eq!(h.state().env.setting("ExportShadow"), Some(1));
    assert!(is_on(&h, "Read text in screenshots for search"), "on by default");
    toggle(&mut h, "Read text in screenshots for search");
    assert_eq!(h.state().env.setting(super::search::SETTING), Some(0));
}

#[test]
fn frame_rate_and_codec_are_for_mp4s_and_h265_needs_an_encoder() {
    let mut h = settings("settings-video", |_| {});
    open(&mut h, Section::Recording);
    click(&mut h, "60");
    assert_eq!(h.state().env.setting(options::RECORD_FPS.name), Some(60));
    assert!(h.get_by_label("H.265").accesskit_node().is_disabled(), "no encoder in the sandbox");

    let mut h = settings("settings-video-hevc", |env| env.sandboxed().hevc = true);
    open(&mut h, Section::Recording);
    click(&mut h, "H.265");
    assert_eq!(h.state().env.setting(options::RECORD_HEVC.name), Some(1));
    click(&mut h, "GIF");
    assert!(h.get_by_label("60").accesskit_node().is_disabled(), "GIFs have their own frame rate");
}

#[test]
fn recording_has_a_sound_card_with_the_computer_and_a_microphone() {
    let mut h = settings("settings-sound", |_| {});
    open(&mut h, Section::Recording);
    assert!(is_on(&h, "Computer sound"), "on by default");
    assert!(h.query_by_label_contains("What the computer plays").is_some());
    assert!(!is_on(&h, "Microphone"), "off by default");
    assert!(h.query_by_value("Default microphone").is_none(), "no list until the microphone is on");

    toggle(&mut h, "Computer sound");
    assert_eq!(h.state().env.setting(options::RECORD_SOUND.name), Some(0));
    toggle(&mut h, "Microphone");
    assert_eq!(h.state().env.setting(options::RECORD_MICROPHONE.name), Some(1));
    assert!(h.query_by_value("Default microphone").is_some());
    assert_eq!(requests(&h), [TrayRequest::SettingsChanged; 2]);
}

#[test]
fn the_microphone_list_has_those_connected_and_keeps_one_that_is_gone() {
    let mics = ["USB Microphone", "Laptop array"].map(|name| SoundDevice { id: name.to_lowercase(), name: name.into() });
    let mut h = settings("settings-microphones", |env| {
        env.sandboxed().microphones = mics.to_vec();
        env.set_setting(options::RECORD_MICROPHONE.name, 1);
    });
    open(&mut h, Section::Recording);
    h.get_by_value("Default microphone").click();
    settle(&mut h);
    assert!(h.query_by_label("USB Microphone").is_some() && h.query_by_label("Laptop array").is_some());
    click(&mut h, "USB Microphone");
    assert_eq!(h.state().env.text_setting(options::RECORD_MICROPHONE_DEVICE).as_deref(), Some("usb microphone"));
    assert_eq!(requests(&h), [TrayRequest::SettingsChanged]);

    let mut h = settings("settings-microphone-gone", |env| {
        env.set_setting(options::RECORD_MICROPHONE.name, 1);
        env.set_text_setting(options::RECORD_MICROPHONE_DEVICE, "unplugged");
    });
    open(&mut h, Section::Recording);
    assert!(h.query_by_value("unplugged (not connected)").is_some());
}

#[test]
fn the_computers_sound_can_come_from_a_chosen_output() {
    let outputs = ["Sonar - Media", "Headphones (Arctis Nova Pro Wireless)"].map(|name| SoundDevice { id: name.to_lowercase(), name: name.into() });
    let mut h = settings("settings-outputs", |env| env.sandboxed().outputs = outputs.to_vec());
    open(&mut h, Section::Recording);
    // Computer sound is on from the start, so its list shows.
    h.get_by_value("Default output").click();
    settle(&mut h);
    click(&mut h, "Headphones (Arctis Nova Pro Wireless)");
    assert_eq!(h.state().env.text_setting(options::RECORD_SOUND_DEVICE).as_deref(), Some("headphones (arctis nova pro wireless)"));
    assert_eq!(requests(&h), [TrayRequest::SettingsChanged]);
    assert!(h.query_by_value("Default microphone").is_none(), "the microphone is off");

    let mut h = settings("settings-sound-off", |env| env.set_setting(options::RECORD_SOUND.name, 0));
    open(&mut h, Section::Recording);
    assert!(h.query_by_value("Default output").is_none(), "no list without the switch");
}

#[test]
fn gifs_have_no_sound() {
    let mut h = settings("settings-sound-gif", |env| env.set_setting(options::RECORD_MICROPHONE.name, 1));
    open(&mut h, Section::Recording);
    assert!(h.query_by_label("GIFs have no sound").is_none());
    click(&mut h, "GIF");
    assert!(h.query_by_label("GIFs have no sound").is_some());
    for label in ["Computer sound", "Microphone"] {
        assert!(h.get_by_role_and_label(eframe::egui::accesskit::Role::CheckBox, label).accesskit_node().is_disabled(), "{label}");
    }
    click(&mut h, "MP4");
    assert!(!h.get_by_role_and_label(eframe::egui::accesskit::Role::CheckBox, "Microphone").accesskit_node().is_disabled());
}

#[test]
fn linux_recording_says_whats_missing_and_offers_hiding_the_library() {
    let mut h = settings("settings-recording-linux", |env| env.tray_app = false);
    open(&mut h, Section::Recording);
    assert!(h.query_by_label_contains("rsnip --record").is_some(), "how to start a recording");
    toggle(&mut h, "Hide the library while recording");
    assert_eq!(h.state().env.setting(options::HIDE_WHILE_CAPTURING.name), Some(0));
    assert!(h.query_by_label_contains("Recording needs").is_none(), "nothing missing in the sandbox");

    let problem = "Recording needs GStreamer's tools".to_string();
    let mut h = settings("settings-recording-linux-missing", |env| env.sandboxed().recording_problem = Some(problem));
    open(&mut h, Section::Recording);
    assert!(h.query_by_label_contains("Recording needs GStreamer's tools").is_some());
}

#[test]
fn checks_for_updates_and_offers_the_new_version() {
    let mut h = settings("settings-updates", |_| {});
    open(&mut h, Section::Updates);
    click(&mut h, "Check now");
    assert!(h.query_by_label_contains("You have the latest version").is_some());

    let release = crate::update::Release { version: crate::update::Version::new(99, 0, 0), page: "https://example.com".into(), installer: None, deb: None, rpm: None };
    h.state().env.sandboxed().latest = Some(release);
    click(&mut h, "Check now");
    assert!(h.query_by_label_contains("RSnip 99.0.0 is available").is_some());
    assert!(h.query_by_label("Download").is_some(), "not installed, so it links to the download");
}

#[test]
fn release_candidates_are_opt_in() {
    let mut h = settings("settings-release-candidates", |_| {});
    open(&mut h, Section::Updates);
    assert!(!is_on(&h, "Get release candidates"));
    toggle(&mut h, "Get release candidates");
    assert!(options::RELEASE_CANDIDATES.read(|n| h.state().env.setting(n)));
}

#[test]
fn installs_an_update_and_restarts_into_it() {
    let mut h = settings("settings-install", |env| env.sandboxed().can_install = true);
    open(&mut h, Section::Updates);
    let release = crate::update::Release { version: crate::update::Version::new(99, 0, 0), page: "https://example.com".into(), installer: None, deb: None, rpm: None };
    h.state().env.sandboxed().latest = Some(release);
    click(&mut h, "Check now");
    click(&mut h, "Install");
    assert_eq!(requests(&h), [TrayRequest::InstallUpdate]);
    if cfg!(windows) {
        // The tray app installs it and restarts RSnip.
        assert!(h.query_by_label_contains("Installing; RSnip will restart.").is_some());
    } else {
        // Linux: installed by the package manager, then the library restarts into it.
        assert!(h.query_by_label_contains("Installed. Restart RSnip").is_some());
        click(&mut h, "Restart RSnip");
        assert_eq!(h.state().env.sandboxed().restarts, 1);
    }
}

#[test]
fn chooses_where_screenshots_and_recordings_are_saved() {
    let dir = tests::sandbox_dir("settings-folder");
    let elsewhere = dir.join("Elsewhere");
    let picked = elsewhere.clone();
    let mut env = Env::sandbox(dir);
    env.tray_app = true;
    env.sandboxed().picked_folder = Some(picked);
    let mut h = Harness::builder().with_size(SIZE).build_eframe(move |cc| SettingsApp::new(&cc.egui_ctx, env));
    settle(&mut h);
    open(&mut h, Section::Library);
    assert!(h.query_by_label("~").is_some(), "the sandbox's home, the library's folder");
    click(&mut h, "Change…");
    settle(&mut h);
    assert_eq!(h.state().env.text_setting(options::SAVE_FOLDER), Some(elsewhere.to_string_lossy().into_owned()));
    assert_eq!(h.state().env.dir, elsewhere, "the library follows");
    assert!(h.query_by_label(&format!("~{}Elsewhere", std::path::MAIN_SEPARATOR)).is_some());
    assert!(requests(&h).contains(&TrayRequest::SettingsChanged), "the tray app saves there from now on");
    click(&mut h, "Open");
    assert_eq!(h.state().env.sandboxed().opened, std::slice::from_ref(&elsewhere));

    // Cancelling the chooser changes nothing.
    h.state().env.sandboxed().picked_folder = None;
    click(&mut h, "Change…");
    settle(&mut h);
    assert_eq!(h.state().env.dir, elsewhere);
}

#[test]
fn offers_to_move_whats_in_the_previous_folder() {
    let dir = tests::sandbox_dir("settings-move");
    let elsewhere = dir.join("Elsewhere");
    std::fs::create_dir_all(dir.join(".rsnip")).unwrap();
    for (file, data) in [("a.png", "png"), ("b.mp4", "mp4"), ("notes.txt", "notes"), (".rsnip/a.png.json", "edits"), (".rsnip/a.png.meta.json", "search")] {
        std::fs::write(dir.join(file), data).unwrap();
    }
    // The new folder has a b.mp4 of its own.
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("b.mp4"), "theirs").unwrap();
    let mut env = Env::sandbox(dir.clone());
    env.tray_app = true;
    env.sandboxed().picked_folder = Some(elsewhere.clone());
    let mut h = Harness::builder().with_size(SIZE).build_eframe(move |cc| SettingsApp::new(&cc.egui_ctx, env));
    settle(&mut h);
    open(&mut h, Section::Library);
    assert!(h.query_by_label("Move them here").is_none(), "nothing to move before the folder changes");

    click(&mut h, "Change…");
    settle(&mut h);
    assert!(h.query_by_label_contains("2 screenshots and recordings are still in ~").is_some(), "not notes.txt");
    click(&mut h, "Move them here");
    settle(&mut h);
    assert!(h.query_by_label_contains("Moved 1 screenshot or recording; 1 stayed").is_some());
    assert_eq!(std::fs::read_to_string(elsewhere.join("a.png")).unwrap(), "png");
    assert_eq!(std::fs::read_to_string(elsewhere.join(".rsnip/a.png.json")).unwrap(), "edits", "with its edits");
    assert_eq!(std::fs::read_to_string(elsewhere.join(".rsnip/a.png.meta.json")).unwrap(), "search", "and search data");
    assert_eq!(std::fs::read_to_string(elsewhere.join("b.mp4")).unwrap(), "theirs", "a name that's taken isn't replaced");
    assert!(dir.join("b.mp4").exists() && dir.join("notes.txt").exists() && !dir.join("a.png").exists());

    // What stayed is still offered, until it's left there.
    assert!(h.query_by_label_contains("1 screenshot or recording is still in ~").is_some());
    click(&mut h, "Leave them");
    settle(&mut h);
    assert!(h.query_by_label("Move them here").is_none());
    assert!(dir.join("b.mp4").exists());
}

#[test]
fn brings_in_the_desktops_screenshots_when_theyre_elsewhere() {
    let title = "Bring in the desktop's screenshots";
    let mut h = settings("settings-bring-in", |_| {});
    open(&mut h, Section::Library);
    assert!(h.query_by_label(title).is_none(), "screenshots don't come from the desktop's tool here");

    let mut h = settings("settings-bring-in-linux", |env| {
        let desktop = env.dir.join("Pictures").join("Screenshots");
        env.sandboxed().desktop_screenshots = Some(desktop);
    });
    open(&mut h, Section::Library);
    assert!(is_on(&h, title), "on unless turned off");
    assert!(h.query_by_label_contains("from ~").is_some(), "says where from");
    toggle(&mut h, title);
    assert!(!is_on(&h, title));
    assert_eq!(h.state().env.setting(options::BRING_IN_SCREENSHOTS.name), Some(0));
    toggle(&mut h, title);
    assert!(h.state().env.setting(options::BRING_IN_SINCE).is_some(), "what's taken from now on");
    assert!(requests(&h).contains(&TrayRequest::SettingsChanged));
}

#[test]
fn clean_up_is_off_until_asked_for_and_says_how_many_are_old_and_when_it_last_ran() {
    let title = format!("Move old screenshots and recordings to the {}", super::platform::RECYCLE_BIN);
    let mut h = settings("settings-clean-up", |env| {
        let now = std::time::SystemTime::now();
        for (name, days) in [("a.png", 40), ("b.png", 35), ("c.mp4", 10), ("d.png", 1)] {
            std::fs::write(env.dir.join(name), b"data").unwrap();
            std::fs::File::options().write(true).open(env.dir.join(name)).unwrap().set_modified(now - Duration::from_secs(days * 86_400)).unwrap();
        }
    });
    open(&mut h, Section::Library);
    assert!(!is_on(&h, &title), "off by default");
    assert!(h.query_by_label_contains("Moved ").is_none(), "hasn't run");
    assert!(h.query_by_label("2 are older than 30 days").is_some(), "30 days unless chosen");
    click(&mut h, "7 days");
    assert_eq!(h.state().env.setting(options::CLEAN_UP_DAYS.name), Some(7));
    assert!(h.query_by_label("3 are older than 7 days").is_some());
    click(&mut h, "365 days");
    assert!(h.query_by_label("Nothing is older than 365 days").is_some());
    toggle(&mut h, &title);
    assert!(is_on(&h, &title));
    assert_eq!(h.state().env.setting(options::CLEAN_UP.name), Some(1));
    assert!(std::fs::exists(h.state().env.dir.join("a.png")).unwrap(), "Settings only counts");

    // What an automatic destination hasn't received yet doesn't count.
    let auto = Destination { auto: true, auto_since: Some(1), ..Destination::new(Kind::Immich { server: "https://photos.example.com".into() }) };
    push::save(&h.state().env, &[auto]);
    click(&mut h, "30 days");
    h.get_by_label("Nothing is older than 30 days");

    let two_hours_ago = crate::push::history::now() - 2 * 3600;
    h.state().env.set_text_setting(options::CLEAN_UP_LAST, &format!("{two_hours_ago} 12"));
    settle(&mut h);
    assert!(h.query_by_label(&format!("Moved 12 to the {}, 2 h ago", super::platform::RECYCLE_BIN)).is_some());
}

#[test]
fn system_info_reports_the_machine_and_copies_it() {
    let mut h = settings("settings-system-info", |env| {
        std::fs::write(env.dir.join("a.png"), b"png").unwrap();
        std::fs::write(env.dir.join("notes.txt"), b"not a screenshot").unwrap();
        env.set_setting(Action::Record.setting(), shortcuts::PRINT_SCREEN);
    });
    open(&mut h, Section::SystemInfo);
    for value in ["RSnip 1.2.3", "Example OS 1.0", "Example CPU @ 3.0GHz, 8 threads", "16.0 GB installed, 9.0 GB available", "xdg-open; missing: curl", "not reported"] {
        assert!(h.query_by_label(value).is_some(), "{value}");
    }
    assert!(h.query_by_label_contains("(1 file)").is_some(), "counts only screenshots and recordings");
    assert!(h.query_by_label_contains("record screen area").is_some(), "the shortcuts");

    h.get_by_label_contains("Copy").click();
    settle(&mut h);
    let copied = h.state().env.sandboxed().copied.clone().expect("copied");
    assert!(copied.starts_with("RSnip System Info\n"));
    assert!(copied.contains("\nSystem\n  OS "), "{copied}");
    assert!(copied.contains("~"), "the home folder is hidden: {copied}");
    assert!(h.query_by_label("Copied to the clipboard.").is_some());

    h.state().env.sandboxed().copied = None;
    h.get_by_label_contains("Report a problem").click();
    settle(&mut h);
    assert!(h.state().env.sandboxed().copied.is_some(), "copied too");
    let opened = h.state().env.sandboxed().opened.clone();
    let form = "https://github.com/dhiorns15/rust-screenshot/issues/new?template=bug_report.yml&system-info=RSnip%20System%20Info%0A";
    assert!(opened.len() == 1 && opened[0].to_string_lossy().starts_with(form), "the bug form, filled in: {opened:?}");
    assert!(h.query_by_label("Opened the bug form with System info filled in.").is_some());
    assert!(h.query_by_label("none").is_some(), "no crashes");
    assert!(h.query_by_label_contains("Report this crash").is_none());
}

#[test]
fn the_bug_form_takes_what_fits_in_an_address() {
    use super::settings_window::issue_url;
    let form = "https://github.com/dhiorns15/rust-screenshot/issues/new?template=bug_report.yml";
    assert_eq!(issue_url("A b\n", Some("crash")), format!("{form}&system-info=A%20b%0A&crash=crash"));
    let long = "x".repeat(8000);
    assert_eq!(issue_url("info", Some(&long)), format!("{form}&system-info=info"), "the crash report stays on the clipboard");
    assert_eq!(issue_url(&"é".repeat(2000), None), form, "nor System info, if it's too long (é is 6 characters encoded)");
}

#[test]
fn system_info_shows_the_last_crash_and_reports_it() {
    let mut h = settings("settings-crash", |env| {
        let panic = crash::Panic {
            time: 1_792_764_312,
            part: "library",
            thread: "main".into(),
            message: "attempt to divide by zero".into(),
            location: Some("src/gallery/doc.rs:12:5".into()),
            backtrace: String::new(),
        };
        crash::save(&env.crash_dir, &panic, None).unwrap();
    });
    open(&mut h, Section::SystemInfo);
    assert!(h.query_by_label_contains("RSnip closed unexpectedly (2026-10-23 14:05 UTC)").is_some());
    assert!(h.query_by_label("2026-10-23 14:05 UTC, library: attempt to divide by zero (src/gallery/doc.rs:12:5)").is_some());
    assert_eq!(h.state().env.setting(crash::SEEN_SETTING), Some(1_792_764_312), "seen, so it isn't mentioned again");

    h.get_by_label_contains("Report this crash").click();
    settle(&mut h);
    let copied = h.state().env.sandboxed().copied.clone().expect("copied");
    assert!(copied.starts_with("RSnip crash report\n"), "the crash first: {copied}");
    assert!(copied.contains("\nRSnip System Info\n"), "then the system");
    let opened = h.state().env.sandboxed().opened.clone();
    assert!(opened.len() == 1 && opened[0].to_string_lossy().contains("&crash=RSnip%20crash%20report%0A"), "the crash in its field: {opened:?}");
    assert!(h.query_by_label("Opened the bug form with the crash report and System info filled in.").is_some());

    h.get_by_label_contains("Open folder").click();
    settle(&mut h);
    assert_eq!(h.state().env.sandboxed().opened.last(), Some(&h.state().env.crash_dir));
}

#[test]
fn system_info_values_line_up_across_sections() {
    let mut h = settings("settings-system-info-columns", |_| {});
    open(&mut h, Section::SystemInfo);
    // A short label in the first section, and one beside the longest label, "Windows' Print Screen capture".
    let x = |value: &str| h.get_by_label(value).rect().left();
    assert!((x("RSnip 1.2.3") - x("MP4, H.264, 30 fps")).abs() < 0.5, "Version vs Record as");
    assert!((x("RSnip 1.2.3") - x("Example OS 1.0")).abs() < 0.5);
}

#[test]
fn system_info_leaves_out_the_tray_app_where_there_is_none() {
    let mut h = settings("settings-system-info-linux", |env| env.tray_app = false);
    open(&mut h, Section::SystemInfo);
    assert!(h.query_by_label("Tray app").is_none());
    assert!(h.query_by_label("Shortcuts").is_none());
    assert!(h.query_by_label("Theme").is_some());
}

/// Types `text` into the box labelled `label` (a password or multi-line box too).
fn type_into(h: &mut Harness<SettingsApp>, label: &str, text: &str) {
    use eframe::egui::accesskit::Role;
    let node = [Role::TextInput, Role::PasswordInput, Role::MultilineTextInput].into_iter().find_map(|role| h.query_by_role_and_label(role, label));
    let node = node.unwrap_or_else(|| panic!("no box labelled {label}"));
    node.click();
    node.type_text(text);
    settle(h);
}

fn is_enabled(h: &Harness<SettingsApp>, label: &str) -> bool {
    !h.get_by_label(label).accesskit_node().is_disabled()
}

fn destinations(h: &Harness<SettingsApp>) -> Vec<crate::push::Destination> {
    crate::push::load(&h.state().env)
}

/// Adds a WebDAV destination called "WebDAV Cloud" (typed after the default name) with password "hunter2".
fn add_webdav(h: &mut Harness<SettingsApp>) {
    if h.state().section != Section::Destinations {
        open(h, Section::Destinations);
    }
    click(h, "Add…");
    click(h, "WebDAV");
    type_into(h, "Name", " Cloud");
    type_into(h, "Folder address", "https://dav.example.com/shots/");
    type_into(h, "User", "me");
    type_into(h, "Password", "hunter2");
    click(h, "Save");
}

#[test]
fn push_to_starts_empty() {
    let mut h = settings("settings-push-empty", |_| {});
    open(&mut h, Section::Destinations);
    assert!(h.query_by_label("Nowhere yet. Add a place to push to.").is_some());
    click(&mut h, "Add…");
    for kind in ["SFTP", "WebDAV", "S3-compatible", "SMB share", "FTPS", "Dropbox", "Discord", "Slack", "Immich", "PhotoPrism", "Custom uploader", "Custom uploader from a ShareX file (.sxcu)…"] {
        assert!(h.query_by_label(kind).is_some(), "{kind}");
    }
}

#[test]
fn adds_a_destination_and_keeps_its_secret_out_of_the_settings() {
    let mut h = settings("settings-push-add", |_| {});
    open(&mut h, Section::Destinations);
    click(&mut h, "Add…");
    click(&mut h, "WebDAV");
    assert!(!is_enabled(&h, "Save"), "nothing filled in yet");
    type_into(&mut h, "Name", " Cloud");
    type_into(&mut h, "Folder address", "https://dav.example.com/shots/");
    type_into(&mut h, "User", "me");
    assert!(!is_enabled(&h, "Save"), "a new one needs its password");
    type_into(&mut h, "Password", "hunter2");
    assert!(is_enabled(&h, "Save"));
    click(&mut h, "Save");

    assert!(h.query_by_label("WebDAV Cloud").is_some());
    assert!(h.query_by_label_contains("WebDAV · https://dav.example.com/shots/").is_some());
    let saved = destinations(&h);
    assert_eq!(saved.len(), 1);
    let d = &saved[0];
    assert_eq!((d.name.as_str(), d.auto, d.auto_since), ("WebDAV Cloud", false, None), "typed after the default name");
    let json = h.state().env.text_setting(options::DESTINATIONS).unwrap();
    assert!(!json.contains("hunter2"), "{json}");
    assert_eq!(h.state().env.sandboxed().secrets.get(&d.id).map(String::as_str), Some("hunter2"));
    assert!(requests(&h).contains(&TrayRequest::SettingsChanged));
}

#[test]
fn a_destination_without_a_secret_needs_only_its_fields() {
    let mut h = settings("settings-push-sftp", |_| {});
    open(&mut h, Section::Destinations);
    click(&mut h, "Add…");
    click(&mut h, "SFTP");
    assert!(!is_enabled(&h, "Save"), "needs a server");
    type_into(&mut h, "Server", "nas.local");
    assert!(is_enabled(&h, "Save"), "the user can come from ~/.ssh/config");
    type_into(&mut h, "User", "me");
    assert!(is_enabled(&h, "Save"));
    type_into(&mut h, "Port", "x");
    assert!(!is_enabled(&h, "Save"), "22x isn't a port");
    click(&mut h, "Cancel");
    assert!(destinations(&h).is_empty());
    assert!(h.state().env.sandboxed().secrets.is_empty());
}

#[test]
fn editing_keeps_the_secret_unless_a_new_one_is_typed() {
    let mut h = settings("settings-push-edit", |_| {});
    add_webdav(&mut h);
    let id = destinations(&h)[0].id.clone();

    click(&mut h, "Edit");
    type_into(&mut h, "User", "2");
    click(&mut h, "Save");
    assert_eq!(destinations(&h)[0].kind, crate::push::Kind::WebDav { url: "https://dav.example.com/shots/".into(), user: "me2".into(), public_url: String::new() });
    assert_eq!(destinations(&h)[0].id, id, "the id stays");
    assert_eq!(h.state().env.sandboxed().secrets.get(&id).map(String::as_str), Some("hunter2"), "kept");

    click(&mut h, "Edit");
    type_into(&mut h, "Password", "swordfish");
    click(&mut h, "Save");
    assert_eq!(h.state().env.sandboxed().secrets.get(&id).map(String::as_str), Some("swordfish"), "replaced");
    assert_eq!(destinations(&h).len(), 1);
}

#[test]
fn test_shows_what_was_found_or_what_went_wrong() {
    let mut h = settings("settings-push-test", |_| {});
    add_webdav(&mut h);
    click(&mut h, "Test");
    assert!(h.query_by_label("WebDAV Cloud: Connected").is_some());
    h.state().env.sandboxed().test_result = Some(Err("Wrong password".into()));
    click(&mut h, "Test");
    assert!(h.query_by_label("WebDAV Cloud: Wrong password").is_some());
    assert!(h.query_by_label("WebDAV Cloud: Connected").is_none());
}

#[test]
fn automatically_and_by_hand_are_remembered_with_the_time() {
    let mut h = settings("settings-push-auto", |_| {});
    add_webdav(&mut h);
    click(&mut h, "Automatically");
    let d = destinations(&h).remove(0);
    assert!(d.auto);
    assert!(d.auto_since.is_some_and(|t| t > 1_700_000_000), "{:?}", d.auto_since);
    click(&mut h, "By hand");
    let d = destinations(&h).remove(0);
    assert_eq!((d.auto, d.auto_since), (false, None));
}

#[test]
fn a_sharex_uploader_fills_in_the_form_and_its_token_is_kept_as_the_secret() {
    let mut h = settings("settings-push-sxcu", |_| {});
    let dir = h.state().env.dir.clone();
    let sxcu = dir.join("zipline.sxcu");
    let json = r#"{"Name":"Zipline","RequestURL":"https://z.example.com/api/upload","Headers":{"Authorization":"tok-123"},"Body":"MultipartFormData","FileFormName":"file","URL":"{json:files[0].url}"}"#;
    std::fs::write(&sxcu, json).unwrap();
    h.state().env.sandboxed().picked_file = Some(sxcu);
    open(&mut h, Section::Destinations);
    click(&mut h, "Add…");
    click(&mut h, "Custom uploader from a ShareX file (.sxcu)…");
    assert!(h.query_by_label("Add Custom uploader").is_some(), "the form, filled in");
    assert!(is_enabled(&h, "Save"));
    click(&mut h, "Save");
    let d = destinations(&h).remove(0);
    assert_eq!(d.name, "Zipline");
    assert!(matches!(&d.kind, crate::push::Kind::Custom { headers, .. } if headers == "Authorization: {secret}"));
    assert_eq!(h.state().env.sandboxed().secrets.get(&d.id).map(String::as_str), Some("tok-123"));
    assert!(!h.state().env.text_setting(options::DESTINATIONS).unwrap().contains("tok-123"));

    // One RSnip can't use says why.
    let bad = dir.join("regex.sxcu");
    std::fs::write(&bad, r#"{"RequestURL":"https://x.example.com","FileFormName":"f","URL":"{regex:(.*)|1}"}"#).unwrap();
    h.state().env.sandboxed().picked_file = Some(bad);
    click(&mut h, "Add…");
    click(&mut h, "Custom uploader from a ShareX file (.sxcu)…");
    assert!(h.query_by_label("Couldn't use regex.sxcu: RSnip doesn't fill in {regex}").is_some());
    assert_eq!(destinations(&h).len(), 1);
}

#[test]
fn a_custom_uploader_may_have_no_token() {
    let mut h = settings("settings-push-custom", |_| {});
    open(&mut h, Section::Destinations);
    click(&mut h, "Add…");
    click(&mut h, "Custom uploader");
    type_into(&mut h, "Upload address", "https://i.example.com/upload");
    assert!(is_enabled(&h, "Save"), "no token needed");
    type_into(&mut h, "Headers", "no colon here");
    assert!(!is_enabled(&h, "Save"));
    assert!(h.query_by_label_contains("Each header needs a name, a colon and its value").is_some());
}

#[test]
fn an_ftps_servers_own_certificate_can_be_trusted() {
    let mut h = settings("settings-push-ftps-trust", |_| {});
    h.state().env.sandboxed().certificate = Some(Ok("12:34:AB".into()));
    open(&mut h, Section::Destinations);
    click(&mut h, "Add…");
    click(&mut h, "FTPS");
    assert!(!is_enabled(&h, "Trust the server's certificate…"), "needs the server first");
    type_into(&mut h, "Server", "nas.local");
    type_into(&mut h, "User", "me");
    type_into(&mut h, "Password", "pw");
    click(&mut h, "Trust the server's certificate…");
    assert!(h.query_by_label("12:34:AB").is_some(), "shown to compare with the server's");
    click(&mut h, "Trust it");
    assert!(h.query_by_label("Trusted, as this one:").is_some());
    click(&mut h, "Save");
    assert!(matches!(&destinations(&h)[0].kind, crate::push::Kind::Ftps { trusted, .. } if trusted == "12:34:AB"));

    click(&mut h, "Edit");
    click(&mut h, "Stop trusting it");
    click(&mut h, "Save");
    assert!(matches!(&destinations(&h)[0].kind, crate::push::Kind::Ftps { trusted, .. } if trusted.is_empty()));

    // One that can't be reached says so.
    h.state().env.sandboxed().certificate = Some(Err("Couldn't find that server: check its name".into()));
    click(&mut h, "Edit");
    click(&mut h, "Trust the server's certificate…");
    assert!(h.query_by_label("Couldn't get the server's certificate: Couldn't find that server: check its name").is_some());
}

/// Starts adding a Dropbox place with the app key "abc123".
fn start_dropbox(h: &mut Harness<SettingsApp>) {
    open(h, Section::Destinations);
    click(h, "Add…");
    click(h, "Dropbox");
}

#[test]
fn dropbox_signs_in_in_the_browser_and_keeps_the_token_as_the_secret() {
    let mut h = settings("settings-push-dropbox", |_| {});
    start_dropbox(&mut h);
    assert!(h.query_by_label("Add Dropbox").is_some());
    assert!(h.query_by_label("Password").is_none() && h.query_by_label("Sign-in").is_none(), "no box for the secret");
    assert!(is_on(&h, "Share link"), "links by default");
    assert!(!is_enabled(&h, "Sign in to Dropbox"), "needs the app key first");
    assert!(!is_enabled(&h, "Save"));
    type_into(&mut h, "App key", "abc123");
    type_into(&mut h, "Folder", "2");
    assert!(h.query_by_label_contains("Sign in to Dropbox first").is_some(), "why it can't be saved");
    assert!(h.query_by_label("Code from Dropbox").is_none(), "nothing to paste yet");

    click(&mut h, "Sign in to Dropbox");
    let opened = h.state().env.sandboxed().opened.last().map(|p| p.to_string_lossy().into_owned()).unwrap();
    let query = opened.strip_prefix("https://www.dropbox.com/oauth2/authorize?").unwrap_or_else(|| panic!("{opened}"));
    let query: Vec<&str> = query.split('&').collect();
    for part in ["client_id=abc123", "response_type=code", "code_challenge_method=S256", "token_access_type=offline"] {
        assert!(query.contains(&part), "{part} in {opened}");
    }
    let challenge = query.iter().find_map(|q| q.strip_prefix("code_challenge=")).unwrap();
    assert_eq!(challenge.len(), 43, "a SHA-256 hash in base64url");
    assert!(!is_enabled(&h, "Finish signing in"), "needs the code");
    assert!(!is_enabled(&h, "Save"));

    type_into(&mut h, "Code from Dropbox", " the-code ");
    click(&mut h, "Finish signing in");
    assert!(h.query_by_label_contains("Signed in. Save to keep it").is_some());
    assert!(is_enabled(&h, "Save"));
    assert!(h.state().env.sandboxed().secrets.is_empty(), "kept only once saved");
    click(&mut h, "Save");

    let saved = destinations(&h);
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].kind, crate::push::Kind::Dropbox { app_key: "abc123".into(), folder: "/Screenshots2".into(), share_link: true });
    assert_eq!(h.state().env.sandboxed().secrets.get(&saved[0].id).map(String::as_str), Some("refresh-for-the-code"));
    let json = h.state().env.text_setting(options::DESTINATIONS).unwrap();
    assert!(!json.contains("refresh-for"), "{json}");
    assert!(h.query_by_label_contains("Dropbox · /Screenshots2").is_some());
}

#[test]
fn dropbox_says_when_the_code_is_refused_and_lets_it_be_tried_again() {
    let mut h = settings("settings-push-dropbox-refused", |env| env.sandboxed().dropbox_token = Some(Err("Dropbox didn't accept that code".into())));
    start_dropbox(&mut h);
    type_into(&mut h, "App key", "abc123");
    click(&mut h, "Sign in to Dropbox");
    type_into(&mut h, "Code from Dropbox", "stale");
    click(&mut h, "Finish signing in");
    assert!(h.query_by_label("Dropbox didn't accept that code").is_some());
    assert!(!is_enabled(&h, "Save"));
    h.state().env.sandboxed().dropbox_token = None;
    click(&mut h, "Finish signing in");
    assert!(is_enabled(&h, "Save"));
    assert!(h.query_by_label("Dropbox didn't accept that code").is_none());
}

#[test]
fn editing_dropbox_keeps_the_sign_in_unless_the_app_changes() {
    let mut h = settings("settings-push-dropbox-edit", |_| {});
    start_dropbox(&mut h);
    type_into(&mut h, "App key", "abc123");
    click(&mut h, "Sign in to Dropbox");
    type_into(&mut h, "Code from Dropbox", "c1");
    click(&mut h, "Finish signing in");
    click(&mut h, "Save");
    let id = destinations(&h)[0].id.clone();

    click(&mut h, "Edit");
    assert!(h.query_by_label_contains("Signed in. Sign in again only to use another account").is_some());
    assert!(is_enabled(&h, "Save"), "no need to sign in again");
    toggle(&mut h, "Share link");
    click(&mut h, "Save");
    assert!(matches!(destinations(&h)[0].kind, crate::push::Kind::Dropbox { share_link: false, .. }));
    assert_eq!(h.state().env.sandboxed().secrets.get(&id).map(String::as_str), Some("refresh-for-c1"), "kept");

    // Another app's sign-in isn't this one's.
    click(&mut h, "Edit");
    type_into(&mut h, "App key", "2");
    assert!(!is_enabled(&h, "Save"));
    assert!(h.query_by_label_contains("The app key has changed: sign in to Dropbox again").is_some());
    click(&mut h, "Sign in to Dropbox");
    type_into(&mut h, "Code from Dropbox", "c2");
    click(&mut h, "Finish signing in");
    click(&mut h, "Save");
    assert!(matches!(&destinations(&h)[0].kind, crate::push::Kind::Dropbox { app_key, .. } if app_key == "abc1232"));
    assert_eq!(h.state().env.sandboxed().secrets.get(&id).map(String::as_str), Some("refresh-for-c2"), "replaced");
}

#[test]
fn the_question_mark_opens_how_to_set_it_up() {
    use crate::gallery::theme::icon;
    let docs = "https://github.com/dhiorns15/rust-screenshot/blob/main/docs/destinations.md";
    let mut h = settings("settings-push-help", |_| {});
    add_webdav(&mut h);
    h.get_by_label(icon::QUESTION).click();
    settle(&mut h);
    assert_eq!(h.state().env.sandboxed().opened.last().map(|p| p.to_string_lossy().into_owned()), Some(format!("{docs}#webdav")));

    click(&mut h, "Add…");
    click(&mut h, "S3-compatible");
    let form_help = h.get_all_by_label(icon::QUESTION).last().unwrap();
    form_help.click();
    settle(&mut h);
    assert_eq!(h.state().env.sandboxed().opened.last().map(|p| p.to_string_lossy().into_owned()), Some(format!("{docs}#s3-compatible")));
}

#[test]
fn recent_pushes_say_what_went_and_what_failed() {
    use crate::push::history::{self, Entry};
    let mut h = settings("settings-push-recent", |_| {});
    let file = h.state().env.history_file.clone();
    let d = crate::push::Destination { name: "Photos".into(), ..crate::push::Destination::new(crate::push::Kind::immich()) };
    history::record(&file, &Entry::new(&d, Path::new("/s/a.png"), true, None));
    history::record(&file, &Entry::new(&d, Path::new("/s/b.png"), false, Some("Immich didn't accept the API key".into())));
    open(&mut h, Section::Destinations);
    assert!(h.query_by_label("Recent pushes").is_some());
    assert!(h.query_by_label("a.png to Photos").is_some() && h.query_by_label("Pushed automatically, just now").is_some());
    assert!(h.query_by_label("Failed by hand, just now: Immich didn't accept the API key").is_some());
}

/// A place called "Photos" that `b.png` (in the library's folder, if `exists`) failed to be pushed to.
fn failed_push(env: &mut Env, exists: bool, saved: bool) -> crate::push::Destination {
    use crate::push::history::{self, Entry};
    let d = crate::push::Destination { name: "Photos".into(), ..crate::push::Destination::new(crate::push::Kind::immich()) };
    let file = env.dir.join("b.png");
    if exists {
        std::fs::write(&file, b"png").unwrap();
    }
    if saved {
        crate::push::save(env, std::slice::from_ref(&d));
    }
    history::record(&env.history_file, &Entry::new(&d, &file, true, Some("Immich didn't accept the API key".into())));
    d
}

#[test]
fn a_failed_push_can_be_tried_again() {
    let mut h = settings("settings-push-retry", |env| {
        failed_push(env, true, true);
    });
    open(&mut h, Section::Destinations);
    assert!(is_enabled(&h, "Retry"));
    click(&mut h, "Retry");
    let pushed = h.state().env.sandboxed().pushed.clone();
    assert_eq!(pushed.len(), 1);
    assert_eq!(pushed[0].0, "Photos");
    assert_eq!(pushed[0].1.file_name().unwrap(), "b.png");
    assert!(h.query_by_label("Pushed by hand, just now").is_some(), "a new row");
    assert!(h.query_by_label("Failed automatically, just now: Immich didn't accept the API key").is_some(), "the old one stays");
    assert_eq!(h.state().env.sandboxed().copied.as_deref(), Some("https://example.com/b.png"));
    assert!(h.query_by_label("Pushed b.png to Photos, link copied").is_some());
    assert!(h.query_by_label("Retry").is_none(), "the newest is a success");
}

#[test]
fn a_retry_that_fails_shows_a_new_row_with_its_own_retry() {
    let mut h = settings("settings-push-retry-fails", |env| {
        failed_push(env, true, true);
    });
    h.state().env.sandboxed().push_result = Some(Err("Still no".into()));
    open(&mut h, Section::Destinations);
    click(&mut h, "Retry");
    assert!(h.query_by_label("Failed by hand, just now: Still no").is_some());
    assert_eq!(h.get_all_by_label("Retry").count(), 1, "only on the newest");
    assert!(is_enabled(&h, "Retry"));
}

#[test]
fn a_failed_push_cant_be_retried_without_its_file_or_place() {
    let mut h = settings("settings-push-retry-file", |env| {
        failed_push(env, false, true);
    });
    open(&mut h, Section::Destinations);
    assert!(!is_enabled(&h, "Retry"), "the file is gone");
    click(&mut h, "Retry");
    assert!(h.state().env.sandboxed().pushed.is_empty());

    let mut h = settings("settings-push-retry-place", |env| {
        failed_push(env, true, false);
    });
    open(&mut h, Section::Destinations);
    assert!(!is_enabled(&h, "Retry"), "the place was removed");
    click(&mut h, "Retry");
    assert!(h.state().env.sandboxed().pushed.is_empty());
}

#[test]
fn only_the_newest_push_of_a_file_to_a_place_can_be_retried() {
    use crate::push::history::{self, Entry};
    let mut h = settings("settings-push-retry-older", |env| {
        let d = failed_push(env, true, true);
        let file = env.dir.join("b.png");
        // A newer failure of the same file to the same place, and an older one of another file.
        history::record(&env.history_file, &Entry::new(&d, &file, false, Some("Timed out".into())));
        history::record(&env.history_file, &Entry::new(&d, &env.dir.join("c.png"), false, Some("Too big".into())));
    });
    open(&mut h, Section::Destinations);
    assert_eq!(h.get_all_by_label("Retry").count(), 2, "b.png's second failure, and c.png's");
}

#[test]
fn removing_asks_first_and_forgets_the_secret() {
    let mut h = settings("settings-push-remove", |_| {});
    add_webdav(&mut h);
    click(&mut h, "Remove");
    assert!(h.query_by_label("Remove WebDAV Cloud?").is_some());
    click(&mut h, "Cancel");
    assert_eq!(destinations(&h).len(), 1);
    assert_eq!(h.state().env.sandboxed().secrets.len(), 1);

    click(&mut h, "Remove");
    click(&mut h, "Remove");
    assert!(destinations(&h).is_empty());
    assert!(h.state().env.sandboxed().secrets.is_empty());
    assert!(h.query_by_label("Nowhere yet. Add a place to push to.").is_some());
}

/// Pixel snapshots of the sections (Linux only, like the library's).
#[cfg(target_os = "linux")]
mod snapshots {
    use super::*;

    fn rendered(name: &str, setup: impl FnOnce(&mut Env)) -> Harness<'static, SettingsApp> {
        let mut env = Env::sandbox(tests::sandbox_dir(name));
        env.tray_app = true;
        setup(&mut env);
        let mut h = Harness::builder()
            .with_size(SIZE)
            .with_pixels_per_point(1.0)
            .wgpu()
            .build_eframe(move |cc| SettingsApp::new(&cc.egui_ctx, env));
        settle(&mut h);
        h
    }

    fn snapshot(h: &mut Harness<SettingsApp>, section: Section, name: &str) {
        if h.state().section != section {
            open(h, section);
        }
        h.remove_cursor();
        settle(h);
        h.snapshot(name);
    }

    #[test]
    fn sections() {
        let _gpu = super::super::tests::gpu();
        let mut h = rendered("snap-settings", |env| {
            // As the tray app stores them: every action, here PrtScn for a region and Shift+PrtScn to record.
            let mut table = shortcuts::defaults();
            shortcuts::assign(&mut table, Action::Record, shortcuts::PRINT_SCREEN | 2 << 16);
            for action in Action::ALL {
                env.set_setting(action.setting(), shortcuts::get(&table, action));
            }
        });
        for (section, name) in [
            (Section::General, "settings_general"),
            (Section::Shortcuts, "settings_shortcuts"),
            (Section::Recording, "settings_recording"),
            (Section::Library, "settings_library"),
            (Section::Updates, "settings_updates"),
            (Section::SystemInfo, "settings_system_info"),
            (Section::About, "settings_about"),
        ] {
            snapshot(&mut h, section, name);
        }
    }

    #[test]
    fn push_to() {
        let _gpu = super::super::tests::gpu();
        let mut h = rendered("snap-settings-push", |_| {});
        snapshot(&mut h, Section::Destinations, "settings_push_empty");

        add_webdav(&mut h);
        click(&mut h, "Automatically");
        click(&mut h, "Add…");
        click(&mut h, "Discord");
        type_into(&mut h, "Webhook URL", "https://discord.com/api/webhooks/1/abc");
        click(&mut h, "Save");
        h.state().env.sandboxed().test_result = Some(Err("The server said no".into()));
        h.get_all_by_label("Test").next().unwrap().click();
        settle(&mut h);
        snapshot(&mut h, Section::Destinations, "settings_push_list");

    }

    /// On its own, so the whole form fits in the window.
    #[test]
    fn push_to_form() {
        let _gpu = super::super::tests::gpu();
        let mut h = rendered("snap-settings-push-form", |_| {});
        open(&mut h, Section::Destinations);
        click(&mut h, "Add…");
        click(&mut h, "Immich");
        type_into(&mut h, "Server address", "https://photos.example.com");
        type_into(&mut h, "API key", "secret-key");
        snapshot(&mut h, Section::Destinations, "settings_push_form");
    }

    /// Part way through signing in: the browser's open, and the code is waiting to be pasted.
    #[test]
    fn push_to_dropbox_form() {
        let _gpu = super::super::tests::gpu();
        let mut h = rendered("snap-settings-push-dropbox", |_| {});
        start_dropbox(&mut h);
        type_into(&mut h, "App key", "abc123xyz789");
        click(&mut h, "Sign in to Dropbox");
        snapshot(&mut h, Section::Destinations, "settings_push_dropbox");
    }

    #[test]
    fn push_to_custom_form() {
        let _gpu = super::super::tests::gpu();
        let mut h = rendered("snap-settings-push-custom", |_| {});
        open(&mut h, Section::Destinations);
        click(&mut h, "Add…");
        click(&mut h, "Custom uploader");
        type_into(&mut h, "Upload address", "https://i.example.com/api/upload");
        type_into(&mut h, "Headers", "Authorization: Bearer {secret}");
        type_into(&mut h, "Link", "{json:data.url}");
        snapshot(&mut h, Section::Destinations, "settings_push_custom");
    }

    #[test]
    fn shortcut_problems() {
        let _gpu = super::super::tests::gpu();
        let mut h = rendered("snap-settings-problems", |env| {
            env.set_setting(Action::Full.setting(), shortcuts::PRINT_SCREEN);
            env.sandboxed().snipping_key = true;
        });
        snapshot(&mut h, Section::Shortcuts, "settings_shortcut_problems");
    }

    #[test]
    fn offer_to_move_the_library() {
        let _gpu = super::super::tests::gpu();
        let mut h = rendered("snap-settings-move", |env| {
            let previous = env.dir.join("Pictures").join("Screenshots");
            std::fs::create_dir_all(&previous).unwrap();
            for name in ["a.png", "b.png", "c.mp4"] {
                std::fs::write(previous.join(name), b"").unwrap();
            }
            env.set_text_setting(options::PREVIOUS_SAVE_FOLDER, &previous.to_string_lossy());
        });
        snapshot(&mut h, Section::Library, "settings_move_library");
    }

    /// Renders the Settings window to a PNG, like `render_window` does the library:
    /// `RSNIP_UI="theme=light click=Shortcuts" RSNIP_UI_OUT=settings.png cargo test render_settings -- --ignored`.
    #[test]
    #[ignore]
    fn render_settings() {
        let _gpu = super::super::tests::gpu();
        let spec = std::env::var("RSNIP_UI").unwrap_or_default();
        let out = std::env::var("RSNIP_UI_OUT").unwrap_or_else(|_| "target/settings.png".into());
        let mut appearance = Appearance::Dark;
        let mut clicks = Vec::new();
        for item in spec.split_whitespace() {
            match item.split_once('=') {
                Some(("theme", "light")) => appearance = Appearance::Light,
                Some(("theme", _)) => appearance = Appearance::Dark,
                Some(("click", label)) => clicks.push(label.replace('_', " ")),
                _ => panic!("unknown RSNIP_UI item {item}"),
            }
        }
        let mut h = rendered("render-settings", |env| env.set_setting(Appearance::SETTING, appearance.to_setting()));
        for label in &clicks {
            h.get_by_label_contains(label).click();
            settle(&mut h);
        }
        h.remove_cursor();
        settle(&mut h);
        h.render().expect("render").save(&out).unwrap();
        println!("wrote {out}");
    }

    #[test]
    fn light_theme() {
        let _gpu = super::super::tests::gpu();
        let mut h = rendered("snap-settings-light", |env| env.set_setting(Appearance::SETTING, Appearance::Light.to_setting()));
        snapshot(&mut h, Section::General, "settings_light");
    }
}

#[test]
fn shortcut_keys_line_up_in_a_column() {
    let mut h = settings("settings-columns", |_| {});
    open(&mut h, Section::Shortcuts);
    let lefts: Vec<f32> = h.get_all_by_label("None").map(|n| n.rect().left()).collect();
    assert_eq!(lefts.len(), shortcuts::COUNT - 1, "all but the region capture are unset");
    assert!(lefts.windows(2).all(|w| (w[0] - w[1]).abs() < 0.5), "{lefts:?}");
}

// Settings → Library & editor → Descriptions.

const DESCRIBE_TITLE: &str = "Describe screenshots with AI";

/// `settings`, in a window tall enough to show the whole of the Library & editor section.
fn tall_settings(name: &str, setup: impl FnOnce(&mut Env)) -> Harness<'static, SettingsApp> {
    let mut env = Env::sandbox(tests::sandbox_dir(name));
    env.tray_app = true;
    setup(&mut env);
    let mut h = Harness::builder().with_size(Vec2::new(SIZE.x, 1300.0)).build_eframe(move |cc| SettingsApp::new(&cc.egui_ctx, env));
    settle(&mut h);
    h
}

/// Screenshots in the sandbox's folder, as many as there are names; `described` have descriptions.
fn shots(env: &Env, names: &[&str], described: &[&str]) {
    for name in names {
        std::fs::write(env.dir.join(name), b"data").unwrap();
    }
    for name in described {
        super::search::record_description(&env.dir.join(name), "Done already.", "claude-opus-5-5").unwrap();
    }
}

/// The Descriptions card, on, with a key saved for Anthropic.
fn descriptions_on(name: &str, setup: impl FnOnce(&mut Env)) -> Harness<'static, SettingsApp> {
    let mut h = tall_settings(name, |env| {
        env.set_setting(options::DESCRIBE.name, 1);
        env.sandboxed().secrets.insert("describe-anthropic".into(), "sk-ant-saved".into());
        setup(env);
    });
    open(&mut h, Section::Library);
    settle(&mut h);
    h
}

#[test]
fn descriptions_are_off_until_asked_for_and_turning_them_on_notes_when() {
    let mut h = tall_settings("settings-describe-off", |_| {});
    open(&mut h, Section::Library);
    assert!(!is_on(&h, DESCRIBE_TITLE), "off by default");
    assert!(h.query_by_label_contains("Each new screenshot is sent to Anthropic or OpenAI, with your own API key").is_some());
    assert!(h.query_by_label("Provider").is_none() && h.query_by_label("Model").is_none(), "nothing to set while it's off");
    assert_eq!(h.state().env.setting(options::DESCRIBE_SINCE), None);

    let before = crate::push::history::now();
    toggle(&mut h, DESCRIBE_TITLE);
    assert!(is_on(&h, DESCRIBE_TITLE));
    assert_eq!(h.state().env.setting(options::DESCRIBE.name), Some(1));
    let since = u64::from(h.state().env.setting(options::DESCRIBE_SINCE).unwrap());
    assert!((before..=before + 10).contains(&since), "only what's taken from now on is described by itself");
    assert_eq!(requests(&h), [TrayRequest::SettingsChanged, TrayRequest::SettingsChanged]);
    assert!(h.query_by_label("Provider").is_some() && h.query_by_label_contains("No key is saved").is_some());
    assert!(h.query_by_value("claude-opus-5-5").is_some(), "the default model");
    assert!(h.query_by_label("About $1.39 per 100 screenshots").is_some());

    toggle(&mut h, DESCRIBE_TITLE);
    assert_eq!(h.state().env.setting(options::DESCRIBE.name), Some(0));
    assert!(h.query_by_label("Provider").is_none());
}

#[test]
fn another_provider_has_its_own_default_model_and_cost() {
    let mut h = descriptions_on("settings-describe-provider", |env| env.set_text_setting(options::DESCRIBE_MODEL, "claude-haiku-5-5"));
    h.state().env.set_text_setting(options::DESCRIBE_STATUS, "Old trouble");
    settle(&mut h);
    assert!(h.query_by_label("About $0.03 per 100 screenshots").is_some(), "Haiku's");
    assert!(h.query_by_label_contains("Last problem describing new screenshots: Old trouble").is_some());

    click(&mut h, "OpenAI");
    assert_eq!(h.state().env.setting(options::DESCRIBE_PROVIDER.name), Some(1));
    assert_eq!(h.state().env.text_setting(options::DESCRIBE_MODEL).as_deref(), Some(""), "the other one's models aren't OpenAI's");
    assert_eq!(h.state().env.text_setting(options::DESCRIBE_STATUS).as_deref(), Some(""));
    assert!(h.query_by_value("gpt-6.1-sol").is_some());
    assert!(h.query_by_label("About $0.45 per 100 screenshots").is_some());
    assert!(h.query_by_label_contains("No key is saved. Get one at platform.openai.com/api-keys").is_some(), "a key for each provider");
    assert!(h.query_by_label_contains("Last problem").is_none());

    click(&mut h, "Anthropic");
    assert_eq!(h.state().env.setting(options::DESCRIBE_PROVIDER.name), Some(0));
    assert!(h.query_by_value("claude-opus-5-5").is_some());
    assert!(h.query_by_label_contains("A key is saved").is_some());
}

#[test]
fn the_key_is_kept_in_the_credential_store_and_the_models_are_listed_with_it() {
    let models = vec!["claude-opus-5-5", "claude-sonnet-5-5", "claude-mystery-1"].into_iter().map(|id| Model { id: id.into() }).collect();
    let mut h = tall_settings("settings-describe-key", |env| {
        env.set_setting(options::DESCRIBE.name, 1);
        env.sandboxed().models = Some(Ok(models));
    });
    open(&mut h, Section::Library);
    assert!(!is_enabled(&h, "Save"), "nothing typed yet");
    assert!(!is_enabled(&h, "Test"), "no key to try");
    assert!(!is_enabled(&h, "Describe the library…"));
    assert!(h.query_by_label("Forget").is_none());
    type_into(&mut h, "API key", " sk-ant-123 ");
    click(&mut h, "Save");
    settle(&mut h);
    assert_eq!(h.state().env.sandboxed().secrets.get("describe-anthropic").map(String::as_str), Some("sk-ant-123"));
    assert!(h.query_by_label_contains("A key is saved").is_some() && h.query_by_label("Forget").is_some());
    let kept = h.state().env.text_setting(options::DESTINATIONS).unwrap_or_default();
    assert!(!kept.contains("sk-ant"), "never among the settings");
    assert!(is_enabled(&h, "Test") && is_enabled(&h, "Describe the library…"));
    assert!(h.query_by_label_contains("Loading the models").is_none());

    // The drop-down has what the provider lists, and the choice is remembered.
    h.get_by_value("claude-opus-5-5").click();
    settle(&mut h);
    assert!(h.query_by_label("claude-sonnet-5-5").is_some());
    click(&mut h, "claude-sonnet-5-5");
    assert_eq!(h.state().env.text_setting(options::DESCRIBE_MODEL).as_deref(), Some("claude-sonnet-5-5"));
    assert!(h.query_by_label("About $0.69 per 100 screenshots").is_some());
    h.get_by_value("claude-sonnet-5-5").click();
    settle(&mut h);
    click(&mut h, "claude-mystery-1");
    assert!(h.query_by_label("Cost unknown for this model").is_some(), "no price in the built-in table");

    click(&mut h, "Forget");
    settle(&mut h);
    assert!(h.state().env.sandboxed().secrets.is_empty());
    assert!(h.query_by_label_contains("No key is saved").is_some() && !is_enabled(&h, "Test"));
}

#[test]
fn a_new_key_clears_what_stopped_the_describing() {
    let mut h = descriptions_on("settings-describe-status", |env| env.set_text_setting(options::DESCRIBE_STATUS, "The key wasn't accepted by Anthropic"));
    assert!(h.query_by_label("Last problem describing new screenshots: The key wasn't accepted by Anthropic").is_some());
    type_into(&mut h, "API key", "sk-ant-new");
    click(&mut h, "Save");
    settle(&mut h);
    assert!(h.query_by_label_contains("Last problem").is_none());
    assert_eq!(h.state().env.text_setting(options::DESCRIBE_STATUS).as_deref(), Some(""), "the describing carries on");
}

#[test]
fn models_that_couldnt_be_listed_say_why_and_can_be_listed_again() {
    let mut h = descriptions_on("settings-describe-models", |env| env.sandboxed().models = Some(Err("The key wasn't accepted by Anthropic".into())));
    assert!(h.query_by_label("Couldn't list the models: The key wasn't accepted by Anthropic").is_some());
    h.state().env.sandboxed().models = Some(Ok(vec![Model { id: "claude-haiku-5-5".into() }]));
    click(&mut h, "Try again");
    settle(&mut h);
    assert!(h.query_by_label_contains("Couldn't list").is_none());
    h.get_by_value("claude-opus-5-5").click();
    settle(&mut h);
    assert!(h.query_by_label("claude-haiku-5-5").is_some());
    assert!(h.query_by_label("claude-opus-5-5").is_some(), "the one in use stays chosen even if it isn't listed");
}

#[test]
fn test_describes_a_sample_and_says_how_it_went() {
    let mut h = descriptions_on("settings-describe-test", |_| {});
    click(&mut h, "Test");
    settle(&mut h);
    assert!(h.query_by_label("It works. claude-opus-5-5 said: A sample, described by claude-opus-5-5").is_some());
    h.state().env.sandboxed().sample_description = Some(Err("The key wasn't accepted by Anthropic".into()));
    click(&mut h, "Test");
    settle(&mut h);
    assert!(h.query_by_label("The key wasn't accepted by Anthropic").is_some() && h.query_by_label_contains("It works").is_none());
}

#[test]
fn describing_the_library_says_what_it_comes_to_then_shows_progress_and_stops() {
    let mut h = descriptions_on("settings-describe-library", |env| {
        shots(env, &["Screenshot a.png", "Screenshot b.png", "Screenshot c.png", "Recording d.mp4", "Recording e.gif"], &["Screenshot c.png"]);
    });
    click(&mut h, "Describe the library…");
    assert!(h.query_by_label("2 screenshots, about $0.03 — Describe them?").is_some(), "not the one described, not the recordings");
    assert!(h.state().env.sandboxed().describing.is_none(), "nothing is sent until it's agreed to");
    click(&mut h, "Cancel");
    assert!(h.query_by_label("Describe the library…").is_some() && h.query_by_label_contains("Describe them?").is_none());

    click(&mut h, "Describe the library…");
    click(&mut h, "Describe");
    {
        let sandbox = h.state().env.sandboxed();
        let job = sandbox.describing.as_ref().expect("started");
        assert_eq!((job.provider, job.model.as_str(), job.files.len()), (Provider::Anthropic, "claude-opus-5-5", 2));
    }
    assert!(h.query_by_label("Described 0 of 2").is_some() && h.query_by_label("Stop").is_some());
    let send = |h: &Harness<SettingsApp>, event| h.state().env.sandboxed().describing.as_ref().unwrap().events.send(event).unwrap();
    let first = h.state().env.dir.join("Screenshot b.png");
    send(&h, Event::Described(first));
    settle(&mut h);
    assert!(h.query_by_label("Described 1 of 2").is_some());

    click(&mut h, "Stop");
    assert!(h.state().env.sandboxed().describing.as_ref().unwrap().stop.load(std::sync::atomic::Ordering::Relaxed));
    assert!(!is_enabled(&h, "Stopping…"));
    send(&h, Event::Finished(None));
    settle(&mut h);
    assert!(h.query_by_label("Stopped after 1 of 2").is_some());
    assert!(is_enabled(&h, "Describe the library…"), "it can be run again to carry on");
}

#[test]
fn describing_the_library_says_how_it_ended() {
    let mut h = descriptions_on("settings-describe-library-end", |env| shots(env, &["Screenshot a.png", "Screenshot b.png", "Screenshot c.png"], &[]));
    let run = |h: &mut Harness<SettingsApp>, events: Vec<Event>| {
        click(h, "Describe the library…");
        assert!(h.query_by_label("3 screenshots, about $0.04 — Describe them?").is_some());
        click(h, "Describe");
        for event in events {
            h.state().env.sandboxed().describing.as_ref().unwrap().events.send(event).unwrap();
        }
        settle(h);
    };
    let file = |name: &str| std::path::PathBuf::from(name);
    run(&mut h, vec![Event::Described(file("a")), Event::Skipped(file("b"), "Anthropic declined to describe it".into()), Event::Described(file("c")), Event::Finished(None)]);
    assert!(h.query_by_label("Described 2 screenshots, 1 couldn't be").is_some());
    run(&mut h, vec![Event::Described(file("a")), Event::Finished(Some("Your Anthropic account is out of credit".into()))]);
    assert!(h.query_by_label("Stopped after 1 of 3: Your Anthropic account is out of credit").is_some());
}

#[test]
fn nothing_to_describe_is_said_not_offered() {
    let mut h = descriptions_on("settings-describe-nothing", |env| shots(env, &["Screenshot a.png", "Recording b.mp4"], &["Screenshot a.png"]));
    click(&mut h, "Describe the library…");
    assert!(h.query_by_label("Every screenshot has a description already").is_some());
    assert!(h.query_by_label_contains("Describe them?").is_none());
}

#[test]
fn the_library_estimate_is_unknown_for_a_model_without_a_price() {
    let mut h = descriptions_on("settings-describe-unknown", |env| {
        env.set_text_setting(options::DESCRIBE_MODEL, "claude-mystery-1");
        shots(env, &["Screenshot a.png"], &[]);
    });
    click(&mut h, "Describe the library…");
    assert!(h.query_by_label("1 screenshot, cost unknown for this model — Describe them?").is_some());
}

/// The card, as the Settings window shows it, on Linux.
#[cfg(target_os = "linux")]
mod description_snapshots {
    use super::*;

    #[test]
    fn descriptions() {
        let _gpu = super::super::tests::gpu();
        let mut env = Env::sandbox(tests::sandbox_dir("snap-settings-descriptions"));
        env.tray_app = true;
        env.set_setting(options::DESCRIBE.name, 1);
        env.sandboxed().secrets.insert("describe-anthropic".into(), "sk-ant-saved".into());
        env.sandboxed().models = Some(Ok(["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5", "claude-fable-5-1"].into_iter().map(|id| Model { id: id.into() }).collect()));
        // 340 screenshots, 10 of them described.
        for i in 0..340 {
            let name = format!("Screenshot 2026-10-{:02} {:06}.png", i % 28 + 1, i);
            std::fs::write(env.dir.join(&name), b"data").unwrap();
            if i < 10 {
                super::super::search::record_description(&env.dir.join(&name), "Done.", "claude-opus-5-5").unwrap();
            }
        }
        let mut h = Harness::builder()
            .with_size(Vec2::new(800.0, 1250.0))
            .with_pixels_per_point(1.0)
            .wgpu()
            .build_eframe(move |cc| SettingsApp::new(&cc.egui_ctx, env));
        settle(&mut h);
        open(&mut h, Section::Library);
        settle(&mut h);
        click(&mut h, "Test");
        settle(&mut h);
        click(&mut h, "Describe the library…");
        h.remove_cursor();
        settle(&mut h);
        h.snapshot("settings_descriptions");
    }
}
