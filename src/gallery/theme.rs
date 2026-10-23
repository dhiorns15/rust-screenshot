//! The look of RSnip's windows: neutral surfaces in a dark or light palette, a blue accent, rounded
//! controls, and Phosphor icons (https://phosphoricons.com, MIT).

use eframe::egui::{self, Color32, CornerRadius, Margin, Response, RichText, Sense, Stroke, Theme, Ui, Vec2, Widget};
pub use egui_phosphor::regular as icon;

use super::editor::Tool;

pub const ACCENT: Color32 = Color32::from_rgb(79, 140, 255);

/// The surfaces and text colours of one theme.
pub struct Palette {
    /// Behind the image being edited.
    pub canvas: Color32,
    pub panel: Color32,
    /// The tool options row and the Settings sidebar, a step down from the panel.
    pub subpanel: Color32,
    /// Grouped controls, inputs and buttons.
    pub raised: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub border: Color32,
    /// Cards in the Settings window, a step up from the panel.
    pub card: Color32,
    pub text: Color32,
    pub muted: Color32,
    /// Disabled icons.
    pub faint: Color32,
    /// An off switch's track.
    pub track: Color32,
    pub input: Color32,
}

pub const DARK: Palette = Palette {
    canvas: Color32::from_rgb(20, 21, 24),
    panel: Color32::from_rgb(29, 30, 34),
    subpanel: Color32::from_rgb(25, 26, 30),
    raised: Color32::from_rgb(39, 41, 46),
    hover: Color32::from_rgb(52, 55, 62),
    pressed: Color32::from_rgb(64, 68, 76),
    border: Color32::from_rgb(46, 48, 54),
    card: Color32::from_rgb(35, 37, 42),
    text: Color32::from_rgb(226, 228, 233),
    muted: Color32::from_rgb(146, 150, 160),
    faint: Color32::from_rgb(84, 87, 95),
    track: Color32::from_rgb(58, 61, 68),
    input: Color32::from_rgb(18, 19, 22),
};

pub const LIGHT: Palette = Palette {
    canvas: Color32::from_rgb(228, 230, 235),
    panel: Color32::from_rgb(250, 250, 252),
    subpanel: Color32::from_rgb(242, 243, 246),
    raised: Color32::from_rgb(232, 234, 238),
    hover: Color32::from_rgb(220, 223, 229),
    pressed: Color32::from_rgb(206, 210, 218),
    border: Color32::from_rgb(218, 221, 227),
    card: Color32::WHITE,
    text: Color32::from_rgb(28, 30, 35),
    muted: Color32::from_rgb(100, 105, 116),
    faint: Color32::from_rgb(178, 182, 190),
    track: Color32::from_rgb(196, 200, 208),
    input: Color32::WHITE,
};

/// The palette of the theme the windows are showing now.
pub fn palette(ctx: &egui::Context) -> &'static Palette {
    match ctx.theme() {
        Theme::Dark => &DARK,
        Theme::Light => &LIGHT,
    }
}

/// Light, dark, or whatever the system uses (stored as the `Theme` setting).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Appearance {
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const SETTING: &'static str = "Theme";
    pub const ALL: [Appearance; 3] = [Appearance::System, Appearance::Light, Appearance::Dark];

    /// Unknown values (or none) mean following the system.
    pub fn from_setting(value: Option<u32>) -> Appearance {
        match value {
            Some(1) => Appearance::Light,
            Some(2) => Appearance::Dark,
            _ => Appearance::System,
        }
    }

    pub fn to_setting(self) -> u32 {
        match self {
            Appearance::System => 0,
            Appearance::Light => 1,
            Appearance::Dark => 2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Appearance::System => "System",
            Appearance::Light => "Light",
            Appearance::Dark => "Dark",
        }
    }

    fn preference(self) -> egui::ThemePreference {
        match self {
            Appearance::System => egui::ThemePreference::System,
            Appearance::Light => egui::ThemePreference::Light,
            Appearance::Dark => egui::ThemePreference::Dark,
        }
    }
}

fn visuals(p: &Palette, dark: bool) -> egui::Visuals {
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = p.panel;
    v.window_fill = p.panel;
    v.faint_bg_color = p.raised;
    v.extreme_bg_color = p.input;
    v.window_stroke = Stroke::new(1.0f32, p.border);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT.gamma_multiply(if dark { 0.45 } else { 0.3 });
    v.selection.stroke = Stroke::new(1.0f32, ACCENT);
    v.override_text_color = Some(p.text);

    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0f32, p.border);
    w.noninteractive.fg_stroke.color = p.text;
    w.inactive.weak_bg_fill = p.raised;
    w.inactive.bg_fill = p.raised;
    w.inactive.bg_stroke = Stroke::NONE;
    w.hovered.weak_bg_fill = p.hover;
    w.hovered.bg_fill = p.hover;
    w.hovered.bg_stroke = Stroke::NONE;
    w.active.weak_bg_fill = p.pressed;
    w.active.bg_fill = p.pressed;
    w.active.bg_stroke = Stroke::NONE;
    w.open.weak_bg_fill = p.hover;
    for state in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        state.corner_radius = CornerRadius::same(6);
        state.expansion = 0.0;
    }
    v
}

/// Sets up both themes and shows the one `appearance` asks for.
pub fn apply(ctx: &egui::Context, appearance: Appearance) {
    ctx.set_visuals_of(Theme::Dark, visuals(&DARK, true));
    ctx.set_visuals_of(Theme::Light, visuals(&LIGHT, false));
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = Vec2::new(6.0, 6.0);
        s.spacing.button_padding = Vec2::new(10.0, 5.0);
        s.spacing.interact_size.y = 26.0;
        s.spacing.menu_margin = Margin::same(6);
    });
    ctx.set_theme(appearance.preference());
}

