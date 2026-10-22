//! Edge and corner handles for resizing a rectangle, shared by crop and shape selection.

use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};

pub(super) const ACCENT: Color32 = Color32::from_rgb(30, 120, 240);

/// How close, in screen points, the pointer must be to an edge to grab it.
const REACH: f32 = 8.0;

/// Which edges of a rectangle a drag moves; none set means the whole rectangle moves.
#[derive(Clone, Copy, Default, PartialEq)]
pub(super) struct Edges {
    pub(super) left: bool,
    pub(super) right: bool,
    pub(super) top: bool,
    pub(super) bottom: bool,
}

impl Edges {
    pub(super) fn any(self) -> bool {
        self.left || self.right || self.top || self.bottom
    }

    fn corner(self) -> bool {
        (self.left || self.right) && (self.top || self.bottom)
    }

    pub(super) fn cursor(self) -> egui::CursorIcon {
        use egui::CursorIcon::*;
        match (self.left, self.right, self.top, self.bottom) {
            (true, _, true, _) | (_, true, _, true) => ResizeNwSe,
            (true, _, _, true) | (_, true, true, _) => ResizeNeSw,
            (true, _, _, _) | (_, true, _, _) => ResizeHorizontal,
            (_, _, true, _) | (_, _, _, true) => ResizeVertical,
            _ => Move,
        }
    }

    /// The edges of `r` within reach of `p` (both in screen points), if any.
    pub(super) fn near(r: Rect, p: Pos2, corners_only: bool) -> Option<Edges> {
        if !r.expand(REACH).contains(p) {
            return None;
        }
        // On a tiny rectangle both edges are in reach; take the closer one.
        let (dl, dr, dt, db) = ((p.x - r.min.x).abs(), (p.x - r.max.x).abs(), (p.y - r.min.y).abs(), (p.y - r.max.y).abs());
        let edges = Edges {
            left: dl <= REACH && dl <= dr,
            right: dr <= REACH && dr < dl,
            top: dt <= REACH && dt <= db,
            bottom: db <= REACH && db < dt,
        };
        let usable = if corners_only { edges.corner() } else { edges.any() };
        usable.then_some(edges)
    }

    /// `start` with these edges dragged by `delta`; with no edges, `start` moved by `delta`.
    pub(super) fn apply(self, start: Rect, delta: Vec2) -> Rect {
        if !self.any() {
            return start.translate(delta);
        }
        let (mut min, mut max) = (start.min, start.max);
        if self.left { min.x += delta.x }
        if self.right { max.x += delta.x }
        if self.top { min.y += delta.y }
        if self.bottom { max.y += delta.y }
        Rect::from_two_pos(min, max)
    }

    /// Like `apply`, but scaling `start` uniformly about the opposite corner.
    pub(super) fn apply_uniform(self, start: Rect, delta: Vec2) -> Rect {
        let stretched = self.apply(start, delta);
        let size = start.size().max(Vec2::splat(1.0));
        // Follow whichever axis moved further.
        let (kx, ky) = (stretched.width() / size.x, stretched.height() / size.y);
        let k = if (kx - 1.0).abs() > (ky - 1.0).abs() { kx } else { ky };
        let anchor = Pos2::new(if self.left { start.max.x } else { start.min.x }, if self.top { start.max.y } else { start.min.y });
        let corner = Pos2::new(
            if self.left { anchor.x - size.x * k } else { anchor.x + size.x * k },
            if self.top { anchor.y - size.y * k } else { anchor.y + size.y * k },
        );
        Rect::from_two_pos(anchor, corner)
    }
}

/// The handles on the corners of `r` and, unless `corners_only`, its edge midpoints.
fn handle_points(r: Rect, corners_only: bool) -> Vec<(Edges, Pos2)> {
    let mut out = Vec::new();
    for (i, x) in [r.min.x, r.center().x, r.max.x].into_iter().enumerate() {
        for (j, y) in [r.min.y, r.center().y, r.max.y].into_iter().enumerate() {
            let middle = i == 1 || j == 1;
            if (i == 1 && j == 1) || (corners_only && middle) {
                continue;
            }
            let edges = Edges { left: i == 0, right: i == 2, top: j == 0, bottom: j == 2 };
            out.push((edges, Pos2::new(x, y)));
        }
    }
    out
}

/// The handle drawn by `paint_handles` under `p`, if any.
pub(super) fn handle_at(r: Rect, p: Pos2, corners_only: bool) -> Option<Edges> {
    let near = handle_points(r, corners_only).into_iter().filter(|(_, h)| h.distance(p) <= REACH);
    near.min_by(|a, b| a.1.distance(p).total_cmp(&b.1.distance(p))).map(|(edges, _)| edges)
}

/// Square handles on the corners of `r` and, unless `corners_only`, its edge midpoints.
pub(super) fn paint_handles(painter: &egui::Painter, r: Rect, corners_only: bool) {
    for (_, p) in handle_points(r, corners_only) {
        let h = Rect::from_center_size(p, Vec2::splat(8.0));
        painter.rect_filled(h, 1.0, Color32::WHITE);
        painter.rect_stroke(h, 1.0, Stroke::new(1.0f32, ACCENT), egui::StrokeKind::Middle);
    }
}
