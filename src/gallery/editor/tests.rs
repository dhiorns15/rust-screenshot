//! Headless tests driving the real editor UI through egui input events.

use ab_glyph::FontVec;
use eframe::egui::{self, Color32, Context, Event, FontId, Key, PointerButton, Pos2, RawInput, Rect, Vec2};
use image::RgbaImage;

use super::{size_slider, Editor, Tool, ToolState, ANNOTATION_FONT, TEXT_SIZES};
use crate::gallery::doc::{Axis, Doc, Shape};

/// Drives the real editor UI headlessly, one egui frame at a time, with the toolbar's Size
/// slider above the canvas.
struct Harness {
    ctx: Context,
    editor: Editor,
    tools: ToolState,
    font: Option<FontVec>,
    time: f64,
    modifiers: egui::Modifiers,
    /// The GPU's texture limit, where a test sets one.
    max_texture_side: Option<usize>,
    /// The clipboard's text, for Ctrl+C, Ctrl+X and Ctrl+V (which the library window handles).
    clipboard: Option<String>,
}

impl Harness {
    fn new() -> Harness {
        let ctx = Context::default();
        let bytes = super::super::doc::load_font_bytes();
        if let Some(bytes) = bytes.clone() {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(ANNOTATION_FONT.to_owned(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
            fonts.families.insert(egui::FontFamily::Name(ANNOTATION_FONT.into()), vec![ANNOTATION_FONT.to_owned()]);
            ctx.set_fonts(fonts);
        }
        let img = RgbaImage::from_pixel(800, 400, image::Rgba([255, 255, 255, 255]));
        let mut h = Harness {
            ctx,
            editor: Editor::new(Doc::from_image(img)),
            tools: ToolState { tool: Tool::Text, text_size: 88.0, ..ToolState::default() },
            font: bytes.and_then(|b| FontVec::try_from_vec(b).ok()),
            time: 0.0,
            modifiers: egui::Modifiers::NONE,
            max_texture_side: None,
            clipboard: None,
        };
        h.frame(vec![]);
        h.frame(vec![]);
        h
    }

    /// Ctrl+C, or with `cut` Ctrl+X, as the library window does them: the selected annotation
    /// onto the clipboard (and away, for a cut).
    fn copy(&mut self, cut: bool) {
        if let Some(text) = self.editor.copy_selected() {
            self.clipboard = Some(text);
            if cut {
                self.editor.delete_selected(&mut self.tools);
            }
        }
        self.frame(vec![]);
    }

    /// Ctrl+V, as the library window does it.
    fn paste(&mut self) {
        if let Some(text) = self.clipboard.clone() {
            self.editor.paste(&text, &mut self.tools, self.font.as_ref());
        }
        self.frame(vec![]);
    }

    /// Presses `key` with Ctrl, and Shift too if `shift`.
    fn ctrl_key(&mut self, shift: bool, key: Key) {
        let modifiers = if shift { egui::Modifiers::COMMAND | egui::Modifiers::SHIFT } else { egui::Modifiers::COMMAND };
        let held = std::mem::replace(&mut self.modifiers, modifiers);
        self.frame(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }]);
        self.modifiers = held;
        self.frame(vec![]);
    }

    fn shapes(&self) -> Vec<Shape> {
        self.editor.doc.current().shapes.clone()
    }

    /// Which annotation is selected.
    fn selected(&self) -> Option<usize> {
        self.editor.selection.as_ref().map(|s| s.index)
    }

    fn frame(&mut self, events: Vec<Event>) {
        self.time += 0.05;
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 600.0))),
            time: Some(self.time),
            events: std::iter::once(Event::ModifiersChanged(self.modifiers)).chain(events).collect(),
            max_texture_side: self.max_texture_side,
            ..Default::default()
        };
        let (editor, tools, font) = (&mut self.editor, &mut self.tools, self.font.as_ref());
        let mut output = self.ctx.run_ui(input, |ui| {
            egui::Panel::top("size").show(ui, |ui| {
                if tools.style_tool() == Tool::Text {
                    ui.add(size_slider(tools.text_size_mut(), TEXT_SIZES));
                } else if tools.uses_width() {
                    ui.add(size_slider(tools.width_mut(), 1.0..=24.0));
                }
            });
            egui::CentralPanel::default().show(ui, |ui| editor.ui(ui, tools, font));
        });
        // Nothing renders here, so the texture uploads are dropped on purpose.
        output.textures_delta.clear();
    }

    fn click_screen(&mut self, pos: Pos2) {
        // Space clicks out so egui never sees them as double or triple clicks.
        self.time += 1.0;
        let button = |pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        self.frame(vec![Event::PointerMoved(pos)]);
        self.frame(vec![button(true)]);
        self.frame(vec![button(false)]);
        self.frame(vec![]);
    }

    /// Clicks at a position given in image pixels.
    fn click(&mut self, x: f32, y: f32) {
        let p = self.editor.canvas.min + Vec2::new(x, y) * self.editor.shown_scale;
        self.click_screen(p);
    }

    fn type_text(&mut self, s: &str) {
        self.frame(vec![Event::Text(s.into())]);
        self.frame(vec![]);
        self.frame(vec![]);
    }

    fn texts(&self) -> Vec<String> {
        self.editor.doc.current().shapes.iter().filter_map(|s| match s {
            Shape::Text { text, .. } => Some(text.clone()),
            _ => None,
        }).collect()
    }

    /// Presses at `from`, moves to `to` over several frames, and releases.
    fn drag_screen(&mut self, from: Pos2, to: Pos2) {
        self.drag_screen_doing(from, to, |_| {});
    }

    /// Like `drag_screen`, with `meanwhile` done half way, while the button is still held.
    fn drag_screen_doing(&mut self, from: Pos2, to: Pos2, meanwhile: impl FnOnce(&mut Harness)) {
        self.time += 1.0;
        let button = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        self.frame(vec![Event::PointerMoved(from)]);
        self.frame(vec![button(from, true)]);
        let mut meanwhile = Some(meanwhile);
        for i in 1..=5 {
            if i == 3 {
                meanwhile.take().unwrap()(self);
            }
            self.frame(vec![Event::PointerMoved(from + (to - from) * i as f32 / 5.0)]);
        }
        self.frame(vec![button(to, false)]);
        self.frame(vec![]);
    }

    fn text_pos(&self) -> Pos2 {
        match &self.editor.doc.current().shapes[0] {
            Shape::Text { pos, .. } => *pos,
            _ => panic!("not text"),
        }
    }
}

