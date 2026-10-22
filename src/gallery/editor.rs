//! The annotation canvas: tools, mouse handling and live previews.

use ab_glyph::FontVec;
use eframe::egui::{
    self, Align2, Color32, ColorImage, FontId, Key, Pos2, Rect, Sense, Stroke, TextureHandle,
    TextureOptions, Ui, Vec2,
};

use super::doc::{arrow_geometry, render, text_bounds, Doc, Shape};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Arrow,
    Box,
    Ellipse,
    Highlight,
    Blur,
    Pen,
    Text,
    Step,
    Crop,
}

pub const TOOLS: [(Tool, &str, Key, &str); 9] = [
    (Tool::Arrow, "Arrow", Key::A, "Drag to draw an arrow (A)"),
    (Tool::Box, "Box", Key::B, "Drag to draw a box (B)"),
    (Tool::Ellipse, "Ellipse", Key::E, "Drag to draw an ellipse (E)"),
    (Tool::Highlight, "Highlight", Key::H, "Drag to highlight an area (H)"),
    (Tool::Blur, "Blur", Key::U, "Drag to blur sensitive content (U)"),
    (Tool::Pen, "Pen", Key::P, "Draw freehand (P)"),
    (Tool::Text, "Text", Key::T, "Click to type text (T)"),
    (Tool::Step, "Step", Key::N, "Click to add numbered steps 1, 2, 3… (N)"),
    (Tool::Crop, "Crop", Key::C, "Drag to select an area to crop, then confirm (C)"),
];

/// egui font family holding the renderer's font, so the text preview matches the saved result.
pub const ANNOTATION_FONT: &str = "annotation";

pub const PALETTE: [Color32; 8] = [
    Color32::from_rgb(230, 40, 40),
    Color32::from_rgb(255, 140, 0),
    Color32::from_rgb(255, 225, 0),
    Color32::from_rgb(40, 180, 80),
    Color32::from_rgb(30, 120, 240),
    Color32::from_rgb(150, 70, 220),
    Color32::from_rgb(20, 20, 20),
    Color32::WHITE,
];

pub struct ToolState {
    pub tool: Tool,
    /// Remembered per tool, so the highlighter stays yellow while arrows stay red.
    colors: [Color32; TOOLS.len()],
    pub width: f32,
    /// Font size in image pixels.
    pub text_size: f32,
    pub text_shadow: bool,
}

pub const TEXT_SIZES: std::ops::RangeInclusive<f32> = 8.0..=300.0;

impl Default for ToolState {
    fn default() -> Self {
        let mut colors = [PALETTE[0]; TOOLS.len()];
        colors[Tool::Highlight as usize] = PALETTE[2];
        ToolState { tool: Tool::Arrow, colors, width: 4.0, text_size: 28.0, text_shadow: true }
    }
}

impl ToolState {
    pub fn color(&self) -> Color32 {
        self.colors[self.tool as usize]
    }

    pub fn color_mut(&mut self) -> &mut Color32 {
        &mut self.colors[self.tool as usize]
    }

    pub fn uses_color(&self) -> bool {
        !matches!(self.tool, Tool::Blur | Tool::Crop)
    }

    fn step_diameter(&self) -> f32 {
        20.0 + self.width * 4.0
    }
}

struct Drag {
    start: Pos2,
    current: Pos2,
    points: Vec<Pos2>,
}

struct PendingText {
    pos: Pos2,
    text: String,
    focused: bool,
    /// Index of the existing text shape being edited; it is hidden from the canvas meanwhile.
    editing: Option<usize>,
    /// The entry field as drawn last frame, in screen points.
    field: Rect,
}

pub struct Editor {
    pub doc: Doc,
    texture: Option<TextureHandle>,
    /// Doc version and hidden shape the texture was rendered for.
    texture_key: (u64, Option<usize>),
    /// `None` fits the image to the window.
    pub zoom: Option<f32>,
    pub shown_scale: f32,
    drag: Option<Drag>,
    text: Option<PendingText>,
    /// Where the image was drawn last frame, in screen points.
    canvas: Rect,
    moving: Option<MovingText>,
    crop: Option<CropSelection>,
    crop_drag: Option<CropDrag>,
}

