//! Settings → Library & editor → Watermark: a line of text or a picture put in a corner of every
//! copy, save and export (the Export menu turns it off and on), with a preview of it on a sample.

use std::path::Path;
use std::time::SystemTime;

use ab_glyph::FontVec;
use eframe::egui::{self, Color32, RichText, Ui};
use image::RgbaImage;

use super::{row, SettingsApp};
use crate::gallery::doc::{self, Corner, Mark, Watermark};
use crate::gallery::env::PickedFolder;
use crate::gallery::{system_info, theme};
use crate::gallery::{load_watermark, WATERMARK_CORNER, WATERMARK_IMAGE, WATERMARK_KIND, WATERMARK_OPACITY, WATERMARK_SCALE, WATERMARK_TEXT};

/// How big the preview is drawn.
const PREVIEW: egui::Vec2 = egui::Vec2::new(320.0, 180.0);

#[derive(Default)]
pub struct WatermarkState {
    /// The file chooser for a picture, while it's open.
    picking: Option<PickedFolder>,
    /// The font text is drawn in, once it's been looked for (and `None` inside if there's none).
    font: Option<Option<FontVec>>,
    /// The preview, made again when the watermark or its picture changes.
    preview: Option<Preview>,
}

struct Preview {
    watermark: Watermark,
    /// When the picture was last changed, so a changed file shows at once.
    modified: Option<SystemTime>,
    texture: egui::TextureHandle,
    /// Why the picture can't be used, if it can't.
    problem: Option<String>,
}