#[test]
fn selected_text_field_fits_large_text() {
    let mut h = Harness::new();
    h.click(100.0, 100.0);
    h.type_text("Testing a long message");
    let pending = h.editor.text.as_ref().expect("text box open");
    let font = FontId::new(h.tools.text_size * h.editor.shown_scale, egui::FontFamily::Name(ANNOTATION_FONT.into()));
    let needed = h.ctx.fonts_mut(|f| f.layout_no_wrap(pending.text.clone(), font, Color32::WHITE).size().x);
    let anchor = h.editor.canvas.min.x + 100.0 * h.editor.shown_scale;
    assert!((pending.field.min.x - anchor).abs() < 4.0, "field drifted from the text: {} vs {}", pending.field.min.x, anchor);
    assert!(pending.field.width() >= needed, "field {} narrower than text {}", pending.field.width(), needed);
}

#[test]
fn text_can_be_reselected_after_clicking_away() {
    let mut h = Harness::new();
    h.click(100.0, 100.0);
    h.type_text("Hello");
    // Click empty image to commit, then click the text again.
    h.click(600.0, 350.0);
    assert_eq!(h.texts(), vec!["Hello"]);
    h.click(110.0, 130.0);
    assert!(h.editor.text.as_ref().is_some_and(|t| t.editing == Some(0)), "clicking committed text should reopen it");
    h.type_text(" world");
    h.click(600.0, 350.0);
    assert_eq!(h.texts(), vec!["Hello world"]);

    // Click outside the image (the grey margin), then back onto the text.
    h.click(110.0, 130.0);
    let outside = h.editor.canvas.right_bottom() + Vec2::new(40.0, 40.0);
    h.click_screen(outside);
    h.click(110.0, 130.0);
    assert!(h.editor.text.as_ref().is_some_and(|t| t.editing == Some(0)), "text should still be editable");
    h.type_text("!");
    h.click(600.0, 350.0);
    // The click put the cursor where it landed, before the "H".
    assert_eq!(h.texts(), vec!["!Hello world"]);
}

#[test]
fn dragging_text_moves_it() {
    let mut h = Harness::new();
    h.click(100.0, 100.0);
    h.type_text("Move me");
    h.click(600.0, 350.0);
    assert_eq!(h.text_pos(), Pos2::new(100.0, 100.0));

    let s = h.editor.shown_scale;
    let grab = h.editor.canvas.min + Vec2::new(120.0, 140.0) * s;
    h.drag_screen(grab, grab + Vec2::new(150.0, 80.0) * s);
    let moved = h.text_pos();
    assert!((moved - Pos2::new(250.0, 180.0)).length() < 2.0, "moved to {moved:?}");
    assert!(h.editor.text.is_none(), "dragging shouldn't open the text for editing");
    assert_eq!(h.texts(), vec!["Move me"]);
    h.editor.doc.undo();
    assert_eq!(h.text_pos(), Pos2::new(100.0, 100.0), "a move is one undo step");
}

#[test]
fn grip_moves_selected_text() {
    let mut h = Harness::new();
    h.click(100.0, 100.0);
    h.type_text("Grip");
    let field = h.editor.text.as_ref().unwrap().field;
    let grip = field.min - Vec2::splat(6.0);
    let s = h.editor.shown_scale;
    h.drag_screen(grip, grip + Vec2::new(60.0, 40.0) * s);
    assert!(h.editor.text.is_some(), "text stays selected while moving it");
    h.frame(vec![Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }]);
    h.frame(vec![]);
    let moved = h.text_pos();
    assert!((moved - Pos2::new(160.0, 140.0)).length() < 2.0, "moved to {moved:?}");
}

fn key(h: &mut Harness, key: Key) {
    h.frame(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }]);
    h.frame(vec![]);
}

