//! RSnip in the background on Linux: an icon in the top bar or panel (a StatusNotifierItem, which
//! Ubuntu's GNOME, KDE and most panels show) for recording the screen and opening the library,
//! Settings and the screenshots folder, that stays when the library is closed. `rsnip --tray` runs
//! it, and the library starts it (see `platform::start_tray`); `rsnip --tray-quit` or Settings stops
//! it. `rsnip --record` asks it to record (starting it, without an icon if need be).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use ksni::blocking::TrayMethods;
use ksni::menu::{MenuItem, StandardItem};

use super::platform;
use super::portal::{ScreenCast, Sources};
use super::recorder::{self, Format, Recording};
use crate::options;

/// What the icon's menu, or another RSnip process, asks for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    OpenLibrary,
    OpenFolder,
    Settings,
    /// Record a screen or window, picked in the desktop's dialog.
    RecordScreen,
    /// Record part of a screen, picked in RSnip's own.
    RecordArea,
    StopRecording,
    Quit,
}

const COMMANDS: [Command; 7] =
    [Command::OpenLibrary, Command::OpenFolder, Command::Settings, Command::RecordScreen, Command::RecordArea, Command::StopRecording, Command::Quit];

impl Command {
    /// As written to the request file.
    fn name(self) -> &'static str {
        match self {
            Command::OpenLibrary => "open",
            Command::OpenFolder => "folder",
            Command::Settings => "settings",
            Command::RecordScreen => "record-screen",
            Command::RecordArea => "record-area",
            Command::StopRecording => "stop",
            Command::Quit => "quit",
        }
    }

    fn parse(name: &str) -> Option<Command> {
        COMMANDS.into_iter().find(|c| c.name() == name)
    }
}

/// The file other RSnip processes (and installer/linux/install.sh) write a request to, checked
/// twice a second.
pub fn request_file() -> PathBuf {
    let user = std::env::var("USER").unwrap_or_default();
    std::env::temp_dir().join(format!("rsnip-tray-{user}.request"))
}

/// Asks the running icon, if any, to go.
pub fn quit() {
    let _ = std::fs::write(request_file(), Command::Quit.name());
}

/// Asks RSnip in the background to record, starting it if it isn't running. Recording again
/// stops it, as on Windows.
pub fn record(area: bool) {
    let command = if area { Command::RecordArea } else { Command::RecordScreen };
    if platform::tray_running() {
        let _ = std::fs::write(request_file(), command.name());
    } else {
        run(Some(command));
    }
}

/// A recording under way, and the screen sharing it records.
struct Active {
    recording: Recording,
    _cast: ScreenCast,
    /// The library was hidden for it.
    hid_library: bool,
}

