//! The camera bubble: the camera's picture in a round window on top of everything, which a
//! recording takes in because it's on the screen (nothing is added to the video itself, so the
//! encoders don't change). `rsnip --camera [camera] [--at X,Y] [--recording]` shows it: by itself,
//! to try it, or for a recording, which starts it as a process of its own (see `Bubble`) and closes
//! it when it stops. With `--at` it sits by that point (in pixels: the recorded area's bottom right)
//! until it's dragged somewhere else, where the desktop lets a window place itself (not on
//! Wayland). Small, medium and large buttons show on hover; for a recording, which would take them
//! in, there are none, and the mouse wheel changes its size instead.
//!
//! The pictures come from Media Foundation on Windows (`mf`), and GStreamer's Video4Linux source
//! elsewhere (`gst`).

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Align2, Color32, ColorImage, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions, Ui, Vec2, ViewportCommand};

use super::env::Env;
use super::platform::SoundDevice;
use super::theme::{self, icon, ACCENT};
use super::APP_ID;
use crate::options;

#[cfg(not(windows))]
pub mod gst;
#[cfg(windows)]
mod mf;

pub const TITLE: &str = "RSnip Camera";
/// The pictures are this many pixels square: enough for the largest bubble at 175%.
pub const SIDE: usize = 480;
/// How far in from the recorded area's corner it sits, in points.
pub const MARGIN: f32 = 16.0;
/// Around the picture, for its ring.
const EDGE: f32 = 3.0;
/// The size buttons, and the close button.
const BUTTON: f32 = 22.0;
/// How long it stays put after being dragged before where it is is remembered, in seconds.
const SETTLED: f64 = 0.5;

/// How big the bubble is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Size {
    Small,
    Medium,
    Large,
}

impl Size {
    const ALL: [Size; 3] = [Size::Small, Size::Medium, Size::Large];

    fn label(self) -> &'static str {
        match self {
            Size::Small => "Small",
            Size::Medium => "Medium",
            Size::Large => "Large",
        }
    }

    /// Across, in points.
    pub fn diameter(self) -> f32 {
        match self {
            Size::Small => 120.0,
            Size::Medium => 180.0,
            Size::Large => 260.0,
        }
    }

    /// The one chosen last.
    fn read(env: &Env) -> Size {
        Size::ALL[options::RECORD_CAMERA_SIZE.read(|name| env.setting(name)) as usize]
    }
}

/// The window for a bubble of `size`: the picture and its ring.
pub fn window_size(size: Size) -> Vec2 {
    Vec2::splat(size.diameter() + 2.0 * EDGE)
}

/// What `rsnip --camera` was asked to show, and how.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Launch {
    /// The camera, by the id Settings keeps (see `cameras`); the first one if `None`.
    pub device: Option<String>,
    /// Where its bottom right goes (less a margin), in pixels.
    pub corner: Option<(i32, i32)>,
    /// What's being recorded (left, top, right and bottom, in pixels): where it was last dragged
    /// to goes instead of `corner` when that's in here.
    pub area: Option<[i32; 4]>,
    /// It's for a recording: if the camera can't be used it says why (to the recorder, see
    /// `Bubble`) and closes, rather than showing why in the bubble.
    pub recording: bool,
}

impl Launch {
    /// What's being recorded, at the bottom right of which it goes unless it was last dragged
    /// somewhere in it.
    pub fn in_area(device: Option<String>, [left, top, right, bottom]: [i32; 4]) -> Launch {
        Launch { device, corner: Some((right, bottom)), area: Some([left, top, right, bottom]), recording: true }
    }

    /// From what follows `--camera`, in any order: the camera (anything that isn't a flag),
    /// `--at X,Y`, `--area LEFT,TOP,RIGHT,BOTTOM` and `--recording`. Flags it doesn't know are
    /// ignored.
    pub fn parse(args: &[String]) -> Launch {
        let mut launch = Launch::default();
        let mut words = args.iter();
        while let Some(word) = words.next() {
            match word.as_str() {
                "--at" => launch.corner = words.next().and_then(|at| point(at)),
                "--area" => {
                    launch.area = words.next().and_then(|area| {
                        let edges: Vec<i32> = area.split(',').map(|n| n.trim().parse().ok()).collect::<Option<_>>()?;
                        edges.try_into().ok()
                    })
                }
                "--recording" => launch.recording = true,
                flag if flag.starts_with("--") => {}
                device if launch.device.is_none() && !device.trim().is_empty() => launch.device = Some(device.into()),
                _ => {}
            }
        }
        launch
    }

    /// The arguments that `parse` reads back as this.
    pub fn args(&self) -> Vec<String> {
        let mut args: Vec<String> = self.device.iter().filter(|d| !d.is_empty()).cloned().collect();
        if let Some((x, y)) = self.corner {
            args.extend(["--at".into(), format!("{x},{y}")]);
        }
        if let Some(area) = self.area {
            args.extend(["--area".into(), area.map(|n| n.to_string()).join(",")]);
        }
        if self.recording {
            args.push("--recording".into());
        }
        args
    }

    /// Where it goes: where it was last dragged to (`remembered`, see `options::RECORD_CAMERA_PLACE`)
    /// if that's in what's being recorded, or else by `corner`.
    fn place(&self, remembered: Option<&str>) -> Option<(i32, i32)> {
        let inside = |(x, y): (i32, i32)| self.area.is_some_and(|[left, top, right, bottom]| (left..=right).contains(&x) && (top..=bottom).contains(&y));
        remembered.and_then(point).filter(|&at| inside(at)).or(self.corner)
    }
}

