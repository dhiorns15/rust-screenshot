//! The rows that Settings and the capture panel have in common, so a setting looks and
//! works the same in both: each reads and writes through `Env`, and tells the running tray app.

use std::sync::mpsc::{Receiver, TryRecvError};

use eframe::egui::{self, Ui};

use super::env::{Env, SoundDeviceLists, TrayRequest};
use super::platform::SoundDevice;
use super::theme::{self, row};
use crate::options::{self, Flag};

/// Saves a setting and tells the tray app to apply it.
pub(super) fn set(env: &Env, name: &str, value: u32) {
    env.set_setting(name, value);
    env.tell_tray(TrayRequest::SettingsChanged);
}

/// The same for a text setting.
fn set_text(env: &Env, name: &str, value: &str) {
    env.set_text_setting(name, value);
    env.tell_tray(TrayRequest::SettingsChanged);
}

/// A switch row for one of the shared options.
pub(super) fn flag_row(ui: &mut Ui, env: &Env, flag: Flag, title: &str, detail: &str) {
    let mut on = flag.read(|name| env.setting(name));
    if row(ui, title, detail, |ui| theme::switch(ui, &mut on, title)).changed() {
        set(env, flag.name, on as u32);
    }
}

/// MP4 or GIF.
pub(super) fn format_row(ui: &mut Ui, env: &Env, detail: &str) {
    let gif = options::RECORD_GIF.read(|name| env.setting(name));
    row(ui, "Format", detail, |ui| {
        theme::group(ui, |ui| {
            // Right to left, so they read MP4, GIF.
            for (label, value) in [("GIF", true), ("MP4", false)] {
                if ui.add(egui::Button::selectable(gif == value, label)).clicked() && gif != value {
                    set(env, options::RECORD_GIF.name, value as u32);
                }
            }
        })
    });
}

/// Frames a second, for MP4s.
pub(super) fn frame_rate_row(ui: &mut Ui, env: &Env, detail: &str) {
    let fps = options::RECORD_FPS.read(|name| env.setting(name));
    row(ui, "Frame rate", detail, |ui| {
        theme::group(ui, |ui| {
            for &value in options::RECORD_FPS.values.iter().rev() {
                if ui.add(egui::Button::selectable(fps == value, value.to_string())).clicked() && fps != value {
                    set(env, options::RECORD_FPS.name, value);
                }
            }
        })
    });
}

/// The outputs and microphones to choose from: looked for in the background when a drop-down first
/// shows, and again each time one is opened, so one plugged in meanwhile is there (`Default` to
/// start over).
#[derive(Default)]
pub(super) struct SoundDevices {
    /// As last found.
    found: Option<SoundDeviceLists>,
    /// Looking for them, until the answer comes.
    looking: Option<Receiver<SoundDeviceLists>>,
    /// Counts the times they've changed. It's in the drop-downs' ids, so they're made anew: egui
    /// keeps an open one the size it opened at, which would hide one added meanwhile.
    changes: u32,
    /// The drop-down that's open, by its id and `changes` then.
    open: Option<(&'static str, u32)>,
}

impl SoundDevices {
    /// The lists as last found, if they have been; it starts looking if they haven't, and takes
    /// the answer if it's come.
    fn lists(&mut self, ui: &Ui, env: &Env) -> Option<&SoundDeviceLists> {
        if self.found.is_none() {
            self.look(ui, env);
        }
        if let Some(looking) = &self.looking {
            match looking.try_recv() {
                Ok(lists) => {
                    if self.found.as_ref().is_some_and(|found| *found != lists) {
                        self.changes += 1;
                    }
                    self.found = Some(lists);
                }
                // The thread is gone without an answer: what was found before, else nothing.
                Err(TryRecvError::Disconnected) => {
                    self.found.get_or_insert_default();
                }
                Err(TryRecvError::Empty) => return self.found.as_ref(),
            }
            self.looking = None;
        }
        self.found.as_ref()
    }

