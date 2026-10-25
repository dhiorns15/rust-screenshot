//! The Settings window driven headlessly (egui_kittest), with sandboxed settings and a pretend
//! tray app, plus pixel snapshots of its sections on Linux.

use std::path::Path;
use std::time::Duration;

use eframe::egui::Vec2;
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::Harness;

use super::env::{Env, TrayRequest};
use super::settings_window::{Section, SettingsApp};
use super::theme::Appearance;
use super::{tests, Dock};
use crate::crash;
use crate::options;
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
    assert_eq!(h.state().sections(), [Section::General, Section::Recording, Section::Library, Section::Destinations, Section::Updates, Section::SystemInfo, Section::About]);
    assert!(h.query_by_label("Start with Windows").is_none(), "only the theme in General");
    assert!(h.query_by_label_contains("Shortcuts").is_none());
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
    for kind in ["SFTP", "WebDAV", "S3-compatible", "SMB share", "FTPS", "Discord", "Slack", "Immich", "PhotoPrism", "Custom uploader", "Custom uploader from a ShareX file (.sxcu)…"] {
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
    assert!(h.query_by_label("Failed just now: Immich didn't accept the API key").is_some());
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