/// A crop area waiting for the user to apply or cancel it.
struct CropSelection {
    /// In image pixels.
    rect: Rect,
    /// Doc version it was drawn on; undo/redo discards it.
    version: u64,
}

/// Which edges of the crop selection a drag moves; none set means the whole selection moves.
#[derive(Clone, Copy, Default)]
struct CropEdges {
    left: bool,
    right: bool,
    top: bool,
    bottom: bool,
}

impl CropEdges {
    fn any(self) -> bool {
        self.left || self.right || self.top || self.bottom
    }

    fn cursor(self) -> egui::CursorIcon {
        use egui::CursorIcon::*;
        match (self.left, self.right, self.top, self.bottom) {
            (true, _, true, _) | (_, true, _, true) => ResizeNwSe,
            (true, _, _, true) | (_, true, true, _) => ResizeNeSw,
            (true, _, _, _) | (_, true, _, _) => ResizeHorizontal,
            (_, _, true, _) | (_, _, _, true) => ResizeVertical,
            _ => Move,
        }
    }

    /// What dragging from `p` does to the selection `r`, both in screen points.
    fn at(r: Rect, p: Pos2) -> Option<CropEdges> {
        const REACH: f32 = 8.0;
        let grown = r.expand(REACH);
        if !grown.contains(p) {
            return None;
        }
        // On a tiny selection both edges are in reach; take the closer one.
        let (dl, dr, dt, db) = ((p.x - r.min.x).abs(), (p.x - r.max.x).abs(), (p.y - r.min.y).abs(), (p.y - r.max.y).abs());
        let edges = CropEdges {
            left: dl <= REACH && dl <= dr,
            right: dr <= REACH && dr < dl,
            top: dt <= REACH && dt <= db,
            bottom: db <= REACH && db < dt,
        };
        (edges.any() || r.contains(p)).then_some(edges)
    }
}

/// The crop selection being resized or moved.
struct CropDrag {
    edges: CropEdges,
    start: Rect,
    origin: Pos2,
}

impl CropDrag {
    /// The selection after dragging by `delta` image pixels, kept inside `bounds`.
    fn adjusted(&self, delta: Vec2, bounds: Rect) -> Rect {
        let e = self.edges;
        if !e.any() {
            let r = self.start.translate(delta);
            let dx = (bounds.min.x - r.min.x).max(0.0) + (bounds.max.x - r.max.x).min(0.0);
            let dy = (bounds.min.y - r.min.y).max(0.0) + (bounds.max.y - r.max.y).min(0.0);
            return r.translate(Vec2::new(dx, dy));
        }
        let (mut min, mut max) = (self.start.min, self.start.max);
        if e.left { min.x += delta.x }
        if e.right { max.x += delta.x }
        if e.top { min.y += delta.y }
        if e.bottom { max.y += delta.y }
        Rect::from_two_pos(min, max).intersect(bounds)
    }
}

/// A committed text annotation being dragged to a new spot.
struct MovingText {
    index: usize,
    /// Pointer position relative to the text's top-left, in image pixels.
    grab: Vec2,
    pos: Pos2,
}

/// The font the text preview is drawn with: the renderer's own font when it's available.
fn preview_font(size: f32, matching_font: bool) -> FontId {
    let family = if matching_font {
        egui::FontFamily::Name(ANNOTATION_FONT.into())
    } else {
        egui::FontFamily::Proportional
    };
    FontId::new(size, family)
}

/// Hard-edged stand-in for the renderer's soft shadow, in screen points at the current zoom.
fn shadow_offset(text_size: f32, scale: f32) -> Vec2 {
    Vec2::splat((text_size * 0.06).max(1.5) * scale)
}

impl Editor {
    pub fn new(doc: Doc) -> Editor {
        Editor {
            doc,
            texture: None,
            texture_key: (u64::MAX, None),
            zoom: None,
            shown_scale: 1.0,
            drag: None,
            text: None,
            canvas: Rect::NOTHING,
            moving: None,
            crop: None,
            crop_drag: None,
        }
    }

