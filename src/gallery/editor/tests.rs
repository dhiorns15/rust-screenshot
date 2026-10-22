//! Headless tests driving the real editor UI through egui input events.

use ab_glyph::FontVec;
use eframe::egui::{self, Color32, Context, Event, FontId, Key, PointerButton, Pos2, RawInput, Rect, Vec2};
use image::RgbaImage;

use super::{Editor, Tool, ToolState, ANNOTATION_FONT};
use crate::gallery::doc::{Doc, Shape};

/// Drives the real editor UI headlessly, one egui frame at a time.
struct Harness {
    ctx: Context,
    editor: Editor,
    tools: ToolState,
    font: Option<FontVec>,
    time: f64,
}

impl Harness {
    fn new() -> Harness {
        let ctx = Context::default();
        let bytes = super::super::doc::load_font_bytes();
        if let Some(bytes) = bytes.clone() {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(ANNOTATION_FONT.to_owned(), egui::FontData::from_owned(bytes));
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
        };
        h.frame(vec![]);
        h.frame(vec![]);
        h
    }

    fn frame(&mut self, events: Vec<Event>) {
        self.time += 0.05;
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 600.0))),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let (editor, tools, font) = (&mut self.editor, &mut self.tools, self.font.as_ref());
        let _ = self.ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| editor.ui(ui, tools, font));
        });
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
        self.time += 1.0;
        let button = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        self.frame(vec![Event::PointerMoved(from)]);
        self.frame(vec![button(from, true)]);
        for i in 1..=5 {
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
    let needed = h.ctx.fonts(|f| f.layout_no_wrap(pending.text.clone(), font, Color32::WHITE).size().x);
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
