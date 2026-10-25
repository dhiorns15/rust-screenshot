//! The whole library window driven headlessly through its accessibility tree (egui_kittest), in a
//! sandbox folder with in-memory settings.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use eframe::egui::{Key, Modifiers, Pos2, Rect, Vec2};
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::Harness;

use super::env::Env;
use super::theme::{icon, label};
use super::*;

const SIZE: Vec2 = Vec2::new(1280.0, 820.0);

/// Screenshots named a, b and c, taken today, today and yesterday.
fn fixtures(name: &str) -> PathBuf {
    let dir = tests::sandbox_dir(name);
    let now = SystemTime::now();
    let day = Duration::from_secs(24 * 60 * 60);
    for (file, shade, taken) in [("Screenshot a.png", 60u8, now), ("Screenshot b.png", 140, now - Duration::from_secs(60)), ("Screenshot c.png", 220, now - day)] {
        let path = dir.join(file);
        image::RgbaImage::from_pixel(320, 200, image::Rgba([shade, shade, 255 - shade, 255])).save(&path).unwrap();
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(taken).unwrap();
    }
    dir
}

fn harness(name: &str) -> Harness<'static, App> {
    harness_at(name, 0.25)
}

/// With `step_dt` seconds per frame (kittest's default is 0.25 s, too slow for a double click).
fn harness_at(name: &str, step_dt: f32) -> Harness<'static, App> {
    let dir = fixtures(name);
    let mut h = Harness::builder().with_size(SIZE).with_step_dt(step_dt).build_eframe(move |cc| App::new(&cc.egui_ctx, Env::sandbox(dir), None));
    settle(&mut h);
    h
}

