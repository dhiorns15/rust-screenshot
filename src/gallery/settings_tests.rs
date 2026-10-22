//! The Settings window driven headlessly (egui_kittest), with sandboxed settings and a pretend
//! tray app, plus pixel snapshots of its sections on Linux.

use std::time::Duration;

use eframe::egui::Vec2;
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::Harness;

use super::env::{Env, TrayRequest};
use super::settings_window::{Section, SettingsApp};
use super::theme::Appearance;
use super::{tests, Dock};
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
    assert_eq!(h.state().sections().len(), 7);
    let h = settings("settings-sections-linux", |env| env.tray_app = false);
    assert_eq!(h.state().sections(), [Section::General, Section::Library, Section::Updates, Section::About]);
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
fn checks_for_updates_and_offers_the_new_version() {
    let mut h = settings("settings-updates", |_| {});
    open(&mut h, Section::Updates);
    click(&mut h, "Check now");
    assert!(h.query_by_label_contains("You have the latest version").is_some());

    let release = crate::update::Release { version: crate::update::Version(99, 0, 0), page: "https://example.com".into(), installer: None };
    h.state().env.sandboxed().latest = Some(release);
    click(&mut h, "Check now");
    assert!(h.query_by_label_contains("RSnip 99.0.0 is available").is_some());
    assert!(h.query_by_label("Download").is_some(), "not installed, so it links to the download");
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
