//! The capture panel driven headlessly (egui_kittest), with sandboxed settings and a pretend tray
//! app, plus pixel snapshots of it on Linux.

use std::time::Duration;

use eframe::egui::accesskit::{Role, Toggled};
use eframe::egui::{Key, Pos2, Vec2, ViewportId};
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::Harness;

use super::capture_panel::{CapturePanel, Moving, WIDTH};
use super::env::{Env, TrayRequest};
use super::platform::SoundDevice;
use super::tests;
use crate::options;

/// The panel as the tray app (Windows) has it, after `setup` has prepared the sandbox.
fn panel(name: &str, setup: impl FnOnce(&mut Env)) -> Harness<'static, CapturePanel> {
    let mut env = Env::sandbox(tests::sandbox_dir(name));
    env.tray_app = true;
    setup(&mut env);
    // Tall, so everything shows; the real window is as high as its content.
    let mut h = Harness::builder().with_size(Vec2::new(WIDTH, 800.0)).build_eframe(move |cc| CapturePanel::new(&cc.egui_ctx, env, None, Default::default()));
    settle(&mut h);
    h
}

/// The panel as Linux has it.
fn linux_panel(name: &str, setup: impl FnOnce(&mut Env)) -> Harness<'static, CapturePanel> {
    panel(name, |env| {
        env.tray_app = false;
        setup(env);
    })
}

