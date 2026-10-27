//! The capture panel (`rsnip --capture-panel`), opened by left-clicking the tray icon (or from the
//! icon's menu): choose what to capture or record, change the options that matter for it, and
//! press the big red button. On Windows it sits above the tray icon, in the corner by where it was
//! clicked, until it's dragged somewhere else (by any empty part of it), and then opens where it
//! was left; elsewhere the desktop places it. It closes on Esc or its ×, and when another window is
//! clicked unless it's pinned to stay open.
//!
//! The switches are the same settings as in the Settings window, with the same rows (see
//! `settings_rows`), and tell the running tray app to apply them. What's being captured, and which
//! tab was last shown, are remembered here only.
//!
//! The panel must be gone before the capture starts, or it would be in the picture: it closes, and
//! `run` starts the capture once its window has been taken down. One pinned open on Windows stays,
//! and the tray app hides it for the capture or recording and brings it back after.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use eframe::egui::{self, Align, Color32, Key, Layout, Margin, Pos2, Rect, RichText, Sense, Ui, Vec2, ViewportCommand};

use super::env::Env;
use super::settings_rows;
use super::theme::{self, icon, Appearance, IconButton, ACCENT};
use super::{platform, APP_ID};
use crate::options;
use crate::shortcuts::{self, Action};

pub const TITLE: &str = "RSnip Capture panel";
pub const WIDTH: f32 = 650.0;
/// Its height until the content has been measured.
const HEIGHT: f32 = 240.0;
/// Inside the window, around the content.
const MARGIN: i8 = 14;
/// How close to the edge of the screen's usable area it goes, in points.
const GAP: f32 = 12.0;
/// The tabs down the left, the options in two columns, and the button, with this between them.
const TABS: f32 = 72.0;
const OPTIONS: f32 = 210.0;
const SWITCHES: f32 = 214.0;
const BUTTON_COLUMN: f32 = 88.0;
const COLUMN_GAP: f32 = 12.0;
/// The width of the names beside the drop-downs.
const LABEL: f32 = 74.0;
/// The round Capture button.
const BUTTON: f32 = 80.0;
/// How long the panel's window has to go from the screen before the capture starts.
const GONE: Duration = Duration::from_millis(150);

/// Where the panel opens, in pixels: the cursor (on the tray icon), or where it was last moved to,
/// and the usable part of the screen that's on (the work area, which leaves out the taskbar).
pub struct Anchor {
    pub cursor: Pos2,
    pub work: Rect,
    /// Pixels to a point on that screen.
    pub scale: f32,
    /// `cursor` is where the panel was moved to: its top left, not the tray icon.
    pub moved: bool,
}

impl Anchor {
    /// The top left of a panel `size` pixels big: with its bottom right by the cursor, or where it
    /// was moved to, kept inside the work area.
    fn top_left(&self, size: Vec2) -> Pos2 {
        let (inside, wanted) = if self.moved { (self.work, self.cursor) } else { (self.work.shrink(GAP * self.scale), self.cursor - size) };
        Pos2::new(wanted.x.min(inside.right() - size.x).max(inside.left()), wanted.y.min(inside.bottom() - size.y).max(inside.top()))
    }
}

/// "x,y", as `CAPTURE_PANEL_PLACE` keeps it.
fn parse_place(text: &str) -> Option<(i32, i32)> {
    let (x, y) = text.split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

/// What the panel has been asked to run: the flag to start RSnip with (see `launch_flag`).
pub type Launch = Rc<Cell<Option<&'static str>>>;

pub fn run() {
    if !platform::claim_single_instance("CapturePanel", TITLE) {
        return;
    }
    // `--at X,Y`: where the tray icon was clicked, in pixels (the cursor has moved to the menu).
    let args: Vec<String> = std::env::args().collect();
    let at = args.iter().skip_while(|a| *a != "--at").nth(1).and_then(|at| at.split_once(','));
    let at = at.and_then(|(x, y)| Some((x.parse().ok()?, y.parse().ok()?)));
    let env = Env::real();
    let moved_to = env.text_setting(options::CAPTURE_PANEL_PLACE).as_deref().and_then(parse_place);
    let anchor = platform::flyout_anchor(at, moved_to);
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
    let launch = Launch::default();
    let asked = launch.clone();
    let _ = eframe::run_native(TITLE, options, Box::new(move |cc| Ok(Box::new(CapturePanel::new(&cc.egui_ctx, env, anchor, asked)))));
    // Its window is gone now; give the screen a moment to show what was behind it.
    if let Some(flag) = launch.take() {
        std::thread::sleep(GONE);
        platform::start_capture(flag);
    }
}

/// The two tabs, down the left.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Image,
    Video,
}

