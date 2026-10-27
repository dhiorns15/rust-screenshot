//! The quick-settings flyout (`rsnip --quick-settings`), opened from the tray icon's menu: the
//! settings people change most, without opening Settings. On Windows it sits above the tray icon,
//! in the corner by where it was clicked; elsewhere the desktop places it. It closes on Esc or when
//! another window is clicked.
//!
//! It reads and writes the same settings as the Settings window, with the same rows (see
//! `settings_rows`), and tells the running tray app to apply them.

use std::time::Duration;

use eframe::egui::{self, Key, Pos2, Rect, RichText, Ui, Vec2, ViewportCommand};

use super::env::Env;
use super::settings_rows;
use super::theme::{self, icon, Appearance};
use super::{platform, APP_ID};
use crate::options;

pub const TITLE: &str = "RSnip Quick settings";
pub const WIDTH: f32 = 320.0;
/// Its height until the content has been measured.
const HEIGHT: f32 = 460.0;
/// Inside the window, around the content.
const MARGIN: i8 = 12;
/// How close to the edge of the screen's usable area it goes, in points.
const GAP: f32 = 12.0;

/// Where the flyout opens, in pixels: the cursor (on the tray icon) and the usable part of the
/// screen it's on (the work area, which leaves out the taskbar).
pub struct Anchor {
    pub cursor: Pos2,
    pub work: Rect,
    /// Pixels to a point on that screen.
    pub scale: f32,
}

impl Anchor {
    /// The top left of a flyout `size` pixels big with its bottom right by the cursor, kept
    /// inside the work area.
    fn top_left(&self, size: Vec2) -> Pos2 {
        let inside = self.work.shrink(GAP * self.scale);
        let wanted = self.cursor - size;
        Pos2::new(wanted.x.min(inside.right() - size.x).max(inside.left()), wanted.y.min(inside.bottom() - size.y).max(inside.top()))
    }
}