    /// Crops to the pending selection.
    fn apply_crop(&mut self, font: Option<&FontVec>) {
        let Some(sel) = self.crop.take() else { return };
        let r = Rect::from_min_max(sel.rect.min.round(), sel.rect.max.round());
        if r.width() >= 1.0 && r.height() >= 1.0 {
            self.doc.crop(r, font);
            self.zoom = None;
        }
    }

    /// The text shape being edited, if it still exists (undo could have removed it).
    fn editing_index(&self) -> Option<usize> {
        let i = self.text.as_ref()?.editing?;
        matches!(self.doc.current().shapes.get(i), Some(Shape::Text { .. })).then_some(i)
    }

    /// The shape left out of the rendered canvas because it's being edited or dragged.
    fn hidden_index(&self) -> Option<usize> {
        self.editing_index().or(self.moving.as_ref().map(|m| m.index))
    }

    /// Commits any text being typed, e.g. before saving or switching tools.
    pub fn finish_text(&mut self, tools: &ToolState) {
        let editing = self.editing_index();
        let Some(pending) = self.text.take() else { return };
        let empty = pending.text.trim().is_empty();
        let shape = Shape::Text {
            pos: pending.pos,
            text: pending.text,
            color: tools.colors[Tool::Text as usize],
            size: tools.text_size,
            shadow: tools.text_shadow,
        };
        match editing {
            Some(i) => {
                let unchanged = matches!((&self.doc.current().shapes[i], &shape),
                    (Shape::Text { pos: pa, text: a, color: ca, size: sa, shadow: ha },
                     Shape::Text { pos: pb, text: b, color: cb, size: sb, shadow: hb })
                        if pa == pb && a == b && ca == cb && sa == sb && ha == hb);
                if !unchanged {
                    self.doc.replace(i, (!empty).then_some(shape));
                }
            }
            None if !empty => self.doc.add(shape),
            None => {}
        }
    }

    /// Reopens an existing text annotation for editing, adopting its colour and size.
    fn edit_text(&mut self, index: usize, tools: &mut ToolState) {
        let Some(Shape::Text { pos, text, color, size, shadow }) = self.doc.current().shapes.get(index).cloned() else {
            return;
        };
        tools.tool = Tool::Text;
        *tools.color_mut() = color;
        tools.text_size = size;
        tools.text_shadow = shadow;
        self.text = Some(PendingText { pos, text, focused: false, editing: Some(index), field: Rect::NOTHING });
    }

    pub fn is_typing(&self) -> bool {
        self.text.is_some()
    }