impl Tab {
    const ALL: [Tab; 2] = [Tab::Image, Tab::Video];

    fn label(self) -> &'static str {
        match self {
            Tab::Image => "Image",
            Tab::Video => "Video",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Tab::Image => icon::CAMERA,
            Tab::Video => icon::VIDEO_CAMERA,
        }
    }

    fn read(env: &Env) -> Tab {
        Tab::ALL[options::CAPTURE_PANEL_TAB.read(|name| env.setting(name)) as usize]
    }
}

/// What an image capture takes in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImageSelection {
    Region,
    Full,
    Last,
    Scrolling,
    Text,
}

impl ImageSelection {
    const ALL: [ImageSelection; 5] = [ImageSelection::Region, ImageSelection::Full, ImageSelection::Last, ImageSelection::Scrolling, ImageSelection::Text];

    fn label(self) -> &'static str {
        match self {
            ImageSelection::Region => "Region",
            ImageSelection::Full => "Full screen",
            ImageSelection::Last => "Last area",
            ImageSelection::Scrolling => "Scrolling",
            ImageSelection::Text => "Text",
        }
    }

    fn help(self) -> &'static str {
        match self {
            ImageSelection::Region => "Drag over what to capture.",
            ImageSelection::Full => "Every screen, at once.",
            ImageSelection::Last => "The same area as last time.",
            ImageSelection::Scrolling => "Pick a window, and RSnip scrolls it for you.",
            ImageSelection::Text => "Drag over text to copy it.",
        }
    }

    /// The command line flag that takes it.
    fn flag(self) -> &'static str {
        match self {
            ImageSelection::Region => "--region",
            ImageSelection::Full => "--full",
            ImageSelection::Last => "--last",
            ImageSelection::Scrolling => "--scroll",
            ImageSelection::Text => "--text",
        }
    }

    /// The shortcut that does the same, on Windows.
    fn action(self) -> Action {
        match self {
            ImageSelection::Region => Action::Region,
            ImageSelection::Full => Action::Full,
            ImageSelection::Last => Action::Last,
            ImageSelection::Scrolling => Action::Scroll,
            ImageSelection::Text => Action::Text,
        }
    }

    /// The ones this system can take (only Windows scrolls a window).
    fn available(env: &Env) -> Vec<ImageSelection> {
        ImageSelection::ALL.into_iter().filter(|s| *s != ImageSelection::Scrolling || env.tray_app).collect()
    }

    /// The one remembered, unless this system can't do it.
    fn read(env: &Env) -> ImageSelection {
        let chosen = ImageSelection::ALL[options::CAPTURE_PANEL_IMAGE.read(|name| env.setting(name)) as usize];
        if ImageSelection::available(env).contains(&chosen) { chosen } else { ImageSelection::Region }
    }
}

/// What a recording takes in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VideoSelection {
    Area,
    /// A screen or window, picked in the desktop's own dialog (Linux).
    ScreenOrWindow,
}

impl VideoSelection {
    const ALL: [VideoSelection; 2] = [VideoSelection::Area, VideoSelection::ScreenOrWindow];

    fn label(self) -> &'static str {
        match self {
            VideoSelection::Area => "Area",
            VideoSelection::ScreenOrWindow => "Screen or window",
        }
    }

    fn flag(self) -> &'static str {
        match self {
            VideoSelection::Area => "--record",
            VideoSelection::ScreenOrWindow => "--record-screen",
        }
    }

    fn available(env: &Env) -> Vec<VideoSelection> {
        VideoSelection::ALL.into_iter().filter(|s| *s != VideoSelection::ScreenOrWindow || !env.tray_app).collect()
    }

    fn read(env: &Env) -> VideoSelection {
        let chosen = VideoSelection::ALL[options::CAPTURE_PANEL_VIDEO.read(|name| env.setting(name)) as usize];
        if VideoSelection::available(env).contains(&chosen) { chosen } else { VideoSelection::Area }
    }
}

