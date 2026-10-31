//! Crop selection: drag an area, adjust it, then confirm or cancel. Cut out uses it too, for the
//! strip it removes.

use ab_glyph::FontVec;
use eframe::egui::{self, Align2, Color32, FontId, Key, Pos2, Rect, Ui, Vec2, Stroke};

use super::handles::{paint_handle, paint_handles, Edges};
use super::Editor;
use crate::gallery::doc::Axis;

/// A crop area waiting for the user to apply or cancel it.
pub(super) struct CropSelection {
    /// In image pixels.
    pub(super) rect: Rect,
    /// Doc version it was drawn on; undo/redo discards it.
    pub(super) version: u64,
    /// Set when it's a strip to cut out rather than an area to crop to.
    pub(super) cut: Option<Axis>,
}

/// What dragging from `p` does to the crop area `r`, both in screen points: its edges, or (no
/// edges) moving it when `p` is inside. A strip to `cut` out only has the two edges across it.
pub(super) fn crop_grab(r: Rect, p: Pos2, cut: Option<Axis>) -> Option<Edges> {
    let edges = Edges::near(r, p, false).map(|e| match cut {
        Some(Axis::Horizontal) => Edges { left: false, right: false, ..e },
        Some(Axis::Vertical) => Edges { top: false, bottom: false, ..e },
        None => e,
    });
    edges.filter(|e| e.any()).or_else(|| r.contains(p).then(Edges::default))
}

/// The strip a drag from `start` to `current` cuts out of an image of `size` (image pixels): one
/// across the image for a drag mostly up or down, one down it for a drag sideways.
pub(super) fn cut_strip(start: Pos2, current: Pos2, size: Vec2) -> (Axis, Rect) {
    let d = current - start;
    if d.y.abs() >= d.x.abs() {
        (Axis::Horizontal, Rect::from_x_y_ranges(0.0..=size.x, start.y.min(current.y)..=start.y.max(current.y)))
    } else {
        (Axis::Vertical, Rect::from_x_y_ranges(start.x.min(current.x)..=start.x.max(current.x), 0.0..=size.y))
    }
}

/// How many whole pixels cutting out the strip `rect` along `axis` removes.
pub(super) fn strip_pixels(axis: Axis, rect: Rect) -> f32 {
    match axis {
        Axis::Horizontal => rect.max.y.round() - rect.min.y.round(),
        Axis::Vertical => rect.max.x.round() - rect.min.x.round(),
    }
}

/// The crop selection being resized or moved.
pub(super) struct CropDrag {
    pub(super) edges: Edges,
    pub(super) start: Rect,
    pub(super) origin: Pos2,
    /// The selection is a strip to cut out, which keeps spanning the image.
    pub(super) cut: Option<Axis>,
}

impl CropDrag {
    /// The selection after dragging by `delta` image pixels, kept inside `bounds`.
    pub(super) fn adjusted(&self, delta: Vec2, bounds: Rect) -> Rect {
        let r = self.edges.apply(self.start, delta);
        let r = if self.edges.any() {
            r.intersect(bounds)
        } else {
            let dx = (bounds.min.x - r.min.x).max(0.0) + (bounds.max.x - r.max.x).min(0.0);
            let dy = (bounds.min.y - r.min.y).max(0.0) + (bounds.max.y - r.max.y).min(0.0);
            r.translate(Vec2::new(dx, dy))
        };
        match self.cut {
            Some(Axis::Horizontal) => Rect::from_x_y_ranges(bounds.x_range(), r.y_range()),
            Some(Axis::Vertical) => Rect::from_x_y_ranges(r.x_range(), bounds.y_range()),
            None => r,
        }
    }
}

impl Editor {
    /// Crops to the pending selection, or cuts out the pending strip.
    pub(super) fn apply_crop(&mut self, font: Option<&FontVec>) {
        let Some(sel) = self.crop.take() else { return };
        let r = Rect::from_min_max(sel.rect.min.round(), sel.rect.max.round());
        match sel.cut {
            Some(Axis::Horizontal) => self.doc.cut_out(Axis::Horizontal, r.min.y, r.max.y, font),
            Some(Axis::Vertical) => self.doc.cut_out(Axis::Vertical, r.min.x, r.max.x, font),
            None if r.width() >= 1.0 && r.height() >= 1.0 => self.doc.crop(r, font),
            None => return,
        }
        self.zoom = None;
    }

