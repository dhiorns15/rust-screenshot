//! The Settings window (`rsnip --settings`): every option in one place, in sections down the left.
//!
//! It reads and writes the same settings as the tray app and tells the running tray app to apply
//! them. Shortcuts are recorded by the tray app (its keyboard hook sees keys like Print Screen
//! before anything else). Settings are re-read every half second, so changes made elsewhere show up.

use std::path::Path;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use eframe::egui::{self, Align, Color32, Layout, RichText, Ui, Vec2};

use super::env::{Env, TrayRequest};
use super::theme::{self, icon, Appearance};
use super::{platform, Dock};
use crate::options::{self, Flag};
use crate::shortcuts::{self, Action};
use crate::update::Release;

pub const TITLE: &str = "RSnip Settings";
const REPO: &str = "https://github.com/dhiorns15/rust-screenshot";

pub fn run() {
    if !platform::claim_single_instance("Settings", TITLE) {
        return;
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_inner_size([800.0, 580.0])
            .with_min_inner_size([660.0, 460.0])
            .with_icon(super::window_icon()),
        ..Default::default()
    };
    let _ = eframe::run_native(TITLE, options, Box::new(|cc| Ok(Box::new(SettingsApp::new(&cc.egui_ctx, Env::real())))));
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    General,
    Capture,
    Shortcuts,
    Recording,
    Library,
    Updates,
    About,
}

impl Section {
    const ALL: [Section; 7] =
        [Section::General, Section::Capture, Section::Shortcuts, Section::Recording, Section::Library, Section::Updates, Section::About];

    pub fn title(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Capture => "Capture",
            Section::Shortcuts => "Shortcuts",
            Section::Recording => "Recording",
            Section::Library => "Library & editor",
            Section::Updates => "Updates",
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
            Section::Updates => icon::ARROWS_CLOCKWISE,
            Section::About => icon::INFO,
        }
    }

    /// Only for the tray app (Windows).
    fn needs_tray_app(self) -> bool {
        matches!(self, Section::Capture | Section::Shortcuts | Section::Recording)
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
}

impl SettingsApp {
    pub fn new(ctx: &egui::Context, env: Env) -> SettingsApp {
        theme::apply(ctx, Appearance::from_setting(env.setting(Appearance::SETTING)));
        let mut fonts = egui::FontDefinitions::default();
        theme::add_icons(&mut fonts);
        ctx.set_fonts(fonts);
        let section = Section::General;
        SettingsApp { env, section, update: UpdateCheck::Idle, note: None }
    }

    pub fn sections(&self) -> Vec<Section> {
        Section::ALL.into_iter().filter(|s| self.env.tray_app || !s.needs_tray_app()).collect()
    }

    fn get(&self, flag: Flag) -> bool {
        flag.read(|name| self.env.setting(name))
    }

    fn set(&self, name: &str, value: u32) {
        self.env.set_setting(name, value);
        self.env.tell_tray(TrayRequest::SettingsChanged);
    }

    /// A switch row for one of the shared options.
    fn flag_row(&mut self, ui: &mut Ui, flag: Flag, title: &str, detail: &str) {
        let mut on = self.get(flag);
        if row(ui, title, detail, |ui| theme::switch(ui, &mut on, title)).changed() {
            self.set(flag.name, on as u32);
        }
    }

    fn nav(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        ui.label(RichText::new("Settings").size(18.0).strong());
        ui.add_space(10.0);
        ui.spacing_mut().item_spacing.y = 2.0;
        for section in self.sections() {
            if theme::nav_item(ui, section.icon(), section.title(), self.section == section).clicked() {
                self.section = section;
                self.note = None;
            }
        }
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
            Section::Updates => self.updates(ui),
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
            return;
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
            ui.separator();
            self.flag_row(ui, options::SHOW_MAGNIFIER, "Show the magnifier", "Zoomed pixels, position and colour by the cursor. Press M while selecting to show or hide it");
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
        theme::card(ui, |ui| {
            let gif = self.get(options::RECORD_GIF);
            row(ui, "Format", "MP4 files are small and smooth; GIFs play almost anywhere, even where videos can't go", |ui| {
                theme::group(ui, |ui| {
                    for (label, value) in [("GIF", true), ("MP4", false)] {
                        if ui.add(egui::Button::selectable(gif == value, label)).clicked() && gif != value {
                            self.set(options::RECORD_GIF.name, value as u32);
                        }
                    }
                })
            });
        });
        ui.label(RichText::new("Recordings stop after 30 minutes.").color(theme::palette(ui).muted));
    }

    fn library(&mut self, ui: &mut Ui) {
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
        theme::card(ui, |ui| {
            ui.label(RichText::new("Added to every copy, save and export").color(theme::palette(ui).muted));
            for (name, title) in [("ExportBorder", "Border"), ("ExportShadow", "Drop shadow")] {
                let mut on = self.env.setting(name) == Some(1);
                if row(ui, title, "", |ui| theme::switch(ui, &mut on, title)).changed() {
                    self.set(name, on as u32);
                }
            }
        });
    }

    fn updates(&mut self, ui: &mut Ui) {
        theme::card(ui, |ui| {
            self.flag_row(ui, options::CHECK_UPDATES, "Check for updates automatically", "Asks GitHub for the latest version once a day; nothing else is sent");
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
                    let install = self.env.tray_app && self.env.installed() && self.env.tray_running();
                    if install && ui.add(theme::primary_button("Install")).clicked() {
                        self.env.tell_tray(TrayRequest::InstallUpdate);
                        self.note = Some("Installing; RSnip will restart.".into());
                    } else if !install && ui.button("Download").clicked() {
                        platform::open(Path::new(&release.page));
                    }
                }
            });
        });
    }

    fn about(&mut self, ui: &mut Ui) {
        theme::card(ui, |ui| {
            ui.label(RichText::new(format!("RSnip {}", self.env.version)).size(18.0).strong());
            ui.label(RichText::new("A small, open-source screenshot tool, written in Rust.").color(theme::palette(ui).muted));
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                let links = [
                    (icon::GITHUB_LOGO, "GitHub", REPO.to_string()),
                    (icon::WARNING, "Report a problem", format!("{REPO}/issues/new")),
                    (icon::INFO, "Contributing", format!("{REPO}/blob/main/CONTRIBUTING.md")),
                ];
                for (icon, label, url) in links {
                    if ui.button(theme::label(icon, label)).clicked() {
                        platform::open(Path::new(&url));
                    }
                }
            });
            ui.label(RichText::new("MIT License").small().color(theme::palette(ui).muted));
        });
    }
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        // Pick up changes made elsewhere, like a shortcut the tray app just recorded.
        ui.ctx().request_repaint_after(Duration::from_millis(500));
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

/// A setting: its title and a line of detail on the left, its control on the right.
fn row<R>(ui: &mut Ui, title: &str, detail: &str, control: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let inner = control(ui);
            if detail.is_empty() {
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| ui.label(title));
            } else {
                ui.with_layout(Layout::top_down(Align::Min), |ui| {
                    ui.label(title);
                    ui.label(RichText::new(detail).small().color(theme::palette(ui).muted));
                });
            }
            inner
        })
        .inner
    })
    .inner
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