/// What the Capture button runs: the flag `rsnip` is started with.
pub fn launch_flag(env: &Env) -> &'static str {
    match Tab::read(env) {
        Tab::Image => ImageSelection::read(env).flag(),
        Tab::Video => VideoSelection::read(env).flag(),
    }
}

pub struct CapturePanel {
    pub env: Env,
    anchor: Option<Anchor>,
    /// The outputs and microphones there are, found when a drop-down first shows.
    sound_devices: settings_rows::SoundDevices,
    /// It has had the focus; from then on, losing it closes the panel.
    had_focus: bool,
    /// How high the content makes the window.
    pub height: f32,
    /// The size and place last asked of the window, in points.
    placed: Option<(Vec2, Pos2)>,
    /// It's been asked to close.
    pub closing: bool,
    /// Set to the flag to run once the window has closed, when Capture was pressed.
    pub launch: Launch,
    /// It's being (or has been) dragged: it stays where it's put, which is remembered.
    dragged: bool,
    /// Where it was last remembered to be, in pixels.
    remembered: Option<Pos2>,
}

impl CapturePanel {
    pub fn new(ctx: &egui::Context, env: Env, anchor: Option<Anchor>, launch: Launch) -> CapturePanel {
        theme::apply(ctx, Appearance::from_setting(env.setting(Appearance::SETTING)));
        let mut fonts = egui::FontDefinitions::default();
        theme::add_icons(&mut fonts);
        ctx.set_fonts(fonts);
        CapturePanel { env, anchor, sound_devices: Default::default(), had_focus: false, height: HEIGHT, placed: None, closing: false, launch, dragged: false, remembered: None }
    }

    /// It stays open when another window is clicked.
    pub fn keep_open(&self) -> bool {
        options::CAPTURE_PANEL_KEEP_OPEN.read(|name| self.env.setting(name))
    }

    fn close(&mut self, ctx: &egui::Context) {
        self.closing = true;
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }

    /// Closes, unless it's pinned open: after opening the library or Settings.
    fn done(&mut self, ctx: &egui::Context) {
        if !self.keep_open() {
            self.close(ctx);
        }
    }

    /// Closes, and asks for the capture to start once the window has gone. Pinned open on Windows,
    /// it starts the capture straight away and stays: the tray app hides it while it captures.
    fn capture(&mut self, ctx: &egui::Context) {
        let flag = launch_flag(&self.env);
        if self.keep_open() && self.env.tray_app {
            self.env.start_capture(flag);
        } else {
            self.launch.set(Some(flag));
            self.close(ctx);
        }
    }

    /// Lets the window be dragged by any part of it that isn't a control, and remembers where it's
    /// left (in pixels, so a scaled screen gets it back in the same place).
    fn movable(&mut self, ui: &mut Ui) {
        let behind = ui.interact(ui.max_rect(), ui.id().with("move"), Sense::drag());
        if behind.drag_started() {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
            self.dragged = true;
            // It's where it's put from now on.
            self.anchor = None;
        }
        if !self.dragged {
            return;
        }
        let scale = ui.ctx().pixels_per_point();
        if let Some(outer) = ui.ctx().input(|i| i.viewport().outer_rect) {
            let at = (outer.min.to_vec2() * scale).round().to_pos2();
            if self.remembered != Some(at) {
                self.env.set_text_setting(options::CAPTURE_PANEL_PLACE, &format!("{},{}", at.x, at.y));
                self.remembered = Some(at);
            }
        }
    }

    /// Asks the window to be as high as the content, and (on Windows) to keep its bottom right by
    /// the cursor, so it grows upwards when the microphone drop-down appears; once it's been moved,
    /// its top left stays put.
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

    /// The shortcut that does what the button does, if there is one (on Windows).
    fn shortcut(&self, tab: Tab) -> Option<String> {
        if !self.env.tray_app {
            return None;
        }
        let action = match tab {
            Tab::Image => ImageSelection::read(&self.env).action(),
            Tab::Video => Action::Record,
        };
        let key = shortcuts::get(&shortcuts::load(|name| self.env.setting(name)), action);
        (key != 0).then(|| platform::shortcut_name(key))
    }