/// `X,Y`.
fn point(text: &str) -> Option<(i32, i32)> {
    let (x, y) = text.split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

/// What a camera sends.
pub enum Event {
    Picture(ColorImage),
    /// Something to tell the user (that the chosen camera wasn't there, so another is shown).
    Note(String),
    /// Why there are no pictures (any more), in a few words.
    Failed(String),
}

/// Where a camera's pictures go (see `open`).
pub struct Sink {
    tx: SyncSender<Event>,
    ctx: Option<egui::Context>,
}

impl Sink {
    /// Passes a picture on, or drops it if the one before hasn't been taken yet. False once
    /// nothing is looking.
    pub fn picture(&self, image: ColorImage) -> bool {
        let looking = !matches!(self.tx.try_send(Event::Picture(image)), Err(TrySendError::Disconnected(_)));
        self.repaint();
        looking
    }

    pub fn note(&self, note: &str) {
        let _ = self.tx.send(Event::Note(note.into()));
        self.repaint();
    }

    fn fail(&self, why: String) {
        let _ = self.tx.send(Event::Failed(why));
        self.repaint();
    }

    fn repaint(&self) {
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }
}

/// Starts the camera `device` (the first one if `None`) on a thread of its own. Its pictures, and
/// what goes wrong, come on the channel, and `ctx` repaints for each; it stops once the channel is
/// dropped.
pub fn open(device: Option<String>, ctx: Option<egui::Context>) -> Receiver<Event> {
    let (tx, rx) = sync_channel(2);
    std::thread::spawn(move || {
        let sink = Sink { tx, ctx };
        if let Err(why) = capture(device.as_deref(), &sink) {
            sink.fail(why);
        }
    });
    rx
}

#[cfg(windows)]
fn capture(device: Option<&str>, sink: &Sink) -> Result<(), String> {
    mf::capture(device, sink)
}

#[cfg(not(windows))]
fn capture(device: Option<&str>, sink: &Sink) -> Result<(), String> {
    gst::capture(device, sink)
}

/// The cameras there are, to choose from in Settings.
pub fn cameras() -> Vec<SoundDevice> {
    #[cfg(windows)]
    return mf::cameras();
    #[cfg(not(windows))]
    return gst::cameras();
}

/// A picture to stand in for a camera's (in tests, and the sandbox): coloured bars, with a lighter
/// band across the middle.
pub fn test_pattern(side: usize) -> ColorImage {
    let bars: [[u8; 3]; 5] = [[200, 60, 60], [230, 170, 40], [70, 170, 90], [60, 120, 210], [150, 80, 190]];
    let mut rgba = Vec::with_capacity(side * side * 4);
    for y in 0..side {
        for x in 0..side {
            let [r, g, b] = bars[x * bars.len() / side];
            let light = (side / 3..side * 2 / 3).contains(&y) as u8 * 40;
            rgba.extend([r.saturating_add(light), g.saturating_add(light), b.saturating_add(light), 255]);
        }
    }
    ColorImage::from_rgba_unmultiplied([side, side], &rgba)
}

/// `rsnip --camera …`: shows the bubble until it's closed (or, for a recording, the recording
/// stops). On standard output it says `shown` once the first picture is, `note: …` for anything
/// to tell the user, and `failed: …` if the camera can't be used (see `Bubble`).
pub fn run(args: &[String]) {
    let mut launch = Launch::parse(args);
    let env = Env::real();
    // Without one named, the one chosen in Settings.
    launch.device = launch.device.or_else(|| env.text_setting(options::RECORD_CAMERA_DEVICE).filter(|d| !d.is_empty()));
    launch.corner = launch.place(env.text_setting(options::RECORD_CAMERA_PLACE).as_deref());
    let window = window_size(Size::read(&env));
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(TITLE)
        .with_inner_size(window)
        .with_decorations(false)
        .with_resizable(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_taskbar(false)
        // What's being recorded keeps the focus.
        .with_active(false)
        .with_icon(super::window_icon())
        .with_app_id(APP_ID);
    if let Some((x, y)) = launch.corner {
        // Near enough until the window knows its own scale (see `CameraBubble::fit`).
        viewport = viewport.with_position(Pos2::new(x as f32, y as f32) - window - Vec2::splat(MARGIN));
    }
    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions { viewport, ..Default::default() };
    // The desktop tests run it in a test's thread (see `tests::bubble_child`).
    #[cfg(all(test, target_os = "linux"))]
    {
        options.event_loop_builder = Some(Box::new(|builder| {
            use winit::platform::x11::EventLoopBuilderExtX11;
            builder.with_any_thread(true);
        }));
    }
    let shown = eframe::run_native(
        TITLE,
        options,
        Box::new(move |cc| {
            // Its window is made, but not shown yet: out of the dock and Alt+Tab, and on top.
            #[cfg(not(windows))]
            if let Some(window) = super::capture_panel::x11_window(cc) {
                let _ = super::capture_panel::keep_off_the_taskbar(window);
                let _ = never_focused(window);
            }
            #[cfg(windows)]
            without_a_frame(cc);
            let events = env.open_camera(launch.device.clone(), &cc.egui_ctx);
            Ok(Box::new(CameraBubble::new(&cc.egui_ctx, env, events, launch, true)))
        }),
    );
    if let Err(e) = shown {
        say(&format!("failed: its window couldn't open ({e})"));
    }
}

/// Asks X11's window manager never to give the bubble the focus, so clicking or dragging it leaves
/// it where it was: with the capture panel, which closes on losing it, or what's being recorded.
#[cfg(not(windows))]
fn never_focused(window: u32) -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::properties::WmHints;
    use x11rb::wrapper::ConnectionExt as _;
    let (conn, _) = x11rb::connect(None)?;
    let mut hints = WmHints::get(&conn, window)?.reply()?.unwrap_or_default();
    hints.input = Some(false);
    hints.set(&conn, window)?;
    conn.sync()?;
    Ok(())
}

/// Takes away what Windows draws around a window without a title bar: winit leaves it the styles
/// of one with a frame and keeps a pixel of frame along the top, for which Windows draws a shadow
/// under the whole window (seen through around the circle, and recorded), a border and round corners.
/// And it never takes the focus, so clicking or dragging it leaves it where it was: with the capture
/// panel, which closes on losing it, or what's being recorded.
#[cfg(windows)]
fn without_a_frame(window: &impl raw_window_handle::HasWindowHandle) {
    use raw_window_handle::RawWindowHandle;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND};
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, MA_NOACTIVATE, STYLESTRUCT, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WM_MOUSEACTIVATE, WM_NCCALCSIZE,
        WM_STYLECHANGING, WS_EX_NOACTIVATE,
    };

    /// All of the window is the picture's, with no frame at all, and clicks don't activate it.
    unsafe extern "system" fn all_client(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
        match message {
            WM_NCCALCSIZE if wparam != 0 => return 0,
            WM_MOUSEACTIVATE => return MA_NOACTIVATE as LRESULT,
            // winit sets the styles it wants when it changes one (showing the window, say).
            WM_STYLECHANGING if wparam as i32 == GWL_EXSTYLE => unsafe { (*(lparam as *mut STYLESTRUCT)).styleNew |= WS_EX_NOACTIVATE },
            _ => {}
        }
        unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
    }

    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else { return };
    let hwnd = handle.hwnd.get() as HWND;
    let corners = DWMWCP_DONOTROUND;
    let border = DWMWA_COLOR_NONE;
    // SAFETY: the window is this thread's and stays for the process; the attributes are the
    // 4-byte values Windows reads for them.
    unsafe {
        SetWindowSubclass(hwnd, Some(all_client), 1, 0);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, GetWindowLongPtrW(hwnd, GWL_EXSTYLE) | WS_EX_NOACTIVATE as isize);
        DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE as u32, (&raw const corners).cast(), 4);
        DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR as u32, (&raw const border).cast(), 4);
        // Has the frame worked out again, without it.
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// Writes a line on standard output, where the recorder reads it.
fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// Where the bubble keeps itself.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Place {
    /// With its bottom right by this point (less `MARGIN`), in pixels.
    Corner(Pos2),
    /// With its bottom right here, in points: it grows and shrinks about that corner.
    BottomRight(Pos2),
    /// Where it's been dragged to, or the desktop put it.
    Free,
}