    /// Looks for them (again), unless it's looking already.
    fn look(&mut self, ui: &Ui, env: &Env) {
        if self.looking.is_none() {
            self.looking = Some(env.sound_devices(ui.ctx()));
        }
    }
}

/// What to record the sound of: the computer's and the microphone, each with its drop-down. All
/// greyed out for GIFs, which have none. `details` adds a line of explanation to each switch.
pub(super) fn sound_rows(ui: &mut Ui, env: &Env, devices: &mut SoundDevices, details: bool) {
    let gif = options::RECORD_GIF.read(|name| env.setting(name));
    let detail = |text: &'static str| if gif { "GIFs have no sound" } else if details { text } else { "" };
    ui.add_enabled_ui(!gif, |ui| {
        flag_row(ui, env, options::RECORD_SOUND, "Computer sound", detail("What the computer plays: videos, calls, alerts. With a mixer such as SteelSeries Sonar, choose the headphones it plays to"));
        if options::RECORD_SOUND.read(|name| env.setting(name)) {
            ui.add_space(6.0);
            output_picker(ui, env, devices);
        }
        ui.separator();
        flag_row(ui, env, options::RECORD_MICROPHONE, "Microphone", if details && !gif { "Your voice, mixed in with the computer's sound" } else { "" });
        if options::RECORD_MICROPHONE.read(|name| env.setting(name)) {
            ui.add_space(6.0);
            microphone_picker(ui, env, devices);
        }
    });
}

/// "Default output" and each one there is, as a drop-down.
pub(super) fn output_picker(ui: &mut Ui, env: &Env, devices: &mut SoundDevices) {
    device_picker(ui, env, devices, "output", "Default output", options::RECORD_SOUND_DEVICE, |l| &l.outputs);
}

/// "Default microphone" and each one that's connected, as a drop-down.
pub(super) fn microphone_picker(ui: &mut Ui, env: &Env, devices: &mut SoundDevices) {
    device_picker(ui, env, devices, "microphone", "Default microphone", options::RECORD_MICROPHONE_DEVICE, |l| &l.microphones);
}

/// The default (`default`) and each of the `devices` that `list` picks, as a drop-down saving the
/// one chosen in `setting`, or greyed out saying it's looking for them until they're found.
/// Opening it looks for them again. It fills the width it's given, and a long name is cut short
/// (it's all there when hovered).
fn device_picker(ui: &mut Ui, env: &Env, devices: &mut SoundDevices, id: &'static str, default: &str, setting: &str, list: fn(&SoundDeviceLists) -> &Vec<SoundDevice>) {
    let width = ui.available_width();
    let Some(list) = devices.lists(ui, env).map(|found| list(found).clone()) else {
        ui.add_enabled_ui(false, |ui| egui::ComboBox::from_id_salt(id).width(width).truncate().selected_text(format!("Looking for {id}s…")).show_ui(ui, |_| {}));
        return;
    };
    let changes = devices.changes;
    // Open before they changed, so open again, sized for them now.
    let reopen = devices.open.is_some_and(|(open, then)| open == id && then != changes);
    let chosen = env.text_setting(setting).unwrap_or_default();
    let missing = !chosen.is_empty() && !list.iter().any(|m| m.id == chosen);
    let missing_name = format!("{chosen} (not connected)");
    let shown = match list.iter().find(|m| m.id == chosen) {
        _ if chosen.is_empty() => default,
        Some(m) => &m.name,
        None => &missing_name,
    };
    let response = egui::ComboBox::from_id_salt((id, changes)).width(width).truncate().selected_text(shown).show_ui(ui, |ui| {
        let mut pick = None;
        if ui.selectable_label(chosen.is_empty(), default).clicked() {
            pick = Some("");
        }
        for m in &list {
            if ui.selectable_label(chosen == m.id, &m.name).clicked() {
                pick = Some(&m.id);
            }
        }
        if missing {
            let _ = ui.selectable_label(true, &missing_name);
        }
        if let Some(id) = pick.filter(|id| *id != chosen) {
            set_text(env, setting, id);
        }
    });
    if reopen {
        // egui's id for a drop-down's popup.
        egui::Popup::open_id(ui.ctx(), response.response.id.with("popup"));
        ui.ctx().request_repaint();
    }
    if response.response.clicked() && response.inner.is_some() {
        devices.look(ui, env);
    }
    if response.inner.is_some() || reopen {
        devices.open = Some((id, changes));
    } else if devices.open.is_some_and(|(open, _)| open == id) {
        devices.open = None;
    }
    response.response.on_hover_text(shown);
}
