//! Text annotations: typing, re-editing, resizing and moving them.

use ab_glyph::FontVec;
use eframe::egui::{self, Color32, FontId, Key, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use super::{Editor, Tool, ToolState, ANNOTATION_FONT, TEXT_SIZES};
use crate::gallery::doc::{text_bounds, Shape};

pub(super) struct PendingText {
    pub(super) pos: Pos2,
    pub(super) text: String,
    pub(super) focused: bool,
    /// Index of the existing text shape being edited; it is hidden from the canvas meanwhile.
    pub(super) editing: Option<usize>,
    /// The entry field as drawn last frame, in screen points.
    pub(super) field: Rect,
}

/// The font the text preview is drawn with: the renderer's own font when it's available.
pub(super) fn preview_font(size: f32, matching_font: bool) -> FontId {
    let family = if matching_font {
        egui::FontFamily::Name(ANNOTATION_FONT.into())
    } else {
        egui::FontFamily::Proportional
    };
    FontId::new(size, family)
}

/// Hard-edged stand-in for the renderer's soft shadow, in screen points at the current zoom.
pub(super) fn shadow_offset(text_size: f32, scale: f32) -> Vec2 {
    Vec2::splat((text_size * 0.06).max(1.5) * scale)
}

impl Editor {
    /// The text shape being edited, if it still exists (undo could have removed it).
    pub(super) fn editing_index(&self) -> Option<usize> {
        let i = self.text.as_ref()?.editing?;
        matches!(self.doc.current().shapes.get(i), Some(Shape::Text { .. })).then_some(i)
    }

    /// Commits any text being typed, e.g. before saving or switching tools.
    pub fn finish_text(&mut self, tools: &ToolState) {
        let editing = self.editing_index();
        let Some(pending) = self.text.take() else { return };
        let empty = pending.text.trim().is_empty();
        let shape = Shape::Text {
            pos: pending.pos,
            text: pending.text,
            color: tools.text_color(),
            size: tools.text_size,
            shadow: tools.text_shadow,
        };
        match editing {
            Some(i) => {
                if self.doc.current().shapes[i] != shape {
                    self.doc.replace(i, (!empty).then_some(shape));
                }
            }
            None if !empty => self.doc.add(shape),
            None => {}
        }
    }

    /// Reopens an existing text annotation for editing, adopting its colour and size.
    pub(super) fn edit_text(&mut self, index: usize, tools: &mut ToolState) {
        let Some(Shape::Text { pos, text, color, size, shadow }) = self.doc.current().shapes.get(index).cloned() else {
            return;
        };
        tools.tool = Tool::Text;
        // Not via the toolbar accessors, which would restyle a selected shape instead.
        self.deselect(tools);
        tools.colors[Tool::Text as usize] = color;
        tools.text_size = size;
        tools.text_shadow = shadow;
        self.text = Some(PendingText { pos, text, focused: false, editing: Some(index), field: Rect::NOTHING });
    }

    pub fn is_typing(&self) -> bool {
        self.text.is_some()
    }

    /// Shows which text can be clicked to edit it (or, with the Text tool, dragged to move it).
    /// `hover` is the pointer in image pixels.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn text_hover(
        &self,
        ui: &Ui,
        response: &egui::Response,
        painter: &egui::Painter,
        hover: Option<Pos2>,
        tools: &ToolState,
        font: Option<&FontVec>,
        to_screen: impl Fn(Pos2) -> Pos2,
    ) {
        if self.manip.is_some() {
            return;
        }
        let hovered = hover.and_then(|p| self.doc.text_at(p, font)).filter(|&i| Some(i) != self.editing_index());
        let Some(i) = hovered else { return };
        if let Shape::Text { pos, text, size, .. } = &self.doc.current().shapes[i] {
            let b = text_bounds(*pos, text, *size, font);
            painter.rect_stroke(Rect::from_two_pos(to_screen(b.min), to_screen(b.max)), 2.0, Stroke::new(1.0f32, Color32::from_white_alpha(140)), egui::StrokeKind::Middle);
        }
        if tools.tool == Tool::Text {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            response.clone().on_hover_text("Click to edit, drag to move");
        } else {
            response.clone().on_hover_text("Double-click to edit text");
        }
    }

    /// The selected text's entry box plus a corner handle for resizing it.
    ///
    /// Losing keyboard focus doesn't commit the text, so it stays selected while the toolbar's
    /// size, colour and shadow controls are used. Enter, clicking the image or switching tools commits.
    pub(super) fn text_box(
        &mut self,
        ui: &mut Ui,
        tools: &mut ToolState,
        scale: f32,
        to_screen: impl Fn(Pos2) -> Pos2,
        matching_font: bool,
    ) {
        let Some(pending) = &mut self.text else { return };
        let color = tools.text_color();
        let font = preview_font(tools.text_size * scale, matching_font);
        let margin = Vec2::new(2.0, 0.0);
        let entry = egui::Area::new(ui.id().with("text-entry"))
            .fixed_pos(to_screen(pending.pos) - margin)
            // Stay anchored to the text even when it runs past the window edge.
            .constrain(false)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                const HINT: &str = "Type, then Enter";
                // Hard-edged stand-in for the soft shadow the renderer draws.
                if tools.text_shadow && !pending.text.is_empty() {
                    let galley = ui.fonts_mut(|f| f.layout_no_wrap(pending.text.clone(), font.clone(), Color32::from_black_alpha(170)));
                    let offset = shadow_offset(tools.text_size, scale);
                    ui.painter().galley(ui.cursor().min + margin + offset, galley, Color32::BLACK);
                }
                // The box grows to fit the text (clip_text(false)); this is just its minimum width.
                let min_width = if pending.text.is_empty() {
                    ui.fonts_mut(|f| f.layout_no_wrap(HINT.into(), font.clone(), color).size().x)
                } else {
                    tools.text_size * scale * 0.5
                };
                let edit = egui::TextEdit::singleline(&mut pending.text)
                    .font(font)
                    .text_color(color)
                    .hint_text(HINT)
                    .margin(egui::Margin::symmetric(margin.x as i8, margin.y as i8))
                    .clip_text(false)
                    .frame(egui::Frame::NONE)
                    .desired_width(min_width);
                let response = ui.add(edit);
                pending.field = response.rect;
                // No background box: the text shows over the image as it will look, with a thin outline marking it selected.
                ui.painter().rect_stroke(response.rect.expand(2.0), 2.0, Stroke::new(1.0f32, Color32::from_rgb(30, 120, 240)), egui::StrokeKind::Middle);
                if !pending.focused {
                    response.request_focus();
                    // Start typing at the end of reopened text.
                    let mut state = egui::text_edit::TextEditState::load(ui.ctx(), response.id).unwrap_or_default();
                    let end = egui::text::CCursor::new(pending.text.chars().count());
                    state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
                    state.store(ui.ctx(), response.id);
                    pending.focused = true;
                }
            });

        let box_rect = entry.response.rect;
        let handle = egui::Area::new(ui.id().with("text-resize"))
            .fixed_pos(box_rect.right_bottom() - Vec2::splat(3.0))
            .constrain(false)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                let (rect, response) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::drag());
                ui.painter().rect_filled(rect, 2.0, Color32::WHITE);
                ui.painter().rect_stroke(rect, 2.0, Stroke::new(1.0f32, Color32::from_rgb(30, 120, 240)), egui::StrokeKind::Middle);
                response.on_hover_text("Drag to resize")
            })
            .inner;
        if handle.hovered() || handle.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
        }
        if handle.dragged() {
            // Scale with the vertical drag relative to the current box height.
            let factor = 1.0 + handle.drag_delta().y / box_rect.height().max(8.0);
            tools.text_size = (tools.text_size * factor).clamp(*TEXT_SIZES.start(), *TEXT_SIZES.end());
        }

        // Dragging inside the box selects letters, so moving selected text uses a grip.
        let grip = egui::Area::new(ui.id().with("text-move"))
            .fixed_pos(box_rect.left_top() - Vec2::splat(12.0))
            .constrain(false)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                let (rect, response) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::drag());
                let accent = Color32::from_rgb(30, 120, 240);
                ui.painter().circle_filled(rect.center(), 6.0, accent);
                // Four-way arrow hint.
                let c = rect.center();
                for d in [Vec2::X, -Vec2::X, Vec2::Y, -Vec2::Y] {
                    ui.painter().line_segment([c, c + d * 4.0], Stroke::new(1.5f32, Color32::WHITE));
                }
                response.on_hover_text("Drag to move")
            })
            .inner;
        if grip.hovered() || grip.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
        }
        if grip.dragged() {
            pending.pos += grip.drag_delta() / scale;
        }

        if ui.input(|i| i.key_pressed(Key::Escape)) {
            self.text = None;
        } else if ui.input(|i| i.key_pressed(Key::Enter)) {
            self.finish_text(tools);
        }
    }
}