impl SettingsApp {
    pub(super) fn watermark(&mut self, ui: &mut Ui) {
        self.picked_watermark();
        let muted = theme::palette(ui).muted;
        let kind = self.env.setting(WATERMARK_KIND).filter(|k| *k <= 2).unwrap_or(0);
        let watermark = load_watermark(&self.env);
        theme::card(ui, |ui| {
            let detail = "Put in a corner of every copy, save and export; turn it off for a while in the editor's Export menu";
            row(ui, "Watermark", detail, |ui| {
                theme::group(ui, |ui| {
                    // Right to left, so they read None, Text, Picture.
                    for (label, value) in [("Picture", 2), ("Text", 1), ("None", 0)] {
                        if ui.add(egui::Button::selectable(kind == value, label)).clicked() && kind != value {
                            self.set(WATERMARK_KIND, value);
                        }
                    }
                })
            });
            if kind == 0 {
                return;
            }
            ui.separator();
            if kind == 1 {
                let mut text = self.env.text_setting(WATERMARK_TEXT).unwrap_or_default();
                row(ui, "Text", "In white with a dark outline, so it reads on anything", |ui| {
                    let edit = ui.add(egui::TextEdit::singleline(&mut text).hint_text("© Your name").desired_width(220.0));
                    edit.widget_info(|| {
                        let mut info = egui::WidgetInfo::text_edit(true, &text, &text, "");
                        info.label = Some("Watermark text".into());
                        info
                    });
                    if edit.changed() {
                        self.env.set_text_setting(WATERMARK_TEXT, &text);
                    }
                });
            } else {
                let path = self.env.text_setting(WATERMARK_IMAGE).filter(|p| !p.trim().is_empty());
                let detail = match &path {
                    Some(path) => system_info::tidy_path(Path::new(path), self.home.as_deref()),
                    None => "A PNG, such as a logo; transparent parts stay clear".to_owned(),
                };
                row(ui, "Picture", &detail, |ui| {
                    let choosing = self.watermarking.picking.is_some();
                    if ui.add_enabled(!choosing, egui::Button::new("Choose…")).clicked() {
                        self.watermarking.picking = Some(self.env.pick_file(ui.ctx(), "Choose a picture for the watermark", ("PNG image", "*.png")));
                    }
                });
            }
            ui.separator();
            let corner = watermark.as_ref().map_or_else(|| self.env.setting(WATERMARK_CORNER).and_then(|i| Corner::ALL.get(i as usize).copied()).unwrap_or_default(), |w| w.corner);
            row(ui, "Corner", "", |ui| {
                theme::group(ui, |ui| {
                    for (i, c) in Corner::ALL.iter().enumerate().rev() {
                        if ui.add(egui::Button::selectable(corner == *c, c.name())).clicked() && corner != *c {
                            self.set(WATERMARK_CORNER, i as u32);
                        }
                    }
                })
            });
            if let Some(w) = &watermark {
                let (mut opacity, mut scale) = (w.opacity, w.scale);
                let of = if matches!(w.mark, Mark::Text(_)) { "% of the height" } else { "% of the width" };
                let detail = if matches!(w.mark, Mark::Text(_)) { "How tall the text is" } else { "How wide the picture is" };
                row(ui, "Opacity", "", |ui| {
                    let slider = ui.add(egui::Slider::new(&mut opacity, 0..=100).suffix(" %"));
                    slider.widget_info(|| egui::WidgetInfo::slider(true, opacity as f64, "Watermark opacity"));
                    if slider.changed() {
                        self.set(WATERMARK_OPACITY, opacity);
                    }
                });
                row(ui, "Size", detail, |ui| {
                    let slider = ui.add(egui::Slider::new(&mut scale, 1..=100).suffix(format!(" {of}")));
                    slider.widget_info(|| egui::WidgetInfo::slider(true, scale as f64, "Watermark size"));
                    if slider.changed() {
                        self.set(WATERMARK_SCALE, scale);
                    }
                });
                ui.add_space(4.0);
                if let Some(preview) = self.watermark_preview(ui.ctx(), w) {
                    if let Some(problem) = &preview.problem {
                        ui.label(RichText::new(problem).color(Color32::from_rgb(255, 120, 110)));
                    }
                    let image = egui::Image::new((preview.texture.id(), PREVIEW)).corner_radius(4);
                    ui.add(image).widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, "Watermark preview"));
                }
            } else {
                let what = if kind == 1 { "Type the text to put on" } else { "Choose a picture to put on" };
                ui.label(RichText::new(what).small().color(muted));
            }
        });
    }

    /// A picture, once chosen.
    fn picked_watermark(&mut self) {
        let Some(Ok(picked)) = self.watermarking.picking.as_ref().map(|rx| rx.try_recv()) else { return };
        self.watermarking.picking = None;
        match picked {
            Ok(Some(file)) => self.env.set_text_setting(WATERMARK_IMAGE, &file.to_string_lossy()),
            Ok(None) => {}
            Err(e) => self.note = Some(format!("Couldn't open the file chooser: {e}")),
        }
    }

    /// The preview of `w` on a sample screenshot, made again if it has changed.
    fn watermark_preview(&mut self, ctx: &egui::Context, w: &Watermark) -> Option<&Preview> {
        let picture = match &w.mark {
            Mark::Image(path) => Some(path.clone()),
            Mark::Text(_) => None,
        };
        let modified = picture.as_ref().and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
        let state = &mut self.watermarking;
        if state.preview.as_ref().is_none_or(|p| p.watermark != *w || p.modified != modified) {
            let font = state.font.get_or_insert_with(|| doc::load_font_bytes().and_then(|b| FontVec::try_from_vec(b).ok()));
            let problem = picture.and_then(|path| picture_problem(&path));
            let effects = doc::Effects { watermark: Some(w.clone()), ..Default::default() };
            let shown = doc::apply_effects(sample(), &effects, font.as_ref());
            let color = egui::ColorImage::from_rgba_unmultiplied([shown.width() as usize, shown.height() as usize], shown.as_raw());
            let texture = ctx.load_texture("watermark-preview", color, egui::TextureOptions::LINEAR);
            state.preview = Some(Preview { watermark: w.clone(), modified, texture, problem });
        }
        state.preview.as_ref()
    }
}

/// Why the picture at `path` can't be a watermark, if it can't.
fn picture_problem(path: &Path) -> Option<String> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    match doc::read_picture(path) {
        Ok(_) => None,
        Err(_) if !path.exists() => Some(format!("{name} isn't there any more, so no watermark is added")),
        Err(e) => Some(format!("{name} can't be read as a PNG ({e}), so no watermark is added")),
    }
}

/// A made-up screenshot, light at the top and dark at the bottom, with lines of "text", for the
/// preview: the watermark should read on both.
fn sample() -> RgbaImage {
    let (w, h) = (PREVIEW.x as u32 * 2, PREVIEW.y as u32 * 2);
    RgbaImage::from_fn(w, h, |x, y| {
        let t = y as f32 / h as f32;
        let mix = |a: f32, b: f32| (a + (b - a) * t) as u8;
        // Lines of different lengths, in two columns.
        let length = 80 + (y / 36 * 53 + x / 300 * 31) % 160;
        let line = (y % 36) < 10 && x % 300 > 40 && x % 300 < 40 + length;
        if line && t < 0.55 {
            image::Rgba([120, 128, 140, 255])
        } else {
            image::Rgba([mix(236.0, 34.0), mix(240.0, 52.0), mix(248.0, 96.0), 255])
        }
    })
}
