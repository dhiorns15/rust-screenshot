//! The Settings window (`rsnip --settings`): every option in one place, in sections down the left.
//!
//! It reads and writes the same settings as the tray app and tells the running tray app to apply
//! them. Shortcuts are recorded by the tray app (its keyboard hook sees keys like Print Screen
//! before anything else). Settings are re-read every half second, so changes made elsewhere show up.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, Align, Color32, Layout, RichText, Ui, Vec2};

use super::env::{Env, MoveReport, PickedFolder, RecordingSupport, TrayRequest};
use super::system_info::{self, Gpu};
use super::settings_rows;
use super::theme::{self, icon, row, Appearance};
use super::{platform, Dock};
use crate::crash::{self, Crash};
use crate::options::{self, Flag};
use crate::shortcuts::{self, Action};
use crate::update::Release;

mod descriptions;
mod push;

pub const TITLE: &str = "RSnip Settings";
const REPO: &str = "https://github.com/dhiorns15/rust-screenshot";
/// How long an address GitHub takes, with room to spare (it refuses one of about 8 KB).
const MAX_URL: usize = 7500;

/// GitHub's bug form (`.github/ISSUE_TEMPLATE/bug_report.yml`) with its System info field, and its
/// crash report field if there's one, filled in: the crash report is left out if both don't fit,
/// then System info.
pub(super) fn issue_url(system_info: &str, crash: Option<&str>) -> String {
    let form = format!("{REPO}/issues/new?template=bug_report.yml");
    let with_info = format!("{form}&system-info={}", crate::push::encode(system_info));
    let with_crash = crash.map(|crash| format!("{with_info}&crash={}", crate::push::encode(crash)));
    [with_crash, Some(with_info)].into_iter().flatten().find(|url| url.len() <= MAX_URL).unwrap_or(form)
}

pub fn run() {
    let request = asked(std::env::args());
    if !platform::claim_single_instance("Settings", TITLE) {
        // The open window picks this up, goes to that section and comes to the front.
        let _ = std::fs::write(platform::request_path("Settings"), request);
        return;
    }
    let _ = std::fs::remove_file(platform::request_path("Settings"));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_inner_size([800.0, 580.0])
            .with_min_inner_size([660.0, 460.0])
            .with_icon(super::window_icon())
            .with_app_id(super::APP_ID),
        ..Default::default()
    };
    let _ = eframe::run_native(
        TITLE,
        options,
        Box::new(move |cc| {
            let mut app = SettingsApp::new(&cc.egui_ctx, Env::real());
            app.go_to(&cc.egui_ctx, request);
            Ok(Box::new(app))
        }),
    );
}

/// The section a launch asks for: `--system-info` to report a crash, `--recording` from the
/// capture panel, `--updates` from the library's "is available" link (to install it), or none ("").
pub(super) fn asked(args: impl IntoIterator<Item = String>) -> &'static str {
    let args: Vec<String> = args.into_iter().collect();
    ["--system-info", "--recording", "--updates"].into_iter().find(|flag| args.iter().any(|a| a == flag)).unwrap_or("")
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    General,
    Capture,
    Shortcuts,
    Recording,
    Library,
    Destinations,
    Updates,
    SystemInfo,
    About,
}

impl Section {
    const ALL: [Section; 9] = [
        Section::General,
        Section::Capture,
        Section::Shortcuts,
        Section::Recording,
        Section::Library,
        Section::Destinations,
        Section::Updates,
        Section::SystemInfo,
        Section::About,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Capture => "Capture",
            Section::Shortcuts => "Shortcuts",
            Section::Recording => "Recording",
            Section::Library => "Library & editor",
            Section::Destinations => "Push to",
            Section::Updates => "Updates",
            Section::SystemInfo => "System info",
            Section::About => "About",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Section::General => icon::GEAR_SIX,
            Section::Capture => icon::CAMERA,
            Section::Shortcuts => icon::KEYBOARD,
            Section::Recording => icon::VIDEO_CAMERA,
            Section::Library => icon::IMAGES,
            Section::Destinations => icon::CLOUD_ARROW_UP,
            Section::Updates => icon::ARROWS_CLOCKWISE,
            Section::SystemInfo => icon::DESKTOP_TOWER,
            Section::About => icon::INFO,
        }
    }

    /// Only for the tray app (Windows).
    fn needs_tray_app(self) -> bool {
        matches!(self, Section::Shortcuts)
    }
}

enum UpdateCheck {
    Idle,
    Checking(Receiver<Result<Option<Release>, String>>),
    Done(Result<Option<Release>, String>),
}

