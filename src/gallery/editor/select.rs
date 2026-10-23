//! Selecting annotations to move, resize, restyle, nudge or delete them.

use ab_glyph::FontVec;
use eframe::egui::{self, Color32, Key, Pos2, Rect, Stroke, Ui, Vec2};

use super::handles::{handle_at, paint_handles, Edges, ACCENT};
use super::preview::paint_shape;
use super::{Editor, Style, Tool, ToolState, TEXT_SIZES};
use crate::gallery::doc::Shape;

/// How far from an annotation, in screen points, a click still picks it.
pub(super) const PICK_SLACK: f32 = 6.0;

pub(super) struct Selection {
    pub(super) index: usize,
    /// Doc version it was made on; any other change to the doc (undo, redo, crop) drops it.
    version: u64,
    /// The style last published to the toolbar, to spot the toolbar's changes.
    style: Style,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Grab {
    Move,
    Edges(Edges),
    /// An arrow or line's start (`false`) or end (`true`).
    Endpoint(bool),
}

/// An annotation being moved or resized; it's drawn live and committed as one undo step.
pub(super) struct Manip {
    pub(super) index: usize,
    grab: Grab,
    /// Where the drag started, in screen points.
    origin: Pos2,
    start: Shape,
    start_bounds: Rect,
    live: Shape,
    /// Whether it stays selected afterwards (not when moving text with the Text tool).
    select: bool,
}

/// Text and steps resize from their corners only, keeping their proportions.
fn uniform(shape: &Shape) -> bool {
    matches!(shape, Shape::Text { .. } | Shape::Step { .. })
}

fn screen_rect(r: Rect, to_screen: impl Fn(Pos2) -> Pos2) -> Rect {
    Rect::from_two_pos(to_screen(r.min), to_screen(r.max))
}

impl Editor {
    pub(super) fn select(&mut self, index: usize, tools: &mut ToolState) {
        let style = Style::of(&self.doc.current().shapes[index]);
        self.selection = Some(Selection { index, version: self.doc.version, style });
        tools.selection = Some(style);
    }

    pub(super) fn deselect(&mut self, tools: &mut ToolState) {
        self.selection = None;
        tools.selection = None;
    }

    pub fn delete_selected(&mut self, tools: &mut ToolState) {
        if let Some(sel) = self.selection.take() {
            self.doc.replace(sel.index, None);
        }
        tools.selection = None;
    }

    /// Drops the selection when it no longer applies, then applies the toolbar's edits to it.
    pub(super) fn sync_selection(&mut self, tools: &mut ToolState) {
        if tools.tool != self.last_tool {
            self.last_tool = tools.tool;
            self.selection = None;
        }
        let stale = self.selection.as_ref().is_some_and(|s| s.version != self.doc.version || s.index >= self.doc.current().shapes.len());
        if stale || matches!(tools.tool, Tool::Text | Tool::Crop | Tool::CopyText) {
            self.selection = None;
        }
        let Some(sel) = &mut self.selection else {
            tools.selection = None;
            return;
        };
        if let Some(wanted) = tools.selection.filter(|w| *w != sel.style && w.kind == sel.style.kind) {
            let shape = wanted.apply(&self.doc.current().shapes[sel.index]);
            self.doc.amend(sel.index, shape, wanted.changed(&sel.style));
            sel.version = self.doc.version;
            sel.style = Style::of(&self.doc.current().shapes[sel.index]);
        }
        tools.selection = Some(sel.style);
    }

    /// What dragging from `p` (screen points) does to the selection, if it starts on it.
    pub(super) fn selection_grab(
        &self,
        p: Pos2,
        to_screen: impl Fn(Pos2) -> Pos2,
        to_img: impl Fn(Pos2) -> Pos2,
        scale: f32,
        font: Option<&FontVec>,
    ) -> Option<Grab> {
        let sel = self.selection.as_ref()?;
        let shape = &self.doc.current().shapes[sel.index];
        if let Shape::Arrow { from, to, .. } | Shape::Line { from, to, .. } = shape {
            if to_screen(*to).distance(p) <= 8.0 {
                return Some(Grab::Endpoint(true));
            }
            if to_screen(*from).distance(p) <= 8.0 {
                return Some(Grab::Endpoint(false));
            }
        } else if let Some(edges) = handle_at(screen_rect(shape.bounds(font), &to_screen), p, uniform(shape)) {
            return Some(Grab::Edges(edges));
        }
        // Highlights are often drawn line after line, so with a drawing tool a new one can start
        // inside the last; their body only moves them with the Select tool.
        let filled = matches!(shape, Shape::Highlight { .. } | Shape::Blur { .. } | Shape::Redact { .. } | Shape::Pixelate { .. });
        let movable = !filled || self.last_tool == Tool::Select;
        (movable && shape.hit(to_img(p), PICK_SLACK / scale, font)).then_some(Grab::Move)
    }

    pub(super) fn begin_manip(&mut self, index: usize, grab: Grab, origin: Pos2, select: bool, font: Option<&FontVec>) {
        let start = self.doc.current().shapes[index].clone();
        let start_bounds = start.bounds(font);
        self.manip = Some(Manip { index, grab, origin, live: start.clone(), start, start_bounds, select });
    }