fn settle(h: &mut Harness<CapturePanel>) {
    for _ in 0..4 {
        h.step();
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn click(h: &mut Harness<CapturePanel>, label: &str) {
    h.get_by_label(label).click();
    settle(h);
}

/// Opens a drop-down, which shows the one chosen as its value.
fn open_list(h: &mut Harness<CapturePanel>, chosen: &str) {
    h.get_by_value(chosen).click();
    settle(h);
}

fn toggle(h: &mut Harness<CapturePanel>, label: &str) {
    h.get_by_role_and_label(Role::CheckBox, label).click();
    settle(h);
}

fn is_on(h: &Harness<CapturePanel>, label: &str) -> bool {
    h.get_by_role_and_label(Role::CheckBox, label).accesskit_node().toggled() == Some(Toggled::True)
}

fn is_disabled(h: &Harness<CapturePanel>, label: &str) -> bool {
    h.get_by_role_and_label(Role::CheckBox, label).accesskit_node().is_disabled()
}

fn requests(h: &Harness<CapturePanel>) -> Vec<TrayRequest> {
    h.state().env.sandboxed().requests.clone()
}

fn mic(id: &str, name: &str) -> SoundDevice {
    SoundDevice { id: id.into(), name: name.into() }
}

fn setting(h: &Harness<CapturePanel>, option: options::Choice) -> u32 {
    option.read(|name| h.state().env.setting(name))
}

/// Tells the window it has the focus, or hasn't.
fn focus(h: &mut Harness<CapturePanel>, focused: bool) {
    h.input_mut().viewports.entry(ViewportId::ROOT).or_default().focused = Some(focused);
    settle(h);
}

/// What pressing Capture (or Enter) is asked to run, and whether the panel closes for it.
fn launched(h: &Harness<CapturePanel>) -> Option<&'static str> {
    h.state().launch.get()
}

#[test]
fn opens_on_image_and_the_tabs_switch_and_are_remembered() {
    let mut h = panel("panel-tabs", |_| {});
    assert_eq!(setting(&h, options::CAPTURE_PANEL_TAB), 0);
    assert!(h.query_by_label("Capture").is_some() && h.query_by_label("Record").is_none());
    assert!(h.query_by_role_and_label(Role::CheckBox, "Open in editor").is_some());
    assert!(h.query_by_role_and_label(Role::CheckBox, "Computer sound").is_none(), "that's on Video");

    click(&mut h, "Video");
    assert_eq!(setting(&h, options::CAPTURE_PANEL_TAB), 1);
    assert!(h.query_by_label("Record").is_some() && h.query_by_label("Capture").is_none(), "the button says what it does");
    assert!(h.query_by_role_and_label(Role::CheckBox, "Computer sound").is_some());
    assert!(h.query_by_role_and_label(Role::CheckBox, "Open in editor").is_none());

    click(&mut h, "Image");
    assert_eq!(setting(&h, options::CAPTURE_PANEL_TAB), 0);
    assert!(requests(&h).is_empty(), "the tray app has no use for it");

    // The next time it's opened, it's on the tab it was left on.
    let again = panel("panel-tabs-again", |env| env.set_setting(options::CAPTURE_PANEL_TAB.name, 1));
    assert!(again.query_by_label("Record").is_some());
}

#[test]
fn the_selections_are_chosen_from_drop_downs_and_remembered() {
    let mut h = panel("panel-selection", |_| {});
    assert!(h.query_by_label("Selection").is_some());
    open_list(&mut h, "Region");
    for choice in ["Region", "Full screen", "Last area", "Scrolling", "Text"] {
        assert!(h.query_by_label(choice).is_some(), "{choice}");
    }
    click(&mut h, "Last area");
    assert_eq!(setting(&h, options::CAPTURE_PANEL_IMAGE), 2);
    assert!(h.query_by_value("Last area").is_some());
    assert!(h.query_by_label("The same area as last time.").is_some(), "with what it does");

    // The video one is its own.
    click(&mut h, "Video");
    assert!(h.query_by_value("Area").is_some());
    assert_eq!(setting(&h, options::CAPTURE_PANEL_IMAGE), 2, "the image one is kept");

    let again = panel("panel-selection-again", |env| env.set_setting(options::CAPTURE_PANEL_IMAGE.name, 4));
    assert!(again.query_by_value("Text").is_some());
}

#[test]
fn only_windows_scrolls_and_only_linux_picks_a_screen_or_window() {
    let mut h = linux_panel("panel-linux-lists", |_| {});
    open_list(&mut h, "Region");
    assert!(h.query_by_label("Full screen").is_some());
    assert!(h.query_by_label("Scrolling").is_none());
    click(&mut h, "Text");
    click(&mut h, "Video");
    open_list(&mut h, "Area");
    assert!(h.query_by_label("Screen or window").is_some());

    let mut h = panel("panel-windows-lists", |_| {});
    click(&mut h, "Video");
    open_list(&mut h, "Area");
    assert!(h.query_by_label("Screen or window").is_none());

    // A scrolling capture remembered somewhere else is a region capture here.
    let mut h = linux_panel("panel-linux-scrolling", |env| env.set_setting(options::CAPTURE_PANEL_IMAGE.name, 3));
    assert!(h.query_by_value("Region").is_some());
    h.key_press(Key::Enter);
    settle(&mut h);
    assert_eq!(launched(&h), Some("--region"));
}

#[test]
fn capture_closes_and_asks_for_what_is_chosen() {
    // Windows: each image selection, then the recording.
    for (index, flag) in [(0, "--region"), (1, "--full"), (2, "--last"), (3, "--scroll"), (4, "--text")] {
        let mut h = panel("panel-capture", |env| env.set_setting(options::CAPTURE_PANEL_IMAGE.name, index));
        assert_eq!(launched(&h), None);
        click(&mut h, "Capture");
        assert!(h.state().closing, "{flag}");
        assert_eq!(launched(&h), Some(flag));
    }
    let mut h = panel("panel-record", |env| env.set_setting(options::CAPTURE_PANEL_TAB.name, 1));
    click(&mut h, "Record");
    assert!(h.state().closing);
    assert_eq!(launched(&h), Some("--record"));

    // Linux: no scrolling, and a screen or window to record.
    for (index, flag) in [(0, "--region"), (1, "--full"), (2, "--last"), (4, "--text")] {
        let mut h = linux_panel("panel-capture-linux", |env| env.set_setting(options::CAPTURE_PANEL_IMAGE.name, index));
        click(&mut h, "Capture");
        assert_eq!(launched(&h), Some(flag));
    }
    for (index, flag) in [(0, "--record"), (1, "--record-screen")] {
        let mut h = linux_panel("panel-record-linux", |env| {
            env.set_setting(options::CAPTURE_PANEL_TAB.name, 1);
            env.set_setting(options::CAPTURE_PANEL_VIDEO.name, index);
        });
        click(&mut h, "Record");
        assert!(h.state().closing);
        assert_eq!(launched(&h), Some(flag));
    }
}

#[test]
fn what_is_picked_here_is_what_capture_runs() {
    let mut h = linux_panel("panel-pick-then-capture", |_| {});
    click(&mut h, "Video");
    open_list(&mut h, "Area");
    click(&mut h, "Screen or window");
    assert_eq!(setting(&h, options::CAPTURE_PANEL_VIDEO), 1);
    click(&mut h, "Record");
    assert_eq!(launched(&h), Some("--record-screen"));
}

#[test]
fn enter_captures_too() {
    let mut h = panel("panel-enter", |env| env.set_setting(options::CAPTURE_PANEL_IMAGE.name, 1));
    h.key_press(Key::Enter);
    settle(&mut h);
    assert!(h.state().closing);
    assert_eq!(launched(&h), Some("--full"));
}

#[test]
fn enter_does_not_capture_while_a_list_is_open() {
    let mut h = panel("panel-enter-list", |_| {});
    open_list(&mut h, "Region");
    h.key_press(Key::Enter);
    settle(&mut h);
    assert_eq!(launched(&h), None);
    assert!(!h.state().closing);
}

#[test]
fn the_button_shows_the_shortcut_for_what_it_does() {
    // Print Screen is the region capture's by default.
    let mut h = panel("panel-shortcut", |_| {});
    assert!(h.query_by_label("PrtScn").is_some());
    open_list(&mut h, "Region");
    click(&mut h, "Full screen");
    assert!(h.query_by_label("PrtScn").is_none(), "the full capture has none");
    click(&mut h, "Video");
    assert!(h.query_by_label("PrtScn").is_none());

    let record = 0x52 | (1 | 2) << 16;
    let mut h = panel("panel-shortcut-record", |env| {
        env.set_setting(options::CAPTURE_PANEL_TAB.name, 1);
        env.set_setting("ShortcutRegion", crate::shortcuts::PRINT_SCREEN);
        env.set_setting("ShortcutRecord", record);
    });
    assert!(h.query_by_label("Ctrl+Shift+R").is_some());
    click(&mut h, "Image");
    assert!(h.query_by_label("PrtScn").is_some());

    // Linux has none.
    let h = linux_panel("panel-shortcut-linux", |_| {});
    assert!(h.query_by_label("PrtScn").is_none());
}

#[test]
fn switches_save_the_option_and_tell_the_tray_app() {
    let mut h = panel("panel-switches", |_| {});
    assert!(is_on(&h, "Save to folder"), "on by default");
    toggle(&mut h, "Save to folder");
    assert!(!is_on(&h, "Save to folder"));
    assert_eq!(h.state().env.setting(options::SAVE_TO_FOLDER.name), Some(0));
    assert_eq!(requests(&h), [TrayRequest::SettingsChanged]);

    assert!(!is_on(&h, "Open in editor"));
    toggle(&mut h, "Open in editor");
    assert_eq!(h.state().env.setting(options::OPEN_EDITOR.name), Some(1));
    toggle(&mut h, "Magnifier");
    assert_eq!(h.state().env.setting(options::SHOW_MAGNIFIER.name), Some(0));
    toggle(&mut h, "Hide RSnip while capturing");
    assert_eq!(h.state().env.setting(options::HIDE_WHILE_CAPTURING.name), Some(0));

    click(&mut h, "Video");
    assert!(is_on(&h, "Computer sound"), "on by default");
    toggle(&mut h, "Computer sound");
    assert_eq!(h.state().env.setting(options::RECORD_SOUND.name), Some(0));
    assert!(!is_on(&h, "Microphone"), "off by default");
    toggle(&mut h, "Microphone");
    assert_eq!(h.state().env.setting(options::RECORD_MICROPHONE.name), Some(1));
    assert_eq!(requests(&h).len(), 6);
    assert!(requests(&h).iter().all(|r| *r == TrayRequest::SettingsChanged));
    assert!(!h.state().closing, "switching doesn't close it");
}

#[test]
fn linux_has_no_magnifier_or_hiding_to_change() {
    let h = linux_panel("panel-linux-switches", |_| {});
    assert!(h.query_by_role_and_label(Role::CheckBox, "Save to folder").is_some());
    assert!(h.query_by_role_and_label(Role::CheckBox, "Open in editor").is_some());
    assert!(h.query_by_label("Magnifier").is_none() && h.query_by_label("Hide RSnip while capturing").is_none());
}

#[test]
fn format_and_frame_rate_are_chosen_here() {
    let mut h = panel("panel-format", |_| {});
    click(&mut h, "Video");
    open_list(&mut h, "30 fps");
    click(&mut h, "60 fps");
    assert_eq!(h.state().env.setting(options::RECORD_FPS.name), Some(60));
    click(&mut h, "GIF");
    assert_eq!(h.state().env.setting(options::RECORD_GIF.name), Some(1));
    assert!(h.get_by_value("60 fps").accesskit_node().is_disabled(), "GIFs have their own frame rate");
    click(&mut h, "MP4");
    assert_eq!(h.state().env.setting(options::RECORD_GIF.name), Some(0));
    assert_eq!(requests(&h).len(), 3);
}

#[test]
fn sound_is_greyed_out_for_gifs() {
    let mut h = panel("panel-gif", |env| {
        env.set_setting(options::CAPTURE_PANEL_TAB.name, 1);
        env.set_setting(options::RECORD_MICROPHONE.name, 1);
    });
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
    let mut h = panel("panel-mic", |env| {
        env.set_setting(options::CAPTURE_PANEL_TAB.name, 1);
        env.sandboxed().microphones = mics;
    });
    assert!(h.query_by_value("Default microphone").is_none(), "only with the microphone on");
    toggle(&mut h, "Microphone");
    open_list(&mut h, "Default microphone");
    assert!(h.query_by_label("USB Microphone").is_some() && h.query_by_label("Laptop array").is_some());
    click(&mut h, "Laptop array");
    assert_eq!(h.state().env.text_setting(options::RECORD_MICROPHONE_DEVICE).as_deref(), Some("array-2"));
    assert!(h.query_by_value("Laptop array").is_some(), "shown by name");
    assert_eq!(requests(&h).len(), 2, "the switch and the choice");

    open_list(&mut h, "Laptop array");
    click(&mut h, "Default microphone");
    assert_eq!(h.state().env.text_setting(options::RECORD_MICROPHONE_DEVICE).as_deref(), Some(""));
}

#[test]
fn a_microphone_that_is_gone_is_said_to_be_not_connected() {
    let h = panel("panel-mic-gone", |env| {
        env.set_setting(options::CAPTURE_PANEL_TAB.name, 1);
        env.set_setting(options::RECORD_MICROPHONE.name, 1);
        env.set_text_setting(options::RECORD_MICROPHONE_DEVICE, "unplugged-3");
    });
    assert!(h.query_by_value("unplugged-3 (not connected)").is_some());
    assert!(requests(&h).is_empty(), "looking at it changes nothing");
}

#[test]
fn the_footer_opens_the_library() {
    let mut h = panel("panel-library", |_| {});
    h.get_by_label_contains("Open library").click();
    settle(&mut h);
    assert_eq!(h.state().env.sandboxed().library_opened, 1);
    assert!(h.state().closing);
    assert_eq!(launched(&h), None, "that's not a capture");
}

#[test]
fn the_gear_opens_settings_and_on_video_settings_on_recording() {
    let mut h = panel("panel-gear", |_| {});
    click(&mut h, "Settings");
    assert_eq!(h.state().env.sandboxed().settings_opened, 1);
    assert_eq!(h.state().env.sandboxed().recording_settings_opened, 0);
    assert!(h.state().closing);

    let mut h = panel("panel-gear-video", |env| env.set_setting(options::CAPTURE_PANEL_TAB.name, 1));
    click(&mut h, "Settings");
    assert_eq!(h.state().env.sandboxed().recording_settings_opened, 1);
    assert_eq!(h.state().env.sandboxed().settings_opened, 0, "not Settings on its first section");
    assert!(h.state().closing);
}

#[test]
fn esc_closes_it() {
    let mut h = panel("panel-esc", |_| {});
    assert!(!h.state().closing);
    h.key_press(Key::Escape);
    settle(&mut h);
    assert!(h.state().closing);
    assert_eq!(launched(&h), None);
}

#[test]
fn esc_closes_a_list_first() {
    let mut h = panel("panel-esc-list", |_| {});
    open_list(&mut h, "Region");
    assert!(h.query_by_label("Full screen").is_some());
    h.key_press(Key::Escape);
    settle(&mut h);
    assert!(h.query_by_label("Full screen").is_none(), "the list closed");
    assert!(!h.state().closing, "but not the panel");
}

#[test]
fn closes_when_it_loses_the_focus_but_not_before_it_has_had_it() {
    let mut h = panel("panel-focus", |_| {});
    // Not focused yet, as when it's just opened.
    focus(&mut h, false);
    assert!(!h.state().closing);
    focus(&mut h, true);
    assert!(!h.state().closing);
    focus(&mut h, false);
    assert!(h.state().closing);
    assert_eq!(launched(&h), None);
}

#[test]
fn pinned_it_stays_open_when_another_window_is_clicked() {
    let mut h = panel("panel-pin", |_| {});
    assert!(!h.state().keep_open());
    click(&mut h, "Keep open");
    assert!(h.state().keep_open(), "remembered");
    focus(&mut h, true);
    focus(&mut h, false);
    assert!(!h.state().closing);
    // Its × still closes it.
    click(&mut h, "Close");
    assert!(h.state().closing);

    let mut h = panel("panel-unpin", |env| env.set_setting(options::CAPTURE_PANEL_KEEP_OPEN.name, 1));
    click(&mut h, "Keep open");
    assert!(!h.state().keep_open());
    focus(&mut h, true);
    focus(&mut h, false);
    assert!(h.state().closing);
}

/// Somewhere on the panel with nothing on it: in the footer, between the library's link and the
/// buttons on the right (the window is as high as its content).
fn empty(h: &Harness<CapturePanel>) -> Pos2 {
    let link = h.get_by_label_contains("Open library").rect();
    Pos2::new(WIDTH / 2.0, link.center().y)
}

/// Tells the window it has the focus, or hasn't, for one frame.
fn focus_once(h: &mut Harness<CapturePanel>, focused: bool) {
    h.input_mut().viewports.entry(ViewportId::ROOT).or_default().focused = Some(focused);
    h.step();
}

/// Clicks at `at` as a hand does, pressing and letting go in different frames (checked straight
/// after, as a move would be given up on a second later).
fn press_and_release(h: &mut Harness<CapturePanel>, at: Pos2) {
    h.hover_at(at);
    h.drag_at(at);
    h.step();
    h.drop_at(at);
    h.step();
}

/// Presses on an empty part of the panel and moves the mouse, still pressed.
fn start_moving(h: &mut Harness<CapturePanel>) {
    let empty = empty(h);
    h.hover_at(empty);
    h.drag_at(empty);
    h.step();
    h.hover_at(empty + Vec2::new(30.0, 0.0));
    h.step();
}

#[test]
fn clicking_it_does_not_move_it() {
    // A move takes the focus away on X11, which closed the panel at every click, even the pin's.
    let mut h = linux_panel("panel-click", |_| {});
    focus(&mut h, true);
    let pin = h.get_by_label("Keep open").rect().center();
    press_and_release(&mut h, pin);
    assert!(h.state().keep_open());
    assert_eq!(h.state().moving, Moving::No);
    let empty = empty(&h);
    press_and_release(&mut h, empty);
    assert_eq!(h.state().moving, Moving::No, "nor a click on an empty part of it");
}

#[test]
fn moving_it_does_not_close_it_while_the_desktop_has_the_focus() {
    let mut h = linux_panel("panel-move", |_| {});
    focus(&mut h, true);
    start_moving(&mut h);
    assert!(matches!(h.state().moving, Moving::Started(_)));
    // On X11 the desktop takes the focus while it moves the window, and the button's release
    // never comes.
    focus_once(&mut h, false);
    assert_eq!(h.state().moving, Moving::LostFocus);
    settle(&mut h);
    assert!(!h.state().closing);
    focus_once(&mut h, true);
    assert_eq!(h.state().moving, Moving::No);
    focus(&mut h, false);
    assert!(h.state().closing, "clicking elsewhere after still closes it");
}

#[test]
fn moved_on_windows_it_still_closes_when_another_window_is_clicked() {
    // Windows keeps the focus while it moves the window, and may not say when the button's let go.
    let mut h = panel("panel-move-windows", |_| {});
    focus(&mut h, true);
    start_moving(&mut h);
    assert!(matches!(h.state().moving, Moving::Started(_)));
    settle(&mut h);
    assert_eq!(h.state().moving, Moving::No);
    focus(&mut h, false);
    assert!(h.state().closing);
}

#[test]
fn pinned_on_windows_it_captures_straight_away_and_stays() {
    let mut h = panel("panel-pin-capture", |env| env.set_setting(options::CAPTURE_PANEL_KEEP_OPEN.name, 1));
    click(&mut h, "Capture");
    assert_eq!(h.state().env.sandboxed().captures_started, ["--region"], "the tray app hides it while it captures");
    assert!(!h.state().closing);
    assert_eq!(launched(&h), None);
    // Opening the library or Settings leaves it open too.
    h.get_by_label_contains("Open library").click();
    settle(&mut h);
    click(&mut h, "Settings");
    assert!(!h.state().closing);
}

#[test]
fn pinned_on_linux_it_still_closes_to_capture() {
    // Nothing would bring it back after the capture, which it would be in.
    let mut h = linux_panel("panel-pin-linux", |env| env.set_setting(options::CAPTURE_PANEL_KEEP_OPEN.name, 1));
    click(&mut h, "Capture");
    assert_eq!(launched(&h), Some("--region"));
    assert!(h.state().closing);
    assert!(h.state().env.sandboxed().captures_started.is_empty());
}

#[test]
fn a_long_device_name_does_not_push_the_button_off_the_panel() {
    let long = "Headphones (2- Arctis Nova Pro Wireless) via a rather long driver name";
    let h = panel("panel-long-output", |env| {
        env.set_setting(options::CAPTURE_PANEL_TAB.name, 1);
        env.set_setting(options::RECORD_MICROPHONE.name, 1);
        env.sandboxed().outputs = vec![mic("arctis", long)];
        env.sandboxed().microphones = vec![mic("motiv", "MOTIV Mix Virtual Input (Shure Virtual Audio)")];
        env.set_text_setting(options::RECORD_SOUND_DEVICE, "arctis");
        env.set_text_setting(options::RECORD_MICROPHONE_DEVICE, "motiv");
    });
    let record = h.get_by_label("Record").rect();
    assert!(record.right() <= WIDTH - 14.0, "the button is inside the panel: {record:?}");
    assert!(h.get_by_label("Close").rect().right() <= WIDTH, "and so is the ×");
    // Both are on the line under the switches, the output's under the options.
    let (output, microphone) = (h.get_by_value(long).rect(), h.get_by_value("MOTIV Mix Virtual Input (Shure Virtual Audio)").rect());
    assert!((output.top() - microphone.top()).abs() < 2.0 && output.right() < microphone.left(), "{output:?} {microphone:?}");
    assert!(output.top() > h.get_by_label("Frame rate").rect().bottom());
}

#[test]
fn is_as_high_as_its_content() {
    let mut h = panel("panel-height", |_| {});
    let image = h.state().height;
    assert!((150.0..400.0).contains(&image), "{image}");
    // Less to show on Linux, where there's no magnifier or hiding.
    let linux = linux_panel("panel-height-linux", |_| {});
    assert!(linux.state().height <= image);
    // Video fits the same, as the options beside the microphone chooser are as tall.
    click(&mut h, "Video");
    let video = h.state().height;
    toggle(&mut h, "Microphone");
    assert!((h.state().height - video).abs() < 40.0, "{video}");
}

/// Pixel snapshots (Linux only, like Settings').
#[cfg(target_os = "linux")]
mod snapshots {
    use super::*;
    use crate::gallery::theme::Appearance;

    fn rendered(name: &str, setup: impl FnOnce(&mut Env)) -> Harness<'static, CapturePanel> {
        let mut env = Env::sandbox(tests::sandbox_dir(name));
        env.tray_app = true;
        setup(&mut env);
        let mut h = Harness::builder()
            .with_size(Vec2::new(WIDTH, 800.0))
            .with_pixels_per_point(1.0)
            .wgpu()
            .build_eframe(move |cc| CapturePanel::new(&cc.egui_ctx, env, None, Default::default()));
        settle(&mut h);
        // As high as the window would be.
        let height = h.state().height;
        h.set_size(Vec2::new(WIDTH, height));
        settle(&mut h);
        h
    }

    fn snapshot(h: &mut Harness<CapturePanel>, name: &str) {
        h.remove_cursor();
        settle(h);
        h.snapshot(name);
    }

    #[test]
    fn the_image_tab() {
        let _gpu = tests::gpu();
        let mut h = rendered("snap-panel-image", |_| {});
        snapshot(&mut h, "capture_panel_image");
    }

    #[test]
    fn the_video_tab() {
        let _gpu = tests::gpu();
        let mut h = rendered("snap-panel-video", |env| {
            env.set_setting(options::CAPTURE_PANEL_TAB.name, 1);
            env.set_setting(options::RECORD_MICROPHONE.name, 1);
            env.sandboxed().microphones = vec![mic("usb-1", "USB Microphone")];
            // A long name, cut short.
            env.sandboxed().outputs = vec![mic("arctis", "Headphones (2- Arctis Nova Pro Wireless)")];
            env.set_text_setting(options::RECORD_SOUND_DEVICE, "arctis");
        });
        snapshot(&mut h, "capture_panel_video");
    }

    #[test]
    fn light_with_a_gif() {
        let _gpu = tests::gpu();
        let mut h = rendered("snap-panel-light", |env| {
            env.set_setting(Appearance::SETTING, Appearance::Light.to_setting());
            env.set_setting(options::CAPTURE_PANEL_TAB.name, 1);
            env.set_setting(options::RECORD_GIF.name, 1);
            env.set_setting("ShortcutRegion", 0);
            env.set_setting("ShortcutRecord", 0x52 | (1 | 2) << 16);
        });
        snapshot(&mut h, "capture_panel_light");
    }
}
