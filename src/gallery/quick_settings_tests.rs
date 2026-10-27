//! The quick-settings flyout driven headlessly (egui_kittest), with sandboxed settings and a
//! pretend tray app, plus pixel snapshots of it on Linux.

use std::time::Duration;

use eframe::egui::accesskit::{Role, Toggled};
use eframe::egui::{Key, Vec2, ViewportId};
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::Harness;

use super::env::{Env, TrayRequest};
use super::platform::Microphone;
use super::quick_settings::{QuickSettings, WIDTH};
use super::tests;
use crate::options;

/// The flyout as the tray app (Windows) has it, after `setup` has prepared the sandbox.
fn flyout(name: &str, setup: impl FnOnce(&mut Env)) -> Harness<'static, QuickSettings> {
    let mut env = Env::sandbox(tests::sandbox_dir(name));
    env.tray_app = true;
    setup(&mut env);
    // Tall, so everything shows; the real window is as high as its content.
    let mut h = Harness::builder().with_size(Vec2::new(WIDTH, 800.0)).build_eframe(move |cc| QuickSettings::new(&cc.egui_ctx, env, None));
    settle(&mut h);
    h
}

fn settle(h: &mut Harness<QuickSettings>) {
    for _ in 0..4 {
        h.step();
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn click(h: &mut Harness<QuickSettings>, label: &str) {
    h.get_by_label(label).click();
    settle(h);
}

/// Opens the microphone drop-down, which shows the one chosen as its value.
fn open_microphones(h: &mut Harness<QuickSettings>, chosen: &str) {
    h.get_by_value(chosen).click();
    settle(h);
}

fn toggle(h: &mut Harness<QuickSettings>, label: &str) {
    h.get_by_role_and_label(Role::CheckBox, label).click();
    settle(h);
}

fn is_on(h: &Harness<QuickSettings>, label: &str) -> bool {
    h.get_by_role_and_label(Role::CheckBox, label).accesskit_node().toggled() == Some(Toggled::True)
}

fn is_disabled(h: &Harness<QuickSettings>, label: &str) -> bool {
    h.get_by_role_and_label(Role::CheckBox, label).accesskit_node().is_disabled()
}

fn requests(h: &Harness<QuickSettings>) -> Vec<TrayRequest> {
    h.state().env.sandboxed().requests.clone()
}

fn mic(id: &str, name: &str) -> Microphone {
    Microphone { id: id.into(), name: name.into() }
}

/// Tells the window it has the focus, or hasn't.
fn focus(h: &mut Harness<QuickSettings>, focused: bool) {
    h.input_mut().viewports.entry(ViewportId::ROOT).or_default().focused = Some(focused);
    settle(h);
}

#[test]
fn switches_save_the_option_and_tell_the_tray_app() {
    let mut h = flyout("quick-switches", |_| {});
    assert!(is_on(&h, "Save screenshots to the folder"), "on by default");
    toggle(&mut h, "Save screenshots to the folder");
    assert!(!is_on(&h, "Save screenshots to the folder"));
    assert_eq!(h.state().env.setting(options::SAVE_TO_FOLDER.name), Some(0));
    assert_eq!(requests(&h), [TrayRequest::SettingsChanged]);

    toggle(&mut h, "Open the editor after capture");
    assert_eq!(h.state().env.setting(options::OPEN_EDITOR.name), Some(1));
    toggle(&mut h, "Show the magnifier");
    assert_eq!(h.state().env.setting(options::SHOW_MAGNIFIER.name), Some(0));
    toggle(&mut h, "Hide RSnip while capturing");
    assert_eq!(h.state().env.setting(options::HIDE_WHILE_CAPTURING.name), Some(0));

    assert!(is_on(&h, "Computer sound"), "on by default");
    toggle(&mut h, "Computer sound");
    assert_eq!(h.state().env.setting(options::RECORD_SOUND.name), Some(0));
    assert!(!is_on(&h, "Microphone"), "off by default");
    toggle(&mut h, "Microphone");
    assert_eq!(h.state().env.setting(options::RECORD_MICROPHONE.name), Some(1));
    assert_eq!(requests(&h).len(), 6);
    assert!(requests(&h).iter().all(|r| *r == TrayRequest::SettingsChanged));
}

#[test]
fn linux_has_no_magnifier_or_hiding_to_change() {
    let h = flyout("quick-linux", |env| env.tray_app = false);
    assert!(h.query_by_role_and_label(Role::CheckBox, "Save screenshots to the folder").is_some());
    assert!(h.query_by_label("Show the magnifier").is_none() && h.query_by_label("Hide RSnip while capturing").is_none());
}

#[test]
fn format_and_frame_rate_are_chosen_here() {
    let mut h = flyout("quick-format", |_| {});
    click(&mut h, "60");
    assert_eq!(h.state().env.setting(options::RECORD_FPS.name), Some(60));
    click(&mut h, "GIF");
    assert_eq!(h.state().env.setting(options::RECORD_GIF.name), Some(1));
    assert!(h.get_by_label("60").accesskit_node().is_disabled(), "GIFs have their own frame rate");
    click(&mut h, "MP4");
    assert_eq!(h.state().env.setting(options::RECORD_GIF.name), Some(0));
    assert_eq!(requests(&h).len(), 3);
}

#[test]
fn sound_is_greyed_out_for_gifs() {
    let mut h = flyout("quick-gif", |env| env.set_setting(options::RECORD_MICROPHONE.name, 1));
    assert!(h.query_by_label("GIFs have no sound").is_none());
    assert!(!is_disabled(&h, "Computer sound") && !is_disabled(&h, "Microphone"));
    click(&mut h, "GIF");
    assert!(h.query_by_label("GIFs have no sound").is_some());
    assert!(is_disabled(&h, "Computer sound") && is_disabled(&h, "Microphone"));
    assert!(h.get_by_value("Default microphone").accesskit_node().is_disabled(), "nor can a microphone be picked");
}

#[test]
fn the_microphone_is_picked_from_those_connected() {
    let mics = vec![mic("usb-1", "USB Microphone"), mic("array-2", "Laptop array")];
    let mut h = flyout("quick-mic", |env| env.sandboxed().microphones = mics);
    assert!(h.query_by_value("Default microphone").is_none(), "only with the microphone on");
    toggle(&mut h, "Microphone");
    open_microphones(&mut h, "Default microphone");
    assert!(h.query_by_label("USB Microphone").is_some() && h.query_by_label("Laptop array").is_some());
    click(&mut h, "Laptop array");
    assert_eq!(h.state().env.text_setting(options::RECORD_MICROPHONE_DEVICE).as_deref(), Some("array-2"));
    assert!(h.query_by_value("Laptop array").is_some(), "shown by name");
    assert_eq!(requests(&h).len(), 2, "the switch and the choice");

    open_microphones(&mut h, "Laptop array");
    click(&mut h, "Default microphone");
    assert_eq!(h.state().env.text_setting(options::RECORD_MICROPHONE_DEVICE).as_deref(), Some(""));
}

#[test]
fn a_microphone_that_is_gone_is_said_to_be_not_connected() {
    let h = flyout("quick-mic-gone", |env| {
        env.set_setting(options::RECORD_MICROPHONE.name, 1);
        env.set_text_setting(options::RECORD_MICROPHONE_DEVICE, "unplugged-3");
    });
    assert!(h.query_by_value("unplugged-3 (not connected)").is_some());
    assert!(requests(&h).is_empty(), "looking at it changes nothing");
}

#[test]
fn all_settings_opens_settings_on_recording_and_closes() {
    let mut h = flyout("quick-all-settings", |_| {});
    assert!(!h.state().closing);
    h.get_by_label_contains("All settings").click();
    settle(&mut h);
    assert_eq!(h.state().env.sandboxed().recording_settings_opened, 1);
    assert_eq!(h.state().env.sandboxed().settings_opened, 0, "not the Settings window on its first section");
    assert!(h.state().closing);
}

#[test]
fn esc_closes_it() {
    let mut h = flyout("quick-esc", |_| {});
    assert!(!h.state().closing);
    h.key_press(Key::Escape);
    settle(&mut h);
    assert!(h.state().closing);
}

#[test]
fn esc_closes_the_microphone_list_first() {
    let mut h = flyout("quick-esc-list", |env| {
        env.set_setting(options::RECORD_MICROPHONE.name, 1);
        env.sandboxed().microphones = vec![mic("usb-1", "USB Microphone")];
    });
    open_microphones(&mut h, "Default microphone");
    assert!(h.query_by_label("USB Microphone").is_some());
    h.key_press(Key::Escape);
    settle(&mut h);
    assert!(h.query_by_label("USB Microphone").is_none(), "the list closed");
    assert!(!h.state().closing, "but not the flyout");
}

#[test]
fn closes_when_it_loses_the_focus_but_not_before_it_has_had_it() {
    let mut h = flyout("quick-focus", |_| {});
    // Not focused yet, as when it's just opened.
    focus(&mut h, false);
    assert!(!h.state().closing);
    focus(&mut h, true);
    assert!(!h.state().closing);
    focus(&mut h, false);
    assert!(h.state().closing);
}

#[test]
fn is_as_high_as_its_content() {
    let mut h = flyout("quick-height", |_| {});
    let height = h.state().height;
    assert!((300.0..700.0).contains(&height), "{height}");
    // More to show with the microphone on.
    toggle(&mut h, "Microphone");
    assert!(h.state().height > height + 20.0);
    // Less on Linux.
    let linux = flyout("quick-height-linux", |env| env.tray_app = false);
    assert!(linux.state().height < height - 50.0);
}

/// Pixel snapshots (Linux only, like Settings').
#[cfg(target_os = "linux")]
mod snapshots {
    use super::*;
    use crate::gallery::theme::Appearance;

    fn rendered(name: &str, setup: impl FnOnce(&mut Env)) -> Harness<'static, QuickSettings> {
        let mut env = Env::sandbox(tests::sandbox_dir(name));
        env.tray_app = true;
        setup(&mut env);
        let mut h = Harness::builder()
            .with_size(Vec2::new(WIDTH, 800.0))
            .with_pixels_per_point(1.0)
            .wgpu()
            .build_eframe(move |cc| QuickSettings::new(&cc.egui_ctx, env, None));
        settle(&mut h);
        // As high as the window would be.
        let height = h.state().height;
        h.set_size(Vec2::new(WIDTH, height));
        settle(&mut h);
        h
    }

    fn snapshot(h: &mut Harness<QuickSettings>, name: &str) {
        h.remove_cursor();
        settle(h);
        h.snapshot(name);
    }

    #[test]
    fn the_flyout() {
        let _gpu = tests::gpu();
        let mut h = rendered("snap-quick", |_| {});
        snapshot(&mut h, "quick_settings");
    }

    #[test]
    fn for_gifs_the_sound_is_greyed_out() {
        let _gpu = tests::gpu();
        let mut h = rendered("snap-quick-gif", |env| env.set_setting(options::RECORD_GIF.name, 1));
        snapshot(&mut h, "quick_settings_gif");
    }

    #[test]
    fn light_with_the_microphone_on() {
        let _gpu = tests::gpu();
        let mut h = rendered("snap-quick-light", |env| {
            env.set_setting(Appearance::SETTING, Appearance::Light.to_setting());
            env.set_setting(options::RECORD_MICROPHONE.name, 1);
            env.set_setting(options::RECORD_FPS.name, 60);
            env.sandboxed().microphones = vec![mic("usb-1", "USB Microphone")];
        });
        snapshot(&mut h, "quick_settings_light");
    }
}