pub struct SettingsApp {
    pub env: Env,
    pub section: Section,
    update: UpdateCheck,
    /// A note under the section title, e.g. while a shortcut is being recorded.
    note: Option<String>,
    /// The window's graphics driver, read on the first frame.
    gpu: Option<Gpu>,
    /// The System info report, taken when that section opens.
    report: Option<Vec<system_info::Section>>,
    /// The latest crash report, read with the System info report.
    crash: Option<Crash>,
    /// Looking for whether there's an H.265 encoder and what's missing for recording, in the
    /// background, when the Recording section first opens.
    checking_recording: Option<Receiver<RecordingSupport>>,
    /// What it found.
    recording_support: Option<RecordingSupport>,
    /// The outputs and microphones there are, looked for when the Recording section opens.
    sound_devices: settings_rows::SoundDevices,
    /// The folder chooser, while it's open.
    picking: Option<PickedFolder>,
    /// The previous save folder and how many screenshots and recordings are still there.
    left_behind: Option<(PathBuf, usize)>,
    /// When that was counted (`None`: count again).
    counted: Option<Instant>,
    /// Moving them to the current folder, while it's under way.
    moving: Option<Receiver<Result<MoveReport, String>>>,
    /// How many are old enough to clean up, for the days it was counted for and when.
    old: Option<(Instant, u32, usize)>,
    installing: Installing,
    /// The background icon is to come back once its old program has gone (Linux: installing the
    /// package stops it, see installer/linux/package/debian/prerm).
    restart_tray: bool,
    push: push::PushState,
    describe: descriptions::DescribeState,
}

/// Installing an update from Settings → Updates.
enum Installing {
    Idle,
    Started(Receiver<Result<(), String>>),
    Done(Result<(), String>),
}

impl SettingsApp {
    pub fn new(ctx: &egui::Context, env: Env) -> SettingsApp {
        theme::apply(ctx, Appearance::from_setting(env.setting(Appearance::SETTING)));
        let mut fonts = egui::FontDefinitions::default();
        theme::add_icons(&mut fonts);
        ctx.set_fonts(fonts);
        let section = Section::General;
        SettingsApp { env, section, update: UpdateCheck::Idle, note: None, gpu: None, report: None, crash: None, checking_recording: None, recording_support: None, sound_devices: Default::default(), picking: None, left_behind: None, counted: None, moving: None, old: None, installing: Installing::Idle, restart_tray: false, push: Default::default(), describe: Default::default() }
    }

    pub fn sections(&self) -> Vec<Section> {
        Section::ALL.into_iter().filter(|s| self.env.tray_app || !s.needs_tray_app()).collect()
    }

    fn get(&self, flag: Flag) -> bool {
        flag.read(|name| self.env.setting(name))
    }

    fn set(&self, name: &str, value: u32) {
        settings_rows::set(&self.env, name, value);
    }

    /// A switch row for one of the shared options.
    fn flag_row(&mut self, ui: &mut Ui, flag: Flag, title: &str, detail: &str) {
        settings_rows::flag_row(ui, &self.env, flag, title, detail);
    }

    fn nav(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        ui.label(RichText::new("Settings").size(18.0).strong());
        ui.add_space(10.0);
        ui.spacing_mut().item_spacing.y = 2.0;
        for section in self.sections() {
            if theme::nav_item(ui, section.icon(), section.title(), self.section == section).clicked() {
                self.open(section);
            }
        }
    }

    fn open(&mut self, section: Section) {
        self.section = section;
        self.note = None;
        self.report = None;
        self.sound_devices = Default::default();
    }

    /// Goes to the section a launch asked for (see `asked`); Updates checks for one too, unless
    /// it's checking or installing already. Without one it stays where it is.
    pub fn go_to(&mut self, ctx: &egui::Context, request: &str) {
        let section = match request {
            "--system-info" => Section::SystemInfo,
            "--recording" => Section::Recording,
            "--updates" => Section::Updates,
            _ => return,
        };
        if section != self.section {
            self.open(section);
        }
        let busy = matches!(self.update, UpdateCheck::Checking(_)) || matches!(self.installing, Installing::Started(_) | Installing::Done(Ok(())));
        if section == Section::Updates && !busy {
            self.update = UpdateCheck::Checking(self.env.check_for_update(ctx));
        }
    }

