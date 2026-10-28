//! Picking the part of the screen to record or capture, on Linux: a full-screen window showing a
//! frame of the screen to drag a rectangle on. `rsnip --pick-area <frame.png> [capture|text|scroll]`
//! prints the area in the frame's pixels as "x y width height", or nothing if it was cancelled.
//!
//! The area is measured on the frame as shown, so it's right whichever screen the window opens on.

use std::path::Path;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Key, Pos2, Rect, Sense, Stroke, StrokeKind, TextureOptions, Vec2};

use super::recorder::Area;
use super::theme::ACCENT;

/// Areas smaller than this (in frame pixels) are taken as a click, not a drag.
const MIN_SIZE: f32 = 16.0;

/// What the area is for, which the window says.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Purpose {
    Record,
    Capture,
    /// Copying the text in it.
    Text,
    /// A scrolling capture of what's in it.
    Scroll,
}

impl Purpose {
    /// As given to `rsnip --pick-area`; recording, if it's missing.
    pub fn parse(name: Option<&str>) -> Purpose {
        match name {
            Some("capture") => Purpose::Capture,
            Some("text") => Purpose::Text,
            Some("scroll") => Purpose::Scroll,
            _ => Purpose::Record,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Purpose::Record => "record",
            Purpose::Capture => "capture",
            Purpose::Text => "text",
            Purpose::Scroll => "scroll",
        }
    }

    /// What to choose, for the window's title and hint.
    fn what(self) -> &'static str {
        match self {
            Purpose::Record => "the area to record",
            Purpose::Capture => "the area to capture",
            Purpose::Text => "the text to copy",
            Purpose::Scroll => "the part that scrolls",
        }
    }
}

/// Shows the frame and waits for an area; `None` if cancelled.
pub fn run(frame: &Path, purpose: Purpose) -> Option<Area> {
    let image = image::open(frame).ok()?.to_rgba8();
    let picked = Arc::new(Mutex::new(None));
    let result = picked.clone();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("RSnip: choose {}", purpose.what()))
            .with_app_id("rsnip")
            .with_fullscreen(true)
            .with_decorations(false),
        ..Default::default()
    };
    let app = Box::new(move |cc: &eframe::CreationContext| -> Result<Box<dyn eframe::App>, _> { Ok(Box::new(Picker::new(&cc.egui_ctx, &image, purpose, result))) });
    eframe::run_native("RSnip area", options, app).ok()?;
    let area = picked.lock().ok()?.take();
    area
}

struct Picker {
    texture: egui::TextureHandle,
    /// The frame's size in pixels.
    size: Vec2,
    /// Where the drag started, in frame pixels.
    start: Option<Pos2>,
    purpose: Purpose,
    picked: Arc<Mutex<Option<Area>>>,
}