pub struct CameraBubble {
    pub env: Env,
    events: Receiver<Event>,
    picture: Option<TextureHandle>,
    /// Why there's no picture, once known.
    pub failed: Option<String>,
    launch: Launch,
    pub size: Size,
    place: Place,
    /// The size and top left last asked of the window, in points.
    asked: Option<(Vec2, Option<Pos2>)>,
    /// What it has said for the recorder, in order.
    pub said: Vec<String>,
    /// It says it on standard output too.
    speak: bool,
    /// How far the mouse wheel has turned over it, in notches, towards the next size.
    wheel: f32,
    /// It's been dragged, so where it is now is remembered (see `remember`).
    dragged: bool,
    /// Where it was last seen (as `options::RECORD_CAMERA_PLACE` has it), and since when (egui's
    /// time, in seconds).
    seen: Option<((i32, i32), f64)>,
    /// What was remembered last.
    remembered: Option<(i32, i32)>,
}

impl CameraBubble {
    pub fn new(ctx: &egui::Context, env: Env, events: Receiver<Event>, launch: Launch, speak: bool) -> CameraBubble {
        let mut fonts = egui::FontDefinitions::default();
        theme::add_icons(&mut fonts);
        ctx.set_fonts(fonts);
        let place = launch.corner.map_or(Place::Free, |(x, y)| Place::Corner(Pos2::new(x as f32, y as f32)));
        CameraBubble { size: Size::read(&env), env, events, picture: None, failed: None, launch, place, asked: None, said: Vec::new(), speak, wheel: 0.0, dragged: false, seen: None, remembered: None }
    }

    fn say(&mut self, line: String) {
        if self.speak {
            say(&line);
        }
        self.said.push(line);
    }