pub fn run() {
    if !platform::claim_single_instance("QuickSettings", TITLE) {
        return;
    }
    // `--at X,Y`: where the tray icon was clicked, in pixels (the cursor has moved to the menu).
    let args: Vec<String> = std::env::args().collect();
    let at = args.iter().skip_while(|a| *a != "--at").nth(1).and_then(|at| at.split_once(','));
    let at = at.and_then(|(x, y)| Some((x.parse().ok()?, y.parse().ok()?)));
    let anchor = platform::flyout_anchor(at);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(TITLE)
        .with_inner_size([WIDTH, HEIGHT])
        .with_decorations(false)
        .with_resizable(false)
        .with_always_on_top()
        .with_taskbar(false)
        .with_icon(super::window_icon())
        .with_app_id(APP_ID);
    if let Some(anchor) = &anchor {
        // Where it should be on that screen; it's put right once the window knows its own scale.
        let corner = anchor.top_left(Vec2::new(WIDTH, HEIGHT) * anchor.scale) / anchor.scale;
        viewport = viewport.with_position(corner);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    let _ = eframe::run_native(TITLE, options, Box::new(move |cc| Ok(Box::new(QuickSettings::new(&cc.egui_ctx, Env::real(), anchor)))));
}

pub struct QuickSettings {
    pub env: Env,
    anchor: Option<Anchor>,
    /// The microphones that are connected, found when the drop-down first shows.
    microphones: Option<Vec<platform::Microphone>>,
    /// It has had the focus; from then on, losing it closes the flyout.
    had_focus: bool,
    /// How high the content makes the window.
    pub height: f32,
    /// The size and place last asked of the window, in points.
    placed: Option<(Vec2, Pos2)>,
    /// It's been asked to close.
    pub closing: bool,
}

impl QuickSettings {
    pub fn new(ctx: &egui::Context, env: Env, anchor: Option<Anchor>) -> QuickSettings {
        theme::apply(ctx, Appearance::from_setting(env.setting(Appearance::SETTING)));
        let mut fonts = egui::FontDefinitions::default();
        theme::add_icons(&mut fonts);
        ctx.set_fonts(fonts);
        QuickSettings { env, anchor, microphones: None, had_focus: false, height: HEIGHT, placed: None, closing: false }
    }

    fn close(&mut self, ctx: &egui::Context) {
        self.closing = true;
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }

    /// Asks the window to be as high as the content, and (on Windows) to keep its bottom right by
    /// the cursor, so it grows upwards when the microphone drop-down appears.
    fn fit(&mut self, ctx: &egui::Context) {
        let size = Vec2::new(WIDTH, self.height);
        let scale = ctx.pixels_per_point();
        let corner = self.anchor.as_ref().map(|anchor| anchor.top_left(size * scale) / scale);
        let moved = |(was_size, was_corner): (Vec2, Pos2)| (was_size - size).abs().max_elem() > 0.5 || corner.is_some_and(|c| (was_corner - c).abs().max_elem() > 0.5);
        if self.placed.is_none_or(moved) {
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
            if let Some(corner) = corner {
                ctx.send_viewport_cmd(ViewportCommand::OuterPosition(corner));
            }
            self.placed = Some((size, corner.unwrap_or_default()));
        }
    }

    fn content(&mut self, ui: &mut Ui) {
        let muted = theme::palette(ui).muted;
        let gif = options::RECORD_GIF.read(|name| self.env.setting(name));
        ui.label(RichText::new("Quick settings").size(16.0).strong());
        let heading = |ui: &mut Ui, text: &str| ui.label(RichText::new(text).size(13.0).strong().color(muted));

        heading(ui, "Capture");
        theme::card(ui, |ui| {
            settings_rows::flag_row(ui, &self.env, options::SAVE_TO_FOLDER, "Save screenshots to the folder", "");
            ui.separator();
            settings_rows::flag_row(ui, &self.env, options::OPEN_EDITOR, "Open the editor after capture", "");
            // On Linux the area is picked on a still of the screen.
            if self.env.tray_app {
                ui.separator();
                settings_rows::flag_row(ui, &self.env, options::SHOW_MAGNIFIER, "Show the magnifier", "");
                ui.separator();
                settings_rows::flag_row(ui, &self.env, options::HIDE_WHILE_CAPTURING, "Hide RSnip while capturing", "");
            }
        });

        heading(ui, "Recording");
        theme::card(ui, |ui| {
            settings_rows::format_row(ui, &self.env, "");
            ui.separator();
            ui.add_enabled_ui(!gif, |ui| settings_rows::frame_rate_row(ui, &self.env, ""));
        });
        theme::card(ui, |ui| settings_rows::sound_rows(ui, &self.env, &mut self.microphones, false));

        if ui.add_sized([ui.available_width(), 30.0], egui::Button::new(theme::label(icon::GEAR_SIX, "All settings…"))).clicked() {
            self.env.open_recording_settings();
            self.close(ui.ctx());
        }
    }
}

impl eframe::App for QuickSettings {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        theme::set_appearance(&ctx, Appearance::from_setting(self.env.setting(Appearance::SETTING)));

        // Closes on losing the focus, once it's had it: it may not have arrived yet.
        match ctx.input(|i| i.viewport().focused) {
            Some(true) => self.had_focus = true,
            Some(false) if self.had_focus => self.close(&ctx),
            _ if !self.had_focus => {
                ctx.send_viewport_cmd(ViewportCommand::Focus);
                ctx.request_repaint_after(Duration::from_millis(100));
            }
            _ => {}
        }
        // Esc first closes a drop-down that's open.
        if ctx.input(|i| i.key_pressed(Key::Escape)) && !egui::Popup::is_any_open(&ctx) {
            self.close(&ctx);
        }

        let p = theme::palette(ui);
        let frame = egui::Frame::NONE.fill(p.panel).stroke(egui::Stroke::new(1.0f32, p.border)).inner_margin(egui::Margin::same(MARGIN));
        let margins = frame.total_margin().sum().y;
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let content = ui.vertical(|ui| self.content(ui)).response.rect;
            self.height = content.height() + margins;
        });
        self.fit(&ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(cursor: (f32, f32)) -> Anchor {
        // A 1920 x 1080 screen with a 40 pixel taskbar along the bottom, at 100%.
        Anchor { cursor: Pos2::new(cursor.0, cursor.1), work: Rect::from_min_max(Pos2::ZERO, Pos2::new(1920.0, 1040.0)), scale: 1.0 }
    }

    #[test]
    fn sits_above_the_tray_icon_in_its_corner() {
        let size = Vec2::new(320.0, 400.0);
        // The icon is in the taskbar, so it's the bottom of the work area that counts.
        assert_eq!(anchor((1850.0, 1060.0)).top_left(size), Pos2::new(1530.0, 1040.0 - GAP - 400.0));
        // Near the left edge, it stays on the screen.
        assert_eq!(anchor((100.0, 1060.0)).top_left(size).x, GAP);
    }

    #[test]
    fn hangs_below_a_taskbar_along_the_top() {
        let top = Anchor { cursor: Pos2::new(1850.0, 10.0), work: Rect::from_min_max(Pos2::new(0.0, 40.0), Pos2::new(1920.0, 1080.0)), scale: 1.0 };
        assert_eq!(top.top_left(Vec2::new(320.0, 400.0)).y, 40.0 + GAP);
    }

    #[test]
    fn keeps_its_gap_in_pixels_on_a_scaled_screen() {
        let scaled = Anchor { cursor: Pos2::new(3000.0, 2100.0), work: Rect::from_min_max(Pos2::ZERO, Pos2::new(3840.0, 2080.0)), scale: 2.0 };
        let corner = scaled.top_left(Vec2::new(640.0, 800.0));
        assert_eq!(corner, Pos2::new(2360.0, 2080.0 - 2.0 * GAP - 800.0));
    }
}