fn image_point(h: &Harness, x: f32, y: f32) -> Pos2 {
    h.editor.canvas.min + Vec2::new(x, y) * h.editor.shown_scale
}

/// A hairline crop of an image wider than the GPU allows scales to a texture 1 pixel tall, not 0.
#[test]
fn a_hairline_image_past_the_texture_limit_still_gets_a_texture() {
    let mut h = Harness::new();
    h.editor = Editor::new(Doc::from_image(RgbaImage::from_pixel(40_000, 1, image::Rgba([255, 255, 255, 255]))));
    h.max_texture_side = Some(1024);
    h.frame(vec![]);
    assert_eq!(h.editor.texture.as_ref().expect("a texture").size(), [1024, 1]);
}

#[test]
fn crop_waits_for_confirmation() {
    let mut h = Harness::new();
    h.tools.tool = Tool::Crop;
    let (from, to) = (image_point(&h, 100.0, 50.0), image_point(&h, 500.0, 250.0));
    h.drag_screen(from, to);
    assert_eq!(h.editor.doc.size(), Vec2::new(800.0, 400.0), "dragging alone shouldn't crop");
    assert!(h.editor.crop.is_some());

    key(&mut h, Key::Escape);
    assert!(h.editor.crop.is_none(), "Esc cancels");
    assert_eq!(h.editor.doc.size(), Vec2::new(800.0, 400.0));

    h.drag_screen(from, to);
    key(&mut h, Key::Enter);
    let size = h.editor.doc.size();
    assert!((size - Vec2::new(400.0, 200.0)).length() < 3.0, "cropped to {size:?}");
}

#[test]
fn crop_selection_can_be_adjusted() {
    let mut h = Harness::new();
    h.tools.tool = Tool::Crop;
    h.drag_screen(image_point(&h, 100.0, 50.0), image_point(&h, 500.0, 250.0));
    // Drag the right edge out, then move the whole selection.
    h.drag_screen(image_point(&h, 500.0, 150.0), image_point(&h, 600.0, 150.0));
    h.drag_screen(image_point(&h, 300.0, 150.0), image_point(&h, 350.0, 200.0));
    let r = h.editor.crop.as_ref().expect("selection kept").rect;
    assert!((r.min - Pos2::new(150.0, 100.0)).length() < 3.0, "min {:?}", r.min);
    assert!((r.max - Pos2::new(650.0, 300.0)).length() < 3.0, "max {:?}", r.max);

    // Switching tools drops the selection without cropping.
    h.tools.tool = Tool::Box;
    h.frame(vec![]);
    assert!(h.editor.crop.is_none());
    assert_eq!(h.editor.doc.size(), Vec2::new(800.0, 400.0));
}

/// Drags between two points given in image pixels.
fn drag(h: &mut Harness, from: (f32, f32), to: (f32, f32)) {
    let (a, b) = (image_point(h, from.0, from.1), image_point(h, to.0, to.1));
    h.drag_screen(a, b);
}

fn first_rect(h: &Harness) -> Rect {
    match &h.editor.doc.current().shapes[0] {
        Shape::Box { rect, .. } => *rect,
        _ => panic!("not a box"),
    }
}

fn r(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1))
}

#[test]
fn drawn_shape_stays_selected_and_moves() {
    let mut h = Harness::new();
    h.tools.tool = Tool::Box;
    drag(&mut h, (100.0, 100.0), (300.0, 200.0));
    assert_eq!(first_rect(&h), r(100.0, 100.0, 300.0, 200.0));
    assert!(h.tools.selection.is_some_and(|s| s.kind == Tool::Box), "new box is selected");

    // Grab the outline away from the handles and move it.
    drag(&mut h, (150.0, 100.0), (200.0, 150.0));
    assert_eq!(first_rect(&h), r(150.0, 150.0, 350.0, 250.0));
    assert_eq!(h.editor.doc.current().shapes.len(), 1, "moving shouldn't draw another box");
    h.editor.doc.undo();
    assert_eq!(first_rect(&h), r(100.0, 100.0, 300.0, 200.0), "a move is one undo step");
}