    /// Takes what the camera has sent: the newest picture, and anything it says.
    fn take_events(&mut self, ctx: &egui::Context) {
        loop {
            match self.events.try_recv() {
                Ok(Event::Picture(image)) => match &mut self.picture {
                    Some(picture) => picture.set(image, TextureOptions::LINEAR),
                    None => {
                        self.picture = Some(ctx.load_texture("camera", image, TextureOptions::LINEAR));
                        self.say("shown".into());
                    }
                },
                Ok(Event::Note(note)) => self.say(format!("note: {note}")),
                Ok(Event::Failed(why)) => {
                    self.say(format!("failed: {why}"));
                    self.failed = Some(why);
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
    }

    /// Changes its size, about its bottom right corner, and remembers it.
    fn resize(&mut self, ctx: &egui::Context, size: Size) {
        if size == self.size {
            return;
        }
        if self.place == Place::Free {
            if let Some(outer) = ctx.input(|i| i.viewport().outer_rect) {
                self.place = Place::BottomRight(outer.max);
            }
        }
        self.size = size;
        self.env.set_setting(options::RECORD_CAMERA_SIZE.name, Size::ALL.iter().position(|s| *s == size).unwrap_or(1) as u32);
    }

    /// Remembers where it is, with its bottom right at `bottom_right` (in points) at egui's time
    /// `now`, once it's stayed there a moment (a drag moves it in many steps), for the next bubble.
    fn remember(&mut self, bottom_right: Pos2, pixels_per_point: f32, now: f64) {
        let corner = (bottom_right + Vec2::splat(MARGIN)) * pixels_per_point;
        let corner = (corner.x.round() as i32, corner.y.round() as i32);
        match self.seen {
            Some((seen, since)) if seen == corner => {
                if now - since >= SETTLED && self.remembered != Some(corner) {
                    self.env.set_text_setting(options::RECORD_CAMERA_PLACE, &format!("{},{}", corner.0, corner.1));
                    self.remembered = Some(corner);
                }
            }
            _ => self.seen = Some((corner, now)),
        }
    }

    /// A size bigger for each notch the mouse wheel turns up, and smaller for each down. A touchpad's
    /// scrolling counts a notch for each 50 points.
    fn turn_wheel(&mut self, ctx: &egui::Context) {
        self.wheel += ctx.input(|i| {
            i.events
                .iter()
                .map(|event| match event {
                    egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta, .. } => delta.y,
                    egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta, .. } => delta.y / 50.0,
                    egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Page, delta, .. } => delta.y * 3.0,
                    _ => 0.0,
                })
                .sum::<f32>()
        });
        if self.wheel.abs() >= 1.0 {
            let now = Size::ALL.iter().position(|s| *s == self.size).unwrap_or(1);
            let next = if self.wheel > 0.0 { (now + 1).min(Size::ALL.len() - 1) } else { now.saturating_sub(1) };
            self.wheel = 0.0;
            self.resize(ctx, Size::ALL[next]);
        }
    }

    /// Asks the window to be the bubble's size, and where it should be, when that changes.
    fn fit(&mut self, ctx: &egui::Context) {
        let window = window_size(self.size);
        let top_left = match self.place {
            Place::Corner(corner) => Some(corner / ctx.pixels_per_point() - window - Vec2::splat(MARGIN)),
            Place::BottomRight(corner) => Some(corner - window),
            Place::Free => None,
        };
        let moved = |(was_size, was_corner): (Vec2, Option<Pos2>)| (was_size - window).abs().max_elem() > 0.5 || was_corner.zip(top_left).is_some_and(|(a, b)| (a - b).abs().max_elem() > 0.5) || was_corner.is_none() != top_left.is_none();
        if self.asked.is_none_or(moved) {
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(window));
            if let Some(top_left) = top_left {
                ctx.send_viewport_cmd(ViewportCommand::OuterPosition(top_left));
            }
            self.asked = Some((window, top_left));
        }
    }

    /// The picture in its circle (mirrored, as people expect to see themselves), or what it's
    /// waiting for, or why there's none.
    fn paint(&self, ui: &Ui, circle: Rect) {
        let painter = ui.painter();
        let (center, r) = (circle.center(), circle.width() / 2.0);
        match (&self.failed, &self.picture) {
            (None, Some(picture)) => {
                let mirrored = Rect::from_min_max(Pos2::new(1.0, 0.0), Pos2::new(0.0, 1.0));
                painter.add(egui::epaint::RectShape::filled(circle, CornerRadius::same(r.round() as u8), Color32::WHITE).with_texture(picture.id(), mirrored));
            }
            _ => {
                painter.circle_filled(center, r, Color32::from_gray(32));
                let (glyph, text) = match &self.failed {
                    Some(why) => (icon::WEBCAM_SLASH, format!("Couldn't use the camera: {why}")),
                    None => (icon::WEBCAM, "Starting the camera…".to_string()),
                };
                let light = Color32::from_gray(215);
                painter.text(center - Vec2::new(0.0, r * 0.3), Align2::CENTER_CENTER, glyph, FontId::proportional((r * 0.36).clamp(18.0, 40.0)), light);
                let mut job = egui::text::LayoutJob::simple(text, FontId::proportional(if r < 80.0 { 10.5 } else { 12.5 }), light, r * 1.45);
                job.halign = Align::Center;
                painter.galley(Pos2::new(center.x, center.y - r * 0.05), painter.layout_job(job), light);
            }
        }
        painter.circle_stroke(center, r, Stroke::new(EDGE, Color32::from_white_alpha(210)));
    }

    /// The size buttons along the bottom, and the close button at the top.
    fn controls(&mut self, ui: &mut Ui, circle: Rect) {
        let ctx = ui.ctx().clone();
        let (center, r) = (circle.center(), circle.width() / 2.0);
        let gap = 4.0;
        let row = 3.0 * BUTTON + 2.0 * gap;
        for (i, size) in Size::ALL.into_iter().enumerate() {
            let x = center.x - row / 2.0 + BUTTON / 2.0 + i as f32 * (BUTTON + gap);
            let at = Rect::from_center_size(Pos2::new(x, center.y + r * 0.62), Vec2::splat(BUTTON));
            // As big as the bubble it makes.
            let dot = 3.0 + 2.5 * i as f32;
            if round_button(ui, at, size.label(), size == self.size, |painter, color| {
                painter.circle_filled(at.center(), dot, color);
            }).clicked() {
                self.resize(&ctx, size);
            }
        }
        let at = Rect::from_center_size(Pos2::new(center.x, center.y - r * 0.62), Vec2::splat(BUTTON));
        let close = |painter: &egui::Painter, color| {
            painter.text(at.center(), Align2::CENTER_CENTER, icon::X, FontId::proportional(13.0), color);
        };
        if round_button(ui, at, "Close", false, close).clicked() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }
}