/// A few frames, for clicks to land and thumbnails to load. (The library keeps asking to repaint,
/// so `run` would never settle.)
fn settle(h: &mut Harness<App>) {
    for _ in 0..4 {
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn click(h: &mut Harness<App>, name: &str) {
    h.get_by_label(name).click();
    settle(h);
}

fn shows(h: &Harness<App>, name: &str) -> bool {
    h.query_by_label(name).is_some()
}

fn disabled(h: &Harness<App>, name: &str) -> bool {
    h.get_by_label(name).accesskit_node().is_disabled()
}

fn rect_of(h: &Harness<App>, name: &str) -> Rect {
    h.get_by_label(name).rect()
}

/// Drags on the canvas between two points given as fractions of the image.
fn drag_on_canvas(h: &mut Harness<App>, from: (f32, f32), to: (f32, f32)) {
    let canvas = h.state().editor.as_ref().unwrap().canvas;
    let at = |(x, y): (f32, f32)| Pos2::new(canvas.min.x + canvas.width() * x, canvas.min.y + canvas.height() * y);
    h.hover_at(at(from));
    h.step();
    h.drag_at(at(from));
    h.step();
    for t in [0.25, 0.5, 0.75, 1.0] {
        let (x, y) = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
        h.hover_at(at((x, y)));
        h.step();
    }
    h.drop_at(at(to));
    settle(h);
}

#[test]
fn opens_the_newest_screenshot_in_the_sandbox() {
    let h = harness("kittest-open");
    let app = h.state();
    assert_eq!(app.entries.len(), 3);
    assert!(app.editor.as_ref().is_some_and(|e| e.doc.path.ends_with("Screenshot a.png")));
    for name in ["Screenshot a", "Screenshot b", "Screenshot c"] {
        assert!(shows(&h, name), "{name} is listed");
    }
}

#[test]
fn tools_select_and_show_only_their_options() {
    let mut h = harness("kittest-tools");
    click(&mut h, "Text");
    assert!(h.state().tools.tool == Tool::Text);
    assert!(shows(&h, "Shadow") && shows(&h, "Red") && shows(&h, "Size"));

    click(&mut h, "Blur");
    assert!(h.state().tools.tool == Tool::Blur);
    assert!(!shows(&h, "Shadow") && !shows(&h, "Red") && !shows(&h, "Size"), "Blur has no options");

    click(&mut h, "Arrow");
    assert!(shows(&h, "Red") && shows(&h, "Size") && !shows(&h, "Shadow"));
    click(&mut h, "Blue");
    assert_eq!(*h.state_mut().tools.color_mut(), PALETTE[4]);
}

#[test]
fn save_is_enabled_by_an_edit_and_undo_disables_it_again() {
    let mut h = harness("kittest-save");
    let save = label(icon::FLOPPY_DISK, "Save");
    assert!(disabled(&h, &save), "nothing to save yet");
    assert!(disabled(&h, "Undo"));

    click(&mut h, "Arrow");
    drag_on_canvas(&mut h, (0.2, 0.2), (0.6, 0.5));
    assert_eq!(h.state().editor.as_ref().unwrap().doc.current().shapes.len(), 1, "the drag drew an arrow");
    assert!(!disabled(&h, &save));

    click(&mut h, "Undo");
    assert!(h.state().editor.as_ref().unwrap().doc.current().shapes.is_empty());
    assert!(disabled(&h, &save));
    assert!(!disabled(&h, "Redo"));
}

#[test]
fn export_menu_offers_what_this_system_supports() {
    let mut h = harness("kittest-export");
    h.get_by_label_contains("Export").click();
    settle(&mut h);
    assert!(h.query_by_label_contains("Save copy…").is_some());
    assert!(h.query_by_label_contains("Save copy as JPG").is_some());
    for windows_only in ["Copy as file", "Pin to screen"] {
        assert_eq!(h.query_by_label_contains(windows_only).is_some(), cfg!(windows), "{windows_only}");
    }
    assert!(h.query_by_label_contains("Copy text (OCR)").is_some(), "wherever text can be read");
    h.get_by_label_contains("Show in folder").click();
    settle(&mut h);
    let open = h.state().editor.as_ref().unwrap().doc.path.clone();
    assert_eq!(h.state().env.sandboxed().shown_in_folder, [open]);
}

#[test]
fn hide_and_show_requests_are_not_files_to_open() {
    let mut h = harness("kittest-hide-show");
    let open = |h: &Harness<App>| h.state().editor.as_ref().unwrap().doc.path.clone();
    let before = open(&h);
    for request in ["--hide", "--show"] {
        std::fs::write(&h.state().env.request_file, request).unwrap();
        h.state_mut().last_scan = std::time::Instant::now() - RESCAN_EVERY;
        settle(&mut h);
        assert!(!h.state().env.request_file.exists(), "{request} was read");
        assert_eq!(open(&h), before, "{request}");
        assert!(h.state().status.is_none(), "{request}: {:?}", h.state().status);
    }
}

#[test]
fn library_moves_to_the_bottom_and_back() {
    let mut h = harness("kittest-dock");
    let (a, b) = (rect_of(&h, "Screenshot a"), rect_of(&h, "Screenshot b"));
    assert!(b.top() > a.top() && a.left() < SIZE.x / 2.0, "a list down the left");

    click(&mut h, "Library along the bottom");
    assert!(h.state().dock == Dock::Bottom);
    assert_eq!(h.state().env.setting(Dock::SETTING), Some(Dock::Bottom.to_setting()));
    let (a, b) = (rect_of(&h, "Screenshot a"), rect_of(&h, "Screenshot b"));
    assert!(a.top() > SIZE.y / 2.0, "a filmstrip along the bottom");
    assert!((a.top() - b.top()).abs() < 1.0 && b.left() >= a.right(), "side by side: {a:?} {b:?}");

    click(&mut h, "Library on the left");
    assert!(h.state().dock == Dock::Left);
    assert_eq!(h.state().env.setting(Dock::SETTING), Some(Dock::Left.to_setting()));
}

#[test]
fn narrow_window_keeps_every_toolbar_button_clear() {
    let mut h = harness("kittest-narrow");
    h.set_size(Vec2::new(760.0, 600.0));
    settle(&mut h);
    let buttons: Vec<(&str, Rect)> =
        TOOLS.iter().map(|t| t.1).chain(["Undo", "Redo"]).map(|name| (name, rect_of(&h, name))).collect();
    for (i, (a, ra)) in buttons.iter().enumerate() {
        assert!(ra.right() <= 760.0 && ra.bottom() <= 600.0, "{a} is inside the window");
        for (b, rb) in &buttons[i + 1..] {
            assert!(!ra.intersects(rb.shrink(0.5)), "{a} overlaps {b}");
        }
    }
}

#[test]
fn typing_a_hex_code_sets_the_colour() {
    let mut h = harness("kittest-hex");
    click(&mut h, "Arrow");
    click(&mut h, "Pick any colour");
    use egui::accesskit::Role::TextInput;
    h.get_by_role_and_label(TextInput, "Hex").focus();
    settle(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    settle(&mut h);
    h.get_by_role_and_label(TextInput, "Hex").type_text("#12ab34");
    settle(&mut h);
    assert_eq!(*h.state_mut().tools.color_mut(), Color32::from_rgb(0x12, 0xAB, 0x34));
    assert!(shows(&h, "Pick any colour"), "the picker stays open while typing");
}

#[test]
fn opening_the_picker_leaves_the_colour_alone() {
    let mut h = harness("kittest-picker-noop");
    click(&mut h, "Arrow");
    let before = *h.state_mut().tools.color_mut();
    click(&mut h, "Pick any colour");
    settle(&mut h);
    assert_eq!(*h.state_mut().tools.color_mut(), before);
}

#[test]
fn a_crash_since_last_time_is_mentioned_once() {
    let dir = fixtures("library-crash");
    let env = Env::sandbox(dir);
    let panic = crate::crash::Panic { time: 500, part: "library", thread: "main".into(), message: "oops".into(), location: None, backtrace: String::new() };
    crate::crash::save(&env.crash_dir, &panic, None).unwrap();
    env.set_setting(crate::crash::SEEN_SETTING, 400);
    let mut h = Harness::builder().with_size(SIZE).build_eframe(move |cc| App::new(&cc.egui_ctx, env, None));
    settle(&mut h);
    h.get_by_label("RSnip closed unexpectedly. Report it").click();
    settle(&mut h);
    assert_eq!(h.state().env.sandboxed().system_info_opened, 1);
    assert_eq!(h.state().env.setting(crate::crash::SEEN_SETTING), Some(500));
    assert!(h.query_by_label("RSnip closed unexpectedly. Report it").is_none());

    // Seen already, so a new library window doesn't mention it.
    let env = Env::sandbox(fixtures("library-crash-seen"));
    crate::crash::save(&env.crash_dir, &panic, None).unwrap();
    env.set_setting(crate::crash::SEEN_SETTING, 500);
    let mut h = Harness::builder().with_size(SIZE).build_eframe(move |cc| App::new(&cc.egui_ctx, env, None));
    settle(&mut h);
    assert!(h.query_by_label("RSnip closed unexpectedly. Report it").is_none());
}

/// A library whose sandbox "reads" `text` in Screenshot b, once the reader has finished.
/// Opens the Copy text tool on screenshot a, with `lines` of text "read" from it, and waits for it.
fn copy_text(name: &str, lines: &[&str]) -> Harness<'static, App> {
    let mut h = harness_at(name, 1.0 / 60.0);
    *h.state().env.sandboxed().words.lock().unwrap() = super::words::example(lines);
    click(&mut h, "Copy text");
    for _ in 0..100 {
        settle(&mut h);
        if h.state().editor.as_ref().unwrap().text_status() != TextStatus::Reading {
            break;
        }
    }
    // The toolbar is drawn before the editor finds out, so it shows the result a frame later.
    settle(&mut h);
    h
}

/// Clicks at `pos` `times` times in a row.
fn click_at(h: &mut Harness<App>, pos: Pos2, times: usize) {
    h.hover_at(pos);
    for _ in 0..times {
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        }
        h.step();
    }
}

/// Where a point given in screenshot pixels is drawn on the canvas, as fractions of the image.
fn at_pixel(x: f32, y: f32) -> (f32, f32) {
    (x / 320.0, y / 200.0)
}

#[test]
fn copy_text_copies_the_words_dragged_across() {
    let mut h = copy_text("kittest-copy-text", &["Hello there", "second line here"]);
    assert_eq!(h.state().editor.as_ref().unwrap().text_status(), TextStatus::Found { words: 5, selected: 0 });
    assert!(disabled(&h, &label(icon::COPY, "Copy selection")), "nothing selected yet");

    // From "Hello" to "line", on the next line.
    drag_on_canvas(&mut h, at_pixel(15.0, 20.0), at_pixel(100.0, 50.0));
    h.event(egui::Event::Copy);
    settle(&mut h);
    assert_eq!(h.state().env.sandboxed().copied.as_deref(), Some("Hello there\nsecond line"));

    click(&mut h, "Copy all text");
    assert_eq!(h.state().env.sandboxed().copied.as_deref(), Some("Hello there\nsecond line here"));
}

#[test]
fn copy_text_selects_a_word_or_a_line_and_ctrl_a_selects_everything() {
    let mut h = copy_text("kittest-copy-text-clicks", &["Hello there", "second line here"]);
    let canvas = h.state().editor.as_ref().unwrap().canvas;
    let at = |(x, y): (f32, f32)| Pos2::new(canvas.min.x + canvas.width() * x, canvas.min.y + canvas.height() * y);
    let selected = |h: &Harness<App>| h.state().editor.as_ref().unwrap().selected_text();

    click_at(&mut h, at(at_pixel(90.0, 20.0)), 1);
    settle(&mut h);
    assert_eq!(selected(&h).as_deref(), Some("there"));

    click_at(&mut h, at(at_pixel(150.0, 50.0)), 2);
    settle(&mut h);
    assert_eq!(selected(&h).as_deref(), Some("second line here"), "double-clicking selects the line");

    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    settle(&mut h);
    assert_eq!(selected(&h).as_deref(), Some("Hello there\nsecond line here"));

    h.key_press(Key::Escape);
    settle(&mut h);
    assert_eq!(selected(&h), None);
    h.event(egui::Event::Copy);
    settle(&mut h);
    assert_eq!(h.state().env.sandboxed().copied, None, "with nothing selected, Ctrl+C copies the image");
}

#[test]
fn copy_text_says_when_theres_none() {
    let h = copy_text("kittest-copy-no-text", &[]);
    assert!(shows(&h, "No text found in this image"));
}

fn library_with_text(name: &str, text: &str, setup: impl FnOnce(&Env) + 'static) -> (Harness<'static, App>, PathBuf) {
    let dir = fixtures(name);
    let env = Env::sandbox(dir.clone());
    env.sandboxed().ocr_text.lock().unwrap().insert("Screenshot b.png".into(), text.into());
    setup(&env);
    let mut h = Harness::builder().with_size(SIZE).build_eframe(move |cc| App::new(&cc.egui_ctx, env, None));
    for _ in 0..200 {
        settle(&mut h);
        if h.state().reading.is_empty() {
            break;
        }
    }
    (h, dir)
}

fn search(h: &mut Harness<App>, text: &str) {
    use egui::accesskit::Role::TextInput;
    h.get_by_role(TextInput).focus();
    settle(h);
    h.get_by_role(TextInput).type_text(text);
    settle(h);
}

#[test]
fn searches_the_text_read_from_screenshots() {
    let (mut h, dir) = library_with_text("kittest-search", "Invoice 2026
Total due", |_| {});
    assert!(h.state().metas.values().all(|m| m.text.is_some()), "every screenshot was read");
    assert_eq!(super::search::load(&dir.join("Screenshot b.png")).text.as_deref(), Some("Invoice 2026
Total due"), "cached beside it");
    search(&mut h, "INVOICE total");
    assert!(shows(&h, "Screenshot b"));
    assert!(!shows(&h, "Screenshot a") && !shows(&h, "Screenshot c"));
}

#[test]
fn reading_text_can_be_turned_off() {
    let (h, _) = library_with_text("kittest-search-off", "Invoice", |env| env.set_setting(super::search::SETTING, 0));
    assert!(h.state().reading.is_empty());
    assert!(h.state().metas.values().all(|m| m.text.is_none()), "nothing was read");
}

#[test]
fn ctrl_and_shift_click_select_several() {
    let mut h = harness("kittest-select");
    click(&mut h, "Screenshot a");
    h.get_by_label("Screenshot c").click_modifiers(Modifiers::COMMAND);
    settle(&mut h);
    assert_eq!(h.state().selected.len(), 2);
    assert!(shows(&h, "2 selected"));

    click(&mut h, "Screenshot a");
    h.get_by_label("Screenshot c").click_modifiers(Modifiers::SHIFT);
    settle(&mut h);
    assert_eq!(h.state().selected.len(), 3, "a range from a to c");

    h.key_press(Key::Escape);
    settle(&mut h);
}

/// The library, with an Immich destination set up in Settings → Push to.
fn with_destination(name: &str) -> (Harness<'static, App>, crate::push::Destination) {
    let h = harness(name);
    let immich = crate::push::Destination::new(crate::push::Kind::Immich { server: "https://photos.example.com".into() });
    crate::push::save(&h.state().env, std::slice::from_ref(&immich));
    (h, immich)
}

#[test]
fn pushes_the_open_screenshot_and_copies_its_link() {
    let (mut h, immich) = with_destination("kittest-push");
    let open = h.state().editor.as_ref().unwrap().doc.path.clone();
    h.get_by_label_contains("Export").click();
    settle(&mut h);
    h.get_by_label_contains("Push to").click();
    settle(&mut h);
    click(&mut h, "Immich");
    settle(&mut h);
    assert_eq!(h.state().env.sandboxed().pushed, [("Immich".to_owned(), open.clone())]);
    assert_eq!(h.state().env.sandboxed().copied.as_deref(), Some("https://example.com/Screenshot a.png"));
    assert!(h.query_by_label_contains("Pushed to Immich, link copied").is_some());
    let pushed = &super::search::load(&open).pushed[&immich.id];
    assert_eq!(pushed.link.as_deref(), Some("https://example.com/Screenshot a.png"), "noted beside it");
    let recent = crate::push::history::load(&h.state().env.history_file);
    assert_eq!((recent[0].file.as_str(), recent[0].auto, recent[0].error.as_deref()), ("Screenshot a.png", false, None), "and in the history");

    // And its link can be copied again later, from the library.
    h.state().env.sandboxed().copied = None;
    h.get_by_label("Screenshot a").click_secondary();
    settle(&mut h);
    click(&mut h, &format!("{}  Copy link (Immich)", icon::LINK));
    assert_eq!(h.state().env.sandboxed().copied.as_deref(), Some("https://example.com/Screenshot a.png"));
}

#[test]
fn shows_links_pushed_by_the_automatic_pusher() {
    let (mut h, immich) = with_destination("kittest-push-elsewhere");
    let b = h.state().env.dir.join("Screenshot b.png");
    // As the automatic pusher does, from the tray app, after the library has listed it.
    super::search::record_push(&b, &immich.id, Some("https://example.com/b".into())).unwrap();
    h.state_mut().rescan();
    h.get_by_label("Screenshot b").click_secondary();
    settle(&mut h);
    click(&mut h, &format!("{}  Copy link (Immich)", icon::LINK));
    assert_eq!(h.state().env.sandboxed().copied.as_deref(), Some("https://example.com/b"));
}

#[test]
fn pushes_the_selected_ones_and_says_what_failed() {
    let (mut h, _) = with_destination("kittest-push-several");
    click(&mut h, "Screenshot a");
    h.get_by_label("Screenshot c").click_modifiers(Modifiers::COMMAND);
    settle(&mut h);
    h.get_by_label("Screenshot c").click_secondary();
    settle(&mut h);
    h.get_by_label_contains("Push to (2)").click();
    settle(&mut h);
    click(&mut h, "Immich");
    settle(&mut h);
    let pushed: Vec<String> = h.state().env.sandboxed().pushed.iter().map(|(_, p)| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(pushed, ["Screenshot a.png", "Screenshot c.png"]);
    assert!(h.query_by_label_contains("Pushed 2 to Immich, links copied").is_some());
    assert_eq!(h.state().env.sandboxed().copied.as_deref(), Some("https://example.com/Screenshot a.png\nhttps://example.com/Screenshot c.png"));

    // A failure says what went wrong, and records nothing.
    h.state().env.sandboxed().push_result = Some(Err("Immich didn't accept the API key".into()));
    h.get_by_label("Screenshot b").click_secondary();
    settle(&mut h);
    h.get_by_label_contains("Push to").click();
    settle(&mut h);
    click(&mut h, "Immich");
    settle(&mut h);
    assert!(h.query_by_label_contains("Couldn't push to Immich: Immich didn't accept the API key").is_some());
    let b = h.state().env.dir.join("Screenshot b.png");
    assert!(super::search::load(&b).pushed.is_empty());
    let recent = crate::push::history::load(&h.state().env.history_file);
    assert_eq!(recent[0].error.as_deref(), Some("Immich didn't accept the API key"), "the history says why");
}

#[test]
fn push_waits_for_edits_to_be_saved() {
    let (mut h, _) = with_destination("kittest-push-edited");
    drag_on_canvas(&mut h, (0.3, 0.3), (0.6, 0.6));
    assert!(h.state().dirty());
    let open = h.state().editor.as_ref().unwrap().doc.path.file_stem().unwrap().to_string_lossy().into_owned();
    h.get_by_label_contains("Export").click();
    settle(&mut h);
    assert!(h.get_by_label_contains("Push to").accesskit_node().is_disabled());
    h.key_press(egui::Key::Escape);
    settle(&mut h);

    // Nor from the library's menu, for it alone or with others.
    h.get_by_label(&open).click_secondary();
    settle(&mut h);
    assert!(h.get_by_label_contains("Push to").accesskit_node().is_disabled());
    h.key_press(egui::Key::Escape);
    settle(&mut h);
    let other = if open == "Screenshot b" { "Screenshot c" } else { "Screenshot b" };
    click(&mut h, &open);
    h.get_by_label(other).click_modifiers(Modifiers::COMMAND);
    settle(&mut h);
    h.get_by_label(other).click_secondary();
    settle(&mut h);
    assert!(h.get_by_label_contains("Push to (2)").accesskit_node().is_disabled());
    h.key_press(egui::Key::Escape);
    settle(&mut h);

    // Another one, not open, can still go.
    h.get_by_label(other).click_modifiers(Modifiers::COMMAND);
    settle(&mut h);
    h.get_by_label(other).click_secondary();
    settle(&mut h);
    assert!(!h.get_by_label_contains("Push to").accesskit_node().is_disabled());
}

/// The library with a 2-second GIF recording, the newest file, besides the screenshots.
fn with_recording(name: &str) -> (Harness<'static, App>, PathBuf) {
    let dir = fixtures(name);
    let gif = dir.join("Recording z.gif");
    super::player::tests::gif(&gif, 20);
    let env = Env::sandbox(dir);
    let mut h = Harness::builder().with_size(SIZE).with_step_dt(1.0 / 60.0).build_eframe(move |cc| App::new(&cc.egui_ctx, env, None));
    settle(&mut h);
    (h, gif)
}

/// Until the player shows a frame.
fn wait_for_frame(h: &mut Harness<App>) {
    for _ in 0..200 {
        settle(h);
        if h.state().player.as_ref().is_some_and(|p| p.showing()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the player showed nothing");
}

#[test]
fn recordings_play_in_the_library() {
    let (mut h, gif) = with_recording("kittest-player");
    click(&mut h, "Recording z");
    assert!(h.state().player.as_ref().is_some_and(|p| p.path == gif) && h.state().editor.is_none(), "it plays in the viewer");
    wait_for_frame(&mut h);
    assert!(shows(&h, "0:00 / 0:02"));
    click(&mut h, icon::PLAY);
    assert!(h.state().player.as_ref().unwrap().playing());
    click(&mut h, icon::PAUSE);
    assert!(!h.state().player.as_ref().unwrap().playing());
    // Back to a screenshot: the editor again.
    click(&mut h, "Screenshot a");
    assert!(h.state().player.is_none() && h.state().editor.is_some());
}

#[test]
fn recordings_are_saved_elsewhere_copied_and_shown_in_their_folder() {
    let (mut h, gif) = with_recording("kittest-recording-actions");
    click(&mut h, "Recording z");
    let elsewhere = gif.with_file_name("Elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    h.state().env.sandboxed().picked_folder = Some(elsewhere.clone());
    let off = |h: &Harness<App>, name: &str| h.get_by_label(name).accesskit_node().is_disabled();
    assert!(off(&h, &super::theme::label(icon::FLOPPY_DISK, "Save")), "nothing to save: Trim saves its own copy");
    assert_eq!(off(&h, &super::theme::label(icon::COPY, "Copy")), cfg!(not(windows)), "copied as a file, where RSnip can");

    h.get_by_label_contains("Export").click();
    settle(&mut h);
    assert!(h.query_by_label_contains("JPG").is_none() && h.query_by_label_contains("OCR").is_none(), "only what applies to a recording");
    h.get_by_label_contains("Save copy to…").click();
    settle(&mut h);
    assert_eq!(std::fs::read(elsewhere.join("Recording z.gif")).unwrap(), std::fs::read(&gif).unwrap());
    // Again: beside the first copy.
    h.get_by_label_contains("Export").click();
    settle(&mut h);
    h.get_by_label_contains("Save copy to…").click();
    settle(&mut h);
    assert!(elsewhere.join("Recording z (2).gif").exists());

    h.get_by_label_contains("Export").click();
    settle(&mut h);
    h.get_by_label_contains("Show in folder").click();
    settle(&mut h);
    assert_eq!(h.state().env.sandboxed().shown_in_folder, [gif]);
}

/// Drags with the mouse on the open player's timeline, from one time to another (in seconds).
fn drag_on_timeline(h: &mut Harness<App>, from: f32, to: f32) {
    let timeline = h.state().player.as_ref().unwrap().timeline;
    let total = 2.0;
    let at = |t: f32| Pos2::new(timeline.left() + timeline.width() * t / total, timeline.center().y);
    h.hover_at(at(from));
    h.step();
    h.drag_at(at(from));
    h.step();
    for i in 1..=5 {
        h.hover_at(at(from + (to - from) * i as f32 / 5.0));
        h.step();
    }
    h.drop_at(at(to));
    settle(h);
}

#[test]
fn trim_handles_drag_and_the_rest_of_the_timeline_seeks() {
    let (mut h, _) = with_recording("kittest-trim-drag");
    click(&mut h, "Recording z");
    wait_for_frame(&mut h);
    click(&mut h, &format!("{} Trim", icon::SCISSORS));
    let trim = |h: &Harness<App>| h.state().player.as_ref().unwrap().trim.unwrap();
    assert_eq!(trim(&h), (Duration::ZERO, Duration::from_secs(2)));

    drag_on_timeline(&mut h, 0.0, 0.5);
    let (from, to) = trim(&h);
    assert!(from.abs_diff(Duration::from_millis(500)) < Duration::from_millis(60) && to == Duration::from_secs(2), "the start handle moved: {from:?}..{to:?}");
    drag_on_timeline(&mut h, 2.0, 1.5);
    let (from2, to) = trim(&h);
    assert!(from2 == from && to.abs_diff(Duration::from_millis(1500)) < Duration::from_millis(60), "the end handle moved: {from2:?}..{to:?}");

    // Between the handles, a drag scrubs and leaves the trim alone.
    drag_on_timeline(&mut h, 0.8, 1.2);
    assert_eq!(trim(&h), (from, to));
    let at = h.state().player.as_ref().unwrap().position();
    assert!(at.abs_diff(Duration::from_millis(1200)) < Duration::from_millis(60), "seeked to {at:?}");
    // The start handle can't pass the end one.
    drag_on_timeline(&mut h, 0.5, 1.9);
    assert_eq!(trim(&h).0, to);
}

#[test]
fn trimming_saves_the_part_kept_as_a_new_recording() {
    let (mut h, gif) = with_recording("kittest-trim");
    click(&mut h, "Recording z");
    wait_for_frame(&mut h);
    click(&mut h, &format!("{} Trim", icon::SCISSORS));
    for (at, button) in [(500, "Start here"), (1500, "End here")] {
        h.state_mut().player.as_mut().unwrap().seek(Duration::from_millis(at));
        settle(&mut h);
        click(&mut h, button);
    }
    assert!(shows(&h, "Keep 0:00.5 to 0:01.5 (1.0 s)"));
    click(&mut h, "Save trimmed copy");
    let copy = gif.with_file_name("Recording z (trimmed).gif");
    assert_eq!(super::player::info(&copy).duration, Some(Duration::from_secs(1)), "0.5 s to 1.5 s");
    assert!(h.state().player.as_ref().is_some_and(|p| p.path == copy), "the copy opens");
    assert!(gif.exists(), "the original stays");
}

/// Pixel snapshots of the window, rendered on a software GPU. Linux only, so every machine renders
/// the same pixels: run them with the `ci/Dockerfile` image, and refresh them after an intended
/// change with `UPDATE_SNAPSHOTS=1` (then look at every changed PNG in `tests/snapshots`).
#[cfg(target_os = "linux")]
mod snapshots {
    use super::*;

    fn window(dir: PathBuf, size: Vec2, setup: impl FnOnce(&Env)) -> Harness<'static, App> {
        let env = Env::sandbox(dir);
        setup(&env);
        let mut h = Harness::builder()
            .with_size(size)
            .with_pixels_per_point(1.0)
            .wgpu()
            .build_eframe(move |cc| App::new(&cc.egui_ctx, env, None));
        // Until every thumbnail has loaded and every screenshot's text has been read.
        for _ in 0..200 {
            settle(&mut h);
            if h.state().thumbs.len() == h.state().entries.len() && h.state().reading.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        h.remove_cursor();
        settle(&mut h);
        h
    }

    fn library(name: &str) -> Harness<'static, App> {
        window(fixtures(name), SIZE, |_| {})
    }

    fn snapshot(h: &mut Harness<App>, name: &str) {
        h.remove_cursor();
        settle(h);
        h.snapshot(name);
    }

    #[test]
    fn player_trimming() {
        let _gpu = super::super::tests::gpu();
        let dir = fixtures("snap-player");
        super::super::player::tests::gif(&dir.join("Recording z.gif"), 20);
        let mut h = window(dir, SIZE, |_| {});
        click(&mut h, "Recording z");
        wait_for_frame(&mut h);
        click(&mut h, &format!("{} Trim", icon::SCISSORS));
        h.state_mut().player.as_mut().unwrap().trim = Some((Duration::from_millis(500), Duration::from_millis(1500)));
        snapshot(&mut h, "player_trim");
    }

    #[test]
    fn library_on_the_left() {
        let _gpu = super::super::tests::gpu();
        snapshot(&mut library("snap-left"), "library_left");
    }

    #[test]
    fn library_along_the_bottom() {
        let _gpu = super::super::tests::gpu();
        let mut h = window(fixtures("snap-bottom"), SIZE, |env| env.set_setting(Dock::SETTING, Dock::Bottom.to_setting()));
        snapshot(&mut h, "library_bottom");
    }

    #[test]
    fn narrow_window() {
        let _gpu = super::super::tests::gpu();
        let mut h = window(fixtures("snap-narrow"), Vec2::new(760.0, 600.0), |_| {});
        snapshot(&mut h, "narrow");
    }

    #[test]
    fn text_tool_and_text() {
        let _gpu = super::super::tests::gpu();
        let mut h = library("snap-text");
        click(&mut h, "Text");
        let canvas = h.state().editor.as_ref().unwrap().canvas;
        let at = canvas.min + canvas.size() * Vec2::new(0.15, 0.3);
        h.hover_at(at);
        h.drag_at(at);
        h.drop_at(at);
        settle(&mut h);
        h.event(egui::Event::Text("Hello RSnip".into()));
        settle(&mut h);
        snapshot(&mut h, "text_tool");
    }

    #[test]
    fn selected_arrow() {
        let _gpu = super::super::tests::gpu();
        let mut h = library("snap-arrow");
        click(&mut h, "Arrow");
        drag_on_canvas(&mut h, (0.2, 0.7), (0.7, 0.25));
        snapshot(&mut h, "selected_arrow");
    }

    #[test]
    fn copy_text_tool() {
        let _gpu = super::super::tests::gpu();
        let words = super::super::words::example(&["Hello there", "second line here", "and a third"]);
        let mut h = window(fixtures("snap-copy-text"), SIZE, move |env| *env.sandboxed().words.lock().unwrap() = words);
        click(&mut h, "Copy text");
        settle(&mut h);
        drag_on_canvas(&mut h, at_pixel(80.0, 20.0), at_pixel(100.0, 50.0));
        snapshot(&mut h, "copy_text_tool");
    }

    #[test]
    fn export_menu() {
        let _gpu = super::super::tests::gpu();
        let mut h = library("snap-export");
        h.get_by_label_contains("Export").click();
        snapshot(&mut h, "export_menu");
    }

    #[test]
    fn colour_picker() {
        let _gpu = super::super::tests::gpu();
        let mut h = library("snap-colour");
        click(&mut h, "Pick any colour");
        snapshot(&mut h, "colour_picker");
    }

    #[test]
    fn empty_library() {
        let _gpu = super::super::tests::gpu();
        let mut h = window(tests::sandbox_dir("snap-empty"), SIZE, |_| {});
        snapshot(&mut h, "empty_library");
    }

    /// Renders the window to a PNG, to look at a UI change without opening it on a desktop or
    /// touching real screenshots:
    ///
    /// ```sh
    /// RSNIP_UI="size=1280x820 dock=bottom click=Text" RSNIP_UI_OUT=ui.png cargo test render_window -- --ignored
    /// ```
    ///
    /// `RSNIP_UI` is space-separated: `size=WxH`, `dock=left|bottom`, `theme=light|dark`, `empty` (no screenshots), and
    /// any number of `click=<label>` (a button's accessible name, e.g. a tool, `Undo`, `Export`, a
    /// colour; use `_` for spaces), applied in order.
    #[test]
    #[ignore]
    fn render_window() {
        let _gpu = super::super::tests::gpu();
        let spec = std::env::var("RSNIP_UI").unwrap_or_default();
        let out = std::env::var("RSNIP_UI_OUT").unwrap_or_else(|_| "target/ui.png".into());
        let mut size = SIZE;
        let mut dock = Dock::Left;
        let mut appearance = super::super::theme::Appearance::Dark;
        let mut empty = false;
        let mut clicks = Vec::new();
        for item in spec.split_whitespace() {
            match item.split_once('=') {
                Some(("size", wh)) => {
                    let (w, h) = wh.split_once('x').expect("size=WxH");
                    size = Vec2::new(w.parse().unwrap(), h.parse().unwrap());
                }
                Some(("dock", "bottom")) => dock = Dock::Bottom,
                Some(("dock", _)) => dock = Dock::Left,
                Some(("theme", "light")) => appearance = super::super::theme::Appearance::Light,
                Some(("theme", _)) => appearance = super::super::theme::Appearance::Dark,
                Some(("click", label)) => clicks.push(label.replace('_', " ")),
                None if item == "empty" => empty = true,
                _ => panic!("unknown RSNIP_UI item {item}"),
            }
        }
        let dir = if empty { tests::sandbox_dir("render-empty") } else { fixtures("render") };
        let mut h = window(dir, size, |env| {
            env.set_setting(Dock::SETTING, dock.to_setting());
            env.set_setting(super::super::theme::Appearance::SETTING, appearance.to_setting());
        });
        for label in &clicks {
            h.get_by_label_contains(label).click();
            settle(&mut h);
        }
        h.remove_cursor();
        settle(&mut h);
        h.render().expect("render").save(&out).unwrap();
        println!("wrote {out}");
    }
}