/// Ctrl+Z, Ctrl+Y and the keys work while the mouse is held down on an annotation (the
/// shortcuts don't know about the canvas), so a drag whose shape is undone or deleted under it
/// must just stop, not draw it back or move whatever is now at its index.
#[test]
fn undo_and_delete_during_a_drag_drop_the_drag() {
    let mut h = Harness::new();
    h.tools.tool = Tool::Box;
    drag(&mut h, (100.0, 100.0), (300.0, 200.0));
    let (grab, to) = (image_point(&h, 150.0, 100.0), image_point(&h, 200.0, 150.0));
    h.drag_screen_doing(grab, to, |h| {
        assert!(h.editor.manip.is_some(), "the box is being moved");
        h.editor.doc.undo();
    });
    assert!(h.editor.doc.current().shapes.is_empty(), "the undone box isn't drawn back by the drag");
    h.editor.doc.redo();
    h.frame(vec![]);
    assert_eq!(first_rect(&h), r(100.0, 100.0, 300.0, 200.0), "and comes back where it was");

    // With two boxes, undoing the second while dragging it leaves the first alone.
    drag(&mut h, (400.0, 100.0), (600.0, 200.0));
    let (grab, to) = (image_point(&h, 450.0, 100.0), image_point(&h, 450.0, 300.0));
    h.drag_screen_doing(grab, to, |h| h.editor.doc.undo());
    assert_eq!(h.editor.doc.current().shapes.len(), 1);
    assert_eq!(first_rect(&h), r(100.0, 100.0, 300.0, 200.0));

    // Delete while dragging removes the box; the drag doesn't bring it back.
    h.tools.tool = Tool::Select;
    h.frame(vec![]);
    h.click(150.0, 100.0);
    let (grab, to) = (image_point(&h, 150.0, 100.0), image_point(&h, 200.0, 150.0));
    h.drag_screen_doing(grab, to, |h| key(h, Key::Delete));
    assert!(h.editor.doc.current().shapes.is_empty());

    // The toolbar's Delete button, clicked in a frame the shortcuts already redid the deletion in.
    h.editor.doc.undo();
    h.frame(vec![]);
    h.click(150.0, 100.0);
    assert!(h.tools.selection.is_some());
    h.editor.doc.redo();
    h.editor.delete_selected(&mut h.tools);
    h.frame(vec![]);
    assert!(h.editor.doc.current().shapes.is_empty() && h.tools.selection.is_none());
}

#[test]
fn handles_resize_shapes() {
    let mut h = Harness::new();
    h.tools.tool = Tool::Box;
    drag(&mut h, (100.0, 100.0), (300.0, 200.0));
    drag(&mut h, (300.0, 200.0), (400.0, 300.0));
    assert_eq!(first_rect(&h), r(100.0, 100.0, 400.0, 300.0));
    // Edge midpoint handles stretch one side.
    drag(&mut h, (100.0, 200.0), (50.0, 200.0));
    assert_eq!(first_rect(&h), r(50.0, 100.0, 400.0, 300.0));

    h.tools.tool = Tool::Arrow;
    drag(&mut h, (100.0, 350.0), (300.0, 350.0));
    drag(&mut h, (300.0, 350.0), (300.0, 250.0));
    match &h.editor.doc.current().shapes[1] {
        Shape::Arrow { from, to, .. } => assert_eq!((*from, *to), (Pos2::new(100.0, 350.0), Pos2::new(300.0, 250.0))),
        _ => panic!("not an arrow"),
    }
}

#[test]
fn text_resizes_from_its_corner() {
    let mut h = Harness::new();
    h.click(100.0, 100.0);
    h.type_text("Hi");
    h.click(600.0, 350.0);
    h.tools.tool = Tool::Select;
    h.frame(vec![]);
    let b = h.editor.doc.current().shapes[0].bounds(h.font.as_ref());
    h.click(b.center().x, b.center().y);
    assert!(h.tools.selection.is_some_and(|s| s.kind == Tool::Text));
    drag(&mut h, (b.max.x, b.max.y), (b.max.x, b.max.y + b.height()));
    match &h.editor.doc.current().shapes[0] {
        Shape::Text { size, pos, .. } => {
            assert!((size - 176.0).abs() < 2.0, "size {size}");
            assert_eq!(*pos, Pos2::new(100.0, 100.0), "the opposite corner stays put");
        }
        _ => panic!("not text"),
    }
}

/// The Size slider shows the selection's size in whole numbers but mustn't write it back
/// rounded or clamped: that would be an edit, and a second undo step.
#[test]
fn the_size_slider_leaves_the_selections_odd_sizes_alone() {
    let mut h = Harness::new();
    h.click(100.0, 100.0);
    h.type_text("Hi");
    h.click(600.0, 350.0);
    h.tools.tool = Tool::Select;
    h.frame(vec![]);
    let b = h.editor.doc.current().shapes[0].bounds(h.font.as_ref());
    h.click(b.center().x, b.center().y);
    drag(&mut h, (b.max.x, b.max.y), (b.max.x, b.max.y + b.height() * 0.55));
    let size = |h: &Harness| match &h.editor.doc.current().shapes[0] {
        Shape::Text { size, .. } => *size,
        _ => panic!("not text"),
    };
    let resized = size(&h);
    assert!(resized.fract() != 0.0, "a corner resize lands between whole sizes: {resized}");
    let version = h.editor.doc.version;
    h.frame(vec![]);
    h.frame(vec![]);
    assert_eq!(h.editor.doc.version, version, "showing the slider edited nothing");
    assert_eq!(size(&h), resized);
    h.editor.doc.undo();
    assert_eq!(size(&h), 88.0, "the resize is one undo step");

    // A step dragged smaller than the slider goes stays that small.
    h.tools.tool = Tool::Step;
    h.frame(vec![]);
    h.click(400.0, 300.0);
    let c = h.editor.doc.current().shapes[1].bounds(h.font.as_ref()).max;
    drag(&mut h, (c.x, c.y), (c.x - 16.0, c.y - 16.0));
    h.frame(vec![]);
    h.frame(vec![]);
    match &h.editor.doc.current().shapes[1] {
        Shape::Step { diameter, .. } => assert!((*diameter - 20.0).abs() < 2.0, "diameter {diameter}"),
        _ => panic!("not a step"),
    }
}

