//! The library window's look: neutral dark surfaces, a blue accent, rounded controls, and Phosphor
//! icons (https://phosphoricons.com, MIT).

use eframe::egui::{self, Color32, Margin, Response, RichText, Rounding, Sense, Stroke, Ui, Vec2, Widget};
pub use egui_phosphor::regular as icon;

use super::editor::Tool;

pub const ACCENT: Color32 = Color32::from_rgb(79, 140, 255);
/// Behind the image being edited.
pub const CANVAS: Color32 = Color32::from_rgb(20, 21, 24);
pub const PANEL: Color32 = Color32::from_rgb(29, 30, 34);
/// The tool options row, a step down from the toolbar.
pub const SUBPANEL: Color32 = Color32::from_rgb(25, 26, 30);
/// Grouped controls, inputs and buttons.
pub const RAISED: Color32 = Color32::from_rgb(39, 41, 46);
pub const HOVER: Color32 = Color32::from_rgb(52, 55, 62);
pub const BORDER: Color32 = Color32::from_rgb(46, 48, 54);
pub const TEXT: Color32 = Color32::from_rgb(226, 228, 233);
pub const MUTED: Color32 = Color32::from_rgb(146, 150, 160);

pub fn apply(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.faint_bg_color = RAISED;
    v.extreme_bg_color = Color32::from_rgb(18, 19, 22);
    v.window_stroke = Stroke::new(1.0f32, BORDER);
    v.window_rounding = Rounding::same(10.0);
    v.menu_rounding = Rounding::same(8.0);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT.gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0f32, ACCENT);
    v.override_text_color = Some(TEXT);

    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0f32, BORDER);
    w.noninteractive.fg_stroke.color = TEXT;
    w.inactive.weak_bg_fill = RAISED;
    w.inactive.bg_fill = RAISED;
    w.inactive.bg_stroke = Stroke::NONE;
    w.hovered.weak_bg_fill = HOVER;
    w.hovered.bg_fill = HOVER;
    w.hovered.bg_stroke = Stroke::NONE;
    w.active.weak_bg_fill = Color32::from_rgb(64, 68, 76);
    w.active.bg_fill = Color32::from_rgb(64, 68, 76);
    w.active.bg_stroke = Stroke::NONE;
    w.open.weak_bg_fill = HOVER;
    for state in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        state.rounding = Rounding::same(6.0);
        state.expansion = 0.0;
    }
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(6.0, 6.0);
        s.spacing.button_padding = Vec2::new(10.0, 5.0);
        s.spacing.interact_size.y = 26.0;
        s.spacing.menu_margin = Margin::same(6.0);
    });
}

pub fn add_icons(fonts: &mut egui::FontDefinitions) {
    egui_phosphor::add_to_fonts(fonts, egui_phosphor::Variant::Regular);
}

pub fn tool_icon(tool: Tool) -> &'static str {
    match tool {
        Tool::Select => icon::CURSOR,
        Tool::Arrow => icon::ARROW_UP_RIGHT,
        Tool::Line => icon::LINE_SEGMENT,
        Tool::Box => icon::SQUARE,
        Tool::Ellipse => icon::CIRCLE,
        Tool::Pen => icon::PEN_NIB,
        Tool::Text => icon::TEXT_T,
        Tool::Step => icon::NUMBER_CIRCLE_ONE,
        Tool::Highlight => icon::HIGHLIGHTER_CIRCLE,
        Tool::Spotlight => icon::FLASHLIGHT,
        Tool::Blur => icon::DROP_HALF,
        Tool::Pixelate => icon::CHECKERBOARD,
        Tool::Redact => icon::EYE_SLASH,
        Tool::Crop => icon::CROP,
    }
}

/// A square icon button; `selected` fills it with the accent colour.
pub struct IconButton {
    icon: &'static str,
    selected: bool,
    size: Vec2,
}

impl IconButton {
    pub fn new(icon: &'static str) -> Self {
        IconButton { icon, selected: false, size: Vec2::new(30.0, 28.0) }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

impl Widget for IconButton {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, response) = ui.allocate_exact_size(self.size, Sense::click());
        if ui.is_rect_visible(rect) {
            let enabled = ui.is_enabled();
            let fill = if self.selected {
                ACCENT
            } else if enabled && response.is_pointer_button_down_on() {
                Color32::from_rgb(64, 68, 76)
            } else if enabled && response.hovered() {
                HOVER
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, 6.0, fill);
            let color = match (self.selected, enabled) {
                (true, _) => Color32::WHITE,
                (false, true) => TEXT,
                (false, false) => Color32::from_rgb(84, 87, 95),
            };
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, self.icon, egui::FontId::proportional(18.0), color);
        }
        response
    }
}

/// The filled accent button for the main action.
pub fn primary_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.into()).color(Color32::WHITE)).fill(ACCENT)
}

/// A rounded container that groups related controls, like a segmented control.
pub fn group<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::none()
        .fill(RAISED)
        .rounding(8.0)
        .inner_margin(Margin::same(2.0))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(add).inner
        })
        .inner
}

/// "Icon Label" for buttons and menu items.
pub fn label(icon: &str, text: &str) -> String {
    format!("{icon}  {text}")
}