    fn update_texture(&mut self, ctx: &egui::Context, font: Option<&FontVec>) {
        let hidden = self.hidden_index();
        let key = (self.doc.version, hidden);
        if self.texture.is_some() && self.texture_key == key {
            return;
        }
        let mut img = match hidden {
            Some(i) => {
                let mut snapshot = self.doc.current().clone();
                snapshot.shapes.remove(i);
                render(&snapshot, font)
            }
            None => render(self.doc.current(), font),
        };
        // Very wide multi-monitor captures can exceed the GPU's texture limit; show a scaled copy.
        let max = ctx.input(|i| i.max_texture_side) as u32;
        if img.width() > max || img.height() > max {
            let k = max as f32 / img.width().max(img.height()) as f32;
            let (w, h) = ((img.width() as f32 * k) as u32, (img.height() as f32 * k) as u32);
            img = image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle);
        }
        let color = ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
        match &mut self.texture {
            Some(tex) => tex.set(color, TextureOptions::LINEAR),
            None => self.texture = Some(ctx.load_texture("canvas", color, TextureOptions::LINEAR)),
        }
        self.texture_key = key;
    }

    pub fn ui(&mut self, ui: &mut Ui, tools: &mut ToolState, font: Option<&FontVec>) {
        self.update_texture(ui.ctx(), font);
        let size = self.doc.size();
        let avail = ui.available_size();
        let fit = ((avail.x - 16.0) / size.x).min((avail.y - 16.0) / size.y).clamp(0.02, 1.0);

        let zoom_delta = ui.input(|i| i.zoom_delta());
        if zoom_delta != 1.0 && ui.rect_contains_pointer(ui.max_rect()) {
            self.zoom = Some((self.zoom.unwrap_or(fit) * zoom_delta).clamp(0.05, 8.0));
        }
        let scale = self.zoom.unwrap_or(fit);
        self.shown_scale = scale;

        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            let shown = size * scale;
            let (outer, _) = ui.allocate_exact_size(shown.max(ui.available_size()), Sense::hover());
            let rect = Rect::from_min_size(outer.min + ((outer.size() - shown) / 2.0).max(Vec2::ZERO), shown);
            self.canvas = rect;
            let response = ui.interact(rect, ui.id().with("canvas"), Sense::click_and_drag());
            let painter = ui.painter_at(outer);

            painter.rect_filled(rect.expand(1.0), 0.0, Color32::from_black_alpha(90));
            if let Some(tex) = &self.texture {
                let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                painter.image(tex.id(), rect, uv, Color32::WHITE);
            }

            let bounds = Rect::from_min_size(Pos2::ZERO, size);
            let to_img = |p: Pos2| {
                let q = ((p - rect.min) / scale).to_pos2();
                Pos2::new(q.x.clamp(0.0, size.x), q.y.clamp(0.0, size.y))
            };
            let to_screen = |p: Pos2| rect.min + p.to_vec2() * scale;

            // A crop selection only lives while the Crop tool is active and the image is unchanged.
            if tools.tool != Tool::Crop || self.crop.as_ref().is_some_and(|c| c.version != self.doc.version) {
                self.crop = None;
                self.crop_drag = None;
            }
            let crop_screen = self.crop.as_ref().map(|c| Rect::from_two_pos(to_screen(c.rect.min), to_screen(c.rect.max)));
            let crop_hover = crop_screen.zip(response.hover_pos()).and_then(|(r, p)| CropEdges::at(r, p));

            if response.hovered() {
                let cursor = match (tools.tool, crop_hover) {
                    (Tool::Text, _) => egui::CursorIcon::Text,
                    (_, Some(edges)) => edges.cursor(),
                    _ => egui::CursorIcon::Crosshair,
                };
                ui.ctx().set_cursor_icon(cursor);
            }

            if response.drag_started() {
                let press = ui.input(|i| i.pointer.press_origin());
                let origin = press.map(to_img);
                let crop_grab = crop_screen.zip(press).and_then(|(r, p)| Some((CropEdges::at(r, p)?, p)));
                if let (Some((edges, origin)), Some(sel)) = (crop_grab, &self.crop) {
                    // Dragging the crop selection's edges or inside it adjusts it.
                    self.crop_drag = Some(CropDrag { edges, start: sel.rect, origin });
                } else if tools.tool == Tool::Text {
                    // With the Text tool, dragging existing text moves it.
                    self.finish_text(tools);
                    let hit = origin.and_then(|p| Some((p, self.doc.text_at(p, font)?)));
                    if let Some((p, index)) = hit {
                        if let Shape::Text { pos, .. } = &self.doc.current().shapes[index] {
                            self.moving = Some(MovingText { index, grab: p - *pos, pos: *pos });
                        }
                    }
                } else if tools.tool != Tool::Step {
                    self.finish_text(tools);
                    self.crop = None;
                    if let Some(start) = origin {
                        self.drag = Some(Drag { start, current: start, points: vec![start] });
                    }
                }
            }
            if let (Some(moving), Some(pos)) = (&mut self.moving, response.interact_pointer_pos()) {
                moving.pos = to_img(pos) - moving.grab;
            }
            if response.drag_stopped() {
                if let Some(moving) = self.moving.take() {
                    if let Some(Shape::Text { pos, .. }) = self.doc.current().shapes.get(moving.index) {
                        if *pos != moving.pos {
                            let mut moved = self.doc.current().shapes[moving.index].clone();
                            if let Shape::Text { pos, .. } = &mut moved {
                                *pos = moving.pos;
                            }
                            self.doc.replace(moving.index, Some(moved));
                        }
                    }
                }
            }
            if let Some(moving) = &self.moving {
                if let Some(Shape::Text { text, color, size, shadow, .. }) = self.doc.current().shapes.get(moving.index) {
                    let font_id = preview_font(size * scale, font.is_some());
                    let at = to_screen(moving.pos);
                    if *shadow {
                        let galley = painter.layout_no_wrap(text.clone(), font_id.clone(), Color32::from_black_alpha(170));
                        painter.galley(at + shadow_offset(*size, scale), galley, Color32::BLACK);
                    }
                    painter.galley(at, painter.layout_no_wrap(text.clone(), font_id, *color), *color);
                }
            }
            if let (Some(drag), Some(pos)) = (&mut self.drag, response.interact_pointer_pos()) {
                let p = to_img(pos);
                drag.current = p;
                if drag.points.last().is_none_or(|last| last.distance(p) * scale >= 1.5) {
                    drag.points.push(p);
                }
            }
            if response.drag_stopped() {
                if let Some(drag) = self.drag.take() {
                    self.commit_drag(drag, tools);
                }
            }
            if let (Some(adjust), Some(pos), Some(sel)) = (&self.crop_drag, response.interact_pointer_pos(), &mut self.crop) {
                sel.rect = adjust.adjusted((pos - adjust.origin) / scale, bounds);
                ui.ctx().set_cursor_icon(adjust.edges.cursor());
            }
            if response.drag_stopped() {
                self.crop_drag = None;
            }

            // Show which text can be clicked to edit it (or, with the Text tool, dragged to move it).
            let hovered_text = response.hover_pos().filter(|_| self.crop.is_none()).and_then(|pos| self.doc.text_at(to_img(pos), font));
            if self.moving.is_some() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            } else if let Some(i) = hovered_text.filter(|&i| Some(i) != self.editing_index()) {
                if let Shape::Text { pos, text, size, .. } = &self.doc.current().shapes[i] {
                    let b = text_bounds(*pos, text, *size, font);
                    painter.rect_stroke(Rect::from_two_pos(to_screen(b.min), to_screen(b.max)), 2.0, Stroke::new(1.0f32, Color32::from_white_alpha(140)));
                }
                if tools.tool == Tool::Text {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                    response.clone().on_hover_text("Click to edit, drag to move");
                } else {
                    response.clone().on_hover_text("Double-click to edit text");
                }
            }
            // Double-clicking text edits it with any tool whose single click does nothing.
            let edits_on_double_click = !matches!(tools.tool, Tool::Text | Tool::Step | Tool::Pen);
            let in_crop = crop_screen.zip(response.interact_pointer_pos()).is_some_and(|(r, p)| r.contains(p));
            if response.double_clicked() && in_crop {
                self.apply_crop(font);
            } else if response.double_clicked() && edits_on_double_click {
                if let Some(pos) = response.interact_pointer_pos() {
                    self.finish_text(tools);
                    if let Some(i) = self.doc.text_at(to_img(pos), font) {
                        self.edit_text(i, tools);
                    }
                }
            }
            if response.clicked() {
                if let Some(pos) = response.interact_pointer_pos() {
                    let p = to_img(pos);
                    // Clicking the image is what deselects text being edited.
                    self.finish_text(tools);
                    match tools.tool {
                        Tool::Text => {
                            match self.doc.text_at(p, font) {
                                Some(i) => self.edit_text(i, tools),
                                None => self.text = Some(PendingText { pos: p, text: String::new(), focused: false, editing: None, field: Rect::NOTHING }),
                            }
                        }
                        Tool::Step => {
                            self.doc.add(Shape::Step {
                                center: p,
                                number: self.doc.next_step_number(),
                                color: tools.color(),
                                diameter: tools.step_diameter(),
                            });
                        }
                        Tool::Pen => self.doc.add(Shape::Pen { points: vec![p], color: tools.color(), width: tools.width }),
                        _ => {}
                    }
                }
            }

            if let Some(drag) = &self.drag {
                preview(&painter, drag, tools, scale, to_screen, bounds, rect);
            }
            self.crop_confirm(ui, &painter, to_screen, font);
            if tools.tool == Tool::Step && response.hovered() {
                if let Some(pos) = response.hover_pos() {
                    let r = tools.step_diameter() * scale / 2.0;
                    painter.circle_stroke(pos, r, Stroke::new(1.5f32, tools.color()));
                    let n = self.doc.next_step_number().to_string();
                    painter.text(pos, Align2::CENTER_CENTER, n, FontId::proportional(r), tools.color());
                }
            }
            self.text_box(ui, tools, scale, to_screen, font.is_some());
        });
    }

    /// The selected text's entry box plus a corner handle for resizing it.
    ///
    /// Losing keyboard focus doesn't commit the text, so it stays selected while the toolbar's
    /// size, colour and shadow controls are used. Enter, clicking the image or switching tools commits.
    fn text_box(
        &mut self,
        ui: &mut Ui,
        tools: &mut ToolState,
        scale: f32,
        to_screen: impl Fn(Pos2) -> Pos2,
        matching_font: bool,
    ) {
        let Some(pending) = &mut self.text else { return };
        let color = tools.colors[Tool::Text as usize];
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
                    let galley = ui.fonts(|f| f.layout_no_wrap(pending.text.clone(), font.clone(), Color32::from_black_alpha(170)));
                    let offset = shadow_offset(tools.text_size, scale);
                    ui.painter().galley(ui.cursor().min + margin + offset, galley, Color32::BLACK);
                }
                // The box grows to fit the text (clip_text(false)); this is just its minimum width.
                let min_width = if pending.text.is_empty() {
                    ui.fonts(|f| f.layout_no_wrap(HINT.into(), font.clone(), color).size().x)
                } else {
                    tools.text_size * scale * 0.5
                };
                let edit = egui::TextEdit::singleline(&mut pending.text)
                    .font(font)
                    .text_color(color)
                    .hint_text(HINT)
                    .margin(margin)
                    .clip_text(false)
                    .frame(false)
                    .desired_width(min_width);
                let response = ui.add(edit);
                pending.field = response.rect;
                // No background box: the text shows over the image as it will look, with a thin outline marking it selected.
                ui.painter().rect_stroke(response.rect.expand(2.0), 2.0, Stroke::new(1.0f32, Color32::from_rgb(30, 120, 240)));
                if !pending.focused {
                    response.request_focus();
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
                ui.painter().rect_stroke(rect, 2.0, Stroke::new(1.0f32, Color32::from_rgb(30, 120, 240)));
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

    fn commit_drag(&mut self, drag: Drag, tools: &ToolState) {
        let rect = Rect::from_two_pos(drag.start, drag.current);
        let big_enough = rect.width() >= 2.0 && rect.height() >= 2.0;
        let color = tools.color();
        let width = tools.width;
        let shape = match tools.tool {
            Tool::Box if big_enough => Shape::Box { rect, color, width },
            Tool::Ellipse if big_enough => Shape::Ellipse { rect, color, width },
            Tool::Highlight if big_enough => Shape::Highlight { rect, color },
            Tool::Blur if big_enough => Shape::Blur { rect },
            Tool::Arrow if drag.start.distance(drag.current) >= 4.0 => {
                Shape::Arrow { from: drag.start, to: drag.current, color, width }
            }
            Tool::Pen => Shape::Pen { points: drag.points, color, width },
            Tool::Crop if big_enough => {
                // Held until the user confirms it.
                self.crop = Some(CropSelection { rect, version: self.doc.version });
                return;
            }
            _ => return,
        };
        self.doc.add(shape);
    }

    /// Shows the pending crop selection with buttons to apply or cancel it (also Enter / Esc).
    fn crop_confirm(&mut self, ui: &mut Ui, painter: &egui::Painter, to_screen: impl Fn(Pos2) -> Pos2, font: Option<&FontVec>) {
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

fn preview(
    painter: &egui::Painter,
    drag: &Drag,
    tools: &ToolState,
    scale: f32,
    to_screen: impl Fn(Pos2) -> Pos2,
    bounds: Rect,
    canvas: Rect,
) {
    let color = tools.color();
    let stroke = Stroke::new((tools.width * scale).max(1.0), color);
    let r = Rect::from_two_pos(to_screen(drag.start), to_screen(drag.current));
    match tools.tool {
        Tool::Box => {
            painter.rect_stroke(r, 0.0, stroke);
        }
        Tool::Ellipse => {
            let points = (0..64)
                .map(|i| {
                    let t = i as f32 / 64.0 * std::f32::consts::TAU;
                    r.center() + Vec2::new(t.cos() * r.width() / 2.0, t.sin() * r.height() / 2.0)
                })
                .collect();
            painter.add(egui::Shape::closed_line(points, stroke));
        }
        Tool::Highlight => {
            painter.rect_filled(r, 0.0, color.gamma_multiply(0.45));
        }
        Tool::Blur => {
            painter.rect_filled(r, 0.0, Color32::from_gray(128).gamma_multiply(0.6));
            painter.rect_stroke(r, 0.0, Stroke::new(1.0f32, Color32::WHITE));
            painter.text(r.center(), Align2::CENTER_CENTER, "Blur", FontId::proportional(14.0), Color32::WHITE);
        }
        Tool::Arrow => {
            let (shaft_end, head) = arrow_geometry(drag.start, drag.current, tools.width);
            painter.line_segment([to_screen(drag.start), to_screen(shaft_end)], stroke);
            painter.add(egui::Shape::convex_polygon(head.iter().map(|p| to_screen(*p)).collect(), color, Stroke::NONE));
        }
        Tool::Pen => {
            let points = drag.points.iter().map(|p| to_screen(*p)).collect();
            painter.add(egui::Shape::line(points, stroke));
        }
        Tool::Crop => {
            let size = Rect::from_two_pos(drag.start, drag.current).intersect(bounds).size();
            crop_shade(painter, r, canvas, size, false);
        }
        Tool::Text | Tool::Step => {}
    }
}

/// Darkens everything outside the crop area `r` (screen points) and labels its size in pixels.
fn crop_shade(painter: &egui::Painter, r: Rect, canvas: Rect, size: Vec2, handles: bool) {
    let shade = Color32::from_black_alpha(150);
    let c = canvas;
    painter.rect_filled(Rect::from_min_max(c.min, Pos2::new(c.max.x, r.min.y)), 0.0, shade);
    painter.rect_filled(Rect::from_min_max(Pos2::new(c.min.x, r.max.y), c.max), 0.0, shade);
    painter.rect_filled(Rect::from_min_max(Pos2::new(c.min.x, r.min.y), Pos2::new(r.min.x, r.max.y)), 0.0, shade);
    painter.rect_filled(Rect::from_min_max(Pos2::new(r.max.x, r.min.y), Pos2::new(c.max.x, r.max.y)), 0.0, shade);
    painter.rect_stroke(r, 0.0, Stroke::new(1.0f32, Color32::WHITE));
    if handles {
        let accent = Color32::from_rgb(30, 120, 240);
        let (xs, ys) = ([r.min.x, r.center().x, r.max.x], [r.min.y, r.center().y, r.max.y]);
        for (i, &x) in xs.iter().enumerate() {
            for (j, &y) in ys.iter().enumerate() {
                if i == 1 && j == 1 {
                    continue;
                }
                let h = Rect::from_center_size(Pos2::new(x, y), Vec2::splat(8.0));
                painter.rect_filled(h, 1.0, Color32::WHITE);
                painter.rect_stroke(h, 1.0, Stroke::new(1.0f32, accent));
            }
        }
    }
    let label = format!("{} × {}", size.x.round(), size.y.round());
    let (at, align) = if r.min.y - c.min.y >= 18.0 {
        (r.left_top() - Vec2::new(0.0, 4.0), Align2::LEFT_BOTTOM)
    } else {
        (r.left_top() + Vec2::new(6.0, 6.0), Align2::LEFT_TOP)
    };
    painter.text(at, align, label, FontId::proportional(13.0), Color32::WHITE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Context, Event, PointerButton, RawInput};
    use image::RgbaImage;

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
}