#[test]
fn select_tool_picks_and_keys_edit() {
    let mut h = Harness::new();
    h.tools.tool = Tool::Box;
    drag(&mut h, (100.0, 100.0), (300.0, 200.0));
    h.tools.tool = Tool::Select;
    h.frame(vec![]);
    assert!(h.tools.selection.is_none(), "switching tools deselects");

    // Inside an outline-only box is empty space.
    h.click(200.0, 150.0);
    assert!(h.tools.selection.is_none());
    h.click(150.0, 100.0);
    assert!(h.tools.selection.is_some());

    key(&mut h, Key::ArrowRight);
    key(&mut h, Key::ArrowRight);
    assert_eq!(first_rect(&h), r(102.0, 100.0, 302.0, 200.0));
    h.editor.doc.undo();
    assert_eq!(first_rect(&h), r(100.0, 100.0, 300.0, 200.0), "nudges share an undo step");

    h.click(150.0, 100.0);
    key(&mut h, Key::Escape);
    assert!(h.tools.selection.is_none());
    h.click(150.0, 100.0);
    key(&mut h, Key::Delete);
    assert!(h.editor.doc.current().shapes.is_empty());
}

#[test]
fn toolbar_restyles_the_selection() {
    let mut h = Harness::new();
    h.tools.tool = Tool::Box;
    drag(&mut h, (100.0, 100.0), (300.0, 200.0));
    let blue = Color32::from_rgb(30, 120, 240);
    *h.tools.color_mut() = blue;
    h.frame(vec![]);
    for w in [8.0, 10.0] {
        *h.tools.width_mut() = w;
        h.frame(vec![]);
    }
    let style = |h: &Harness| match &h.editor.doc.current().shapes[0] {
        Shape::Box { color, width, .. } => (*color, *width),
        _ => panic!("not a box"),
    };
    assert_eq!(style(&h), (blue, 10.0));
    assert_eq!(h.tools.width, 4.0, "the Box tool's own default is untouched");
    h.editor.doc.undo();
    assert_eq!(style(&h), (blue, 4.0), "slider changes share an undo step");
    h.editor.doc.undo();
    assert_eq!(style(&h).1, 4.0);
    assert_ne!(style(&h).0, blue);
}

#[test]
fn highlights_can_start_inside_the_last_one() {
    let mut h = Harness::new();
    h.tools.tool = Tool::Highlight;
    drag(&mut h, (100.0, 100.0), (400.0, 130.0));
    drag(&mut h, (150.0, 120.0), (450.0, 160.0));
    assert_eq!(h.editor.doc.current().shapes.len(), 2);
    // The Select tool still moves them by the body.
    h.tools.tool = Tool::Select;
    h.frame(vec![]);
    drag(&mut h, (300.0, 150.0), (300.0, 250.0));
    match &h.editor.doc.current().shapes[1] {
        Shape::Highlight { rect, .. } => assert_eq!(rect.min, Pos2::new(150.0, 220.0)),
        _ => panic!("not a highlight"),
    }
}

#[test]
fn shift_snaps_lines_and_endpoints_drag() {
    let mut h = Harness::new();
    h.tools.tool = Tool::Line;
    h.modifiers = egui::Modifiers::SHIFT;
    drag(&mut h, (100.0, 100.0), (300.0, 112.0));
    h.modifiers = egui::Modifiers::NONE;
    match &h.editor.doc.current().shapes[0] {
        Shape::Line { from, to, .. } => {
            assert_eq!(from.y, 100.0);
            assert!((to.y - 100.0).abs() < 0.01, "snapped flat, got {to:?}");
        }
        _ => panic!("not a line"),
    }
    // Lines are selected after drawing and keep endpoint handles.
    drag(&mut h, (100.0, 100.0), (100.0, 200.0));
    match &h.editor.doc.current().shapes[0] {
        Shape::Line { from, .. } => assert_eq!(*from, Pos2::new(100.0, 200.0)),
        _ => panic!("not a line"),
    }
}

/// The first shape's area and magnified copy, if it's a zoom.
fn zoom_rects(h: &Harness) -> (Rect, Rect) {
    match &h.editor.doc.current().shapes[0] {
        Shape::Zoom { rect, inset, .. } => (*rect, *inset),
        _ => panic!("not a zoom"),
    }
}