    fn content(&mut self, ui: &mut Ui) {
        let tab = Tab::read(&self.env);
        let p = theme::palette(ui);
        ui.spacing_mut().item_spacing = Vec2::new(COLUMN_GAP, 8.0);
        // The rail behind the tabs is drawn once the height of the options is known.
        let rail_fill = ui.painter().add(egui::Shape::Noop);
        let mut rail = Rect::NOTHING;
        let main = ui.horizontal_top(|ui| {
            rail = egui::Frame::NONE.inner_margin(Margin::same(6)).show(ui, |ui| self.tabs(ui, tab)).response.rect;
            ui.vertical(|ui| {
                ui.set_width(OPTIONS);
                match tab {
                    Tab::Image => self.image_options(ui),
                    Tab::Video => self.video_options(ui),
                }
            });
            ui.vertical(|ui| {
                ui.set_width(SWITCHES);
                match tab {
                    Tab::Image => self.image_switches(ui),
                    Tab::Video => self.video_switches(ui),
                }
            });
            // The button is centred beside however high the options come to.
            let height = ui.min_rect().height();
            let shortcut = self.shortcut(tab);
            let extra = if shortcut.is_some() { 20.0 } else { 0.0 };
            ui.vertical_centered(|ui| {
                ui.set_width(BUTTON_COLUMN);
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.add_space(((height - BUTTON - extra) / 2.0).max(0.0));
                if capture_button(ui, if tab == Tab::Video { "Record" } else { "Capture" }).clicked() {
                    self.capture(ui.ctx());
                }
                if let Some(shortcut) = shortcut {
                    ui.label(RichText::new(shortcut).size(12.0).color(theme::palette(ui).muted));
                }
            });
        });
        let rail = Rect::from_min_max(rail.min, Pos2::new(rail.max.x, main.response.rect.bottom()));
        ui.painter().set(rail_fill, egui::epaint::RectShape::filled(rail, 10, p.subpanel));

        ui.add_space(2.0);
        ui.separator();
        self.footer(ui, tab);
    }

    /// Image and Video, one above the other.
    fn tabs(&mut self, ui: &mut Ui, shown: Tab) {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.vertical(|ui| {
            ui.set_width(TABS - 12.0);
            for tab in Tab::ALL {
                if tab_button(ui, tab, tab == shown).clicked() && tab != shown {
                    self.env.set_setting(options::CAPTURE_PANEL_TAB.name, tab as u32);
                }
            }
        });
    }

