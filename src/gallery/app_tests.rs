//! The whole library window driven headlessly through its accessibility tree (egui_kittest), in a
//! sandbox folder with in-memory settings.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use eframe::egui::{Key, Modifiers, Pos2, Rect, Vec2};
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::Harness;

use super::env::Env;
use super::gallery::Sort;
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
    harness_set_up(name, step_dt, |_| {})
}

/// With `setup` run on the sandbox (its settings) before the window opens.
fn harness_with(name: &str, setup: impl FnOnce(&Env) + 'static) -> Harness<'static, App> {
    harness_set_up(name, 0.25, setup)
}

fn harness_set_up(name: &str, step_dt: f32, setup: impl FnOnce(&Env) + 'static) -> Harness<'static, App> {
    let env = Env::sandbox(fixtures(name));
    setup(&env);
    let mut h = Harness::builder().with_size(SIZE).with_step_dt(step_dt).build_eframe(move |cc| App::new(&cc.egui_ctx, env, None));
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
fn saving_a_jpg_writes_a_png_beside_it_and_leaves_the_jpg_as_it_was() {
    let dir = fixtures("kittest-save-jpg");
    let jpg = dir.join("Screenshot d.jpg");
    image::RgbImage::from_pixel(320, 200, image::Rgb([200, 90, 90])).save(&jpg).unwrap();
    let before = std::fs::read(&jpg).unwrap();
    let env = Env::sandbox(dir.clone());
    let mut h = Harness::builder().with_size(SIZE).build_eframe(move |cc| App::new(&cc.egui_ctx, env, Some(jpg.clone())));
    settle(&mut h);
    click(&mut h, "Arrow");
    drag_on_canvas(&mut h, (0.2, 0.2), (0.6, 0.5));
    click(&mut h, &label(icon::FLOPPY_DISK, "Save"));

    let png = dir.join("Screenshot d.png");
    assert!(shows(&h, "Saved Screenshot d.png"));
    assert!(std::fs::read(&png).unwrap().starts_with(b"\x89PNG"), "the edit is a PNG, under a PNG's name");
    assert_eq!(std::fs::read(dir.join("Screenshot d.jpg")).unwrap(), before, "the JPG isn't written over with PNG data");
    assert!(h.state().editor.as_ref().is_some_and(|e| e.doc.path == png && !e.doc.dirty()), "the editor goes on with the PNG");
    assert!(super::doc::has_layers(&png), "its edits stay editable, beside the PNG");

    // Saved again, it's the PNG that's written; a second JPG edit goes to a copy, not over it.
    drag_on_canvas(&mut h, (0.3, 0.6), (0.7, 0.8));
    click(&mut h, &label(icon::FLOPPY_DISK, "Save"));
    assert!(shows(&h, "Saved Screenshot d.png"));
    assert_eq!(png_path(&dir.join("Screenshot d.jpg")), dir.join("Screenshot d (edited).png"));
}

#[test]
fn export_menu_offers_what_this_system_supports() {
    let mut h = harness("kittest-export");
    h.get_by_label_contains("Export").click();
    settle(&mut h);
    assert!(h.query_by_label_contains("Save copy…").is_some());
    assert!(h.query_by_label_contains("Save copy as JPG").is_some());
    assert_eq!(h.query_by_label_contains("Pin to screen").is_some(), cfg!(windows), "Windows only");
    h.get_by_label_contains("Copy as file").click();
    settle(&mut h);
    let copied = h.state().env.sandboxed().copied_files.clone();
    assert!(copied.len() == 1 && copied[0].extension().is_some_and(|e| e == "png"), "the edited image as a PNG: {copied:?}");
    h.get_by_label_contains("Export").click();
    settle(&mut h);
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
fn dates_fold_and_unfold_in_either_layout() {
    for (dock, name) in [(Dock::Left, "kittest-fold-left"), (Dock::Bottom, "kittest-fold-bottom")] {
        let mut h = harness_with(name, move |env| env.set_setting(Dock::SETTING, dock.to_setting()));
        assert!(shows(&h, "Today, 2") && shows(&h, "Yesterday, 1") && shows(&h, "Screenshot c"), "dates with how many they have");

        click(&mut h, "Yesterday, 1");
        assert!(!shows(&h, "Screenshot c"), "folded away");
        assert!(shows(&h, "Yesterday, 1") && shows(&h, "Screenshot a") && shows(&h, "Screenshot b"), "its heading stays, the rest is untouched");

        click(&mut h, "Yesterday, 1");
        assert!(shows(&h, "Screenshot c"), "unfolded again");
    }
}

#[test]
fn folding_the_open_ones_date_leaves_it_open_but_not_selected() {
    let mut h = harness("kittest-fold-open");
    click(&mut h, "Screenshot a");
    assert_eq!(h.state().selected.len(), 1);
    click(&mut h, "Today, 2");
    assert!(!shows(&h, "Screenshot a"));
    assert!(h.state().selected.is_empty(), "not selected where it can't be seen");
    assert!(h.state().editor.as_ref().is_some_and(|e| e.doc.path.ends_with("Screenshot a.png")), "still open in the editor");
}

#[test]
fn shift_click_skips_what_is_folded_away() {
    let mut h = harness("kittest-fold-range");
    // Older than a month, so its own date, below yesterday's.
    let old = h.state().env.dir.join("Screenshot e.png");
    image::RgbaImage::from_pixel(320, 200, image::Rgba([90, 90, 90, 255])).save(&old).unwrap();
    std::fs::File::options().write(true).open(&old).unwrap().set_modified(SystemTime::now() - Duration::from_secs(400 * 24 * 60 * 60)).unwrap();
    h.state_mut().rescan();
    click(&mut h, "Yesterday, 1");
    click(&mut h, "Screenshot a");
    h.get_by_label("Screenshot e").click_modifiers(Modifiers::SHIFT);
    settle(&mut h);
    let names: HashSet<String> = h.state().selected.iter().map(|p| p.file_stem().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["Screenshot a", "Screenshot b", "Screenshot e"].map(String::from).into(), "not c, which is folded");
}

#[test]
fn folded_dates_are_remembered_except_today() {
    let mut h = harness("kittest-fold-saved");
    click(&mut h, "Yesterday, 1");
    click(&mut h, "Today, 2");
    let saved = h.state().env.text_setting(library::FOLDED_SETTING).unwrap();
    assert_eq!(saved, "Yesterday", "Today is always there when the library opens");

    // A new window with the same settings.
    let h = harness_with("kittest-fold-saved-again", move |env| env.set_text_setting(library::FOLDED_SETTING, &format!("Today\n{saved}")));
    assert!(!shows(&h, "Screenshot c") && shows(&h, "Yesterday, 1"), "still folded");
    assert!(shows(&h, "Screenshot a") && shows(&h, "Screenshot b"), "Today isn't, even if it was kept");
}

#[test]
fn searching_shows_what_is_folded_away() {
    let mut h = harness("kittest-fold-search");
    click(&mut h, "Yesterday, 1");
    assert!(!shows(&h, "Screenshot c"));
    search(&mut h, "screenshot");
    assert!(shows(&h, "Screenshot c"), "a match is shown, folded or not");
    assert!(shows(&h, "Screenshot a") && shows(&h, "Screenshot b"));
    assert!(!shows(&h, "Yesterday, 1"), "headings can't be folded while searching");
}

/// The window on the gallery page, with nothing open.
fn gallery(name: &str) -> Harness<'static, App> {
    harness_with(name, |env| env.set_setting(Page::SETTING, Page::Gallery.to_setting()))
}

fn selected_names(h: &Harness<App>) -> Vec<String> {
    let mut names: Vec<String> = h.state().selected.iter().map(|p| p.file_stem().unwrap().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

#[test]
fn the_toolbar_switches_between_the_editor_and_the_gallery() {
    let mut h = harness("kittest-page");
    assert!(shows(&h, "Library along the bottom") && shows(&h, "Arrow") && shows(&h, "Undo"));
    click(&mut h, "Gallery");
    assert!(h.state().page == Page::Gallery);
    assert_eq!(h.state().env.setting(Page::SETTING), Some(Page::Gallery.to_setting()), "remembered");
    assert!(!shows(&h, "Library along the bottom") && !shows(&h, "Arrow") && !shows(&h, "Undo"), "no library or tools here");
    assert!(shows(&h, "Editor") && shows(&h, "Today, 2") && shows(&h, "Screenshot c"));

    click(&mut h, "Editor");
    assert!(h.state().page == Page::Editor);
    assert_eq!(h.state().env.setting(Page::SETTING), Some(Page::Editor.to_setting()));
    assert!(shows(&h, "Library along the bottom") && shows(&h, "Arrow"));
    assert!(h.state().editor.as_ref().is_some_and(|e| e.doc.path.ends_with("Screenshot a.png")), "still open");
}

#[test]
fn the_gallery_page_is_remembered_and_opens_nothing_by_itself() {
    let h = gallery("kittest-page-saved");
    assert!(h.state().page == Page::Gallery);
    assert!(h.state().editor.is_none(), "the gallery is for choosing");
    assert!(shows(&h, "Screenshot a"));
    assert!(Page::from_setting(None) == Page::Editor && Page::from_setting(Some(7)) == Page::Editor, "junk means the editor");
}

#[test]
fn clicking_in_the_gallery_selects_without_opening() {
    let mut h = gallery("kittest-gallery-click");
    click(&mut h, "Screenshot b");
    assert_eq!(selected_names(&h), ["Screenshot b"]);
    assert!(h.state().editor.is_none() && h.state().page == Page::Gallery, "not opened");

    h.get_by_label("Screenshot c").click_modifiers(Modifiers::COMMAND);
    settle(&mut h);
    assert_eq!(selected_names(&h), ["Screenshot b", "Screenshot c"]);
    assert!(shows(&h, "2 selected"));
    click(&mut h, "Screenshot a");
    h.get_by_label("Screenshot c").click_modifiers(Modifiers::SHIFT);
    settle(&mut h);
    assert_eq!(selected_names(&h), ["Screenshot a", "Screenshot b", "Screenshot c"], "a range, across dates");
}

#[test]
fn double_clicking_in_the_gallery_opens_it_in_the_editor() {
    let mut h = harness_set_up("kittest-gallery-open", 1.0 / 60.0, |env| env.set_setting(Page::SETTING, Page::Gallery.to_setting()));
    let at = rect_of(&h, "Screenshot b").center();
    click_at(&mut h, at, 2);
    settle(&mut h);
    assert!(h.state().page == Page::Editor, "on to the editor");
    assert!(h.state().editor.as_ref().is_some_and(|e| e.doc.path.ends_with("Screenshot b.png")));
    assert_eq!(h.state().env.setting(Page::SETTING), Some(Page::Editor.to_setting()));
}

#[test]
fn recordings_open_in_the_player_from_the_gallery() {
    let (mut h, gif) = with_recording("kittest-gallery-recording");
    click(&mut h, "Gallery");
    assert!(shows(&h, "Recording z"));
    // Long enough after the click on Gallery for the double click not to count with it.
    for _ in 0..30 {
        settle(&mut h);
    }
    let at = rect_of(&h, "Recording z").center();
    click_at(&mut h, at, 2);
    settle(&mut h);
    assert!(h.state().page == Page::Editor);
    assert!(h.state().player.as_ref().is_some_and(|p| p.path == gif));
}

#[test]
fn arrows_enter_ctrl_a_and_escape_work_the_gallery() {
    let mut h = gallery("kittest-gallery-keys");
    // Three across, so a and b share a line and c, a date of its own, has the next.
    h.key_press(Key::ArrowRight);
    settle(&mut h);
    assert_eq!(selected_names(&h), ["Screenshot a"], "the first, with nothing selected");
    h.key_press(Key::ArrowRight);
    settle(&mut h);
    assert_eq!(selected_names(&h), ["Screenshot b"]);
    h.key_press(Key::ArrowRight);
    settle(&mut h);
    assert_eq!(selected_names(&h), ["Screenshot c"], "on to the next line");
    h.key_press(Key::ArrowLeft);
    settle(&mut h);
    assert_eq!(selected_names(&h), ["Screenshot b"]);
    h.key_press(Key::ArrowDown);
    settle(&mut h);
    assert_eq!(selected_names(&h), ["Screenshot c"]);
    h.key_press(Key::ArrowUp);
    settle(&mut h);
    assert_eq!(selected_names(&h), ["Screenshot a"], "the same column, on the line above");
    assert!(h.state().editor.is_none(), "moving doesn't open anything");

    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    settle(&mut h);
    assert_eq!(h.state().selected.len(), 3);
    h.key_press(Key::Escape);
    settle(&mut h);
    assert!(h.state().selected.is_empty());

    click(&mut h, "Screenshot c");
    h.key_press(Key::Enter);
    settle(&mut h);
    assert!(h.state().page == Page::Editor);
    assert!(h.state().editor.as_ref().is_some_and(|e| e.doc.path.ends_with("Screenshot c.png")));
}

#[test]
fn the_gallery_leaves_the_keys_to_a_text_box() {
    let mut h = gallery("kittest-gallery-typing");
    use egui::accesskit::Role::TextInput;
    h.get_by_role(TextInput).focus();
    settle(&mut h);
    h.key_press(Key::ArrowRight);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    settle(&mut h);
    assert!(h.state().selected.is_empty(), "the arrows and Ctrl+A were for the search box");
}

/// Tall enough for all six under four headings: two and three days ago can be this week, earlier
/// this month or last month, depending on the day the tests run.
const TALL: Vec2 = Vec2::new(SIZE.x, SIZE.y + 400.0);

/// The gallery of `fixtures` and a recording, with Screenshot 2 and Screenshot 10 from earlier
/// days, and files of different sizes: b is noise and c is big, so the sizes are all different.
fn sortable(name: &str, setup: impl FnOnce(&Env) + 'static) -> Harness<'static, App> {
    let dir = fixtures(name);
    let day = Duration::from_secs(24 * 60 * 60);
    let write = |file: &str, img: image::RgbaImage, ago: Duration| {
        let path = dir.join(file);
        img.save(&path).unwrap();
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(SystemTime::now() - ago).unwrap();
    };
    let solid = |w, h, shade: u8| image::RgbaImage::from_pixel(w, h, image::Rgba([shade, 90, 200, 255]));
    write("Screenshot b.png", image::RgbaImage::from_fn(320, 200, |x, y| image::Rgba([(x * 7 + y * 13) as u8, (x ^ y) as u8, (x * y) as u8, 255])), Duration::from_secs(60));
    write("Screenshot c.png", solid(640, 400, 220), day);
    write("Screenshot 2.png", solid(100, 100, 10), 2 * day);
    write("Screenshot 10.png", solid(100, 50, 30), 3 * day);
    super::player::tests::gif(&dir.join("Recording z.gif"), 20);
    let env = Env::sandbox(dir);
    env.set_setting(Page::SETTING, Page::Gallery.to_setting());
    setup(&env);
    let mut h = Harness::builder().with_size(TALL).with_step_dt(1.0 / 60.0).build_eframe(move |cc| App::new(&cc.egui_ctx, env, None));
    settle(&mut h);
    h
}

/// The names of what's on screen, in reading order.
fn reading_order(h: &Harness<App>) -> Vec<String> {
    let mut shown: Vec<(f32, f32, String)> = h
        .state()
        .entries
        .iter()
        .filter_map(|e| h.query_by_label(&e.name).map(|node| (node.rect().top().round(), node.rect().left(), e.name.clone())))
        .collect();
    shown.sort_by(|a, b| a.partial_cmp(b).unwrap());
    shown.into_iter().map(|(_, _, name)| name).collect()
}

/// The names of everything, by file size.
fn by_size(h: &Harness<App>, largest_first: bool) -> Vec<String> {
    let mut sizes: Vec<(u64, String)> = h.state().entries.iter().map(|e| (e.len, e.name.clone())).collect();
    sizes.sort();
    assert!(sizes.windows(2).all(|w| w[0].0 != w[1].0), "the files' sizes differ: {sizes:?}");
    if largest_first {
        sizes.reverse();
    }
    sizes.into_iter().map(|(_, name)| name).collect()
}

#[test]
fn the_gallery_sorts_by_date_either_way() {
    let mut h = sortable("kittest-sort-date", |_| {});
    let newest_first = ["Recording z", "Screenshot a", "Screenshot b", "Screenshot c", "Screenshot 2", "Screenshot 10"].map(String::from).to_vec();
    assert_eq!(reading_order(&h), newest_first);
    assert!(shows(&h, "Today, 3") && shows(&h, "Yesterday, 1"), "headings by date");

    click(&mut h, "Newest first");
    assert!(shows(&h, "Oldest first"), "the button says what it is now");
    assert_eq!(reading_order(&h), newest_first.iter().rev().cloned().collect::<Vec<_>>());
    assert!(rect_of(&h, "Yesterday, 1").top() < rect_of(&h, "Today, 3").top(), "the oldest dates first");
}

#[test]
fn the_gallery_sorts_by_name_without_headings() {
    let mut h = sortable("kittest-sort-name", |_| {});
    click(&mut h, "Name");
    let a_to_z = ["Recording z", "Screenshot 2", "Screenshot 10", "Screenshot a", "Screenshot b", "Screenshot c"].map(String::from).to_vec();
    assert_eq!(reading_order(&h), a_to_z, "numbers by their value, ignoring case");
    assert!(shows(&h, "A–Z") && !shows(&h, "Today, 3") && !shows(&h, "Yesterday, 1"), "no headings");

    click(&mut h, "A–Z");
    assert!(shows(&h, "Z–A"));
    assert_eq!(reading_order(&h), a_to_z.iter().rev().cloned().collect::<Vec<_>>());
}

#[test]
fn the_gallery_sorts_by_size_without_headings() {
    let mut h = sortable("kittest-sort-size", |_| {});
    click(&mut h, "Size");
    assert_eq!(reading_order(&h), by_size(&h, true));
    assert!(shows(&h, "Largest first") && !shows(&h, "Today, 3") && !shows(&h, "Yesterday, 1"), "no headings");

    click(&mut h, "Largest first");
    assert!(shows(&h, "Smallest first"));
    assert_eq!(reading_order(&h), by_size(&h, false));
}

#[test]
fn the_gallery_sorts_by_type_with_headings_that_fold() {
    let mut h = sortable("kittest-sort-type", |_| {});
    click(&mut h, "Type");
    assert!(shows(&h, "Screenshots first") && shows(&h, "Screenshots, 5") && shows(&h, "Recordings, 1"), "headings by type");
    let screenshots = ["Screenshot a", "Screenshot b", "Screenshot c", "Screenshot 2", "Screenshot 10"];
    let expected: Vec<String> = screenshots.iter().chain(&["Recording z"]).map(|s| s.to_string()).collect();
    assert_eq!(reading_order(&h), expected, "newest first within each");

    click(&mut h, "Recordings, 1");
    assert!(!shows(&h, "Recording z") && shows(&h, "Screenshot a"), "a type folds like a date");
    assert_eq!(h.state().env.text_setting(library::FOLDED_SETTING).as_deref(), Some("type:Recordings"));
    click(&mut h, "Recordings, 1");

    click(&mut h, "Screenshots first");
    assert!(shows(&h, "Recordings first"));
    let mut expected = vec!["Recording z".to_string()];
    expected.extend(screenshots.map(String::from));
    assert_eq!(reading_order(&h), expected);
}

#[test]
fn the_gallery_remembers_how_it_is_sorted() {
    let mut h = sortable("kittest-sort-saved", |_| {});
    click(&mut h, "Size");
    click(&mut h, "Largest first");
    assert_eq!(h.state().env.setting(Sort::SETTING), Some(Sort::Size.to_setting()));
    assert_eq!(h.state().env.setting(Sort::DESCENDING), Some(0));

    let (sort, descending) = (h.state().env.setting(Sort::SETTING), h.state().env.setting(Sort::DESCENDING));
    let h = sortable("kittest-sort-saved-again", move |env| {
        env.set_setting(Sort::SETTING, sort.unwrap());
        env.set_setting(Sort::DESCENDING, descending.unwrap());
    });
    assert!(shows(&h, "Smallest first"));
    assert_eq!(reading_order(&h), by_size(&h, false));
}

#[test]
fn a_narrow_window_keeps_every_gallery_toolbar_button_clear() {
    let mut h = gallery("kittest-gallery-narrow");
    h.set_size(Vec2::new(760.0, 600.0));
    settle(&mut h);
    click(&mut h, "Screenshot a");
    h.get_by_label("Screenshot c").click_modifiers(Modifiers::COMMAND);
    settle(&mut h);
    let names = ["Editor", "Gallery", "Date", "Name", "Size", "Type", "Newest first", "Open the folder", "Settings", "2 selected", "Copy as files", "Delete", "Clear"];
    let buttons: Vec<(&str, Rect)> = names.iter().map(|&name| (name, rect_of(&h, name))).collect();
    for (i, (a, ra)) in buttons.iter().enumerate() {
        assert!(ra.right() <= 760.0 && ra.bottom() <= 600.0, "{a} is inside the window");
        for (b, rb) in &buttons[i + 1..] {
            assert!(!ra.intersects(rb.shrink(0.5)), "{a} overlaps {b}");
        }
    }
}

/// The gallery with Screenshot b noted as captured from Chrome, read to say "Invoice 2026 / Total
/// due", and pushed to Immich, which has a link. Returns it and Screenshot b's path.
fn described(name: &str) -> (Harness<'static, App>, PathBuf) {
    let dir = fixtures(name);
    let env = Env::sandbox(dir.clone());
    env.sandboxed().ocr_text.lock().unwrap().insert("Screenshot b.png".into(), "Invoice 2026\nTotal due".into());
    env.set_setting(Page::SETTING, Page::Gallery.to_setting());
    let immich = crate::push::Destination::new(crate::push::Kind::Immich { server: "https://photos.example.com".into() });
    crate::push::save(&env, std::slice::from_ref(&immich));
    let mut h = Harness::builder().with_size(SIZE).build_eframe(move |cc| App::new(&cc.egui_ctx, env, None));
    for _ in 0..200 {
        settle(&mut h);
        if h.state().reading.is_empty() {
            break;
        }
    }
    let b = dir.join("Screenshot b.png");
    super::search::record_source(&b, "Google Chrome", "Invoices - Acme");
    super::search::record_push(&b, &immich.id, Some("https://example.com/b".into())).unwrap();
    h.state_mut().rescan();
    settle(&mut h);
    (h, b)
}

#[test]
fn the_info_panel_describes_a_screenshot() {
    let (mut h, b) = described("kittest-info");
    assert!(shows(&h, "Select a screenshot or recording to see its details."), "nothing selected yet");
    click(&mut h, "Screenshot b");
    let entry = h.state().entries.iter().find(|e| e.path == b).unwrap();
    let (len, modified) = (entry.len, entry.modified);
    for text in [
        "Screenshot b.png".to_string(),
        "PNG screenshot".into(),
        "320 × 200 px".into(),
        super::info::human_size(len),
        super::info::date_and_time(modified),
        "Google Chrome — Invoices - Acme".into(),
        "Immich".into(),
        "Invoice 2026\nTotal due".into(),
    ] {
        assert!(shows(&h, &text), "{text:?}");
    }
    assert!(!shows(&h, "Edited") && !shows(&h, "Length"), "not edited, and nothing to play");

    click(&mut h, "Copy link");
    assert_eq!(h.state().env.sandboxed().copied.as_deref(), Some("https://example.com/b"));
    click(&mut h, "Copy text");
    assert_eq!(h.state().env.sandboxed().copied.as_deref(), Some("Invoice 2026\nTotal due"));
    click(&mut h, "Show in folder");
    assert_eq!(h.state().env.sandboxed().shown_in_folder, std::slice::from_ref(&b));
    click(&mut h, "Copy as file");
    assert_eq!(h.state().env.sandboxed().copied_files, std::slice::from_ref(&b));
    assert!(shows(&h, "Delete"));

    // What isn't known is left out.
    click(&mut h, "Screenshot a");
    assert!(shows(&h, "Screenshot a.png") && !shows(&h, "From") && !shows(&h, "Pushed to") && !shows(&h, "Text"));

    click(&mut h, "Open");
    assert!(h.state().page == Page::Editor && h.state().editor.as_ref().is_some_and(|e| e.doc.path.ends_with("Screenshot a.png")));
}

#[test]
fn the_info_panel_says_when_a_screenshot_has_annotations() {
    let mut h = gallery("kittest-info-edited");
    let a = h.state().env.dir.join("Screenshot a.png");
    let mut doc = super::doc::Doc::open(&a).unwrap();
    doc.add(super::doc::Shape::Redact { rect: egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(20.0, 20.0)), color: egui::Color32::BLACK });
    render(doc.current(), None).save(&a).unwrap();
    doc.mark_saved(a.clone());
    doc.save_layers(false).unwrap();
    h.state_mut().rescan();
    click(&mut h, "Screenshot a");
    assert!(shows(&h, "With annotations you can still change"));
    click(&mut h, "Screenshot b");
    assert!(!shows(&h, "Edited"), "only the one that has them");
}

#[test]
fn the_info_panel_describes_a_recording() {
    let (mut h, gif) = with_recording("kittest-info-recording");
    click(&mut h, "Gallery");
    click(&mut h, "Recording z");
    let (width, height) = super::player::info(&gif).size.unwrap();
    for text in ["Recording z.gif".to_string(), "GIF recording".into(), "0:02".into(), format!("{width} × {height} px")] {
        assert!(shows(&h, &text), "{text:?}");
    }
    assert!(shows(&h, "Length"));
}

#[test]
fn the_info_panel_says_which_recordings_have_sound() {
    let (mut h, _) = with_recording("kittest-info-sound");
    let dir = h.state().env.dir.clone();
    crate::mp4::tests::recording(&dir.join("Recording loud.mp4"), true);
    crate::mp4::tests::recording(&dir.join("Recording quiet.mp4"), false);
    h.state_mut().rescan();
    click(&mut h, "Gallery");
    click(&mut h, "Recording loud");
    assert!(shows(&h, "0:03, with sound"));
    click(&mut h, "Recording quiet");
    assert!(shows(&h, "0:03") && !shows(&h, "0:03, with sound"));
    click(&mut h, "Recording z");
    assert!(shows(&h, "0:02"), "a GIF is silent");
}

#[test]
fn the_info_panel_totals_several_selected() {
    let mut h = gallery("kittest-info-several");
    click(&mut h, "Screenshot a");
    h.get_by_label("Screenshot c").click_modifiers(Modifiers::COMMAND);
    settle(&mut h);
    let total: u64 = h.state().entries.iter().filter(|e| e.name != "Screenshot b").map(|e| e.len).sum();
    assert!(shows(&h, &format!("2 selected, {} in all", super::info::human_size(total))));
    assert!(shows(&h, "Delete (2)") && !shows(&h, "Screenshot a.png"), "the bulk actions, not one's details");

    click(&mut h, "Copy as files (2)");
    let names: Vec<String> = h.state().env.sandboxed().copied_files.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["Screenshot a.png", "Screenshot c.png"]);
}

#[test]
fn the_info_panel_needs_a_wide_window() {
    let mut h = gallery("kittest-info-narrow");
    click(&mut h, "Screenshot a");
    assert!(shows(&h, "Show in folder"));
    h.set_size(Vec2::new(800.0, 600.0));
    settle(&mut h);
    assert!(!shows(&h, "Show in folder") && shows(&h, "Screenshot a"), "the gallery keeps the room");
    h.set_size(SIZE);
    settle(&mut h);
    assert!(shows(&h, "Show in folder"));
}

#[test]
fn the_library_page_has_no_info_panel() {
    let mut h = harness("kittest-info-editor");
    click(&mut h, "Screenshot b");
    assert!(!shows(&h, "Show in folder") && !shows(&h, "File size"));
}

#[test]
fn the_gallery_has_the_right_click_menu() {
    let mut h = gallery("kittest-gallery-menu");
    h.get_by_label("Screenshot b").click_secondary();
    settle(&mut h);
    for item in ["Open in default app", "Show in folder", "Copy as files", "Delete"] {
        assert!(shows(&h, item), "{item}");
    }
    assert!(h.query_by_label_contains("Push to").is_some());
    click(&mut h, "Show in folder");
    assert_eq!(h.state().env.sandboxed().shown_in_folder, [h.state().env.dir.join("Screenshot b.png")]);
}

#[test]
fn editor_keys_do_nothing_on_the_gallery_page() {
    let mut h = harness("kittest-gallery-shortcuts");
    h.key_press(Key::B);
    settle(&mut h);
    assert!(h.state().tools.tool == Tool::Box, "B picks the Box tool in the editor");
    click(&mut h, "Gallery");
    h.key_press(Key::P);
    settle(&mut h);
    assert!(h.state().tools.tool == Tool::Box, "but P doesn't pick the Pen tool behind the gallery");
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

#[test]
fn a_new_version_is_installed_from_settings_where_it_can_be() {
    let release = crate::update::Release { version: crate::update::Version::new(99, 0, 0), page: "https://example.com/v99".into(), installer: None, deb: None, rpm: None };
    for can_install in [true, false] {
        let mut h = harness(&format!("kittest-update-{can_install}"));
        h.state().env.sandboxed().can_install = can_install;
        h.state_mut().newer = Some(release.clone());
        settle(&mut h);
        h.get_by_label("RSnip 99.0.0 is available").click();
        settle(&mut h);
        let sandbox = h.state().env.sandboxed();
        if can_install {
            assert_eq!((sandbox.update_settings_opened, sandbox.opened.len()), (1, 0), "Settings → Updates, to install it");
        } else {
            assert_eq!((sandbox.update_settings_opened, &sandbox.opened[..]), (0, &[std::path::PathBuf::from("https://example.com/v99")][..]), "the download page");
        }
    }
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

/// Opens the Redact tool on screenshot a, with `lines` of text "read" from it, and clicks Auto-redact.
fn auto_redact(name: &str, lines: &[&str]) -> Harness<'static, App> {
    let mut h = harness_at(name, 1.0 / 60.0);
    *h.state().env.sandboxed().words.lock().unwrap() = super::words::example(lines);
    click(&mut h, "Redact");
    click(&mut h, &label(icon::MAGIC_WAND, "Auto-redact"));
    // Reads the text first, then covers what it finds.
    for _ in 0..100 {
        settle(&mut h);
        if h.state().editor.as_ref().unwrap().auto_redact_note().is_some() {
            break;
        }
    }
    settle(&mut h);
    h
}

#[test]
fn auto_redact_covers_what_it_finds_in_one_undo_step() {
    let mut h = auto_redact("kittest-auto-redact", &["Mail dan@example.com now", "server 10.0.0.1 and 555-123-4567"]);
    let shapes = |h: &Harness<App>| h.state().editor.as_ref().unwrap().doc.current().shapes.clone();
    let color = h.state().tools.color();
    let rects: Vec<_> = shapes(&h)
        .into_iter()
        .map(|s| match s {
            super::doc::Shape::Redact { rect, color: c } if c == color => rect,
            _ => panic!("not a redact box in the tool's colour"),
        })
        .collect();
    assert_eq!(rects.len(), 3);
    // "dan@example.com" starts 60 px in, and is 150 px wide; boxes have 2 px around them.
    assert_eq!(rects[0], Rect::from_min_max(Pos2::new(58.0, 8.0), Pos2::new(212.0, 32.0)));
    assert!(shows(&h, "Redacted 3: undo brings them back"));

    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    settle(&mut h);
    assert!(shapes(&h).is_empty(), "one undo takes them all back");
    assert!(!shows(&h, "Redacted 3: undo brings them back"));
}

#[test]
fn auto_redact_says_when_theres_nothing_to_redact() {
    let h = auto_redact("kittest-auto-redact-nothing", &["Nothing secret here", "version 1.2.3"]);
    assert!(h.state().editor.as_ref().unwrap().doc.current().shapes.is_empty());
    assert!(shows(&h, "Nothing to redact"));
}

#[test]
fn auto_redact_needs_text_reading() {
    let mut h = harness_at("kittest-auto-redact-unavailable", 1.0 / 60.0);
    click(&mut h, "Redact");
    assert!(shows(&h, &label(icon::MAGIC_WAND, "Auto-redact")));
    h.state_mut().editor.as_mut().unwrap().reader = None;
    settle(&mut h);
    assert!(!shows(&h, &label(icon::MAGIC_WAND, "Auto-redact")));
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
fn the_library_copies_files_on_every_system() {
    let mut h = harness("kittest-copy-files");
    click(&mut h, "Screenshot a");
    h.get_by_label("Screenshot c").click_modifiers(Modifiers::COMMAND);
    settle(&mut h);
    h.get_by_label("Screenshot c").click_secondary();
    settle(&mut h);
    h.get_by_label("Copy as files (2)").click();
    settle(&mut h);
    let names: Vec<String> = h.state().env.sandboxed().copied_files.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["Screenshot a.png", "Screenshot c.png"]);
    assert!(h.query_by_label("Copied 2 files").is_some());
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
    h.get_by_label(&super::theme::label(icon::COPY, "Copy")).click();
    settle(&mut h);
    assert_eq!(h.state().env.sandboxed().copied_files, std::slice::from_ref(&gif), "copied as a file, on Linux too");
    assert!(h.query_by_label_contains("Copied as a file").is_some());

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

#[test]
fn the_speaker_button_mutes_the_sound_and_is_remembered() {
    let dir = fixtures("kittest-mute");
    super::player::tests::gif(&dir.join("Recording g.gif"), 20);
    crate::mp4::tests::recording(&dir.join("Recording s.mp4"), true);
    crate::mp4::tests::recording(&dir.join("Recording q.mp4"), false);
    let env = Env::sandbox(dir);
    let mut h = Harness::builder().with_size(SIZE).with_step_dt(1.0 / 60.0).build_eframe(move |cc| App::new(&cc.egui_ctx, env, None));
    settle(&mut h);
    let speakers = |h: &Harness<App>| (shows(h, icon::SPEAKER_HIGH), shows(h, icon::SPEAKER_SLASH));
    let muted = |h: &Harness<App>| h.state().env.setting("PlayerMuted");

    // Only for a recording with sound.
    for silent in ["Recording g", "Recording q"] {
        click(&mut h, silent);
        assert!(h.state().player.as_ref().is_some_and(|p| p.path.to_string_lossy().contains(silent)));
        assert_eq!(speakers(&h), (false, false), "{silent} has no sound");
    }
    click(&mut h, "Recording s");
    assert_eq!(speakers(&h), (true, false));
    assert_eq!(muted(&h), None);

    click(&mut h, icon::SPEAKER_HIGH);
    assert_eq!(speakers(&h), (false, true));
    assert!(h.state().player.as_ref().unwrap().muted);
    assert_eq!(muted(&h), Some(1));

    // The next recording with sound opens muted, until it's unmuted.
    click(&mut h, "Recording q");
    click(&mut h, "Recording s");
    assert_eq!(speakers(&h), (false, true), "still muted");
    click(&mut h, icon::SPEAKER_SLASH);
    assert_eq!(speakers(&h), (true, false));
    assert_eq!(muted(&h), Some(0));
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
        // Until every thumbnail shown has loaded and every screenshot's text has been read.
        for _ in 0..200 {
            settle(&mut h);
            let app = h.state();
            if (app.thumbs.len() == app.entries.len() || (!app.thumbs.is_empty() && app.requested.is_empty())) && app.reading.is_empty() {
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
    fn player_with_sound() {
        let _gpu = super::super::tests::gpu();
        let dir = fixtures("snap-player-sound");
        super::super::player::tests::gif(&dir.join("Recording z.gif"), 20);
        let mut h = window(dir, SIZE, |_| {});
        click(&mut h, "Recording z");
        wait_for_frame(&mut h);
        // A GIF stands in for a recording with sound, which can't be drawn without a decoder.
        h.state_mut().player.as_mut().unwrap().info.sound = true;
        snapshot(&mut h, "player_sound");
        click(&mut h, icon::SPEAKER_HIGH);
        snapshot(&mut h, "player_muted");
    }

    #[test]
    fn library_on_the_left() {
        let _gpu = super::super::tests::gpu();
        snapshot(&mut library("snap-left"), "library_left");
    }

    #[test]
    fn the_gallery() {
        let _gpu = super::super::tests::gpu();
        let mut h = window(fixtures("snap-gallery"), SIZE, |env| env.set_setting(Page::SETTING, Page::Gallery.to_setting()));
        snapshot(&mut h, "gallery");
    }

    #[test]
    fn the_gallery_with_a_screenshot_described() {
        let _gpu = super::super::tests::gpu();
        let dir = fixtures("snap-gallery-info");
        // Taller than usual, to show all three dates. On a day long gone, so the date shown
        // does not change from one day to the next.
        let b = dir.join("Screenshot b.png");
        let taken = SystemTime::UNIX_EPOCH + Duration::from_secs(1_579_080_600);
        std::fs::File::options().write(true).open(&b).unwrap().set_modified(taken).unwrap();
        let immich = crate::push::Destination::new(crate::push::Kind::Immich { server: "https://photos.example.com".into() });
        super::super::search::record_source(&b, "Google Chrome", "Invoices - Acme");
        super::super::search::record_push(&b, &immich.id, Some("https://example.com/b".into())).unwrap();
        let mut meta = super::super::search::load(&b);
        meta.text = Some("Invoice 2026\nTotal due: 1,240.00\nPay by 30 November".into());
        meta.stamp = super::super::doc::FileStamp::of(&b);
        super::super::search::save(&b, &meta).unwrap();
        let mut h = window(dir, Vec2::new(1280.0, 900.0), move |env| {
            env.set_setting(Page::SETTING, Page::Gallery.to_setting());
            crate::push::save(env, std::slice::from_ref(&immich));
        });
        click(&mut h, "Screenshot b");
        snapshot(&mut h, "gallery_info");
    }

    #[test]
    fn library_with_a_date_folded() {
        let _gpu = super::super::tests::gpu();
        let mut h = window(fixtures("snap-folded"), SIZE, |env| env.set_text_setting(library::FOLDED_SETTING, "Yesterday"));
        snapshot(&mut h, "library_folded");
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
    /// `RSNIP_UI` is space-separated: `size=WxH`, `dock=left|bottom`, `page=editor|gallery`, `theme=light|dark`, `empty` (no screenshots), and
    /// any number of `click=<label>` (a button's accessible name, e.g. a tool, `Undo`, `Export`, a
    /// colour; use `_` for spaces, and a `ctrl+` before it to Ctrl+click), applied in order.
    #[test]
    #[ignore]
    fn render_window() {
        let _gpu = super::super::tests::gpu();
        let spec = std::env::var("RSNIP_UI").unwrap_or_default();
        let out = std::env::var("RSNIP_UI_OUT").unwrap_or_else(|_| "target/ui.png".into());
        let mut size = SIZE;
        let mut dock = Dock::Left;
        let mut page = Page::Editor;
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
                Some(("page", "gallery")) => page = Page::Gallery,
                Some(("page", _)) => page = Page::Editor,
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
            env.set_setting(Page::SETTING, page.to_setting());
            env.set_setting(super::super::theme::Appearance::SETTING, appearance.to_setting());
        });
        for label in &clicks {
            // `ctrl+Screenshot_b` adds it to the selection.
            match label.strip_prefix("ctrl+") {
                Some(label) => h.get_by_label_contains(label).click_modifiers(Modifiers::COMMAND),
                None => h.get_by_label_contains(label).click(),
            }
            settle(&mut h);
        }
        h.remove_cursor();
        settle(&mut h);
        h.render().expect("render").save(&out).unwrap();
        println!("wrote {out}");
    }
}

#[test]
fn a_screenshot_deleted_elsewhere_doesnt_stay_open() {
    let mut h = harness("kittest-vanished");
    click(&mut h, "Screenshot b");
    let b = h.state().editor.as_ref().unwrap().doc.path.clone();
    // As the clean-up, or Explorer, would.
    std::fs::remove_file(&b).unwrap();
    h.state_mut().rescan();
    settle(&mut h);
    assert!(h.state().editor.is_none(), "saving it would bring it back");
    assert!(h.query_by_label("Screenshot b").is_none());

    // Unless it has edits, which would be lost.
    click(&mut h, "Screenshot a");
    let a = h.state().editor.as_ref().unwrap().doc.path.clone();
    let redact = super::doc::Shape::Redact { rect: egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(20.0, 20.0)), color: egui::Color32::BLACK };
    h.state_mut().editor.as_mut().unwrap().doc.add(redact);
    std::fs::remove_file(&a).unwrap();
    h.state_mut().rescan();
    settle(&mut h);
    assert!(h.state().editor.as_ref().is_some_and(|e| e.doc.path == a));
}

#[test]
fn the_info_panel_shows_the_description_and_which_model_wrote_it() {
    let (mut h, b) = described("kittest-info-description");
    super::search::record_description(&b, "A bar chart of sales.\nKeywords: chart, sales", "claude-opus-5-5").unwrap();
    h.state_mut().metas.insert(b.clone(), super::search::load(&b));
    click(&mut h, "Screenshot b");
    assert!(shows(&h, "Description") && shows(&h, "A bar chart of sales.\nKeywords: chart, sales") && shows(&h, "Described by claude-opus-5-5"));
    assert!(shows(&h, "Invoice 2026\nTotal due"), "and the text read from it");
    click(&mut h, "Screenshot a");
    assert!(!shows(&h, "Description") && !shows(&h, "Described by claude-opus-5-5"), "only what has one");

    // A long one, as Claude writes them (some 350 to 700 characters), is shown whole.
    let long = format!("{}\nKeywords: terminal, Rust, cargo build, release folder, rsnip, rust-screenshot", "A Windows desktop with a terminal building a Rust project. ".repeat(10));
    super::search::record_description(&b, &long, "claude-sonnet-5-5").unwrap();
    h.state_mut().metas.insert(b.clone(), super::search::load(&b));
    click(&mut h, "Screenshot b");
    assert!(shows(&h, long.trim()), "keywords and all");
}

#[test]
fn searches_the_descriptions_too_and_reading_text_keeps_them() {
    let (mut h, dir) = library_with_text("kittest-search-description", "Invoice 2026", |env| {
        let shot = env.dir.join("Screenshot c.png");
        super::search::record_description(&shot, "A bar chart of quarterly sales.\nKeywords: chart, revenue", "claude-opus-5-5").unwrap();
    });
    let c = super::search::load(&dir.join("Screenshot c.png"));
    assert!(c.text.is_some(), "its text was read");
    assert_eq!(c.described_by.as_deref(), Some("claude-opus-5-5"), "and the description is still there");
    search(&mut h, "REVENUE bar");
    assert!(shows(&h, "Screenshot c"));
    assert!(!shows(&h, "Screenshot a") && !shows(&h, "Screenshot b"));
}