/// Shows the icon until it's told to quit, doing `first` first. Without `first`, returns at once if
/// it's already running, or if this desktop has nowhere to show it. With it, it carries on (without
/// the icon, if need be) until that's done, and stays only where it would have anyway.
pub fn run(first: Option<Command>) {
    let wanted = options::RUN_IN_BACKGROUND.read(platform::setting);
    if (first.is_none() && !wanted) || !platform::claim_single_instance("Tray", "") {
        return;
    }
    let requests = request_file();
    let _ = std::fs::remove_file(&requests);
    let (tx, rx) = channel();
    let icon = match (Icon { tx: tx.clone(), recording: false }).spawn() {
        Ok(handle) => Some(handle),
        Err(e) if first.is_none() => {
            eprintln!("RSnip can't show an icon on this desktop ({e}). On GNOME, the AppIndicator extension adds them.");
            return;
        }
        Err(_) => None,
    };
    if let Some(first) = first {
        let _ = tx.send(first);
    }
    // Otherwise it's here for `first` only, and goes once that's done.
    let stay = wanted && icon.is_some();
    if stay {
        // Pushing new screenshots and recordings by themselves (see `auto_push`).
        super::auto_push::start(notify);
    }
    let mut active: Option<Active> = None;
    let mut shown_recording = false;
    let mut brought_in = Instant::now();
    loop {
        // New screenshots from the desktop's screenshot tool, into RSnip's folder (see `bring_in`).
        if brought_in.elapsed() >= Duration::from_secs(2) {
            brought_in = Instant::now();
            super::bring_in::run(&platform::legacy_save_folder(), &platform::screenshots_dir(), platform::setting, platform::set_setting);
        }
        let command = match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(command) => Some(command),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Command::Quit),
        };
        let requested = std::fs::read_to_string(&requests).ok().inspect(|_| {
            let _ = std::fs::remove_file(&requests);
        });
        let command = command.or(requested.as_deref().map(str::trim).and_then(Command::parse));
        // Recording again stops it.
        let command = match command {
            Some(Command::RecordScreen | Command::RecordArea) if active.is_some() => Some(Command::StopRecording),
            other => other,
        };
        match command {
            Some(Command::Quit) => break,
            Some(Command::OpenLibrary) => platform::spawn_self(&[]),
            Some(Command::OpenFolder) => platform::open(&platform::screenshots_dir()),
            Some(Command::Settings) => platform::open_settings(false),
            Some(Command::RecordScreen) => active = start_recording(false),
            Some(Command::RecordArea) => active = start_recording(true),
            Some(Command::StopRecording) => finish(active.take()),
            None => {}
        }
        // Stopped by the desktop (the shared window closed, say), or too long.
        if active.as_mut().is_some_and(|a| a.recording.ended() || a.recording.started.elapsed() >= recorder::MAX_LENGTH) {
            finish(active.take());
        }
        if !stay && active.is_none() {
            break;
        }
        match &icon {
            Some(icon) if icon.is_closed() => break,
            Some(icon) if shown_recording != active.is_some() => {
                shown_recording = active.is_some();
                icon.update(|i| i.recording = shown_recording);
            }
            _ => {}
        }
    }
    finish(active.take());
    if let Some(icon) = icon {
        // Quit may have come from the menu: let its reply reach the panel first.
        std::thread::sleep(Duration::from_millis(200));
        icon.shutdown().wait();
    }
}

/// Asks the desktop what to record and starts; says why not if it can't.
fn start_recording(area: bool) -> Option<Active> {
    let hide = options::HIDE_WHILE_CAPTURING.read(platform::setting) && platform::instance_running("Gallery");
    if hide {
        tell_library("--hide");
        // It checks twice a second, then the desktop animates it away.
        std::thread::sleep(Duration::from_millis(900));
    }
    let started = begin(area);
    if hide && !matches!(started, Ok(Some(_))) {
        tell_library("--show");
    }
    match started {
        Ok(Some((recording, cast))) => Some(Active { recording, _cast: cast, hid_library: hide }),
        Ok(None) => None,
        Err(e) => {
            notify("Couldn't record", &e);
            None
        }
    }
}

/// The portal's picker, then RSnip's area picker if `area`, then the recording; `None` if the
/// user cancelled.
fn begin(area: bool) -> Result<Option<(Recording, ScreenCast)>, String> {
    recorder::available()?;
    let sources = if area { Sources::Screen } else { Sources::ScreenOrWindow };
    let Some(cast) = ScreenCast::start(sources)? else { return Ok(None) };
    let frame = recorder::snapshot(&cast)?;
    let size = (frame.width(), frame.height());
    let crop = if area {
        let file = std::env::temp_dir().join(format!("rsnip-area-{}.png", std::process::id()));
        frame.save(&file).map_err(|e| e.to_string())?;
        let picked = pick_area(&file);
        let _ = std::fs::remove_file(&file);
        match picked {
            Some(a) if (a.width, a.height) == size => None,
            Some(a) => Some(a),
            None => return Ok(None),
        }
    } else {
        None
    };
    let get = platform::setting;
    let format = Format { gif: options::RECORD_GIF.read(get), hevc: options::RECORD_HEVC.read(get), fps: options::RECORD_FPS.read(get) };
    let recording = recorder::start(&cast, size, crop, format, &platform::screenshots_dir())?;
    Ok(Some((recording, cast)))
}

/// Runs `rsnip --pick-area` on a frame of the screen.
fn pick_area(frame: &Path) -> Option<recorder::Area> {
    let exe = std::env::current_exe().ok()?;
    let out = std::process::Command::new(exe).arg("--pick-area").arg(frame).output().ok()?;
    let numbers: Vec<u32> = String::from_utf8_lossy(&out.stdout).split_whitespace().filter_map(|n| n.parse().ok()).collect();
    let [x, y, width, height] = numbers[..] else { return None };
    Some(recorder::Area { x, y, width, height })
}