/// Shows the theme `appearance` asks for, if it isn't already.
pub fn set_appearance(ctx: &egui::Context, appearance: Appearance) {
    let preference = appearance.preference();
    if ctx.options(|o| o.theme_preference) != preference {
        ctx.set_theme(preference);
    }
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
        Tool::CopyText => icon::TEXTBOX,
    }
}

/// A square icon button; `selected` fills it with the accent colour. `label` names it for screen
/// readers and tests, and `captioned` also writes it under the icon.
pub struct IconButton {
    icon: &'static str,
    label: &'static str,
    selected: bool,
    captioned: bool,
}

/// The size of the name under a captioned button's icon.
const CAPTION: f32 = 11.0;

impl IconButton {
    pub fn new(icon: &'static str, label: &'static str) -> Self {
        IconButton { icon, label, selected: false, captioned: false }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn captioned(mut self, captioned: bool) -> Self {
        self.captioned = captioned;
        self
    }

    /// How big it is.
    pub fn size(&self, ui: &Ui) -> Vec2 {
        if !self.captioned {
            return Vec2::new(30.0, 28.0);
        }
        let caption = ui.painter().layout_no_wrap(self.label.to_owned(), egui::FontId::proportional(CAPTION), Color32::WHITE);
        Vec2::new((caption.size().x + 10.0).max(40.0).ceil(), 44.0)
    }
}

impl Widget for IconButton {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, response) = ui.allocate_exact_size(self.size(ui), Sense::click());
        response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), self.selected, self.label));
        if ui.is_rect_visible(rect) {
            let p = palette(ui);
            let enabled = ui.is_enabled();
            let fill = if self.selected {
                ACCENT
            } else if enabled && response.is_pointer_button_down_on() {
                p.pressed
            } else if enabled && response.hovered() {
                p.hover
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, 6.0, fill);
            let color = match (self.selected, enabled) {
                (true, _) => Color32::WHITE,
                (false, true) => p.text,
                (false, false) => p.faint,
            };
            let icon = egui::FontId::proportional(18.0);
            if self.captioned {
                let top = rect.center_top();
                ui.painter().text(top + Vec2::new(0.0, 15.0), egui::Align2::CENTER_CENTER, self.icon, icon, color);
                let caption = egui::FontId::proportional(CAPTION);
                ui.painter().text(top + Vec2::new(0.0, 34.0), egui::Align2::CENTER_CENTER, self.label, caption, color);
            } else {
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, self.icon, icon, color);
            }
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
    group_frame(ui).show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.horizontal(add).inner
    })
    .inner
}

/// A `group` of controls of the same height, lined up along their tops (centring them in the row
/// would offset any taller than a line of text).
pub fn group_top<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    group_frame(ui).show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.horizontal_top(add).inner
    })
    .inner
}

fn group_frame(ui: &Ui) -> egui::Frame {
    egui::Frame::NONE.fill(palette(ui).raised).corner_radius(8).inner_margin(Margin::same(2))
}

/// "Icon Label" for buttons and menu items.
pub fn label(icon: &str, text: &str) -> String {
    format!("{icon}  {text}")
}

/// An on/off switch; `label` names it for screen readers and tests. Clicking it flips `on` and
/// marks the response changed.
pub fn switch(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(Vec2::new(38.0, 22.0), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let enabled = ui.is_enabled();
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, *on, label));
    if ui.is_rect_visible(rect) {
        let p = palette(ui);
        let track = match (*on, enabled) {
            (true, true) => ACCENT,
            (true, false) => ACCENT.gamma_multiply(0.4),
            (false, _) if response.hovered() && enabled => p.pressed,
            (false, _) => p.track,
        };
        ui.painter().rect_filled(rect, 11.0, track);
        let x = if *on { rect.right() - 11.0 } else { rect.left() + 11.0 };
        let knob = if enabled { Color32::WHITE } else { Color32::from_gray(150) };
        ui.painter().circle_filled(egui::pos2(x, rect.center().y), 8.0, knob);
    }
    response
}

/// A full-width item in a sidebar: icon and text on the left, filled when selected.
pub fn nav_item(ui: &mut Ui, icon: &str, text: &str, selected: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, text));
    if ui.is_rect_visible(rect) {
        let p = palette(ui);
        let fill = if selected {
            ACCENT.gamma_multiply(0.22)
        } else if response.hovered() {
            p.hover
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, 8.0, fill);
        let color = if selected { ACCENT } else { p.text };
        let y = rect.center().y;
        ui.painter().text(egui::pos2(rect.left() + 12.0, y), egui::Align2::LEFT_CENTER, icon, egui::FontId::proportional(17.0), color);
        let text_color = if selected { p.text } else { p.muted };
        ui.painter().text(egui::pos2(rect.left() + 40.0, y), egui::Align2::LEFT_CENTER, text, egui::FontId::proportional(14.5), text_color);
    }
    response
}

/// A rounded card grouping related settings.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = palette(ui);
    egui::Frame::NONE
        .fill(p.card)
        .stroke(Stroke::new(1.0f32, p.border))
        .corner_radius(10)
        .inner_margin(Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearance_setting_round_trips_and_ignores_junk() {
        for a in Appearance::ALL {
            assert_eq!(Appearance::from_setting(Some(a.to_setting())), a);
        }
        assert_eq!(Appearance::from_setting(None), Appearance::System);
        assert_eq!(Appearance::from_setting(Some(9)), Appearance::System);
    }
}
