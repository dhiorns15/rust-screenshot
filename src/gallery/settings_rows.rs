//! The rows that Settings and the capture panel have in common, so a setting looks and
//! works the same in both: each reads and writes through `Env`, and tells the running tray app.

use eframe::egui::{self, Ui};

use super::env::{Env, TrayRequest};
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

/// The outputs and microphones to choose from, read when a drop-down first shows (`Default` to read
/// them again).
#[derive(Default)]
pub(super) struct SoundDevices {
    outputs: Option<Vec<SoundDevice>>,
    microphones: Option<Vec<SoundDevice>>,
}

/// What to record the sound of: the computer's and the microphone, each with its drop-down. All
/// greyed out for GIFs, which have none. `details` adds a line of explanation to each switch.
pub(super) fn sound_rows(ui: &mut Ui, env: &Env, devices: &mut SoundDevices, details: bool) {
    let gif = options::RECORD_GIF.read(|name| env.setting(name));
    let detail = |text: &'static str| if gif { "GIFs have no sound" } else if details { text } else { "" };
    ui.add_enabled_ui(!gif, |ui| {
        flag_row(ui, env, options::RECORD_SOUND, "Computer sound", detail("What the computer plays: videos, calls, alerts. With a mixer such as SteelSeries Sonar, choose the headphones it plays to"));
        if options::RECORD_SOUND.read(|name| env.setting(name)) {
            output_picker(ui, env, devices);
        }
        ui.separator();
        flag_row(ui, env, options::RECORD_MICROPHONE, "Microphone", if details && !gif { "Your voice, mixed in with the computer's sound" } else { "" });
        if options::RECORD_MICROPHONE.read(|name| env.setting(name)) {
            microphone_picker(ui, env, devices);
        }
    });
}

/// "Default output" and each one there is, as a drop-down.
pub(super) fn output_picker(ui: &mut Ui, env: &Env, devices: &mut SoundDevices) {
    let list = devices.outputs.get_or_insert_with(|| env.outputs());
    device_picker(ui, env, "output", "Default output", options::RECORD_SOUND_DEVICE, list);
}

/// "Default microphone" and each one that's connected, as a drop-down.
pub(super) fn microphone_picker(ui: &mut Ui, env: &Env, devices: &mut SoundDevices) {
    let list = devices.microphones.get_or_insert_with(|| env.microphones());
    device_picker(ui, env, "microphone", "Default microphone", options::RECORD_MICROPHONE_DEVICE, list);
}

/// The default (`default`) and each of `list`, as a drop-down saving the one chosen in `setting`.
fn device_picker(ui: &mut Ui, env: &Env, id: &str, default: &str, setting: &str, list: &[SoundDevice]) {
    let chosen = env.text_setting(setting).unwrap_or_default();
    let missing = !chosen.is_empty() && !list.iter().any(|m| m.id == chosen);
    let missing_name = format!("{chosen} (not connected)");
    let shown = match list.iter().find(|m| m.id == chosen) {
        _ if chosen.is_empty() => default,
        Some(m) => &m.name,
        None => &missing_name,
    };
    ui.add_space(6.0);
    egui::ComboBox::from_id_salt(id).width(ui.available_width()).selected_text(shown).show_ui(ui, |ui| {
        let mut pick = None;
        if ui.selectable_label(chosen.is_empty(), default).clicked() {
            pick = Some("");
        }
        for m in list {
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
}