#[test]
fn a_zooms_copy_moves_on_its_own_and_its_area_moves_both() {
    let mut h = Harness::new();
    h.tools.tool = Tool::Zoom;
    drag(&mut h, (100.0, 100.0), (200.0, 150.0));
    assert_eq!(zoom_rects(&h), (r(100.0, 100.0, 200.0, 150.0), r(216.0, 75.0, 416.0, 175.0)), "a copy twice the size on the right");
    assert!(h.tools.selection.is_some_and(|s| s.kind == Tool::Zoom), "the new zoom is selected");
    assert!(h.tools.uses_color() && h.tools.uses_width(), "its colour and line width are in the options row");

    // Dragging the copy moves just the copy.
    drag(&mut h, (300.0, 125.0), (300.0, 225.0));
    assert_eq!(zoom_rects(&h), (r(100.0, 100.0, 200.0, 150.0), r(216.0, 175.0, 416.0, 275.0)));
    // Dragging the area's outline moves both.
    drag(&mut h, (150.0, 100.0), (170.0, 110.0));
    assert_eq!(zoom_rects(&h), (r(120.0, 110.0, 220.0, 160.0), r(236.0, 185.0, 436.0, 285.0)));
    assert_eq!(h.editor.doc.current().shapes.len(), 1, "moving shouldn't draw another zoom");
    h.editor.doc.undo();
    h.editor.doc.undo();
    assert_eq!(zoom_rects(&h).1, r(216.0, 75.0, 416.0, 175.0), "each move is one undo step");

    // With the Select tool, the copy is picked and moved on its own too.
    h.tools.tool = Tool::Select;
    h.frame(vec![]);
    drag(&mut h, (300.0, 125.0), (350.0, 125.0));
    assert_eq!(zoom_rects(&h), (r(100.0, 100.0, 200.0, 150.0), r(266.0, 75.0, 466.0, 175.0)));
}

#[test]
fn cut_out_removes_a_strip_once_confirmed() {
    let mut h = Harness::new();
    h.tools.tool = Tool::Box;
    drag(&mut h, (100.0, 50.0), (300.0, 350.0));
    h.tools.tool = Tool::CutOut;
    // Without the Size slider above it, the image settles a little higher.
    h.frame(vec![]);
    h.frame(vec![]);

    // Mostly downwards: a strip across the whole image.
    drag(&mut h, (400.0, 100.0), (410.0, 200.0));
    let sel = h.editor.crop.as_ref().expect("a strip waiting to be confirmed");
    assert_eq!((sel.cut, sel.rect), (Some(Axis::Horizontal), r(0.0, 100.0, 800.0, 200.0)));
    assert_eq!(h.editor.doc.size(), Vec2::new(800.0, 400.0), "dragging alone shouldn't cut");
    key(&mut h, Key::Escape);
    assert!(h.editor.crop.is_none(), "Esc cancels");
    assert_eq!(h.editor.doc.size(), Vec2::new(800.0, 400.0));

    // Dragging its bottom edge down widens it; it still spans the image.
    drag(&mut h, (400.0, 100.0), (410.0, 200.0));
    drag(&mut h, (300.0, 200.0), (350.0, 250.0));
    assert_eq!(h.editor.crop.as_ref().unwrap().rect, r(0.0, 100.0, 800.0, 250.0));
    key(&mut h, Key::Enter);
    assert_eq!(h.editor.doc.size(), Vec2::new(800.0, 250.0), "150 px taken out");
    assert!(h.editor.doc.current().shapes.is_empty(), "the box is flattened into the image");
    h.editor.doc.undo();
    h.frame(vec![]);
    assert_eq!(h.editor.doc.size(), Vec2::new(800.0, 400.0), "one undo step");
    assert_eq!(h.editor.doc.current().shapes.len(), 1);

    // Sideways: a strip down the image. Switching to Crop drops it.
    drag(&mut h, (500.0, 100.0), (600.0, 110.0));
    assert_eq!(h.editor.crop.as_ref().map(|s| (s.cut, s.rect)), Some((Some(Axis::Vertical), r(500.0, 0.0, 600.0, 400.0))));
    h.tools.tool = Tool::Crop;
    h.frame(vec![]);
    assert!(h.editor.crop.is_none());

    // A strip under 2 px is no strip.
    h.tools.tool = Tool::CutOut;
    h.frame(vec![]);
    drag(&mut h, (500.0, 100.0), (500.0, 101.0));
    assert!(h.editor.crop.is_none());
}

/// The rects of the boxes drawn, in drawing order.
fn box_rects(h: &Harness) -> Vec<Rect> {
    h.shapes()
        .iter()
        .map(|s| match s {
            Shape::Box { rect, .. } => *rect,
            _ => panic!("not a box"),
        })
        .collect()
}

#[test]
fn a_copied_annotation_pastes_beside_itself_and_is_selected() {
    let mut h = Harness::new();
    h.tools.tool = Tool::Box;
    drag(&mut h, (100.0, 100.0), (300.0, 200.0));
    h.copy(false);
    assert!(h.clipboard.as_deref().is_some_and(|t| t.starts_with("{\"rsnip-shape\":")), "copied as {:?}", h.clipboard);
    assert_eq!(h.shapes().len(), 1, "copying changes nothing");

    h.paste();
    assert_eq!(box_rects(&h), [r(100.0, 100.0, 300.0, 200.0), r(116.0, 116.0, 316.0, 216.0)]);
    assert_eq!(h.selected(), Some(1), "the pasted one is selected");
    assert!(h.tools.selection.is_some_and(|s| s.kind == Tool::Box));
    // Again: further on, so it isn't hidden under the last one.
    h.paste();
    assert_eq!(box_rects(&h)[2], r(132.0, 132.0, 332.0, 232.0));
    assert_eq!(h.selected(), Some(2));

    h.editor.doc.undo();
    h.frame(vec![]);
    assert_eq!(h.shapes().len(), 2, "a paste is one undo step");

    // Pasted where it would go off the image, it's kept on it.
    h.tools.tool = Tool::Box;
    drag(&mut h, (600.0, 300.0), (795.0, 395.0));
    h.copy(false);
    h.paste();
    assert_eq!(box_rects(&h).last(), Some(&r(605.0, 305.0, 800.0, 400.0)));
}

