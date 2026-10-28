//! Settings → Shortcuts on Linux. On GNOME, each action's key is a custom shortcut in GNOME's own
//! settings (see `platform::gnome_keys`), recorded here from the keys pressed in this window, and
//! Print Screen can be taken from GNOME's screenshot tool, as the Windows app takes it from
//! Windows'. Elsewhere it gives the commands, to add in the desktop's own keyboard settings.

use std::sync::mpsc::Receiver;

use eframe::egui::{self, Align, Color32, Layout, RichText, Ui, Vec2};

use super::{key_cap, row, warning, SettingsApp};
use crate::gallery::platform::gnome_keys::{self, Mods, Run, State, PRINT};
use crate::gallery::theme;
use crate::options;
use crate::shortcuts::{self, Action};

#[derive(Default)]
pub struct GnomeShortcuts {
    /// Reading GNOME's settings when the section opens, or changing them and reading them again
    /// (off the UI thread: see `Env::with_gsettings`).
    busy: Option<Receiver<Outcome>>,
    /// What was last read, or why it couldn't be.
    shown: Option<Result<State, String>>,
    /// The action whose new key is being waited for.
    recording: Option<Action>,
    /// Super is held down (egui's modifiers leave it out, except on a Mac).
    super_held: bool,
}

impl GnomeShortcuts {
    /// Opening the section reads GNOME's settings again. A change still under way carries on, so
    /// what it has to remember isn't lost.
    pub fn reopen(&mut self) {
        self.shown = None;
        self.recording = None;
    }
}

/// What reading or changing GNOME's settings came to.
struct Outcome {
    /// What to say about a change.
    note: Option<String>,
    /// GNOME's screenshot keys to remember (`options::GNOME_SCREENSHOT_KEYS`), or to forget ("").
    remember: Option<String>,
    /// The settings, read after any change.
    state: Result<State, String>,
}

/// An action's name here: scrolling capture needs X11.
fn title(action: Action) -> String {
    match action {
        Action::Scroll => format!("{} (X11)", action.label()),
        _ => action.label().to_owned(),
    }
}

/// The fixed-width first column of a row, so what follows lines up.
fn first_column(ui: &mut Ui, text: &str) {
    ui.allocate_ui_with_layout(Vec2::new(190.0, 26.0), Layout::left_to_right(Align::Center), |ui| {
        ui.set_min_width(190.0);
        ui.label(text);
    });
}

impl SettingsApp {
    pub(super) fn linux_shortcuts(&mut self, ui: &mut Ui) {
        if !gnome_keys::is_gnome(&self.env.current_desktop()) {
            return self.other_desktop_shortcuts(ui, None);
        }
        if self.gnome.shown.is_none() && self.gnome.busy.is_none() {
            self.gnome.busy = Some(self.env.with_gsettings(ui.ctx(), |run, program| Outcome { note: None, remember: None, state: gnome_keys::read(run, program) }));
            self.poll_gnome();
        }
        match self.gnome.shown.clone() {
            None => {
                ui.label(RichText::new("Reading GNOME's keyboard shortcuts…").color(theme::palette(ui).muted));
            }
            Some(Err(problem)) => self.other_desktop_shortcuts(ui, Some(&problem)),
            Some(Ok(state)) => {
                let busy = self.gnome.busy.is_some();
                ui.add_enabled_ui(!busy, |ui| self.gnome_shortcuts(ui, &state));
            }
        }
    }

    /// Takes in what reading or changing GNOME's settings came to, once it has (every frame, as a
    /// change carries on in another section).
    pub(super) fn poll_gnome(&mut self) {
        let Some(outcome) = self.gnome.busy.as_ref().and_then(|rx| rx.try_recv().ok()) else { return };
        self.gnome.busy = None;
        if let Some(note) = outcome.note {
            self.note = Some(note);
        }
        if let Some(keys) = outcome.remember {
            self.env.set_text_setting(options::GNOME_SCREENSHOT_KEYS, &keys);
        }
        self.gnome.shown = Some(outcome.state);
    }