    /// Shows the pending crop selection with buttons to apply or cancel it (also Enter / Esc).
    pub(super) fn crop_confirm(&mut self, ui: &mut Ui, painter: &egui::Painter, to_screen: impl Fn(Pos2) -> Pos2, font: Option<&FontVec>) {
        let Some(sel) = &self.crop else { return };
        let r = Rect::from_two_pos(to_screen(sel.rect.min), to_screen(sel.rect.max));
        let action = match sel.cut {
            Some(axis) => {
                cut_shade(painter, r, self.canvas, axis, strip_pixels(axis, sel.rect), true);
                "✔ Cut out"
            }
            None => {
                crop_shade(painter, r, self.canvas, sel.rect.size(), true);
                "✔ Crop"
            }
        };

        let (mut apply, mut cancel) = (false, false);
        // Below the selection's bottom-right corner; egui keeps the Area on screen.
        egui::Area::new(ui.id().with("crop-confirm"))
            .fixed_pos(r.right_bottom() + Vec2::new(0.0, 10.0))
            .pivot(Align2::RIGHT_TOP)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        apply = ui.button(action).on_hover_text("Enter, or double-click inside").clicked();
                        cancel = ui.button("Cancel").on_hover_text("Esc").clicked();
                    });
                });
            });
        let typing = ui.ctx().egui_wants_keyboard_input();
        apply |= !typing && ui.input(|i| i.key_pressed(Key::Enter));
        cancel |= !typing && ui.input(|i| i.key_pressed(Key::Escape));
        if apply {
            self.apply_crop(font);
        } else if cancel {
            self.crop = None;
        }
    }
}

/// Fills the part of `canvas` outside `r` (both screen points) with `shade`.
pub(super) fn shade_outside(painter: &egui::Painter, r: Rect, canvas: Rect, shade: Color32) {
    let (c, r) = (canvas, r.intersect(canvas));
    painter.rect_filled(Rect::from_min_max(c.min, Pos2::new(c.max.x, r.min.y)), 0.0, shade);
    painter.rect_filled(Rect::from_min_max(Pos2::new(c.min.x, r.max.y), c.max), 0.0, shade);
    painter.rect_filled(Rect::from_min_max(Pos2::new(c.min.x, r.min.y), Pos2::new(r.min.x, r.max.y)), 0.0, shade);
    painter.rect_filled(Rect::from_min_max(Pos2::new(r.max.x, r.min.y), Pos2::new(c.max.x, r.max.y)), 0.0, shade);
}

/// Darkens everything outside the crop area `r` (screen points) and labels its size in pixels.
pub(super) fn crop_shade(painter: &egui::Painter, r: Rect, canvas: Rect, size: Vec2, handles: bool) {
    let c = canvas;
    shade_outside(painter, r, c, Color32::from_black_alpha(150));
    painter.rect_stroke(r, 0.0, Stroke::new(1.0f32, Color32::WHITE), egui::StrokeKind::Middle);
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

/// Darkens the strip `r` (screen points) that cutting out along `axis` removes, with a dashed line
/// down its middle and how many pixels go; `handles` on the two edges that can be dragged.
pub(super) fn cut_shade(painter: &egui::Painter, r: Rect, canvas: Rect, axis: Axis, pixels: f32, handles: bool) {
    let (c, r) = (canvas, r.intersect(canvas));
    painter.rect_filled(r, 0.0, Color32::from_black_alpha(150));
    painter.rect_stroke(r, 0.0, Stroke::new(1.0f32, Color32::WHITE), egui::StrokeKind::Middle);
    let (middle, edges) = match axis {
        Axis::Horizontal => ([r.left_center(), r.right_center()], [r.center_top(), r.center_bottom()]),
        Axis::Vertical => ([r.center_top(), r.center_bottom()], [r.left_center(), r.right_center()]),
    };
    painter.extend(egui::Shape::dashed_line(&middle, Stroke::new(1.5f32, Color32::WHITE), 6.0, 4.0));
    if handles {
        edges.into_iter().for_each(|p| paint_handle(painter, p));
    }
    // Beside the strip, where there's room: above or below one across, right or left of one down.
    let label = format!("Removes {pixels} px");
    let (at, align) = match axis {
        Axis::Horizontal if r.min.y - c.min.y >= 18.0 => (r.center_top() - Vec2::new(0.0, 4.0), Align2::CENTER_BOTTOM),
        Axis::Horizontal => (r.center_bottom() + Vec2::new(0.0, 4.0), Align2::CENTER_TOP),
        Axis::Vertical if c.max.x - r.max.x >= 100.0 => (r.right_top() + Vec2::new(6.0, 6.0), Align2::LEFT_TOP),
        Axis::Vertical => (r.left_top() + Vec2::new(-6.0, 6.0), Align2::RIGHT_TOP),
    };
    painter.text(at, align, label, FontId::proportional(13.0), Color32::WHITE);
}