/// A small round button on the picture, named `name`, with `glyph` drawn in it.
fn round_button(ui: &mut Ui, at: Rect, name: &str, selected: bool, glyph: impl FnOnce(&egui::Painter, Color32)) -> egui::Response {
    let response = ui.interact(at, ui.id().with(name), Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, name));
    let fill = if selected {
        ACCENT
    } else if response.hovered() {
        Color32::from_black_alpha(220)
    } else {
        Color32::from_black_alpha(150)
    };
    ui.painter().circle_filled(at.center(), BUTTON / 2.0, fill);
    glyph(ui.painter(), Color32::WHITE);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

impl eframe::App for CameraBubble {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // Round: around the circle is see-through.
        [0.0; 4]
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.take_events(&ctx);
        // For a recording it says why and goes, rather than being recorded saying so.
        if self.failed.is_some() && self.launch.recording {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }
        let window = ui.max_rect();
        let circle = Rect::from_center_size(window.center(), Vec2::splat(self.size.diameter()));
        // First, so the buttons drawn over it take their own clicks.
        let behind = ui.interact(circle, ui.id().with("bubble"), Sense::click_and_drag());
        if behind.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
            // It stays where it's put, which is remembered.
            self.place = Place::Free;
            self.dragged = true;
        }
        // For the capture panel, which mustn't close when the focus goes to the bubble (see
        // `Bubble::touched`).
        if ctx.input(|i| i.pointer.any_pressed() || i.pointer.any_released()) {
            self.say("touched".into());
        }
        self.paint(ui, circle);
        if self.launch.recording {
            if behind.hovered() {
                self.turn_wheel(&ctx);
            }
        } else if ui.rect_contains_pointer(window) {
            self.controls(ui, circle);
        }
        self.fit(&ctx);
        if self.dragged {
            let (outer, now) = ctx.input(|i| (i.viewport().outer_rect, i.time));
            if let Some(outer) = outer {
                self.remember(outer.max, ctx.pixels_per_point(), now);
                if self.remembered != self.seen.map(|(at, _)| at) {
                    ctx.request_repaint_after(Duration::from_secs_f64(SETTLED));
                }
            }
        }
    }
}

/// The camera bubble shown for a recording: its own process, and what it says (see `run`).
pub struct Bubble {
    /// Or why it couldn't be started, to tell the user.
    process: Result<(Process, JoinHandle<()>), String>,
    said: Arc<Mutex<Said>>,
}

/// The bubble's process, closed when dropped, so it doesn't stay on the screen after a recording
/// that went without stopping it.
struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// What the bubble has said about the camera.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
struct Said {
    shown: bool,
    /// When it was last clicked or let go of.
    touched: Option<Instant>,
    notes: Vec<String>,
    failed: Option<String>,
}

impl Said {
    fn hear(&mut self, line: &str) {
        if line == "shown" {
            self.shown = true;
        } else if line == "touched" {
            self.touched = Some(Instant::now());
        } else if let Some(note) = line.strip_prefix("note: ") {
            self.notes.push(note.into());
        } else if let Some(why) = line.strip_prefix("failed: ") {
            self.failed.get_or_insert_with(|| why.into());
        }
    }

    /// What to tell the user once the recording is saved, if anything: why the camera isn't in it
    /// (or in all of it). `crashed` if the bubble ended by itself without saying why.
    fn note(&self, crashed: bool) -> Option<String> {
        let mut notes = self.notes.clone();
        match (&self.failed, self.shown) {
            (Some(why), false) => notes.push(format!("Couldn't use the camera ({why}), so it was recorded without it.")),
            (Some(why), true) => notes.push(format!("The camera stopped ({why}), so the rest was recorded without it.")),
            (None, _) if crashed => notes.push("The camera closed unexpectedly, so it isn't in all of the recording.".into()),
            (None, _) => {}
        }
        (!notes.is_empty()).then(|| notes.join(" "))
    }
}

impl Bubble {
    /// Shows the bubble as `launch` says.
    pub fn start(launch: &Launch) -> Bubble {
        Bubble::start_with(launch, None)
    }

    /// Shows it as `launch` says, repainting `ctx` each time it says something (see `touched`).
    pub fn start_with(launch: &Launch, ctx: Option<egui::Context>) -> Bubble {
        let said = Arc::new(Mutex::new(Said::default()));
        let mut command = command(launch);
        // And if the recorder goes without closing it (killed, say), it goes too.
        #[cfg(not(windows))]
        unsafe {
            std::os::unix::process::CommandExt::pre_exec(&mut command, || {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                Ok(())
            });
        }
        let process = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
        let process = process.map_err(|e| format!("Couldn't show the camera ({e}), so it was recorded without it.")).map(|mut child| {
            let out = child.stdout.take().expect("piped");
            let hearing = said.clone();
            let reading = std::thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    hearing.lock().unwrap().hear(line.trim());
                    if let Some(ctx) = &ctx {
                        ctx.request_repaint();
                    }
                }
            });
            (Process(child), reading)
        });
        Bubble { process, said }
    }

    /// No bubble, for `why` (to tell the user): Linux records a window by itself.
    #[cfg_attr(windows, allow(dead_code))]
    pub fn left_out(why: String) -> Bubble {
        Bubble { process: Err(why), said: Default::default() }
    }

    /// Closes it, and says what to tell the user about the camera, if anything.
    pub fn stop(self) -> Option<String> {
        let (process, reading) = match self.process {
            Ok(process) => process,
            Err(why) => return Some(why),
        };
        let mut process = process;
        let ended = process.0.try_wait().ok().flatten();
        drop(process);
        let _ = reading.join();
        let said = self.said.lock().unwrap().clone();
        said.note(ended.is_some_and(|status| !status.success()))
    }

    /// Its first picture is showing.
    #[cfg(test)]
    pub fn shown(&self) -> bool {
        self.said.lock().unwrap().shown
    }

    #[cfg(test)]
    pub fn id(&self) -> Option<u32> {
        self.process.as_ref().ok().map(|(process, _)| process.0.id())
    }

    /// Its process has ended by itself.
    #[cfg(test)]
    pub fn ended(&mut self) -> bool {
        self.process.as_mut().is_ok_and(|(process, _)| process.0.try_wait().ok().flatten().is_some())
    }

    /// When it was last clicked or let go of: the focus may have gone to it.
    pub fn touched(&self) -> Option<Instant> {
        self.said.lock().unwrap().touched
    }

    /// One that isn't shown, for the sandbox.
    pub fn stand_in() -> Bubble {
        Bubble { process: Err(String::new()), said: Default::default() }
    }

    /// As if it had been clicked.
    #[cfg(test)]
    pub fn touch(&self) {
        self.said.lock().unwrap().hear("touched");
    }
}