    fn gnome_shortcuts(&mut self, ui: &mut Ui, state: &State) {
        // Before the buttons, so a key pressed for a shortcut doesn't press one of them too.
        self.record_key(ui);
        let print_action = state.print_screen();
        if print_action.is_some() && state.gnome_has_print {
            warning(ui, "GNOME also opens its own screenshot tool when you press Print Screen.", |ui| {
                if ui.button("Take Print Screen from it").clicked() {
                    self.take_print_screen(ui.ctx(), print_action.unwrap_or(Action::Region));
                }
            });
        }
        theme::card(ui, |ui| {
            for (i, action) in Action::ALL.into_iter().enumerate() {
                if i > 0 {
                    ui.separator();
                }
                let key = state.keys[action.index()].as_deref();
                let recording = self.gnome.recording == Some(action);
                ui.horizontal(|ui| {
                    first_column(ui, &title(action));
                    if recording {
                        ui.label(RichText::new("Press the keys…").color(theme::ACCENT));
                    } else {
                        key_cap(ui, key.map(gnome_keys::label).as_deref());
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add_enabled(key.is_some() && !recording, egui::Button::new("Clear")).clicked() {
                            self.change_gnome(ui.ctx(), move |run, program| {
                                gnome_keys::clear(run, program, action)?;
                                Ok((format!("{} has no shortcut now.", action.label()), None))
                            });
                        }
                        if recording {
                            if ui.button("Cancel").clicked() {
                                self.gnome.recording = None;
                                self.note = None;
                            }
                        } else {
                            let set = ui.button("Set…");
                            if set.clicked() {
                                // So Enter or Space, pressed for the shortcut, doesn't press it again.
                                set.surrender_focus();
                                self.gnome.recording = Some(action);
                                self.gnome.super_held = false;
                                self.note = Some(format!("Press the new shortcut for {} (Esc cancels).", action.label().to_lowercase()));
                            }
                        }
                    });
                });
            }
        });
        let keys = state.keys.clone().map(|key| key.map(|k| gnome_keys::normalize(&k)));
        for (a, b) in shortcuts::conflicts(&keys) {
            let key = state.keys[a.index()].as_deref().map(gnome_keys::label).unwrap_or_default();
            let text = format!("{key} is set for both {} and {}; GNOME runs only one of them.", a.label(), b.label().to_lowercase());
            ui.label(RichText::new(text).color(Color32::from_rgb(255, 120, 110)));
        }
        theme::card(ui, |ui| {
            let mut on = print_action.is_some();
            let switch = "Use Print Screen for RSnip";
            let detail = "Print Screen runs the action below instead of GNOME's screenshot tool, which gets it back when this is off";
            let chosen = print_action.unwrap_or_else(|| Action::from_index(options::PRINT_SCREEN_ACTION.read(|n| self.env.setting(n)) as usize).unwrap_or(Action::Region));
            if row(ui, switch, detail, |ui| theme::switch(ui, &mut on, switch)).changed() {
                if on {
                    self.take_print_screen(ui.ctx(), chosen);
                } else {
                    self.give_back_print_screen(ui.ctx());
                }
            }
            ui.separator();
            row(ui, "Print Screen runs", "", |ui| {
                let mut pick = None;
                egui::ComboBox::from_id_salt("print-screen-action").width(200.0).selected_text(title(chosen)).show_ui(ui, |ui| {
                    for action in Action::ALL {
                        if ui.selectable_label(action == chosen, title(action)).clicked() && action != chosen {
                            pick = Some(action);
                        }
                    }
                });
                if let Some(action) = pick {
                    self.env.set_setting(options::PRINT_SCREEN_ACTION.name, action.index() as u32);
                    if print_action.is_some() {
                        self.change_gnome(ui.ctx(), move |run, program| {
                            gnome_keys::set(run, program, action, PRINT)?;
                            Ok((format!("Print Screen now runs {}.", action.label().to_lowercase()), None))
                        });
                    }
                }
            });
        });
        let how = "These are custom shortcuts in GNOME's Keyboard settings, so they work on X11 and Wayland, even when RSnip \
                   isn't running. Keys GNOME already uses for something else don't reach this window, so they can't be chosen.";
        ui.label(RichText::new(how).color(theme::palette(ui).muted));
    }

    /// While a shortcut is being recorded, takes this frame's key presses (so nothing else reacts
    /// to them) and saves the first that makes a shortcut; Esc cancels.
    fn record_key(&mut self, ui: &mut Ui) {
        let Some(action) = self.gnome.recording.filter(|_| self.gnome.busy.is_none()) else { return };
        let events = ui.input_mut(|i| {
            let keys: Vec<egui::Event> = i.events.iter().filter(|e| matches!(e, egui::Event::Key { .. })).cloned().collect();
            i.events.retain(|e| !matches!(e, egui::Event::Key { .. }));
            keys
        });
        for event in events {
            let egui::Event::Key { key, physical_key, pressed, repeat, modifiers } = event else { continue };
            let super_key = |k: egui::Key| matches!(k, egui::Key::SuperLeft | egui::Key::SuperRight);
            if super_key(key) || physical_key.is_some_and(super_key) {
                self.gnome.super_held = pressed;
                continue;
            }
            let modifier = matches!(key, egui::Key::ShiftLeft | egui::Key::ShiftRight | egui::Key::ControlLeft | egui::Key::ControlRight | egui::Key::AltLeft | egui::Key::AltRight);
            if !pressed || repeat || modifier {
                continue;
            }
            let mods = Mods { shift: modifiers.shift, ctrl: modifiers.ctrl, alt: modifiers.alt, super_key: self.gnome.super_held };
            if key == egui::Key::Escape && mods == Mods::default() {
                self.gnome.recording = None;
                self.note = None;
                return;
            }
            match gnome_keys::accelerator(key, physical_key, mods) {
                Err(why) => self.note = Some(why.to_owned()),
                Ok(accel) => {
                    self.gnome.recording = None;
                    self.change_gnome(ui.ctx(), move |run, program| {
                        let (shown, runs) = (gnome_keys::label(&accel), action.label().to_lowercase());
                        let note = match gnome_keys::set(run, program, action, &accel)? {
                            Some(before) => format!("{shown} now runs {runs}, instead of {}.", before.label().to_lowercase()),
                            None => format!("{shown} now runs {runs}."),
                        };
                        Ok((note, None))
                    });
                    return;
                }
            }
        }
    }

    /// Gives Print Screen to `action`, remembering GNOME's screenshot keys from before, to give
    /// back.
    fn take_print_screen(&mut self, ctx: &egui::Context, action: Action) {
        self.change_gnome(ctx, move |run, program| {
            let before = gnome_keys::take_print_screen(run, program, action)?;
            Ok((format!("Print Screen now runs {}.", action.label().to_lowercase()), before))
        });
    }

    fn give_back_print_screen(&mut self, ctx: &egui::Context) {
        let saved = self.env.text_setting(options::GNOME_SCREENSHOT_KEYS).filter(|s| !s.trim().is_empty());
        self.change_gnome(ctx, move |run, program| {
            gnome_keys::give_back_print_screen(run, program, saved.as_deref())?;
            Ok(("Print Screen opens GNOME's screenshot tool again.".to_owned(), Some(String::new())))
        });
    }

    /// Changes GNOME's settings with `change`, which says what to say about it and any of GNOME's
    /// screenshot keys to remember, then reads them again.
    fn change_gnome(&mut self, ctx: &egui::Context, change: impl FnOnce(Run, &str) -> Result<(String, Option<String>), String> + Send + 'static) {
        self.gnome.busy = Some(self.env.with_gsettings(ctx, |run, program| {
            let (note, remember) = match change(run, program) {
                Ok((note, remember)) => (note, remember),
                Err(e) => (format!("Couldn't change GNOME's shortcuts: {e}"), None),
            };
            Outcome { note: Some(note), remember, state: gnome_keys::read(run, program) }
        }));
    }

    /// The commands to give keys elsewhere, as GNOME's settings can't be used (`problem` says why,
    /// on GNOME).
    fn other_desktop_shortcuts(&mut self, ui: &mut Ui, problem: Option<&str>) {
        let muted = theme::palette(ui).muted;
        if let Some(problem) = problem {
            ui.label(RichText::new(format!("Couldn't read GNOME's keyboard shortcuts: {problem}")).color(muted));
        }
        let how = "To run these with keys, add each as a custom shortcut in your desktop's keyboard settings (on KDE \
                   Plasma: System Settings > Keyboard > Shortcuts > Add New > Command or Script).";
        ui.label(RichText::new(how).color(muted));
        let program = self.env.program();
        theme::card(ui, |ui| {
            for (i, action) in Action::ALL.into_iter().enumerate() {
                if i > 0 {
                    ui.separator();
                }
                let command = gnome_keys::command(&program, action);
                ui.horizontal(|ui| {
                    first_column(ui, &title(action));
                    ui.label(RichText::new(&command).monospace());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button(theme::label(theme::icon::COPY, "Copy")).clicked() {
                            self.copy(&command, "Copied the command.");
                        }
                    });
                });
            }
        });
        ui.label(RichText::new("Running the recording's command again stops the recording.").color(muted));
    }
}