#[test]
fn pasting_a_zoom_moves_both_its_area_and_its_copy() {
    let mut h = Harness::new();
    h.tools.tool = Tool::Zoom;
    drag(&mut h, (100.0, 100.0), (200.0, 150.0));
    h.copy(false);
    h.paste();
    match &h.shapes()[1] {
        Shape::Zoom { rect, inset, .. } => assert_eq!((*rect, *inset), (r(116.0, 116.0, 216.0, 166.0), r(232.0, 91.0, 432.0, 191.0))),
        _ => panic!("not a zoom"),
    }
    assert_eq!(h.selected(), Some(1));
    let pasted = crate::gallery::doc::render(h.editor.doc.current(), h.font.as_ref());
    assert_eq!(pasted.dimensions(), (800, 400), "renders");
}

#[test]
fn ctrl_d_duplicates_the_selection_without_the_clipboard() {
    let mut h = Harness::new();
    h.tools.tool = Tool::Box;
    drag(&mut h, (100.0, 100.0), (300.0, 200.0));
    h.ctrl_key(false, Key::D);
    assert_eq!(box_rects(&h), [r(100.0, 100.0, 300.0, 200.0), r(116.0, 116.0, 316.0, 216.0)]);
    assert_eq!(h.selected(), Some(1));
    h.ctrl_key(false, Key::D);
    assert_eq!(box_rects(&h)[2], r(132.0, 132.0, 332.0, 232.0), "a duplicate of the duplicate");
    assert_eq!(h.clipboard, None);
    h.editor.doc.undo();
    h.frame(vec![]);
    assert_eq!(h.shapes().len(), 2, "one undo step each");

    // A step gets the next number.
    h.tools.tool = Tool::Step;
    h.frame(vec![]);
    h.click(500.0, 300.0);
    h.ctrl_key(false, Key::D);
    let numbers: Vec<u32> = h.shapes().iter().filter_map(|s| if let Shape::Step { number, .. } = s { Some(*number) } else { None }).collect();
    assert_eq!(numbers, [1, 2]);

    // With nothing selected, nothing happens.
    key(&mut h, Key::Escape);
    let before = h.shapes().len();
    h.ctrl_key(false, Key::D);
    assert_eq!(h.shapes().len(), before);
}

#[test]
fn a_cut_annotation_pastes_back() {
    let mut h = Harness::new();
    h.tools.tool = Tool::Box;
    drag(&mut h, (100.0, 100.0), (300.0, 200.0));
    h.copy(true);
    assert!(h.shapes().is_empty(), "cut removes it");
    assert_eq!(h.selected(), None);
    h.paste();
    assert_eq!(box_rects(&h), [r(116.0, 116.0, 316.0, 216.0)]);
    assert_eq!(h.selected(), Some(0));
}

#[test]
fn only_a_copied_annotation_pastes() {
    let mut h = Harness::new();
    h.tools.tool = Tool::Box;
    drag(&mut h, (100.0, 100.0), (300.0, 200.0));
    let shape = serde_json::to_string(&h.shapes()[0]).unwrap();
    for text in ["hello", "{\"rsnip-shape\": 1}", shape.as_str()] {
        h.clipboard = Some(text.into());
        h.paste();
        assert_eq!(h.shapes().len(), 1, "{text:?} pasted nothing");
    }

    // Copying with nothing selected leaves the clipboard as it was.
    key(&mut h, Key::Escape);
    h.clipboard = Some("before".into());
    h.copy(false);
    h.copy(true);
    assert_eq!(h.clipboard.as_deref(), Some("before"));
    assert_eq!(h.shapes().len(), 1);
}

#[test]
fn ctrl_brackets_move_the_selection_up_and_down_the_drawing_order() {
    let mut h = Harness::new();
    h.tools.tool = Tool::Box;
    let (a, b, c) = (r(100.0, 100.0, 200.0, 200.0), r(300.0, 100.0, 400.0, 200.0), r(500.0, 100.0, 600.0, 200.0));
    for rect in [a, b, c] {
        drag(&mut h, (rect.min.x, rect.min.y), (rect.max.x, rect.max.y));
    }
    h.tools.tool = Tool::Select;
    h.frame(vec![]);
    h.click(150.0, 100.0);
    assert_eq!(h.selected(), Some(0));

    let steps = [
        (false, Key::CloseBracket, [b, a, c], 1),
        (true, Key::CloseBracket, [b, c, a], 2),
        (false, Key::OpenBracket, [b, a, c], 1),
        (true, Key::OpenBracket, [a, b, c], 0),
    ];
    for (shift, k, order, at) in steps {
        let before = box_rects(&h);
        h.ctrl_key(shift, k);
        assert_eq!(box_rects(&h), order, "Ctrl+{}{k:?}", if shift { "Shift+" } else { "" });
        assert_eq!(h.selected(), Some(at), "stays selected");
        h.editor.doc.undo();
        h.frame(vec![]);
        assert_eq!(box_rects(&h), before, "one undo step");
        h.editor.doc.redo();
        h.frame(vec![]);
        h.editor.select(at, &mut h.tools);
    }

    // As typed with Shift on a US keyboard, and already at the top: nothing to do, no undo step.
    h.ctrl_key(true, Key::CloseCurlyBracket);
    assert_eq!(box_rects(&h), [b, c, a]);
    let version = h.editor.doc.version;
    h.ctrl_key(false, Key::CloseBracket);
    assert_eq!(h.editor.doc.version, version);
}

