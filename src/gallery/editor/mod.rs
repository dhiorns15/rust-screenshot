//! The annotation canvas: tools, mouse handling and live previews.

mod crop;
mod handles;
mod preview;
mod select;
#[cfg(test)]
mod tests;
mod text;
mod tools;

use ab_glyph::FontVec;
use eframe::egui::{self, Align2, Color32, ColorImage, FontId, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions, Ui, Vec2};

use super::doc::{render, Doc, Shape};
use crop::{crop_grab, CropDrag, CropSelection};
use preview::{preview, shape_from_drag};
use select::{Grab, Manip, Selection, PICK_SLACK};
use text::PendingText;
pub use tools::{Style, Tool, ToolState, ANNOTATION_FONT, PALETTE, TEXT_SIZES, TOOLS};

struct Drag {
    start: Pos2,
    current: Pos2,
    points: Vec<Pos2>,
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
    manip: Option<Manip>,
    selection: Option<Selection>,
    /// The tool active last frame; switching tools drops the selection.
    last_tool: Tool,
    crop: Option<CropSelection>,
    crop_drag: Option<CropDrag>,
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
            manip: None,
            selection: None,
            last_tool: Tool::Select,
            crop: None,
            crop_drag: None,
        }
    }

    /// The shape left out of the rendered canvas because it's being edited or dragged.
    fn hidden_index(&self) -> Option<usize> {
        self.editing_index().or(self.manip.as_ref().map(|m| m.index))
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
        self.sync_selection(tools);
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
            let crop_hover = crop_screen.zip(response.hover_pos()).and_then(|(r, p)| crop_grab(r, p));

            if response.hovered() {
                let cursor = match (tools.tool, crop_hover) {
                    (Tool::Text, _) => egui::CursorIcon::Text,
                    (_, Some(edges)) => edges.cursor(),
                    (Tool::Select, _) => egui::CursorIcon::Default,
                    _ => egui::CursorIcon::Crosshair,
                };
                ui.ctx().set_cursor_icon(cursor);
            }

            if response.drag_started() {
                let press = ui.input(|i| i.pointer.press_origin());
                let origin = press.map(to_img);
                let on_crop = crop_screen.zip(press).and_then(|(r, p)| Some((crop_grab(r, p)?, p)));
                let on_selection = press.and_then(|p| Some((self.selection_grab(p, to_screen, to_img, scale, font)?, p)));
                if let (Some((edges, origin)), Some(sel)) = (on_crop, &self.crop) {
                    // Dragging the crop selection's edges or inside it adjusts it.
                    self.crop_drag = Some(CropDrag { edges, start: sel.rect, origin });
                } else if let (Some((grab, p)), Some(index)) = (on_selection, self.selection.as_ref().map(|s| s.index)) {
                    self.begin_manip(index, grab, p, true, font);
                } else if tools.tool == Tool::Text {
                    // With the Text tool, dragging existing text moves it.
                    self.finish_text(tools);
                    if let (Some(index), Some(p)) = (origin.and_then(|o| self.doc.text_at(o, font)), press) {
                        self.begin_manip(index, Grab::Move, p, false, font);
                    }
                } else if tools.tool == Tool::Select {
                    self.finish_text(tools);
                    match (origin.and_then(|o| self.doc.shape_at(o, PICK_SLACK / scale, font)), press) {
                        (Some(index), Some(p)) => self.begin_manip(index, Grab::Move, p, true, font),
                        _ => self.deselect(tools),
                    }
                } else if tools.tool != Tool::Step {
                    self.finish_text(tools);
                    self.crop = None;
                    self.deselect(tools);
                    if let Some(start) = origin {
                        self.drag = Some(Drag { start, current: start, points: vec![start] });
                    }
                }
            }
            self.update_manip(response.interact_pointer_pos(), scale);
            if response.drag_stopped() {
                self.end_manip(tools);
            }
            self.paint_manip(&painter, scale, to_screen, font.is_some());
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

            self.selection_hover(ui, &painter, response.hover_pos(), tools, to_screen, to_img, scale, font);
            let text_hover = response.hover_pos().filter(|_| self.crop.is_none()).map(to_img);
            self.text_hover(ui, &response, &painter, text_hover, tools, font, to_screen);
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
                    let on_selection = self.selection_grab(pos, to_screen, to_img, scale, font).is_some();
                    match tools.tool {
                        Tool::Select => match self.doc.shape_at(p, PICK_SLACK / scale, font) {
                            Some(i) => self.select(i, tools),
                            None => self.deselect(tools),
                        },
                        Tool::Text => {
                            match self.doc.text_at(p, font) {
                                Some(i) => self.edit_text(i, tools),
                                None => self.text = Some(PendingText { pos: p, text: String::new(), focused: false, editing: None, field: Rect::NOTHING }),
                            }
                        }
                        Tool::Step if !on_selection => {
                            self.doc.add(Shape::Step {
                                center: p,
                                number: self.doc.next_step_number(),
                                color: tools.color(),
                                diameter: tools.step_diameter(),
                            });
                            self.select(self.doc.current().shapes.len() - 1, tools);
                        }
                        Tool::Pen => {
                            self.deselect(tools);
                            self.doc.add(Shape::Pen { points: vec![p], color: tools.color(), width: tools.width });
                        }
                        // Clicking off the selection with a drawing tool deselects it.
                        _ if !on_selection => self.deselect(tools),
                        _ => {}
                    }
                }
            }

            if let Some(drag) = &self.drag {
                preview(&painter, drag, tools, scale, to_screen, bounds, rect);
            }
            self.paint_selection(&painter, to_screen, font);
            self.selection_keys(ui, tools);
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

    fn commit_drag(&mut self, drag: Drag, tools: &mut ToolState) {
        if tools.tool == Tool::Crop {
            let rect = Rect::from_two_pos(drag.start, drag.current);
            if rect.width() >= 2.0 && rect.height() >= 2.0 {
                // Held until the user confirms it.
                self.crop = Some(CropSelection { rect, version: self.doc.version });
            }
            return;
        }
        let Some(shape) = shape_from_drag(&drag, tools) else { return };
        let pen = matches!(shape, Shape::Pen { .. });
        self.doc.add(shape);
        // What was just drawn stays selected so it can be adjusted straight away. Not freehand
        // strokes, which are often drawn over each other.
        if !pen {
            self.select(self.doc.current().shapes.len() - 1, tools);
        }
    }
}
