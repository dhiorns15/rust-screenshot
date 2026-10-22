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
    let dir = fixtures(name);
    let mut h = Harness::builder().with_size(SIZE).build_eframe(move |cc| App::new(&cc.egui_ctx, Env::sandbox(dir), None));
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
    for windows_only in ["Copy as file", "Copy text (OCR)", "Pin to screen"] {
        assert_eq!(h.query_by_label_contains(windows_only).is_some(), cfg!(windows), "{windows_only}");
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
        // Until every thumbnail has loaded.
        for _ in 0..200 {
            settle(&mut h);
            if h.state().thumbs.len() == h.state().entries.len() {
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
    fn library_on_the_left() {
        snapshot(&mut library("snap-left"), "library_left");
    }

    #[test]
    fn library_along_the_bottom() {
        let mut h = window(fixtures("snap-bottom"), SIZE, |env| env.set_setting(Dock::SETTING, Dock::Bottom.to_setting()));
        snapshot(&mut h, "library_bottom");
    }

    #[test]
    fn narrow_window() {
        let mut h = window(fixtures("snap-narrow"), Vec2::new(760.0, 600.0), |_| {});
        snapshot(&mut h, "narrow");
    }

    #[test]
    fn text_tool_and_text() {
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
        let mut h = library("snap-arrow");
        click(&mut h, "Arrow");
        drag_on_canvas(&mut h, (0.2, 0.7), (0.7, 0.25));
        snapshot(&mut h, "selected_arrow");
    }

    #[test]
    fn export_menu() {
        let mut h = library("snap-export");
        h.get_by_label_contains("Export").click();
        snapshot(&mut h, "export_menu");
    }

    #[test]
    fn colour_picker() {
        let mut h = library("snap-colour");
        click(&mut h, "Pick any colour");
        snapshot(&mut h, "colour_picker");
    }

    #[test]
    fn empty_library() {
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
    /// `RSNIP_UI` is space-separated: `size=WxH`, `dock=left|bottom`, `empty` (no screenshots), and
    /// any number of `click=<label>` (a button's accessible name, e.g. a tool, `Undo`, `Export`, a
    /// colour; use `_` for spaces), applied in order.
    #[test]
    #[ignore]
    fn render_window() {
        let spec = std::env::var("RSNIP_UI").unwrap_or_default();
        let out = std::env::var("RSNIP_UI_OUT").unwrap_or_else(|_| "target/ui.png".into());
        let mut size = SIZE;
        let mut dock = Dock::Left;
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
                Some(("click", label)) => clicks.push(label.replace('_', " ")),
                None if item == "empty" => empty = true,
                _ => panic!("unknown RSNIP_UI item {item}"),
            }
        }
        let dir = if empty { tests::sandbox_dir("render-empty") } else { fixtures("render") };
        let mut h = window(dir, size, |env| env.set_setting(Dock::SETTING, dock.to_setting()));
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