/// Stops a recording, says where it went, and brings the library back.
fn finish(active: Option<Active>) {
    let Some(active) = active else { return };
    let hid = active.hid_library;
    match active.recording.stop() {
        Ok(recorded) => {
            let name = recorded.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let note = recorded.note.map_or(String::new(), |n| format!(" {n}"));
            // Copied as a file, as on Windows, to paste straight into a chat or email. This thread
            // lasts as long as the icon, so the clipboard has it to give.
            let detail = if super::platform::copy_files(std::slice::from_ref(&recorded.path)).is_ok() { "copied as a file" } else { "in your screenshots folder" };
            notify("Recording saved", &format!("{name}: {detail}.{note}"));
        }
        Err(e) => notify("Couldn't save the recording", &e),
    }
    if hid {
        tell_library("--show");
    }
}

/// Asks the library to hide or come back (see `App::poll`).
fn tell_library(request: &str) {
    let _ = std::fs::write(super::env::request_file(), request);
}

/// A desktop notification; nothing if the desktop has none.
pub(super) fn notify(summary: &str, body: &str) {
    let Ok(conn) = zbus::blocking::Connection::session() else { return };
    let hints: HashMap<&str, zbus::zvariant::Value> = HashMap::new();
    let _ = conn.call_method(
        Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications",
        Some("org.freedesktop.Notifications"),
        "Notify",
        &("RSnip", 0u32, "rsnip", summary, body, Vec::<&str>::new(), hints, -1i32),
    );
}

struct Icon {
    tx: Sender<Command>,
    recording: bool,
}

impl Icon {
    fn item(label: &str, command: Command) -> MenuItem<Icon> {
        let activate = Box::new(move |icon: &mut Icon| {
            let _ = icon.tx.send(command);
        });
        StandardItem { label: label.into(), activate, ..Default::default() }.into()
    }
}

impl ksni::Tray for Icon {
    fn id(&self) -> String {
        "rsnip".into()
    }

    fn title(&self) -> String {
        if self.recording { "RSnip: recording" } else { "RSnip" }.into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        [22, 32, 48, 64]
            .into_iter()
            .map(|size| {
                let mut bgra = crate::icon_art::render(size);
                if self.recording {
                    recording_dot(&mut bgra, size);
                }
                // ARGB, from the BGRA the art is drawn in.
                let data = bgra.chunks_exact(4).flat_map(|p| [p[3], p[2], p[1], p[0]]).collect();
                ksni::Icon { width: size, height: size, data }
            })
            .collect()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        let description = if self.recording { "Recording. Click to stop" } else { "Click to open the library" };
        ksni::ToolTip { title: "RSnip".into(), description: description.into(), ..Default::default() }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send(if self.recording { Command::StopRecording } else { Command::OpenLibrary });
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let record = if self.recording {
            vec![Icon::item("Stop recording", Command::StopRecording)]
        } else {
            vec![Icon::item("Record screen or window…", Command::RecordScreen), Icon::item("Record an area…", Command::RecordArea)]
        };
        record
            .into_iter()
            .chain([
                MenuItem::Separator,
                Icon::item("Open library", Command::OpenLibrary),
                Icon::item("Open screenshots folder", Command::OpenFolder),
                Icon::item("Settings", Command::Settings),
                MenuItem::Separator,
                Icon::item("Quit RSnip", Command::Quit),
            ])
            .collect()
    }
}

/// A red dot in the icon's corner while recording.
fn recording_dot(bgra: &mut [u8], size: i32) {
    let r = size as f32 * 0.22;
    let c = size as f32 - r - 1.0;
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)).sqrt();
            let alpha = (r - d + 0.5).clamp(0.0, 1.0);
            if alpha > 0.0 {
                let i = (y * size + x) as usize * 4;
                for (k, v) in [0x30u8, 0x30, 0xE0, 0xFF].into_iter().enumerate() {
                    bgra[i + k] = (bgra[i + k] as f32 * (1.0 - alpha) + v as f32 * alpha) as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip() {
        for c in COMMANDS {
            assert_eq!(Command::parse(c.name()), Some(c));
        }
        assert_eq!(Command::parse("junk"), None);
    }
}