/// RSnip, to show the bubble as `launch` says.
#[cfg(not(test))]
fn command(launch: &Launch) -> Command {
    #[cfg(windows)]
    let program = std::env::current_exe().unwrap_or_else(|_| "rsnip.exe".into());
    #[cfg(not(windows))]
    let program = super::platform::program();
    let mut command = Command::new(program);
    command.arg("--camera").args(launch.args());
    command
}

/// The arguments for `tests::bubble_child`, which stands in for RSnip in tests.
#[cfg(test)]
const BUBBLE_ARGS: &str = "RSNIP_BUBBLE_ARGS";

#[cfg(test)]
fn command(launch: &Launch) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", "gallery::camera::tests::bubble_child", "--nocapture", "--test-threads=1"]).env(BUBBLE_ARGS, launch.args().join("\n"));
    command
}

#[cfg(test)]
pub(super) mod tests {


    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;

    use super::*;
    use crate::gallery::tests as gallery;

    /// What `bubble_child` says, separated by `|`, instead of showing the bubble: it then stays
    /// until it's stopped, unless the last thing it said was `failed: …`, or it's `crash`.
    pub const FAKE: &str = "RSNIP_BUBBLE_FAKE";

    /// Run by `Bubble::start` in tests, in a child process: the bubble, or what `FAKE` says.
    #[test]
    fn bubble_child() {
        let Ok(args) = std::env::var(BUBBLE_ARGS) else { return };
        let args: Vec<String> = args.split('\n').map(String::from).collect();
        // The test's own name is on the line so far.
        say("");
        let Ok(fake) = std::env::var(FAKE) else { return super::run(&args) };
        assert!(Launch::parse(&args).recording, "for a recording: {args:?}");
        for line in fake.split('|') {
            if line == "crash" {
                std::process::exit(3);
            }
            say(line);
        }
        if !fake.rsplit('|').next().unwrap_or_default().starts_with("failed: ") {
            std::thread::sleep(Duration::from_secs(60));
        }
    }

    #[test]
    fn reads_what_it_was_asked() {
        let args = |text: &str| text.split_whitespace().map(String::from).collect::<Vec<_>>();
        assert_eq!(Launch::parse(&[]), Launch::default(), "the first camera, where the desktop puts it");
        let full = Launch { device: Some("/dev/video2".into()), corner: Some((-1200, 840)), area: None, recording: true };
        assert_eq!(Launch::parse(&args("/dev/video2 --at -1200,840 --recording")), full);
        assert_eq!(Launch::parse(&args("--recording --at -1200,840 /dev/video2")), full, "in any order");
        assert_eq!(Launch::parse(&full.args()), full);
        assert_eq!(Launch::parse(&args("--at nowhere --junk")), Launch::default());
        // Windows' ids are long, and have no spaces, but its names do.
        let windows = Launch { device: Some(r"\\?\usb#vid_046d&pid_085e&mi_00#7&1a2b3c4d&0&0000#{e5323777-f976-4f5b-9b55-b94699c46e44}\global".into()), corner: None, area: None, recording: false };
        assert_eq!(Launch::parse(&windows.args()), windows);
        assert_eq!(Launch { device: Some(String::new()), ..Launch::default() }.args(), Vec::<String>::new(), "empty is the first one");
        let area = Launch::in_area(None, [-1920, 0, -400, 900]);
        assert_eq!(area.args(), args("--at -400,900 --area -1920,0,-400,900 --recording"));
        assert_eq!(Launch::parse(&area.args()), area);
    }

    #[test]
    fn goes_where_it_was_last_dragged_when_thats_in_what_is_recorded() {
        let area = Launch::in_area(None, [100, 50, 1300, 850]);
        assert_eq!(area.place(None), Some((1300, 850)), "by the corner to begin with");
        assert_eq!(area.place(Some("400,300")), Some((400, 300)));
        assert_eq!(area.place(Some("1300,850")), Some((1300, 850)), "the edges are in it");
        assert_eq!(area.place(Some("1500,300")), Some((1300, 850)), "not in it");
        assert_eq!(area.place(Some("nowhere")), Some((1300, 850)));
        // By itself, it goes where it's asked.
        let alone = Launch { corner: Some((900, 700)), ..Launch::default() };
        assert_eq!(alone.place(Some("400,300")), Some((900, 700)));
    }