    /// Another launch's request (see `run`): to that section, and to the front.
    fn poll_request(&mut self, ctx: &egui::Context) {
        let Ok(request) = std::fs::read_to_string(&self.env.settings_request_file) else { return };
        let _ = std::fs::remove_file(&self.env.settings_request_file);
        self.go_to(ctx, request.trim());
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn content(&mut self, ui: &mut Ui) {
        ui.label(RichText::new(self.section.title()).size(22.0).strong());
        if let Some(note) = &self.note {
            ui.label(RichText::new(note).color(theme::ACCENT));
        }
        ui.add_space(10.0);
        ui.spacing_mut().item_spacing.y = 10.0;
        match self.section {
            Section::General => self.general(ui),
            Section::Capture => self.capture(ui),
            Section::Shortcuts => self.shortcuts(ui),
            Section::Recording => self.recording(ui),
            Section::Library => self.library(ui),
            Section::Destinations => self.destinations(ui),
            Section::Updates => self.updates(ui),
            Section::SystemInfo => self.system_info(ui),
            Section::About => self.about(ui),
        }
    }

    fn general(&mut self, ui: &mut Ui) {
        theme::card(ui, |ui| {
            let current = Appearance::from_setting(self.env.setting(Appearance::SETTING));
            row(ui, "Theme", "System follows your light or dark mode", |ui| {
                theme::group(ui, |ui| {
                    // Right to left, so they read System, Light, Dark.
                    for appearance in Appearance::ALL.into_iter().rev() {
                        if ui.add(egui::Button::selectable(current == appearance, appearance.label())).clicked() && current != appearance {
                            self.env.set_setting(Appearance::SETTING, appearance.to_setting());
                            theme::set_appearance(ui.ctx(), appearance);
                        }
                    }
                })
            });
        });
        if !self.env.tray_app {
            return self.background(ui);
        }
        theme::card(ui, |ui| {
            let mut on = self.env.start_with_windows();
            let title = "Start with Windows";
            if row(ui, title, "Run RSnip in the tray when you sign in", |ui| theme::switch(ui, &mut on, title)).changed()
                && !self.env.set_start_with_windows(on)
            {
                self.note = Some("Couldn't change that: writing to the registry failed.".into());
            }
            ui.separator();
            self.flag_row(ui, options::OPEN_LIBRARY_ON_LAUNCH, "Open the library when RSnip starts", "Otherwise it starts quietly in the tray");
            ui.separator();
            self.flag_row(ui, options::NOTIFY, "Show notifications", "After each capture, and about updates");
        });
    }

    /// Linux's background icon (see `background`).
    fn background(&mut self, ui: &mut Ui) {
        let mut on = self.get(options::RUN_IN_BACKGROUND);
        theme::card(ui, |ui| {
            let title = "Keep RSnip in the background";
            let detail = "An icon in the top bar or panel stays when the library closes, for recording and opening it again";
            if row(ui, title, detail, |ui| theme::switch(ui, &mut on, title)).changed() {
                self.env.set_setting(options::RUN_IN_BACKGROUND.name, on as u32);
                if on {
                    self.env.start_tray();
                } else {
                    self.env.tell_tray(TrayRequest::Quit);
                }
            }
            ui.separator();
            ui.add_enabled_ui(on, |ui| {
                let mut start = self.env.start_with_windows();
                let title = "Start when you sign in";
                if row(ui, title, "Put the icon there when you sign in, without opening the library", |ui| theme::switch(ui, &mut start, title)).changed()
                    && !self.env.set_start_with_windows(start)
                {
                    self.note = Some("Couldn't change that: writing the autostart entry failed.".into());
                }
            });
        });
        if on && !self.env.tray_running() {
            let text = "The icon isn't showing: this desktop may not show app icons. On GNOME, the AppIndicator extension adds them.";
            ui.label(RichText::new(text).color(theme::palette(ui).muted));
            // It may just be starting.
            ui.ctx().request_repaint_after(Duration::from_secs(1));
        }
    }

    fn capture(&mut self, ui: &mut Ui) {
        theme::card(ui, |ui| {
            let folder = self.env.dir.display().to_string();
            self.flag_row(ui, options::SAVE_TO_FOLDER, "Save screenshots to the folder", &folder);
            let open = theme::label(icon::FOLDER_OPEN, "Open folder");
            if ui.button(open).clicked() {
                platform::open(&self.env.dir);
            }
            ui.separator();
            self.flag_row(ui, options::OPEN_EDITOR, "Open the editor after capture", "Every capture opens straight in the editor");
            // On Linux the area is picked on a still of the screen, and Recording has its own Hide.
            if !self.env.tray_app {
                return;
            }
            ui.separator();
            self.flag_row(ui, options::SHOW_MAGNIFIER, "Show the magnifier", "Zoomed pixels, position and colour by the cursor. Press M while selecting to show or hide it");
            ui.separator();
            self.flag_row(ui, options::HIDE_WHILE_CAPTURING, "Hide RSnip while capturing", "The library and Settings step aside during a capture or recording, and come back after");
        });
        ui.label(RichText::new("Every capture is also copied to the clipboard.").color(theme::palette(ui).muted));
    }

    fn shortcuts(&mut self, ui: &mut Ui) {
        let running = self.env.tray_running();
        if self.env.snipping_key() {
            warning(ui, "Windows also opens its own screen capture when you press Print Screen.", |ui| {
                if ui.button(theme::label(icon::ARROW_SQUARE_OUT, "Open Windows keyboard settings")).clicked() {
                    platform::open(Path::new("ms-settings:easeofaccess-keyboard"));
                }
            });
        }
        if !running {
            warning(ui, "RSnip isn't running, so shortcuts can't be changed or used.", |ui| {
                if ui.button(theme::label(icon::PLAY, "Start RSnip")).clicked() {
                    self.env.start_tray();
                }
            });
        }
        let table = shortcuts::load(|name| self.env.setting(name));
        theme::card(ui, |ui| {
            for (i, action) in Action::ALL.into_iter().enumerate() {
                if i > 0 {
                    ui.separator();
                }
                let key = shortcuts::get(&table, action);
                ui.horizontal(|ui| {
                    // A fixed-width column, so the keys line up.
                    ui.allocate_ui_with_layout(Vec2::new(190.0, 26.0), Layout::left_to_right(Align::Center), |ui| {
                        ui.set_min_width(190.0);
                        ui.label(action.label());
                    });
                    key_chip(ui, key);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add_enabled(key != 0, egui::Button::new("Clear")).clicked() {
                            self.set(action.setting(), 0);
                            // The others are stored too, so the defaults don't come back.
                            for other in Action::ALL.into_iter().filter(|&a| a != action) {
                                self.env.set_setting(other.setting(), shortcuts::get(&table, other));
                            }
                        }
                        if ui.add_enabled(running, egui::Button::new("Change")).clicked() {
                            self.env.tell_tray(TrayRequest::RecordShortcut(action));
                            self.note = Some(format!("Press the new shortcut for {} (Esc cancels).", action.label().to_lowercase()));
                        }
                    });
                });
            }
        });
        for (a, b) in shortcuts::conflicts(&table) {
            let key = platform::shortcut_name(shortcuts::get(&table, a));
            let text = format!("{key} is set for both {} and {}; only {} will run.", a.label(), b.label().to_lowercase(), a.label().to_lowercase());
            ui.label(RichText::new(text).color(Color32::from_rgb(255, 120, 110)));
        }
        theme::card(ui, |ui| {
            self.flag_row(ui, options::WIN_SHIFT_S, "Also take over Win+Shift+S", "Region capture instead of the Windows Snipping Tool");
        });
        ui.label(RichText::new("Giving an action Print Screen is how you choose what Print Screen does.").color(theme::palette(ui).muted));
    }

    fn recording(&mut self, ui: &mut Ui) {
        if self.recording_support.is_none() {
            let checking = self.checking_recording.get_or_insert_with(|| self.env.recording_support(ui.ctx()));
            self.recording_support = checking.try_recv().ok();
        }
        // Until it's known, H.265 is described as if it can be had, but can't be chosen.
        let hevc_available = self.recording_support.as_ref().map(|s| s.hevc);
        let hevc = self.get(options::RECORD_HEVC);
        if let Some(problem) = self.recording_support.as_ref().and_then(|s| s.problems[hevc as usize].clone()) {
            warning(ui, &problem, |_| {});
        }
        let gif = self.get(options::RECORD_GIF);
        theme::card(ui, |ui| {
            settings_rows::format_row(ui, &self.env, "MP4 files are small and smooth; GIFs play almost anywhere, even where videos can't go");
        });
        let mp4_only = |detail: &'static str| if gif { "For MP4s; GIFs are 12 frames a second" } else { detail };
        theme::card(ui, |ui| {
            ui.add_enabled_ui(!gif, |ui| {
                settings_rows::frame_rate_row(ui, &self.env, mp4_only("Frames a second: 60 is smoothest for scrolling and animation, 30 suits most recordings"));
                ui.separator();
                let hevc = self.get(options::RECORD_HEVC);
                let detail = if hevc_available != Some(false) {
                    "H.265 files are about a third smaller, but some older devices and websites can't play them"
                } else {
                    platform::HEVC_MISSING
                };
                row(ui, "Video codec", mp4_only(detail), |ui| {
                    theme::group(ui, |ui| {
                        let h265 = ui.add_enabled(hevc_available == Some(true) || hevc, egui::Button::selectable(hevc, "H.265"));
                        if h265.clicked() && !hevc {
                            self.set(options::RECORD_HEVC.name, 1);
                        }
                        if ui.add(egui::Button::selectable(!hevc, "H.264")).clicked() && hevc {
                            self.set(options::RECORD_HEVC.name, 0);
                        }
                    })
                });
            });
        });
        theme::card(ui, |ui| {
            settings_rows::sound_rows(ui, &self.env, &mut self.sound_devices, true);
        });
        if !self.env.tray_app {
            theme::card(ui, |ui| {
                self.flag_row(ui, options::HIDE_WHILE_CAPTURING, "Hide the library while recording", "It steps aside while you pick what to record, and comes back after");
            });
            let how = "Record from RSnip's icon in the top bar or panel. For a keyboard shortcut, add a custom one in your \
                       desktop's keyboard settings that runs \"rsnip --record\" (an area) or \"rsnip --record-screen\" \
                       (a screen or window); it stops the recording too.";
            ui.label(RichText::new(how).color(theme::palette(ui).muted));
        }
        ui.label(RichText::new("Recordings stop after 30 minutes.").color(theme::palette(ui).muted));
    }

    fn library(&mut self, ui: &mut Ui) {
        self.save_folder(ui);
        theme::card(ui, |ui| {
            let dock = Dock::from_setting(self.env.setting(Dock::SETTING));
            row(ui, "Library position", "Along the bottom gives the editor the whole width", |ui| {
                theme::group(ui, |ui| {
                    for (label, value) in [("Bottom", Dock::Bottom), ("Left", Dock::Left)] {
                        if ui.add(egui::Button::selectable(dock == value, label)).clicked() && dock != value {
                            self.set(Dock::SETTING, value.to_setting());
                        }
                    }
                })
            });
        });
        if self.env.ocr.is_some() {
            theme::card(ui, |ui| {
                let mut on = self.env.setting(super::search::SETTING) != Some(0);
                let title = "Read text in screenshots for search";
                let detail = format!("Uses {}; the text stays on this computer", platform::TEXT_RECOGNITION);
                if row(ui, title, &detail, |ui| theme::switch(ui, &mut on, title)).changed() {
                    self.set(super::search::SETTING, on as u32);
                }
            });
        }
        theme::card(ui, |ui| {
            ui.label(RichText::new("Added to every copy, save and export").color(theme::palette(ui).muted));
            for (name, title) in [("ExportBorder", "Border"), ("ExportShadow", "Drop shadow")] {
                let mut on = self.env.setting(name) == Some(1);
                if row(ui, title, "", |ui| theme::switch(ui, &mut on, title)).changed() {
                    self.set(name, on as u32);
                }
            }
        });
        self.clean_up(ui);
        self.descriptions(ui);
    }

    /// Moving old screenshots and recordings to the Recycle Bin, and how many that would be.
    fn clean_up(&mut self, ui: &mut Ui) {
        let bin = platform::RECYCLE_BIN;
        let days = options::CLEAN_UP_DAYS.read(|name| self.env.setting(name));
        // Counted again now and then, as each old one's push record is read.
        if self.old.is_none_or(|(at, counted_for, _)| counted_for != days || at.elapsed() >= Duration::from_secs(5)) {
            let files = super::library_files_with_times(&self.env.dir);
            let due = super::clean_up::due(&files, days, &crate::push::load(&self.env), SystemTime::now());
            self.old = Some((Instant::now(), days, due.len()));
        }
        let old = self.old.map_or(0, |(_, _, n)| n);
        let found = match old {
            _ if !platform::has_recycle_bin(&self.env.dir) => format!("RSnip's folder is on a network or removable drive, which has no {bin}, so nothing is moved"),
            0 => format!("Nothing is older than {days} days"),
            1 => format!("1 is older than {days} days"),
            n => format!("{n} are older than {days} days"),
        };
        let last = super::clean_up::last_run(self.env.text_setting(options::CLEAN_UP_LAST)).map(|(at, moved)| format!("Moved {moved} to the {bin}, {}", crate::push::history::ago(at, crate::push::history::now())));
        theme::card(ui, |ui| {
            let detail = "Anything an automatic Push to destination hasn't received yet stays, so nothing goes before it's backed up";
            self.flag_row(ui, options::CLEAN_UP, &format!("Move old screenshots and recordings to the {bin}"), detail);
            ui.separator();
            row(ui, "After", "Counted from when each was last saved or edited", |ui| {
                theme::group(ui, |ui| {
                    // Right to left, so they read shortest to longest.
                    for &value in options::CLEAN_UP_DAYS.values.iter().rev() {
                        if ui.add(egui::Button::selectable(days == value, format!("{value} days"))).clicked() && days != value {
                            self.set(options::CLEAN_UP_DAYS.name, value);
                        }
                    }
                })
            });
            let muted = theme::palette(ui).muted;
            ui.label(RichText::new(found).small().color(muted));
            if let Some(last) = last {
                ui.label(RichText::new(last).small().color(muted));
            }
        });
    }

    /// Where screenshots and recordings are saved, and choosing another folder.
    fn save_folder(&mut self, ui: &mut Ui) {
        if let Some(picked) = self.picking.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.picking = None;
            match picked {
                Ok(Some(dir)) => {
                    if dir != self.env.dir {
                        // So what's there can be moved too.
                        self.env.set_text_setting(options::PREVIOUS_SAVE_FOLDER, &self.env.dir.to_string_lossy());
                        self.counted = None;
                    }
                    self.env.set_text_setting(crate::options::SAVE_FOLDER, &dir.to_string_lossy());
                    self.env.tell_tray(TrayRequest::SettingsChanged);
                    self.note = Some(format!("New screenshots and recordings go to {}", dir.display()));
                }
                Ok(None) => {}
                Err(e) => self.note = Some(format!("Couldn't open the folder chooser: {e}")),
            }
        }
        self.env.refresh_dir();
        let env_dir = self.env.dir.clone();
        let home = self.env.host().home.clone();
        let shown = system_info::tidy_path(&env_dir, home.as_deref());
        if let Some(result) = self.moving.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.moving = None;
            self.counted = None;
            self.note = Some(match result {
                Ok(r) if r.already_there == 0 => format!("Moved {} to {shown}", items(r.moved)),
                Ok(r) => format!("Moved {}; {} stayed, as {shown} has files with the same names", items(r.moved), r.already_there),
                Err(e) => format!("Couldn't move them all: {e}"),
            });
        }
        // Counted again now and then, as they may be moved or deleted elsewhere.
        if self.moving.is_none() && self.counted.is_none_or(|at| at.elapsed() >= Duration::from_secs(2)) {
            self.left_behind = self.env.previous_folder().map(|dir| {
                let n = super::library_files(&dir).len();
                (dir, n)
            });
            self.counted = Some(Instant::now());
        }
        theme::card(ui, |ui| {
            row(ui, "Save screenshots and recordings to", &shown, |ui| {
                let choosing = self.picking.is_some();
                if ui.add_enabled(!choosing, egui::Button::new(if choosing { "Choosing…" } else { "Change…" })).clicked() {
                    self.picking = Some(self.env.pick_folder(ui.ctx(), "Save screenshots and recordings to", &env_dir));
                }
                if ui.button("Open").clicked() {
                    let _ = std::fs::create_dir_all(&env_dir);
                    self.env.open(&env_dir);
                }
            });
            ui.label(RichText::new("The library shows this folder. What's already saved stays where it is unless you move it.").small().color(theme::palette(ui).muted));
            if let Some((from, n)) = self.left_behind.clone().filter(|(_, n)| *n > 0) {
                ui.separator();
                let still = if n == 1 { "is still in" } else { "are still in" };
                let title = format!("{} {still} {}", items(n), system_info::tidy_path(&from, home.as_deref()));
                row(ui, &title, "With their edits and the text found in them", |ui| {
                    if self.moving.is_some() {
                        ui.add_enabled(false, egui::Button::new("Moving…"));
                    } else {
                        if ui.add(theme::primary_button("Move them here")).clicked() {
                            self.moving = Some(self.env.move_library(ui.ctx(), &from, &env_dir));
                        }
                        if ui.button("Leave them").clicked() {
                            self.env.set_text_setting(options::PREVIOUS_SAVE_FOLDER, &env_dir.to_string_lossy());
                            self.counted = None;
                        }
                    }
                });
            }
            // On Linux, screenshots come from the desktop's own tool, which keeps to its folder.
            if let Some(desktop) = self.env.desktop_screenshots().filter(|d| *d != env_dir) {
                ui.separator();
                let title = "Bring in the desktop's screenshots";
                let detail = format!("Moves new ones here from {}, where the desktop's screenshot tool saves them", system_info::tidy_path(&desktop, home.as_deref()));
                let mut on = self.get(options::BRING_IN_SCREENSHOTS);
                if row(ui, title, &detail, |ui| theme::switch(ui, &mut on, title)).changed() {
                    self.set(options::BRING_IN_SCREENSHOTS.name, on as u32);
                    // What's taken from now on; what's there already is for Move them here.
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                    self.env.set_setting(options::BRING_IN_SINCE, now as u32);
                }
            }
        });
    }

    fn updates(&mut self, ui: &mut Ui) {
        theme::card(ui, |ui| {
            self.flag_row(ui, options::CHECK_UPDATES, "Check for updates automatically", "Asks GitHub for the latest version once a day; nothing else is sent");
            ui.separator();
            self.flag_row(ui, options::RELEASE_CANDIDATES, "Get release candidates", "Try new versions before they're released, and help find their bugs");
            if self.env.tray_app {
                ui.separator();
                let installed = self.env.installed();
                let detail = if installed { "Installs new versions as soon as they're found, when you're not busy" } else { "Only for copies set up with the installer" };
                ui.add_enabled_ui(installed, |ui| self.flag_row(ui, options::AUTO_UPDATE, "Install updates automatically", detail));
            }
        });
        theme::card(ui, |ui| {
            if let UpdateCheck::Checking(rx) = &self.update {
                if let Ok(result) = rx.try_recv() {
                    self.update = UpdateCheck::Done(result);
                }
            }
            let current = self.env.version;
            let status = match &self.update {
                UpdateCheck::Idle => format!("RSnip {current}"),
                UpdateCheck::Checking(_) => "Checking…".into(),
                UpdateCheck::Done(Ok(None)) => format!("You have the latest version, {current}."),
                UpdateCheck::Done(Ok(Some(release))) => format!("RSnip {} is available (you have {current}).", release.version),
                UpdateCheck::Done(Err(e)) => format!("Couldn't check: {e}"),
            };
            let available = match &self.update {
                UpdateCheck::Done(Ok(Some(release))) => Some(release.clone()),
                _ => None,
            };
            row(ui, &status, "", |ui| {
                let checking = matches!(self.update, UpdateCheck::Checking(_));
                if ui.add_enabled(!checking, egui::Button::new("Check now")).clicked() {
                    self.update = UpdateCheck::Checking(self.env.check_for_update(ui.ctx()));
                }
                if let Some(release) = available {
                    let install = self.env.can_install_update();
                    match &self.installing {
                        Installing::Done(Ok(())) if !cfg!(windows) => {
                            if ui.add(theme::primary_button("Restart RSnip")).clicked() {
                                self.env.restart_after_update();
                                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        }
                        Installing::Started(_) => {
                            ui.add_enabled(false, egui::Button::new("Installing…"));
                        }
                        _ if install => {
                            if ui.add(theme::primary_button("Install")).clicked() {
                                self.installing = Installing::Started(self.env.install_update(ui.ctx(), &release));
                                if cfg!(windows) {
                                    self.note = Some("Installing; RSnip will restart.".into());
                                }
                            }
                        }
                        _ => {
                            if ui.button("Download").clicked() {
                                platform::open(Path::new(&release.page));
                            }
                        }
                    }
                }
            });
            if let Installing::Started(rx) = &self.installing {
                if let Ok(result) = rx.try_recv() {
                    if !cfg!(windows) {
                        self.note = Some(match &result {
                            Ok(()) => "Installed. Restart RSnip to use the new version.".into(),
                            Err(e) => format!("Couldn't install the update: {e}"),
                        });
                    }
                    self.restart_tray = !cfg!(windows) && result.is_ok() && self.get(options::RUN_IN_BACKGROUND);
                    self.installing = Installing::Done(result);
                }
            }
        });
    }

    /// Copies `text` and says whether it worked.
    fn copy(&mut self, text: &str, done: &str) {
        self.note = Some(match self.env.copy_text(text) {
            Ok(()) => done.into(),
            Err(e) => format!("Couldn't copy it: {e}"),
        });
    }

    fn system_info(&mut self, ui: &mut Ui) {
        if self.report.is_none() {
            self.report = Some(system_info::report(&self.env, &self.env.host(), self.gpu.as_ref(), Some(ui.ctx())));
            self.crash = crash::list(&self.env.crash_dir).into_iter().next();
            // Seen here, so the library and tray app stop mentioning it.
            if let Some(c) = crash::unseen(&self.env.crash_dir, self.env.setting(crash::SEEN_SETTING)) {
                self.env.set_setting(crash::SEEN_SETTING, c.time as u32);
            }
        }
        let text = system_info::text(self.report.as_deref().unwrap_or_default());
        if let Some(c) = self.crash.clone() {
            warning(ui, &format!("RSnip closed unexpectedly ({}).", crash::utc(c.time)), |ui| {
                ui.label(RichText::new(&c.summary).monospace().small());
                ui.horizontal(|ui| {
                    if ui.button(theme::label(icon::BUG, "Report this crash")).clicked() {
                        let report = std::fs::read_to_string(&c.path).unwrap_or_default();
                        self.report_problem(&text, Some(&report));
                    }
                    if ui.button(theme::label(icon::FOLDER_OPEN, "Open folder")).clicked() {
                        self.env.open(&self.env.crash_dir);
                    }
                });
            });
        }
        theme::card(ui, |ui| {
            row(ui, "Include this when you report a problem", "It has no screenshots or file names, and your home folder shows as ~", |ui| {
                if ui.button(theme::label(icon::WARNING, "Report a problem")).clicked() {
                    self.report_problem(&text, None);
                }
                if ui.button(theme::label(icon::COPY, "Copy")).clicked() {
                    self.copy(&text, "Copied to the clipboard.");
                }
            });
        });
        // One label column for every section, so the values line up down the page.
        let labels = self.report.iter().flatten().flat_map(|s| &s.rows).map(|(label, _)| {
            egui::WidgetText::from(*label).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body).size().x
        });
        let label_width = labels.fold(150.0, f32::max);
        for (i, section) in self.report.iter().flatten().enumerate() {
            theme::card(ui, |ui| {
                ui.label(RichText::new(section.title).strong());
                egui::Grid::new(("system-info", i)).num_columns(2).spacing([16.0, 4.0]).min_col_width(label_width).show(ui, |ui| {
                    for (label, value) in &section.rows {
                        ui.label(RichText::new(*label).color(theme::palette(ui).muted));
                        ui.add(egui::Label::new(RichText::new(value).monospace()).wrap());
                        ui.end_row();
                    }
                });
            });
        }
    }

    /// Opens GitHub's bug form with System info (`text`) and the crash report filled in, as far as
    /// they fit in an address; they're copied too, for what didn't.
    fn report_problem(&mut self, text: &str, crash: Option<&str>) {
        let url = issue_url(text, crash);
        let note = match (url.contains("&crash="), url.contains("&system-info="), crash.is_some()) {
            (true, _, _) => "Opened the bug form with the crash report and System info filled in.",
            (false, true, true) => "Opened the bug form with System info filled in; paste the crash report from the clipboard.",
            (false, true, false) => "Opened the bug form with System info filled in.",
            (false, false, _) => "Copied; paste it into the bug form.",
        };
        let copied = crash.map_or_else(|| text.to_owned(), |crash| format!("{crash}\n{text}"));
        self.copy(&copied, note);
        self.env.open(Path::new(&url));
    }

    fn about(&mut self, ui: &mut Ui) {
        theme::card(ui, |ui| {
            ui.label(RichText::new(format!("RSnip {}", self.env.version)).size(18.0).strong());
            ui.label(RichText::new("A small, open-source screenshot tool, written in Rust.").color(theme::palette(ui).muted));
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button(theme::label(icon::GITHUB_LOGO, "GitHub")).clicked() {
                    platform::open(Path::new(REPO));
                }
                if ui.button(theme::label(icon::WARNING, "Report a problem")).clicked() {
                    let report = system_info::report(&self.env, &self.env.host(), self.gpu.as_ref(), Some(ui.ctx()));
                    self.report_problem(&system_info::text(&report), None);
                }
                if ui.button(theme::label(icon::INFO, "Contributing")).clicked() {
                    platform::open(Path::new(&format!("{REPO}/blob/main/CONTRIBUTING.md")));
                }
            });
            ui.label(RichText::new("MIT License").small().color(theme::palette(ui).muted));
        });
    }
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        if self.gpu.is_none() {
            self.gpu = system_info::gpu(frame);
        }
        // Pick up changes made elsewhere, like a shortcut the tray app just recorded, and other
        // launches' requests.
        ui.ctx().request_repaint_after(Duration::from_millis(500));
        self.poll_request(ui.ctx());
        // From the program just installed (see `platform::program`).
        if self.restart_tray && !self.env.tray_running() {
            self.restart_tray = false;
            self.env.start_tray();
        }
        theme::set_appearance(ui.ctx(), Appearance::from_setting(self.env.setting(Appearance::SETTING)));
        egui::Panel::left("sections")
            .frame(egui::Frame::NONE.fill(theme::palette(ui).subpanel).inner_margin(egui::Margin::same(14)))
            .resizable(false)
            .exact_size(210.0)
            .show(ui, |ui| self.nav(ui));
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(theme::palette(ui).panel).inner_margin(egui::Margin::same(24))).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| self.content(ui));
        });
    }
}

/// "1 screenshot or recording", "42 screenshots and recordings".
fn items(n: usize) -> String {
    if n == 1 { "1 screenshot or recording".into() } else { format!("{n} screenshots and recordings") }
}

/// A shortcut as a key cap, or "None".
fn key_chip(ui: &mut Ui, key: u32) {
    if key == 0 {
        ui.label(RichText::new("None").color(theme::palette(ui).muted));
        return;
    }
    egui::Frame::NONE.fill(theme::palette(ui).raised).corner_radius(6).inner_margin(egui::Margin::symmetric(10, 3)).show(ui, |ui| {
        ui.label(RichText::new(platform::shortcut_name(key)).monospace());
    });
}

/// A highlighted card with a message and an action.
fn warning(ui: &mut Ui, text: &str, action: impl FnOnce(&mut Ui)) {
    egui::Frame::NONE
        .fill(Color32::from_rgb(58, 46, 24))
        .stroke(egui::Stroke::new(1.0f32, Color32::from_rgb(120, 92, 40)))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(16, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(icon::WARNING).size(16.0).color(Color32::from_rgb(255, 196, 90)));
                ui.label(text);
            });
            action(ui);
        });
}