    /// A name beside its control.
    fn field(ui: &mut Ui, name: &str, control: impl FnOnce(&mut Ui)) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.allocate_ui_with_layout(Vec2::new(LABEL, 26.0), Layout::left_to_right(Align::Center), |ui| {
                // A fixed-width column, so the controls line up.
                ui.set_min_size(Vec2::new(LABEL, 26.0));
                ui.label(RichText::new(name).color(theme::palette(ui).muted));
            });
            control(ui);
        });
    }

    /// A drop-down filling what's left of the row, choosing among `items`.
    fn drop_down<T: Copy + PartialEq>(ui: &mut Ui, id: &str, items: &[T], chosen: T, label: impl Fn(T) -> String) -> Option<T> {
        let mut picked = None;
        egui::ComboBox::from_id_salt(id).width(ui.available_width()).selected_text(label(chosen)).show_ui(ui, |ui| {
            for &item in items {
                if ui.selectable_label(item == chosen, label(item)).clicked() && item != chosen {
                    picked = Some(item);
                }
            }
        });
        picked
    }

    fn image_options(&mut self, ui: &mut Ui) {
        let chosen = ImageSelection::read(&self.env);
        Self::field(ui, "Selection", |ui| {
            let items = ImageSelection::available(&self.env);
            if let Some(pick) = Self::drop_down(ui, "image-selection", &items, chosen, |s| s.label().into()) {
                self.env.set_setting(options::CAPTURE_PANEL_IMAGE.name, ImageSelection::ALL.iter().position(|s| *s == pick).unwrap_or(0) as u32);
            }
        });
        ui.label(RichText::new(chosen.help()).size(12.0).color(theme::palette(ui).muted));
    }

    fn image_switches(&mut self, ui: &mut Ui) {
        settings_rows::flag_row(ui, &self.env, options::OPEN_EDITOR, "Open in editor", "");
        settings_rows::flag_row(ui, &self.env, options::SAVE_TO_FOLDER, "Save to folder", "");
        // On Linux the area is picked on a still of the screen.
        if self.env.tray_app {
            settings_rows::flag_row(ui, &self.env, options::SHOW_MAGNIFIER, "Magnifier", "");
            settings_rows::flag_row(ui, &self.env, options::HIDE_WHILE_CAPTURING, "Hide RSnip while capturing", "");
        }
    }

    fn video_options(&mut self, ui: &mut Ui) {
        let gif = options::RECORD_GIF.read(|name| self.env.setting(name));
        Self::field(ui, "Selection", |ui| {
            let items = VideoSelection::available(&self.env);
            let chosen = VideoSelection::read(&self.env);
            if let Some(pick) = Self::drop_down(ui, "video-selection", &items, chosen, |s| s.label().into()) {
                self.env.set_setting(options::CAPTURE_PANEL_VIDEO.name, VideoSelection::ALL.iter().position(|s| *s == pick).unwrap_or(0) as u32);
            }
        });
        Self::field(ui, "Format", |ui| {
            theme::group(ui, |ui| {
                for (label, value) in [("MP4", false), ("GIF", true)] {
                    if ui.add(egui::Button::selectable(gif == value, label)).clicked() && gif != value {
                        settings_rows::set(&self.env, options::RECORD_GIF.name, value as u32);
                    }
                }
            });
        });
        // GIFs have a frame rate of their own.
        ui.add_enabled_ui(!gif, |ui| {
            Self::field(ui, "Frame rate", |ui| {
                let fps = options::RECORD_FPS.read(|name| self.env.setting(name));
                if let Some(pick) = Self::drop_down(ui, "frame-rate", options::RECORD_FPS.values, fps, |fps| format!("{fps} fps")) {
                    settings_rows::set(&self.env, options::RECORD_FPS.name, pick);
                }
            });
        });
    }

    fn video_switches(&mut self, ui: &mut Ui) {
        let gif = options::RECORD_GIF.read(|name| self.env.setting(name));
        ui.add_enabled_ui(!gif, |ui| {
            settings_rows::flag_row(ui, &self.env, options::RECORD_SOUND, "Computer sound", "");
            if options::RECORD_SOUND.read(|name| self.env.setting(name)) {
                settings_rows::output_picker(ui, &self.env, &mut self.sound_devices);
            }
            settings_rows::flag_row(ui, &self.env, options::RECORD_MICROPHONE, "Microphone", "");
            if options::RECORD_MICROPHONE.read(|name| self.env.setting(name)) {
                settings_rows::microphone_picker(ui, &self.env, &mut self.sound_devices);
            }
        });
        if gif {
            ui.label(RichText::new("GIFs have no sound").size(12.0).color(theme::palette(ui).muted));
        }
    }

    /// The library, and Settings.
    fn footer(&mut self, ui: &mut Ui, tab: Tab) {
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new(theme::label(icon::FOLDER_OPEN, "Open library & editor")).frame(false)).clicked() {
                self.env.open_library();
                self.done(ui.ctx());
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(IconButton::new(icon::X, "Close")).on_hover_text("Close (Esc)").clicked() {
                    self.close(ui.ctx());
                }
                if ui.add(IconButton::new(icon::GEAR_SIX, "Settings")).on_hover_text("Settings").clicked() {
                    match tab {
                        Tab::Image => self.env.open_settings(),
                        Tab::Video => self.env.open_recording_settings(),
                    }
                    self.done(ui.ctx());
                }
                let keep_open = self.keep_open();
                let hint = if keep_open { "Stays open when you click elsewhere: click to let it close" } else { "Keep it open when you click elsewhere" };
                if ui.add(IconButton::new(icon::PUSH_PIN, "Keep open").selected(keep_open)).on_hover_text(hint).clicked() {
                    self.env.set_setting(options::CAPTURE_PANEL_KEEP_OPEN.name, !keep_open as u32);
                }
            });
        });
    }
}

/// A tab: its icon over its name, filled when it's the one showing.
fn tab_button(ui: &mut Ui, tab: Tab, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 58.0), Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, tab.label()));
    if ui.is_rect_visible(rect) {
        let p = theme::palette(ui);
        let fill = if selected {
            ACCENT.gamma_multiply(0.22)
        } else if response.hovered() {
            p.hover
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, 8.0, fill);
        let (icon_color, name_color) = if selected { (ACCENT, p.text) } else { (p.muted, p.muted) };
        let top = rect.center_top();
        ui.painter().text(top + Vec2::new(0.0, 19.0), egui::Align2::CENTER_CENTER, tab.icon(), egui::FontId::proportional(22.0), icon_color);
        ui.painter().text(top + Vec2::new(0.0, 43.0), egui::Align2::CENTER_CENTER, tab.label(), egui::FontId::proportional(12.5), name_color);
    }
    response
}

