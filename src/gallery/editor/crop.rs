//! Crop selection: drag an area, adjust it, then confirm or cancel.

use ab_glyph::FontVec;
use eframe::egui::{self, Align2, Color32, FontId, Key, Pos2, Rect, Ui, Vec2, Stroke};

use super::handles::{paint_handles, Edges};
use super::Editor;

/// A crop area waiting for the user to apply or cancel it.
pub(super) struct CropSelection {
    /// In image pixels.
    pub(super) rect: Rect,
    /// Doc version it was drawn on; undo/redo discards it.
    pub(super) version: u64,
}

/// What dragging from `p` does to the crop area `r`, both in screen points: its edges, or (no
/// edges) moving it when `p` is inside.
pub(super) fn crop_grab(r: Rect, p: Pos2) -> Option<Edges> {
    Edges::near(r, p, false).or_else(|| r.contains(p).then(Edges::default))
}

/// The crop selection being resized or moved.
pub(super) struct CropDrag {
    pub(super) edges: Edges,
    pub(super) start: Rect,
    pub(super) origin: Pos2,
}

impl CropDrag {
    /// The selection after dragging by `delta` image pixels, kept inside `bounds`.
    pub(super) fn adjusted(&self, delta: Vec2, bounds: Rect) -> Rect {
        let r = self.edges.apply(self.start, delta);
        if self.edges.any() {
            return r.intersect(bounds);
        }
        let dx = (bounds.min.x - r.min.x).max(0.0) + (bounds.max.x - r.max.x).min(0.0);
        let dy = (bounds.min.y - r.min.y).max(0.0) + (bounds.max.y - r.max.y).min(0.0);
        r.translate(Vec2::new(dx, dy))
    }
}

impl Editor {
    /// Crops to the pending selection.
    pub(super) fn apply_crop(&mut self, font: Option<&FontVec>) {
        let Some(sel) = self.crop.take() else { return };
        let r = Rect::from_min_max(sel.rect.min.round(), sel.rect.max.round());
        if r.width() >= 1.0 && r.height() >= 1.0 {
            self.doc.crop(r, font);
            self.zoom = None;
        }
    }

    /// Shows the pending crop selection with buttons to apply or cancel it (also Enter / Esc).
    pub(super) fn crop_confirm(&mut self, ui: &mut Ui, painter: &egui::Painter, to_screen: impl Fn(Pos2) -> Pos2, font: Option<&FontVec>) {
        let Some(sel) = &self.crop else { return };
        let r = Rect::from_two_pos(to_screen(sel.rect.min), to_screen(sel.rect.max));
        crop_shade(painter, r, self.canvas, sel.rect.size(), true);

        let (mut apply, mut cancel) = (false, false);
        // Below the selection's bottom-right corner; egui keeps the Area on screen.
        egui::Area::new(ui.id().with("crop-confirm"))
            .fixed_pos(r.right_bottom() + Vec2::new(0.0, 10.0))
            .pivot(Align2::RIGHT_TOP)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        apply = ui.button("✔ Crop").on_hover_text("Enter, or double-click inside").clicked();
                        cancel = ui.button("Cancel").on_hover_text("Esc").clicked();
                    });
                });
            });
        let typing = ui.ctx().wants_keyboard_input();
        apply |= !typing && ui.input(|i| i.key_pressed(Key::Enter));
        cancel |= !typing && ui.input(|i| i.key_pressed(Key::Escape));
        if apply {
            self.apply_crop(font);
        } else if cancel {
            self.crop = None;
        }
    }
}

/// Darkens everything outside the crop area `r` (screen points) and labels its size in pixels.
pub(super) fn crop_shade(painter: &egui::Painter, r: Rect, canvas: Rect, size: Vec2, handles: bool) {
    let shade = Color32::from_black_alpha(150);
    let c = canvas;
    painter.rect_filled(Rect::from_min_max(c.min, Pos2::new(c.max.x, r.min.y)), 0.0, shade);
    painter.rect_filled(Rect::from_min_max(Pos2::new(c.min.x, r.max.y), c.max), 0.0, shade);
    painter.rect_filled(Rect::from_min_max(Pos2::new(c.min.x, r.min.y), Pos2::new(r.min.x, r.max.y)), 0.0, shade);
    painter.rect_filled(Rect::from_min_max(Pos2::new(r.max.x, r.min.y), Pos2::new(c.max.x, r.max.y)), 0.0, shade);
    painter.rect_stroke(r, 0.0, Stroke::new(1.0f32, Color32::WHITE));
    if handles {
        paint_handles(painter, r, false);
    }
    let label = format!("{} × {}", size.x.round(), size.y.round());
    let (at, align) = if r.min.y - c.min.y >= 18.0 {
        (r.left_top() - Vec2::new(0.0, 4.0), Align2::LEFT_BOTTOM)
    } else {
        (r.left_top() + Vec2::new(6.0, 6.0), Align2::LEFT_TOP)
    };
    painter.text(at, align, label, FontId::proportional(13.0), Color32::WHITE);
}