    #[test]
    fn remembers_where_its_dragged_once_it_stays_there() {
        let mut h = bubble("bubble-remembers", |_| {});
        let remembered = |h: &Harness<CameraBubble>| h.state().env.text_setting(options::RECORD_CAMERA_PLACE);
        let bubble = h.state_mut();
        // Its bottom right at 500, 400 points, at 1.5 pixels a point: and the margin.
        bubble.remember(Pos2::new(500.0, 400.0), 1.5, 10.0);
        bubble.remember(Pos2::new(510.0, 400.0), 1.5, 10.2);
        bubble.remember(Pos2::new(510.0, 400.0), 1.5, 10.4);
        assert_eq!(remembered(&h), None, "still moving");
        h.state_mut().remember(Pos2::new(510.0, 400.0), 1.5, 10.8);
        assert_eq!(remembered(&h).as_deref(), Some("789,624"));
    }

    #[test]
    fn says_when_its_touched() {
        let mut h = bubble("bubble-touched", |_| {});
        h.drag_at(Pos2::new(60.0, 60.0));
        h.step();
        assert!(h.state().said.contains(&"touched".to_string()), "{:?}", h.state().said);
        let mut said = Said::default();
        said.hear("touched");
        assert!(said.touched.is_some());
        assert_eq!(said.note(false), None, "nothing to tell about the recording");
    }

    #[test]
    fn tells_the_recorder_what_became_of_the_camera() {
        let heard = |lines: &[&str]| {
            let mut said = Said::default();
            lines.iter().for_each(|line| said.hear(line));
            said
        };
        assert_eq!(heard(&["shown"]).note(false), None);
        assert_eq!(heard(&[]).note(false), None, "closed with its ×, or stopped before it showed");
        assert_eq!(heard(&["failed: it's in use by another app"]).note(false).unwrap(), "Couldn't use the camera (it's in use by another app), so it was recorded without it.");
        assert_eq!(heard(&["shown", "failed: it was disconnected"]).note(false).unwrap(), "The camera stopped (it was disconnected), so the rest was recorded without it.");
        let note = "The chosen camera isn't connected, so the first one was shown.";
        assert_eq!(heard(&["note: The chosen camera isn't connected, so the first one was shown.", "shown"]).note(false).as_deref(), Some(note));
        assert_eq!(heard(&["shown"]).note(true).unwrap(), "The camera closed unexpectedly, so it isn't in all of the recording.");
        assert_eq!(heard(&["random", "failed: one", "failed: two"]).failed.as_deref(), Some("one"), "the first reason");
    }

    /// Waits for `done`, for up to 20 s (the child is the whole test binary, which may be slow to
    /// start on a busy machine).
    fn wait_for(mut done: impl FnMut() -> bool, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn recording(device: Option<&str>) -> Launch {
        Launch { device: device.map(String::from), corner: Some((1200, 800)), area: None, recording: true }
    }

    /// Starts the bubble with `FAKE` saying `fake`. The variable is the same for each, so each
    /// bubble sets it just before it starts, under a lock.
    fn fake_bubble(fake: &str, launch: &Launch) -> Bubble {
        static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
        let _lock = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var(FAKE, fake);
        Bubble::start(launch)
    }

    #[test]
    fn the_recorder_starts_and_stops_the_bubble() {
        let bubble = fake_bubble("shown", &recording(Some("/dev/video2")));
        wait_for(|| bubble.shown(), "the bubble to show the camera");
        assert_eq!(bubble.stop(), None, "stopped, it closes, with nothing to say");

        // A camera that can't be used: the recording goes on, and says why.
        let mut busy = fake_bubble("failed: it's in use by another app", &recording(None));
        assert!(busy.id().is_some());
        wait_for(|| busy.ended(), "the bubble to say why and close");
        assert_eq!(busy.stop().unwrap(), "Couldn't use the camera (it's in use by another app), so it was recorded without it.");

        // One that goes by itself, without saying why, after showing another camera.
        let mut crashed = fake_bubble("note: The chosen camera isn't connected, so the first one was shown.|shown|crash", &recording(Some("gone")));
        wait_for(|| crashed.ended(), "the bubble to end");
        assert_eq!(
            crashed.stop().unwrap(),
            "The chosen camera isn't connected, so the first one was shown. The camera closed unexpectedly, so it isn't in all of the recording."
        );

        assert_eq!(Bubble::left_out("Not with a window.".into()).stop().as_deref(), Some("Not with a window."));
    }

    fn bubble(name: &str, setup: impl FnOnce(&mut Env)) -> Harness<'static, CameraBubble> {
        let mut env = Env::sandbox(gallery::sandbox_dir(name));
        setup(&mut env);
        let size = window_size(Size::read(&env));
        let launch = Launch { device: None, corner: Some((1200, 800)), area: None, recording: false };
        let mut h = Harness::builder().with_size(size).build_eframe(move |cc| {
            let events = env.open_camera(None, &cc.egui_ctx);
            CameraBubble::new(&cc.egui_ctx, env, events, launch, false)
        });
        settle(&mut h);
        h
    }

    fn settle(h: &mut Harness<CameraBubble>) {
        for _ in 0..4 {
            h.step();
        }
    }

    fn asked(h: &Harness<CameraBubble>) -> Vec<ViewportCommand> {
        h.output().viewport_output[&egui::ViewportId::ROOT].commands.clone()
    }