/// The big round red button, with `name` in it.
fn capture_button(ui: &mut Ui, name: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(BUTTON), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    if ui.is_rect_visible(rect) {
        let fill = if response.is_pointer_button_down_on() {
            Color32::from_rgb(190, 38, 32)
        } else if response.hovered() {
            Color32::from_rgb(238, 74, 64)
        } else {
            Color32::from_rgb(222, 52, 44)
        };
        ui.painter().circle_filled(rect.center(), BUTTON / 2.0, fill);
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, name, egui::FontId::proportional(15.0), Color32::WHITE);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

impl eframe::App for CapturePanel {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        theme::set_appearance(&ctx, Appearance::from_setting(self.env.setting(Appearance::SETTING)));

        // Closes on losing the focus, once it's had it (it may not have arrived yet), unless it's
        // pinned open.
        match ctx.input(|i| i.viewport().focused) {
            Some(true) => self.had_focus = true,
            Some(false) if self.had_focus && !self.keep_open() => self.close(&ctx),
            _ if !self.had_focus => {
                ctx.send_viewport_cmd(ViewportCommand::Focus);
                ctx.request_repaint_after(Duration::from_millis(100));
            }
            _ => {}
        }
        // Esc first closes a drop-down that's open.
        let list_open = egui::Popup::is_any_open(&ctx);
        if ctx.input(|i| i.key_pressed(Key::Escape)) && !list_open {
            self.close(&ctx);
        }
        if ctx.input(|i| i.key_pressed(Key::Enter)) && !list_open && !self.closing {
            self.capture(&ctx);
        }

        let p = theme::palette(ui);
        let frame = egui::Frame::NONE.fill(p.panel).stroke(egui::Stroke::new(1.0f32, p.border)).inner_margin(egui::Margin::same(MARGIN));
        let margins = frame.total_margin().sum().y;
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            // First, so the controls drawn over it take their own clicks.
            self.movable(ui);
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
        Anchor { cursor: Pos2::new(cursor.0, cursor.1), work: Rect::from_min_max(Pos2::ZERO, Pos2::new(1920.0, 1040.0)), scale: 1.0, moved: false }
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
        let top = Anchor { cursor: Pos2::new(1850.0, 10.0), work: Rect::from_min_max(Pos2::new(0.0, 40.0), Pos2::new(1920.0, 1080.0)), scale: 1.0, moved: false };
        assert_eq!(top.top_left(Vec2::new(320.0, 400.0)).y, 40.0 + GAP);
    }

    #[test]
    fn keeps_its_gap_in_pixels_on_a_scaled_screen() {
        let scaled = Anchor { cursor: Pos2::new(3000.0, 2100.0), work: Rect::from_min_max(Pos2::ZERO, Pos2::new(3840.0, 2080.0)), scale: 2.0, moved: false };
        let corner = scaled.top_left(Vec2::new(640.0, 800.0));
        assert_eq!(corner, Pos2::new(2360.0, 2080.0 - 2.0 * GAP - 800.0));
    }

    #[test]
    fn opens_where_it_was_moved_to_kept_on_the_screen() {
        let moved = |x, y| Anchor { cursor: Pos2::new(x, y), work: Rect::from_min_max(Pos2::ZERO, Pos2::new(1920.0, 1040.0)), scale: 1.0, moved: true };
        let size = Vec2::new(650.0, 240.0);
        assert_eq!(moved(300.0, 200.0).top_left(size), Pos2::new(300.0, 200.0), "its top left, where it was left");
        // Against the edge is fine; past it, it comes back on.
        assert_eq!(moved(0.0, 0.0).top_left(size), Pos2::ZERO);
        assert_eq!(moved(1800.0, 1000.0).top_left(size), Pos2::new(1920.0 - 650.0, 1040.0 - 240.0));
    }

    #[test]
    fn reads_where_it_was_moved_to() {
        assert_eq!(parse_place("120,-40"), Some((120, -40)), "a screen left of or above the main one");
        assert_eq!(parse_place(" 5 , 6 "), Some((5, 6)));
        assert_eq!(parse_place("nowhere"), None);
        assert_eq!(parse_place(""), None);
    }
}