    /// Follows the pointer (screen points) while an annotation is moved or resized.
    pub(super) fn update_manip(&mut self, pointer: Option<Pos2>, scale: f32) {
        let (Some(m), Some(p)) = (&mut self.manip, pointer) else { return };
        let delta = (p - m.origin) / scale;
        m.live = match m.grab {
            Grab::Move => m.start.translated(delta),
            Grab::Endpoint(head) => {
                let mut s = m.start.clone();
                if let Shape::Arrow { from, to, .. } | Shape::Line { from, to, .. } = &mut s {
                    *(if head { to } else { from }) += delta;
                }
                s
            }
            Grab::Edges(edges) => {
                let to = if uniform(&m.start) { edges.apply_uniform(m.start_bounds, delta) } else { edges.apply(m.start_bounds, delta) };
                let mut s = m.start.resized(m.start_bounds, to);
                match &mut s {
                    Shape::Text { size, .. } => *size = size.clamp(*TEXT_SIZES.start(), *TEXT_SIZES.end()),
                    Shape::Step { diameter, .. } => *diameter = diameter.max(8.0),
                    _ => {}
                }
                s
            }
        };
    }

    /// Commits the move or resize as one undo step.
    pub(super) fn end_manip(&mut self, tools: &mut ToolState) {
        let Some(m) = self.manip.take() else { return };
        if m.live != m.start {
            self.doc.replace(m.index, Some(m.live));
        }
        if m.select {
            self.select(m.index, tools);
        }
    }

    pub(super) fn paint_manip(&self, painter: &egui::Painter, scale: f32, to_screen: impl Fn(Pos2) -> Pos2, matching_font: bool) {
        if let Some(m) = &self.manip {
            paint_shape(painter, &m.live, scale, to_screen, self.canvas, matching_font);
        }
    }

    /// Outlines the selection and draws its handles.
    pub(super) fn paint_selection(&self, painter: &egui::Painter, to_screen: impl Fn(Pos2) -> Pos2, font: Option<&FontVec>) {
        let shape = match (&self.manip, &self.selection) {
            (Some(m), _) if m.select => &m.live,
            (None, Some(sel)) => &self.doc.current().shapes[sel.index],
            _ => return,
        };
        if let Shape::Arrow { from, to, .. } | Shape::Line { from, to, .. } = shape {
            for end in [from, to] {
                painter.circle(to_screen(*end), 5.0, Color32::WHITE, Stroke::new(1.5f32, ACCENT));
            }
            return;
        }
        let r = screen_rect(shape.bounds(font), &to_screen).expand(2.0);
        painter.rect_stroke(r, 0.0, Stroke::new(1.0f32, ACCENT), egui::StrokeKind::Middle);
        paint_handles(painter, r, uniform(shape));
    }

    /// Cursor feedback for grabbing the selection, or picking an annotation with the Select tool.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn selection_hover(
        &self,
        ui: &Ui,
        painter: &egui::Painter,
        hover: Option<Pos2>,
        tools: &ToolState,
        to_screen: impl Fn(Pos2) -> Pos2,
        to_img: impl Fn(Pos2) -> Pos2,
        scale: f32,
        font: Option<&FontVec>,
    ) {
        let cursor = |grab: Grab| match grab {
            Grab::Move => egui::CursorIcon::Move,
            Grab::Edges(edges) => edges.cursor(),
            Grab::Endpoint(_) => egui::CursorIcon::Crosshair,
        };
        if let Some(m) = &self.manip {
            let icon = if m.grab == Grab::Move { egui::CursorIcon::Grabbing } else { cursor(m.grab) };
            ui.ctx().set_cursor_icon(icon);
            return;
        }
        let Some(p) = hover else { return };
        if let Some(grab) = self.selection_grab(p, &to_screen, &to_img, scale, font) {
            ui.ctx().set_cursor_icon(cursor(grab));
        } else if tools.tool == Tool::Select {
            if let Some(i) = self.doc.shape_at(to_img(p), PICK_SLACK / scale, font) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
                // Text gets its own outline from the text hover.
                let shape = &self.doc.current().shapes[i];
                if !matches!(shape, Shape::Text { .. }) {
                    let r = screen_rect(shape.bounds(font), &to_screen).expand(2.0);
                    painter.rect_stroke(r, 0.0, Stroke::new(1.0f32, Color32::from_white_alpha(140)), egui::StrokeKind::Middle);
                }
            }
        }
    }

    /// Delete/Backspace removes the selection, Esc deselects, arrow keys nudge it (Shift: 10 px).
    pub(super) fn selection_keys(&mut self, ui: &Ui, tools: &mut ToolState) {
        let Some(index) = self.selection.as_ref().map(|s| s.index) else { return };
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let (delete, escape, nudge) = ui.input(|i| {
            let step = if i.modifiers.shift { 10.0 } else { 1.0 };
            let mut d = Vec2::ZERO;
            for (key, dir) in [(Key::ArrowLeft, -Vec2::X), (Key::ArrowRight, Vec2::X), (Key::ArrowUp, -Vec2::Y), (Key::ArrowDown, Vec2::Y)] {
                if i.key_pressed(key) {
                    d += dir * step;
                }
            }
            (i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace), i.key_pressed(Key::Escape), d)
        });
        if delete {
            self.delete_selected(tools);
        } else if escape {
            self.deselect(tools);
        } else if nudge != Vec2::ZERO {
            let moved = self.doc.current().shapes[index].translated(nudge);
            self.doc.amend(index, moved, "nudge");
            self.select(index, tools);
        }
    }
}
