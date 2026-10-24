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
    assert_eq!(h.state().sections().len(), 8);
    let h = settings("settings-sections-linux", |env| env.tray_app = false);
    assert_eq!(h.state().sections(), [Section::General, Section::Recording, Section::Library, Section::Updates, Section::SystemInfo, Section::About]);
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

    let release = crate::update::Release { version: crate::update::Version::new(99, 0, 0), page: "https://example.com".into(), installer: None };
    h.state().env.sandboxed().latest = Some(release);
    click(&mut h, "Check now");
    assert!(h.query_by_label_contains("RSnip 99.0.0 is available").is_some());
    assert!(h.query_by_label("Download").is_some(), "not installed, so it links to the download");
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
    assert!(h.state().env.sandboxed().copied.is_some(), "copied, ready to paste");
    assert_eq!(h.state().env.sandboxed().opened, [Path::new("https://github.com/dhiorns15/rust-screenshot/issues/new")]);
    assert!(h.query_by_label("none").is_some(), "no crashes");
    assert!(h.query_by_label_contains("Report this crash").is_none());
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
    assert_eq!(h.state().env.sandboxed().opened.len(), 1, "the new issue page");

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
    fn shortcut_problems() {
        let _gpu = super::super::tests::gpu();
        let mut h = rendered("snap-settings-problems", |env| {
            env.set_setting(Action::Full.setting(), shortcuts::PRINT_SCREEN);
            env.sandboxed().snipping_key = true;
        });
        snapshot(&mut h, Section::Shortcuts, "settings_shortcut_problems");
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