/// Random input for the panic hunt: xorshift, so a seed replays the same run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    /// Somewhere on screen, mostly on the image.
    fn point(&mut self, h: &Harness) -> Pos2 {
        if self.below(5) == 0 {
            Pos2::new(self.below(1000) as f32, self.below(600) as f32)
        } else {
            let c = h.editor.canvas;
            Pos2::new(c.min.x + self.below(c.width().max(1.0) as usize) as f32, c.min.y + self.below(c.height().max(1.0) as usize) as f32)
        }
    }
}

/// Throws `steps` random clicks, drags, keys, typing, tool switches and undo/redo at the editor,
/// checking it never panics and the document stays consistent (photocraft's "panic hunt").
/// Every hundred steps it says how far it has got (shown with `--nocapture`), so a long run can
/// be told from a stuck one.
fn panic_hunt(seed: u64, steps: usize) {
    let started = std::time::Instant::now();
    let mut h = Harness::new();
    let mut rng = Rng(seed | 1);
    let keys = [Key::Delete, Key::Backspace, Key::Escape, Key::Enter, Key::ArrowLeft, Key::ArrowRight, Key::ArrowUp, Key::ArrowDown, Key::A];
    // Ctrl+ (with Shift if true): duplicate, and up and down the drawing order.
    let shortcuts = [(false, Key::D), (false, Key::CloseBracket), (true, Key::CloseBracket), (false, Key::OpenBracket), (true, Key::OpenBracket)];
    for step in 0..steps {
        if step > 0 && step % 100 == 0 {
            println!("panic hunt: step {step} of {steps} after {:.0?}", started.elapsed());
        }
        match rng.below(13) {
            0 => h.tools.tool = crate::gallery::editor::TOOLS[rng.below(crate::gallery::editor::TOOLS.len())].0,
            1 | 2 => {
                let p = rng.point(&h);
                h.click_screen(p);
            }
            3..=5 => {
                let (a, b) = (rng.point(&h), rng.point(&h));
                h.drag_screen(a, b);
            }
            6 => {
                h.modifiers = if rng.below(3) == 0 { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE };
                let k = keys[rng.below(keys.len())];
                key(&mut h, k);
                h.modifiers = egui::Modifiers::NONE;
            }
            7 => h.type_text(["a", "Hi ", "123", " "][rng.below(4)]),
            8 | 9 => {
                // Undo, redo or a key while the mouse is still held down.
                let (a, b) = (rng.point(&h), rng.point(&h));
                let (what, k) = (rng.below(3), keys[rng.below(keys.len())]);
                h.drag_screen_doing(a, b, |h| match what {
                    0 => h.editor.doc.undo(),
                    1 => h.editor.doc.redo(),
                    _ => key(h, k),
                });
            }
            12 => match rng.below(4) {
                0 => h.copy(rng.below(2) == 0),
                1 => h.paste(),
                _ => {
                    let (shift, k) = shortcuts[rng.below(shortcuts.len())];
                    h.ctrl_key(shift, k);
                }
            },
            _ => {
                // What the toolbar's Undo and Redo do.
                h.editor.finish_text(&h.tools);
                if rng.below(2) == 0 {
                    h.editor.doc.undo();
                } else {
                    h.editor.doc.redo();
                }
                h.frame(vec![]);
            }
        }
        let doc = &h.editor.doc;
        let size = doc.size();
        assert!(size.x >= 1.0 && size.y >= 1.0, "step {step}: image is {size:?}");
        if step % 50 == 0 {
            let img = crate::gallery::doc::render(doc.current(), h.font.as_ref());
            assert_eq!((img.width() as f32, img.height() as f32), (size.x, size.y), "step {step}: render size");
        }
    }
    println!("panic hunt: {steps} steps in {:.0?}", started.elapsed());
}

#[test]
fn panic_hunt_short() {
    panic_hunt(0x5eed, 60);
}

/// A long hunt, run on request (CI runs it on Linux):
/// `RSNIP_HUNT_STEPS=20000 RSNIP_HUNT_SEED=42 cargo test panic_hunt_long -- --ignored --nocapture`.
/// Without a seed it picks one and prints it, so a failure can be replayed.
#[test]
#[ignore]
fn panic_hunt_long() {
    let env = |name| std::env::var(name).ok().and_then(|v| v.parse().ok());
    let seed = env("RSNIP_HUNT_SEED").unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64);
    let steps = env("RSNIP_HUNT_STEPS").unwrap_or(400) as usize;
    println!("panic hunt: seed {seed}, {steps} steps");
    panic_hunt(seed, steps);
}