    #[test]
    fn shows_the_camera_with_size_buttons_on_hover() {
        let mut h = bubble("bubble-sizes", |_| {});
        assert_eq!(h.state().said, ["shown"]);
        assert_eq!(h.state().size, Size::Medium, "medium to begin with");
        assert!(h.query_by_label("Large").is_none(), "the buttons show on hover");
        h.hover_at(Pos2::new(90.0, 90.0));
        settle(&mut h);
        for name in ["Small", "Medium", "Large", "Close"] {
            assert!(h.query_by_label(name).is_some(), "{name}");
        }
        h.get_by_label("Large").click();
        h.step();
        assert_eq!(h.state().size, Size::Large);
        assert!(asked(&h).contains(&ViewportCommand::InnerSize(Vec2::splat(266.0))), "{:?}", asked(&h));
        // About its bottom right, which stays by the corner.
        let top_left = Pos2::new(1200.0, 800.0) - Vec2::splat(266.0 + MARGIN);
        assert!(asked(&h).contains(&ViewportCommand::OuterPosition(top_left)), "{:?}", asked(&h));
        assert_eq!(h.state().env.setting(options::RECORD_CAMERA_SIZE.name), Some(2), "remembered");

        // The next one is as big as the last.
        let again = bubble("bubble-sizes-again", |env| env.set_setting(options::RECORD_CAMERA_SIZE.name, 0));
        assert_eq!(again.state().size, Size::Small);
    }

    #[test]
    fn for_a_recording_the_wheel_sizes_it_with_no_buttons_to_record() {
        let env = Env::sandbox(gallery::sandbox_dir("bubble-wheel"));
        let mut h = Harness::builder().with_size(window_size(Size::Medium)).build_eframe(move |cc| {
            let events = env.open_camera(None, &cc.egui_ctx);
            CameraBubble::new(&cc.egui_ctx, env, events, recording(None), false)
        });
        settle(&mut h);
        h.hover_at(Pos2::new(90.0, 90.0));
        settle(&mut h);
        assert!(h.query_by_label("Large").is_none() && h.query_by_label("Close").is_none(), "no buttons in the recording");
        let wheel = |h: &mut Harness<CameraBubble>, unit, y: f32| {
            h.event(egui::Event::MouseWheel { unit, delta: Vec2::new(0.0, y), modifiers: egui::Modifiers::NONE, phase: egui::TouchPhase::Move });
            h.step();
        };
        wheel(&mut h, egui::MouseWheelUnit::Line, 1.0);
        assert_eq!(h.state().size, Size::Large, "a notch up");
        wheel(&mut h, egui::MouseWheelUnit::Line, 1.0);
        assert_eq!(h.state().size, Size::Large, "as big as it goes");
        // A touchpad's little moves add up.
        wheel(&mut h, egui::MouseWheelUnit::Point, -30.0);
        assert_eq!(h.state().size, Size::Large);
        wheel(&mut h, egui::MouseWheelUnit::Point, -30.0);
        assert_eq!(h.state().size, Size::Medium);
        wheel(&mut h, egui::MouseWheelUnit::Line, -3.0);
        assert_eq!(h.state().size, Size::Small);
        assert_eq!(h.state().env.setting(options::RECORD_CAMERA_SIZE.name), Some(0), "remembered");
    }

    #[test]
    fn sits_by_the_recorded_areas_corner_until_dragged() {
        let mut h = bubble("bubble-corner", |_| {});
        // 1200, 800 in pixels, at 1 pixel a point: its bottom right 16 points in.
        let window = window_size(Size::Medium);
        let top_left = Pos2::new(1200.0, 800.0) - window - Vec2::splat(MARGIN);
        let placed = |h: &Harness<CameraBubble>| h.state().asked.and_then(|(_, at)| at);
        assert_eq!(placed(&h), Some(top_left));
        h.drag_at(Pos2::new(60.0, 60.0));
        h.step();
        h.hover_at(Pos2::new(80.0, 80.0));
        h.step();
        assert!(asked(&h).contains(&ViewportCommand::StartDrag));
        h.drop_at(Pos2::new(80.0, 80.0));
        settle(&mut h);
        assert_eq!(h.state().place, Place::Free, "it stays where it's put");
    }

    #[test]
    fn a_camera_that_cant_be_used_says_why() {
        let h = bubble("bubble-failed", |env| env.sandboxed().camera_error = Some("no camera was found".into()));
        assert_eq!(h.state().said, ["failed: no camera was found"]);
        assert!(h.state().failed.is_some());
        assert!(!asked(&h).contains(&ViewportCommand::Close), "by itself, it stays to say why");

        // For a recording it goes, once it's said why.
        let env = Env::sandbox(gallery::sandbox_dir("bubble-failed-recording"));
        env.sandboxed().camera_error = Some("it's in use by another app".into());
        let mut h = Harness::builder().with_size(window_size(Size::Medium)).build_eframe(move |cc| {
            let events = env.open_camera(None, &cc.egui_ctx);
            CameraBubble::new(&cc.egui_ctx, env, events, recording(None), false)
        });
        h.step();
        h.step();
        assert_eq!(h.state().said, ["failed: it's in use by another app"]);
        assert!(asked(&h).contains(&ViewportCommand::Close));
    }

    #[test]
    fn the_test_pattern_is_square() {
        let pattern = test_pattern(60);
        assert_eq!(pattern.size, [60, 60]);
        assert_ne!(pattern.pixels[0], pattern.pixels[59], "bars");
    }

    #[cfg(target_os = "linux")]
    mod snapshots {
        use super::*;

        #[test]
        fn the_bubble() {
            let _gpu = gallery::gpu();
            let env = Env::sandbox(gallery::sandbox_dir("snap-bubble"));
            env.set_setting(options::RECORD_CAMERA_SIZE.name, 1);
            let mut h = Harness::builder().with_size(window_size(Size::Medium)).with_pixels_per_point(1.0).wgpu().build_eframe(move |cc| {
                let events = env.open_camera(None, &cc.egui_ctx);
                CameraBubble::new(&cc.egui_ctx, env, events, Launch::default(), false)
            });
            settle(&mut h);
            h.hover_at(Pos2::new(93.0, 60.0));
            settle(&mut h);
            h.snapshot("camera_bubble");
        }
    }
}