impl Picker {
    fn new(ctx: &egui::Context, image: &image::RgbaImage, purpose: Purpose, picked: Arc<Mutex<Option<Area>>>) -> Picker {
        let size = [image.width() as usize, image.height() as usize];
        let texture = ctx.load_texture("frame", egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()), TextureOptions::LINEAR);
        Picker { texture, size: Vec2::new(size[0] as f32, size[1] as f32), start: None, purpose, picked }
    }

    fn finish(&self, ctx: &egui::Context, area: Option<Area>) {
        if let Ok(mut picked) = self.picked.lock() {
            *picked = area;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

/// The selection between two points in frame pixels, inside the frame.
pub fn area_between(a: Pos2, b: Pos2, size: Vec2) -> Option<Area> {
    let r = Rect::from_two_pos(a, b).intersect(Rect::from_min_size(Pos2::ZERO, size));
    (r.width() >= MIN_SIZE && r.height() >= MIN_SIZE).then(|| Area {
        x: r.min.x.round() as u32,
        y: r.min.y.round() as u32,
        width: r.width().round() as u32,
        height: r.height().round() as u32,
    })
}

impl eframe::App for Picker {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let screen = ui.max_rect();
        // The frame fitted to the window.
        let scale = (screen.width() / self.size.x).min(screen.height() / self.size.y);
        let shown = Rect::from_center_size(screen.center(), self.size * scale);
        let to_frame = |p: Pos2| (((p - shown.min) / scale).to_pos2()).clamp(Pos2::ZERO, self.size.to_pos2());
        let to_screen = |p: Pos2| shown.min + p.to_vec2() * scale;

        let response = ui.allocate_rect(screen, Sense::drag());
        let painter = ui.painter();
        painter.rect_filled(screen, 0.0, Color32::BLACK);
        painter.image(self.texture.id(), shown, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        ctx.set_cursor_icon(egui::CursorIcon::Crosshair);

        if response.drag_started() {
            self.start = response.interact_pointer_pos().map(to_frame);
        }
        let current = response.interact_pointer_pos().map(to_frame);
        let selection = self.start.zip(current).map(|(a, b)| Rect::from_two_pos(to_screen(a), to_screen(b)));
        // Dim everything but the selection.
        let dim = Color32::from_black_alpha(130);
        match selection {
            Some(sel) => {
                for r in [
                    Rect::from_min_max(shown.min, Pos2::new(shown.max.x, sel.min.y)),
                    Rect::from_min_max(Pos2::new(shown.min.x, sel.max.y), shown.max),
                    Rect::from_min_max(Pos2::new(shown.min.x, sel.min.y), Pos2::new(sel.min.x, sel.max.y)),
                    Rect::from_min_max(Pos2::new(sel.max.x, sel.min.y), Pos2::new(shown.max.x, sel.max.y)),
                ] {
                    painter.rect_filled(r, 0.0, dim);
                }
                painter.rect_stroke(sel, 0.0, Stroke::new(2.0f32, ACCENT), StrokeKind::Outside);
                let (w, h) = ((sel.width() / scale).round(), (sel.height() / scale).round());
                let label = Pos2::new(sel.min.x, (sel.min.y - 6.0).max(screen.min.y + 18.0));
                painter.text(label, Align2::LEFT_BOTTOM, format!("{w} × {h}"), FontId::proportional(14.0), Color32::WHITE);
            }
            None => {
                painter.rect_filled(shown, 0.0, dim);
            }
        }

        let hint = format!("Drag over {}   ·   Enter: the whole screen   ·   Esc: cancel", self.purpose.what());
        let galley = painter.layout_no_wrap(hint,FontId::proportional(16.0), Color32::WHITE);
        let pill = Rect::from_center_size(Pos2::new(screen.center().x, screen.min.y + 40.0), galley.size() + Vec2::new(32.0, 16.0));
        painter.rect_filled(pill, CornerRadius::same(20), Color32::from_black_alpha(200));
        painter.galley(pill.center() - galley.size() / 2.0, galley, Color32::WHITE);

        // One too small to be meant is ignored: drag again.
        if response.drag_stopped() {
            if let Some(area) = self.start.take().zip(current).and_then(|(a, b)| area_between(a, b, self.size)) {
                self.finish(&ctx, Some(area));
            }
        }
        if ctx.input(|i| i.key_pressed(Key::Enter)) {
            let whole = Area { x: 0, y: 0, width: self.size.x as u32, height: self.size.y as u32 };
            self.finish(&ctx, Some(whole));
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) || ctx.input(|i| i.viewport().close_requested()) && self.picked.lock().is_ok_and(|p| p.is_none()) {
            self.finish(&ctx, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;

    use super::*;

    /// The picker on a 1920 × 1080 frame, in a window half that size.
    fn picker() -> (Harness<'static, Picker>, Arc<Mutex<Option<Area>>>) {
        let picked = Arc::new(Mutex::new(None));
        let result = picked.clone();
        let image = image::RgbaImage::from_pixel(1920, 1080, image::Rgba([40, 80, 160, 255]));
        let h = Harness::builder().with_size(Vec2::new(960.0, 540.0)).build_eframe(move |cc| Picker::new(&cc.egui_ctx, &image, Purpose::Record, result));
        (h, picked)
    }

    #[test]
    fn says_what_its_for() {
        assert_eq!(["record", "capture", "text", "scroll", "junk"].map(|n| Purpose::parse(Some(n))), [Purpose::Record, Purpose::Capture, Purpose::Text, Purpose::Scroll, Purpose::Record]);
        assert_eq!(Purpose::parse(None), Purpose::Record, "as it was before screenshots");
        for p in [Purpose::Record, Purpose::Capture, Purpose::Text, Purpose::Scroll] {
            assert_eq!(Purpose::parse(Some(p.name())), p);
        }
    }

    #[test]
    fn dragging_picks_the_area_in_the_frames_pixels() {
        let (mut h, picked) = picker();
        h.step();
        h.hover_at(Pos2::new(100.0, 50.0));
        h.drag_at(Pos2::new(100.0, 50.0));
        h.step();
        for x in [200.0, 300.0, 400.0] {
            h.hover_at(Pos2::new(x, 250.0));
            h.step();
        }
        h.drop_at(Pos2::new(400.0, 250.0));
        h.step();
        h.step();
        assert_eq!(*picked.lock().unwrap(), Some(Area { x: 200, y: 100, width: 600, height: 400 }));
    }

    #[test]
    fn enter_picks_the_whole_frame_and_esc_nothing() {
        let (mut h, picked) = picker();
        h.step();
        h.key_press(Key::Enter);
        h.step();
        assert_eq!(*picked.lock().unwrap(), Some(Area { x: 0, y: 0, width: 1920, height: 1080 }));

        let (mut h, picked) = picker();
        h.step();
        h.key_press(Key::Escape);
        h.step();
        assert_eq!(*picked.lock().unwrap(), None);
    }

    #[test]
    fn an_area_is_inside_the_frame_and_big_enough() {
        let size = Vec2::new(1920.0, 1080.0);
        assert_eq!(area_between(Pos2::new(500.0, 400.0), Pos2::new(100.0, 50.0), size), Some(Area { x: 100, y: 50, width: 400, height: 350 }));
        assert_eq!(area_between(Pos2::new(1800.0, 1000.0), Pos2::new(2500.0, 1500.0), size), Some(Area { x: 1800, y: 1000, width: 120, height: 80 }));
        assert_eq!(area_between(Pos2::new(10.0, 10.0), Pos2::new(15.0, 100.0), size), None, "a click, not a drag");
    }
}
