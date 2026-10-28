//! What the library and Settings windows read and change outside themselves: the screenshots
//! folder, the settings, the running tray app, and so on. Tests get a sandbox instead: a temporary
//! folder, in-memory settings, and a record of what would have been done, so they never touch the
//! real ones.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
use std::sync::Arc;

use image::RgbaImage;

use super::describer::{self, Event};
use super::platform;
use super::system_info::Host;
use super::words::Word;
use crate::describe::{self, Described, Model, Provider, Service};
use crate::push::{CodeEvent, Destination, Kind, Saved};
use crate::shortcuts::Action;
use crate::update::{Release, Version};

/// The folder chooser's answer: the folder, or `None` if it was cancelled.
pub type PickedFolder = Receiver<Result<Option<PathBuf>, String>>;
/// Reads the text in an image file (for search).
pub type Ocr = Arc<dyn Fn(&Path) -> Result<String, String> + Send + Sync>;
/// Reads the text in an image word by word, with where each word is (for the Copy text tool).
pub type ReadWords = Arc<dyn Fn(&RgbaImage) -> Result<Vec<Word>, String> + Send + Sync>;

pub struct Env {
    pub dir: PathBuf,
    /// Another `--gallery` launch drops the file it wants opened here.
    pub request_file: PathBuf,
    /// Ask GitHub for a newer release (only where the tray app doesn't).
    pub check_updates: bool,
    /// This system has the tray app, with its capture, shortcut and startup options (Windows).
    pub tray_app: bool,
    /// Scrolling captures can be taken: on Windows, and on Linux on an X11 desktop.
    pub scroll_capture: bool,
    /// This build's version (a fixed one in the sandbox, so snapshots survive version bumps).
    pub version: Version,
    /// The player plays recordings' sound through the speakers (not in the sandbox).
    pub play_sound: bool,
    /// Text recognition for search, where the system has it (Windows, or Tesseract elsewhere).
    pub ocr: Option<Ocr>,
    /// The same, word by word, for copying text out of an image.
    pub read_words: Option<ReadWords>,
    /// Where crash reports are written (see `crash`).
    pub crash_dir: PathBuf,
    /// What was pushed where (see `push::history`).
    pub history_file: PathBuf,
    /// Another `--settings` launch drops the section it wants here (see `settings_window::run`).
    pub settings_request_file: PathBuf,
    settings: Store,
    #[cfg_attr(not(test), allow(dead_code))]
    sandbox: Option<RefCell<Sandbox>>,
}

/// How pushing files to a destination is going (see `Env::push_files`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PushEvent {
    /// Starting on the file at this place in the list.
    Pushing(usize),
    /// Pushed, with the link to it there if it has one.
    Pushed(PathBuf, Option<String>),
    Failed(PathBuf, String),
    /// That's all of them.
    Done,
}

/// What the Settings window asks of the running tray app.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TrayRequest {
    /// Re-read the settings and apply them (shortcuts, Win+Shift+S, ...).
    SettingsChanged,
    /// Show the "press the new shortcut" prompt for this action.
    RecordShortcut(Action),
    /// Check for an update and install it.
    InstallUpdate,
    /// Stop running in the background (Linux's icon).
    Quit,
}

/// The outputs whose sound can be recorded, the microphones that are connected, and the cameras
/// (see `Env::sound_devices`).
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct SoundDeviceLists {
    pub outputs: Vec<platform::SoundDevice>,
    pub microphones: Vec<platform::SoundDevice>,
    pub cameras: Vec<platform::SoundDevice>,
}

/// What Settings → Recording says about recording here (see `Env::recording_support`).
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct RecordingSupport {
    /// Recordings can be H.265.
    pub hevc: bool,
    /// What's missing for recording as H.264, then as H.265, if anything.
    pub problems: [Option<String>; 2],
}

/// What moving the library to another folder did (see `Env::move_library`).
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct MoveReport {
    pub moved: usize,
    /// Left behind, as the new folder has files with the same names.
    pub already_there: usize,
}

/// The outside world, as far as tests are concerned.
#[derive(Default)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct Sandbox {
    pub tray_running: bool,
    pub requests: Vec<TrayRequest>,
    pub start_with_windows: bool,
    pub settings_opened: usize,
    pub system_info_opened: usize,
    pub recording_settings_opened: usize,
    pub update_settings_opened: usize,
    pub library_opened: usize,
    /// Captures started straight away (from a capture panel that stays open).
    pub captures_started: Vec<&'static str>,
    pub trays_started: usize,
    /// Windows' own "Use the Print screen key to open screen capture".
    pub snipping_key: bool,
    /// There's an H.265 encoder for recordings.
    pub hevc: bool,
    /// What's missing for recording (Linux's GStreamer, say).
    pub recording_problem: Option<String>,
    /// The microphones that are connected.
    pub microphones: Vec<platform::SoundDevice>,
    /// The outputs whose sound can be recorded.
    pub outputs: Vec<platform::SoundDevice>,
    /// The cameras there are.
    pub cameras: Vec<platform::SoundDevice>,
    /// Looking for the outputs, microphones and cameras waits for the test to answer (on
    /// `sound_device_lookups`), as a slow sound server would have it.
    pub hold_sound_devices: bool,
    /// Each time the outputs, microphones and cameras were looked for, with where to answer.
    pub sound_device_lookups: Vec<Sender<SoundDeviceLists>>,
    /// Why the camera can't be used (`None`: it shows a test pattern).
    pub camera_error: Option<String>,
    /// Each camera opened, with where to send more of what it sends.
    pub cameras_opened: Vec<(Option<String>, SyncSender<super::camera::Event>)>,
    /// Each camera bubble shown by itself (by the capture panel).
    pub bubbles_shown: Vec<super::camera::Launch>,
    pub latest: Option<Release>,
    /// The text "read" from each file name, for search.
    pub ocr_text: Arc<std::sync::Mutex<BTreeMap<String, String>>>,
    /// The words "read" from any image, for the Copy text tool.
    pub words: Arc<std::sync::Mutex<Vec<Word>>>,
    /// What System info reports about the machine.
    pub host: Host,
    /// The folder "picked" in the folder chooser (`None`: cancelled).
    pub picked_folder: Option<PathBuf>,
    /// The file "picked" in the file chooser (`None`: cancelled).
    pub picked_file: Option<PathBuf>,
    /// Updates can be installed from Settings.
    pub can_install: bool,
    /// Times RSnip was asked to restart into an update it installed.
    pub restarts: usize,
    /// The text last put on the clipboard.
    pub copied: Option<String>,
    /// The files last put on the clipboard.
    pub copied_files: Vec<PathBuf>,
    /// Files, folders and links opened in other apps.
    pub opened: Vec<PathBuf>,
    /// Why opening a link in the browser failed (`None`: it opened).
    pub open_error: Option<String>,
    /// Files shown in their folder in the file manager.
    pub shown_in_folder: Vec<PathBuf>,
    /// Where the desktop's screenshot tool saves (Linux), if anywhere.
    pub desktop_screenshots: Option<PathBuf>,
    /// The secrets saved for destinations, by id.
    pub secrets: BTreeMap<String, String>,
    /// What testing a destination finds (`None`: it connects).
    pub test_result: Option<Result<String, String>>,
    /// The fingerprint an FTPS server's certificate has (`None`: a made-up one).
    pub certificate: Option<Result<String, String>>,
    /// The refresh token a code pasted back after signing in gives (Dropbox's; `None`: one made
    /// from the code).
    pub pasted_code_token: Option<Result<String, String>>,
    /// Each sign-in with a code shown in Settings: what to, where to say how it's going (the test
    /// does), and whether it was cancelled.
    pub code_sign_ins: Vec<(Kind, Sender<CodeEvent>, Arc<AtomicBool>)>,
    /// This build has no app of RSnip's to sign in to Google Drive or OneDrive with.
    pub without_sign_in_apps: bool,
    /// Files pushed, with the destination's name.
    pub pushed: Vec<(String, PathBuf)>,
    /// What pushing a file gives (`None`: a link ending in its name).
    pub push_result: Option<Result<Option<String>, String>>,
    /// The models the provider lists for describing screenshots (`None`: its default one).
    pub models: Option<Result<Vec<Model>, String>>,
    /// What describing the sample picture gives (`None`: a description saying who wrote it).
    pub sample_description: Option<Result<Described, String>>,
    /// The library being described by hand, until the test says how it went.
    pub describing: Option<Describing>,
    /// The desktop, as `$XDG_CURRENT_DESKTOP` names it (Linux).
    pub current_desktop: String,
    /// GNOME's settings, as `gsettings` would change them (for its keyboard shortcuts).
    pub gsettings: platform::gnome_keys::Fake,
}

/// Describing the library, in the sandbox: what it was asked for, and how to say how it's going.
#[cfg_attr(not(test), allow(dead_code))]
pub struct Describing {
    pub provider: Provider,
    pub model: String,
    pub files: Vec<PathBuf>,
    pub stop: Arc<AtomicBool>,
    pub events: Sender<Event>,
}

enum Store {
    /// The registry on Windows, `settings.json` elsewhere.
    Platform,
    /// Numbers, and text.
    #[cfg_attr(not(test), allow(dead_code))]
    Memory(RefCell<BTreeMap<String, u32>>, RefCell<BTreeMap<String, String>>),
}

impl Env {
    pub fn real() -> Env {
        Env {
            dir: platform::screenshots_dir(),
            request_file: request_file(),
            check_updates: cfg!(not(windows)) && crate::options::CHECK_UPDATES.read(platform::setting),
            tray_app: cfg!(windows),
            scroll_capture: scrolls(),
            version: Version::current(),
            play_sound: true,
            ocr: platform::ocr(),
            read_words: platform::read_words(),
            crash_dir: crate::crash::dir(),
            history_file: crate::push::history::file(),
            settings_request_file: platform::request_path("Settings"),
            settings: Store::Platform,
            sandbox: None,
        }
    }

    /// A sandbox in `dir`, with settings kept in memory, no network access, and a running tray app.
    #[cfg(test)]
    pub fn sandbox(dir: PathBuf) -> Env {
        let sandbox = Sandbox { tray_running: true, host: super::system_info::example_host(&dir), ..Default::default() };
        let words = sandbox.words.clone();
        let read_words: ReadWords = Arc::new(move |_| Ok(words.lock().unwrap().clone()));
        let texts = sandbox.ocr_text.clone();
        let ocr: Ocr = Arc::new(move |path| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            Ok(texts.lock().unwrap().get(name.as_ref()).cloned().unwrap_or_default())
        });
        Env {
            request_file: dir.join("request.txt"),
            crash_dir: dir.join("crashes"),
            history_file: dir.join("push-history.jsonl"),
            settings_request_file: dir.join("settings-request.txt"),
            dir,
            check_updates: false,
            tray_app: cfg!(windows),
            scroll_capture: cfg!(windows),
            version: Version::new(1, 2, 3),
            play_sound: false,
            ocr: Some(ocr),
            read_words: Some(read_words),
            settings: Store::Memory(RefCell::default(), RefCell::default()),
            sandbox: Some(RefCell::new(sandbox)),
        }
    }

    /// What happened in the sandbox (tests only).
    #[cfg(test)]
    pub fn sandboxed(&self) -> std::cell::RefMut<'_, Sandbox> {
        self.sandbox.as_ref().expect("a sandbox").borrow_mut()
    }

    pub fn setting(&self, name: &str) -> Option<u32> {
        match &self.settings {
            Store::Platform => platform::setting(name),
            Store::Memory(values, _) => values.borrow().get(name).copied(),
        }
    }

    pub fn set_setting(&self, name: &str, value: u32) {
        match &self.settings {
            Store::Platform => platform::set_setting(name, value),
            Store::Memory(values, _) => {
                values.borrow_mut().insert(name.to_owned(), value);
            }
        }
    }

    pub fn text_setting(&self, name: &str) -> Option<String> {
        match &self.settings {
            Store::Platform => platform::text_setting(name),
            Store::Memory(_, texts) => texts.borrow().get(name).cloned(),
        }
    }

    pub fn set_text_setting(&self, name: &str, value: &str) {
        match &self.settings {
            Store::Platform => platform::set_text_setting(name, value),
            Store::Memory(_, texts) => {
                texts.borrow_mut().insert(name.to_owned(), value.to_owned());
            }
        }
    }

    /// Follows a change of the folder screenshots are saved to, made in Settings; true if it moved.
    pub fn refresh_dir(&mut self) -> bool {
        let dir = match &self.settings {
            Store::Platform => platform::screenshots_dir(),
            Store::Memory(..) => match self.text_setting(crate::options::SAVE_FOLDER) {
                Some(dir) => dir.into(),
                None => return false,
            },
        };
        let moved = dir != self.dir;
        self.dir = dir;
        moved
    }

    /// The folder screenshots were saved to before it was changed in Settings, while it isn't the
    /// current one: as remembered then, else (a change made before RSnip remembered it) the one it
    /// saved to before there was a choice, if it did.
    pub fn previous_folder(&self) -> Option<PathBuf> {
        let remembered = self.text_setting(crate::options::PREVIOUS_SAVE_FOLDER).filter(|d| !d.trim().is_empty()).map(PathBuf::from);
        let previous = match (&self.sandbox, remembered) {
            (_, Some(dir)) => Some(dir),
            (Some(_), None) => None,
            (None, None) => Some(platform::legacy_save_folder()).filter(|d| crate::options::saved_to_by_rsnip(d)),
        };
        previous.filter(|d| *d != self.dir)
    }

    /// Where the desktop's own screenshot tool saves, when screenshots come from it (Linux).
    pub fn desktop_screenshots(&self) -> Option<PathBuf> {
        match &self.sandbox {
            Some(s) => s.borrow().desktop_screenshots.clone(),
            None if cfg!(windows) => None,
            None => Some(platform::legacy_save_folder()),
        }
    }

    /// Moves new screenshots from the desktop's screenshot tool into this folder, if it's another
    /// (Linux; see `bring_in`); how many moved.
    #[cfg(not(windows))]
    pub fn bring_in_screenshots(&self) -> usize {
        let Some(from) = self.desktop_screenshots() else { return 0 };
        super::bring_in::run(&from, &self.dir, |name| self.setting(name), |name, value| self.set_setting(name, value))
    }

    /// Moves the screenshots and recordings in `from` to `to`, with their edits and search data;
    /// the report arrives on the channel, and `ctx` repaints when it does.
    pub fn move_library(&self, ctx: &eframe::egui::Context, from: &Path, to: &Path) -> Receiver<Result<MoveReport, String>> {
        let (tx, rx) = channel();
        let (from, to) = (from.to_owned(), to.to_owned());
        if self.sandbox.is_some() {
            let _ = tx.send(super::move_library(&from, &to));
        } else {
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(super::move_library(&from, &to));
                ctx.request_repaint();
            });
        }
        rx
    }

    /// Asks for a folder with the system's folder chooser, starting in `current`; the answer
    /// (`None` if it was cancelled) arrives on the channel, and `ctx` repaints when it does.
    pub fn pick_folder(&self, ctx: &eframe::egui::Context, title: &str, current: &Path) -> PickedFolder {
        let (tx, rx) = channel();
        match &self.sandbox {
            Some(s) => {
                let _ = tx.send(Ok(s.borrow().picked_folder.clone()));
            }
            None => {
                let (ctx, title, current) = (ctx.clone(), title.to_owned(), current.to_owned());
                std::thread::spawn(move || {
                    let _ = tx.send(platform::pick_folder(&title, &current));
                    ctx.request_repaint();
                });
            }
        }
        rx
    }

    /// Asks for a file with the system's file chooser, offering those matching `filter` (a name
    /// and a pattern like "*.sxcu"); the answer arrives as `pick_folder`'s does.
    pub fn pick_file(&self, ctx: &eframe::egui::Context, title: &str, filter: (&'static str, &'static str)) -> PickedFolder {
        let (tx, rx) = channel();
        match &self.sandbox {
            Some(s) => {
                let _ = tx.send(Ok(s.borrow().picked_file.clone()));
            }
            None => {
                let (ctx, title) = (ctx.clone(), title.to_owned());
                std::thread::spawn(move || {
                    let _ = tx.send(platform::pick_file(&title, filter));
                    ctx.request_repaint();
                });
            }
        }
        rx
    }

    pub fn start_with_windows(&self) -> bool {
        match &self.sandbox {
            Some(s) => s.borrow().start_with_windows,
            None => platform::start_with_windows(),
        }
    }

    /// False if it couldn't be changed.
    pub fn set_start_with_windows(&self, on: bool) -> bool {
        match &self.sandbox {
            Some(s) => {
                s.borrow_mut().start_with_windows = on;
                true
            }
            None => platform::set_start_with_windows(on),
        }
    }

    pub fn tray_running(&self) -> bool {
        match &self.sandbox {
            Some(s) => s.borrow().tray_running,
            None => platform::tray_running(),
        }
    }

    pub fn tell_tray(&self, request: TrayRequest) {
        match &self.sandbox {
            Some(s) => s.borrow_mut().requests.push(request),
            None => platform::tell_tray(request),
        }
    }

    pub fn start_tray(&self) {
        match &self.sandbox {
            Some(s) => s.borrow_mut().trays_started += 1,
            None => platform::start_tray(),
        }
    }

    pub fn open_settings(&self) {
        match &self.sandbox {
            Some(s) => s.borrow_mut().settings_opened += 1,
            None => platform::open_settings(false),
        }
    }

    /// Opens the library window.
    pub fn open_library(&self) {
        match &self.sandbox {
            Some(s) => s.borrow_mut().library_opened += 1,
            None => platform::open_library(),
        }
    }

    /// Runs a capture or recording (`--region`, `--record`, ...) in the tray app.
    pub fn start_capture(&self, flag: &'static str) {
        match &self.sandbox {
            Some(s) => s.borrow_mut().captures_started.push(flag),
            None => platform::start_capture(flag),
        }
    }

    /// Opens Settings on Recording (from the capture panel).
    pub fn open_recording_settings(&self) {
        match &self.sandbox {
            Some(s) => s.borrow_mut().recording_settings_opened += 1,
            None => platform::open_recording_settings(),
        }
    }

    /// Opens Settings on Updates, checking for one (from the library's "is available" link, where
    /// this copy can install it).
    pub fn open_update_settings(&self) {
        match &self.sandbox {
            Some(s) => s.borrow_mut().update_settings_opened += 1,
            None => platform::open_update_settings(),
        }
    }

    pub fn open_system_info(&self) {
        match &self.sandbox {
            Some(s) => s.borrow_mut().system_info_opened += 1,
            None => platform::open_settings(true),
        }
    }

    /// The desktop, as `$XDG_CURRENT_DESKTOP` names it (`ubuntu:GNOME`), for Settings → Shortcuts.
    pub fn current_desktop(&self) -> String {
        match &self.sandbox {
            Some(s) => s.borrow().current_desktop.clone(),
            None => std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
        }
    }

    /// Runs `gsettings` with `args`, to read and change GNOME's keyboard shortcuts (see
    /// `gnome_keys`): what it printed, or why it failed.
    pub fn gsettings(&self, args: &[&str]) -> Result<String, String> {
        match &self.sandbox {
            Some(s) => s.borrow_mut().gsettings.run(args),
            None => real_gsettings(args),
        }
    }

    /// The program a keyboard shortcut runs: this one (see `platform::program`), or the package's
    /// in the sandbox.
    pub fn program(&self) -> String {
        if self.sandbox.is_some() {
            return "/usr/bin/rsnip".to_owned();
        }
        #[cfg(not(windows))]
        return platform::program().to_string_lossy().into_owned();
        #[cfg(windows)]
        return std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    }

    /// Runs `work` with `gsettings` (as `Env::gsettings` runs it) and the program shortcuts run, to
    /// read or change GNOME's keyboard shortcuts (see `gnome_keys`). It's done on a thread, as
    /// that runs `gsettings` several times over, which could be slow; the answer arrives on the
    /// channel, and `ctx` repaints when it does.
    pub fn with_gsettings<T: Send + 'static>(&self, ctx: &eframe::egui::Context, work: impl FnOnce(platform::gnome_keys::Run, &str) -> T + Send + 'static) -> Receiver<T> {
        let (tx, rx) = channel();
        let program = self.program();
        if self.sandbox.is_some() {
            let _ = tx.send(work(&|args| self.gsettings(args), &program));
        } else {
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(work(&real_gsettings, &program));
                ctx.request_repaint();
            });
        }
        rx
    }

    /// Windows itself opens its screen capture on Print Screen, which can get in the way.
    pub fn snipping_key(&self) -> bool {
        match &self.sandbox {
            Some(s) => s.borrow().snipping_key,
            None => platform::snipping_key(),
        }
    }

    /// Whether recordings can be H.265 here, and what's missing for recording, if anything. It's
    /// found on a thread, as GStreamer can take a while to say (Linux), and `ctx` repaints when it
    /// has.
    pub fn recording_support(&self, ctx: &eframe::egui::Context) -> Receiver<RecordingSupport> {
        let (tx, rx) = channel();
        match &self.sandbox {
            Some(s) => {
                let s = s.borrow();
                let _ = tx.send(RecordingSupport { hevc: s.hevc, problems: [s.recording_problem.clone(), s.recording_problem.clone()] });
            }
            None => {
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(RecordingSupport { hevc: platform::hevc_available(), problems: [false, true].map(platform::recording_problem) });
                    ctx.request_repaint();
                });
            }
        }
        rx
    }

    /// Looks for the outputs whose sound can be recorded, the microphones that are connected and
    /// the cameras. It's done on a thread, as a sound server can be slow to answer (or not answer,
    /// see `recorder::pactl`), and `ctx` repaints when they're found.
    pub fn sound_devices(&self, ctx: &eframe::egui::Context) -> Receiver<SoundDeviceLists> {
        let (tx, rx) = channel();
        match &self.sandbox {
            Some(s) => {
                let mut s = s.borrow_mut();
                if !s.hold_sound_devices {
                    let _ = tx.send(SoundDeviceLists { outputs: s.outputs.clone(), microphones: s.microphones.clone(), cameras: s.cameras.clone() });
                }
                s.sound_device_lookups.push(tx);
            }
            None => {
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(SoundDeviceLists { outputs: platform::outputs(), microphones: platform::microphones(), cameras: super::camera::cameras() });
                    ctx.request_repaint();
                });
            }
        }
        rx
    }

    /// Starts the camera `device` (the first one if `None`) for the camera's bubble: its pictures
    /// come on the channel, and `ctx` repaints for each (see `camera::open`).
    pub fn open_camera(&self, device: Option<String>, ctx: &eframe::egui::Context) -> Receiver<super::camera::Event> {
        match &self.sandbox {
            Some(s) => {
                let mut s = s.borrow_mut();
                let (tx, rx) = sync_channel(2);
                let _ = tx.send(match &s.camera_error {
                    Some(why) => super::camera::Event::Failed(why.clone()),
                    None => super::camera::Event::Picture(super::camera::test_pattern(120)),
                });
                s.cameras_opened.push((device, tx));
                rx
            }
            None => super::camera::open(device, Some(ctx.clone())),
        }
    }

    /// Shows the camera's bubble in a process of its own, as `launch` says; `ctx` repaints when it
    /// says something (see `camera::Bubble::touched`).
    pub fn show_bubble(&self, launch: &super::camera::Launch, ctx: &eframe::egui::Context) -> super::camera::Bubble {
        match &self.sandbox {
            Some(s) => {
                s.borrow_mut().bubbles_shown.push(launch.clone());
                super::camera::Bubble::stand_in()
            }
            None => super::camera::Bubble::start_with(launch, Some(ctx.clone())),
        }
    }

    /// Asks for the latest release, if it's newer than this build (see `update::check`); the
    /// answer arrives on the channel, and `ctx` repaints when it does.
    pub fn check_for_update(&self, ctx: &eframe::egui::Context) -> Receiver<Result<Option<Release>, String>> {
        let (tx, rx) = channel();
        match &self.sandbox {
            Some(s) => {
                let _ = tx.send(Ok(s.borrow().latest.clone()));
            }
            None => {
                let ctx = ctx.clone();
                let release_candidates = crate::options::RELEASE_CANDIDATES.read(|n| self.setting(n));
                std::thread::spawn(move || {
                    let _ = tx.send(crate::update::check(release_candidates));
                    ctx.request_repaint();
                });
            }
        }
        rx
    }

    /// The facts about this machine that System info reports.
    pub fn host(&self) -> Host {
        match &self.sandbox {
            Some(s) => s.borrow().host.clone(),
            None => platform::host(),
        }
    }

    pub fn copy_text(&self, text: &str) -> Result<(), String> {
        match &self.sandbox {
            Some(s) => {
                s.borrow_mut().copied = Some(text.to_owned());
                Ok(())
            }
            None => platform::copy_text(text),
        }
    }

    /// Puts files on the clipboard, to paste into a folder, chat or email.
    pub fn copy_files(&self, paths: &[PathBuf]) -> Result<(), String> {
        match &self.sandbox {
            Some(s) => {
                s.borrow_mut().copied_files = paths.to_vec();
                Ok(())
            }
            None => platform::copy_files(paths),
        }
    }

    /// Opens a file or folder in its app, or a link in the browser.
    pub fn open(&self, path: &Path) {
        match &self.sandbox {
            Some(s) => s.borrow_mut().opened.push(path.to_owned()),
            None => platform::open(path),
        }
    }

    /// Opens a link in the browser as `open` does; whether it could arrives on the channel, and
    /// `ctx` repaints when it does.
    pub fn open_link(&self, ctx: &eframe::egui::Context, path: &Path) -> Receiver<Result<(), String>> {
        let (tx, rx) = channel();
        match &self.sandbox {
            Some(s) => {
                s.borrow_mut().opened.push(path.to_owned());
                let _ = tx.send(s.borrow().open_error.clone().map_or(Ok(()), Err));
            }
            None => {
                let ctx = ctx.clone();
                platform::open_then(path, move |opened| {
                    let _ = tx.send(opened);
                    ctx.request_repaint();
                });
            }
        }
        rx
    }

    /// Opens the file manager on `path`'s folder, with it selected.
    pub fn show_in_folder(&self, path: &Path) {
        match &self.sandbox {
            Some(s) => s.borrow_mut().shown_in_folder.push(path.to_owned()),
            None => platform::show_in_folder(path),
        }
    }

    /// The secret saved in the system's credential store for the destination `id`.
    pub fn secret(&self, id: &str) -> Result<Option<String>, String> {
        match &self.sandbox {
            Some(s) => Ok(s.borrow().secrets.get(id).cloned()),
            None => crate::secrets::get(id),
        }
    }

    /// Saves the secret for the destination `id`, replacing any; `label` names it for the user.
    pub fn set_secret(&self, id: &str, label: &str, secret: &str) -> Result<(), String> {
        match &self.sandbox {
            Some(s) => {
                s.borrow_mut().secrets.insert(id.to_owned(), secret.to_owned());
                Ok(())
            }
            None => crate::secrets::set(id, label, secret),
        }
    }

    /// Forgets the secret for the destination `id`.
    pub fn delete_secret(&self, id: &str) -> Result<(), String> {
        match &self.sandbox {
            Some(s) => {
                s.borrow_mut().secrets.remove(id);
                Ok(())
            }
            None => crate::secrets::delete(id),
        }
    }

    /// Saves a secret as `set_secret` does, but off the UI thread, as the keyring can wait for the
    /// user to unlock it; the answer arrives on the channel, and `ctx` repaints when it does.
    pub fn save_secret(&self, ctx: &eframe::egui::Context, id: &str, label: &str, secret: &str) -> Receiver<Result<(), String>> {
        let (tx, rx) = channel();
        if self.sandbox.is_some() {
            let _ = tx.send(self.set_secret(id, label, secret));
        } else {
            let (ctx, id, label, secret) = (ctx.clone(), id.to_owned(), label.to_owned(), secret.to_owned());
            std::thread::spawn(move || {
                let _ = tx.send(crate::secrets::set(&id, &label, &secret));
                ctx.request_repaint();
            });
        }
        rx
    }

    /// Forgets a secret as `delete_secret` does, but off the UI thread (see `save_secret`).
    pub fn forget_secret(&self, ctx: &eframe::egui::Context, id: &str) -> Receiver<Result<(), String>> {
        let (tx, rx) = channel();
        if self.sandbox.is_some() {
            let _ = tx.send(self.delete_secret(id));
        } else {
            let (ctx, id) = (ctx.clone(), id.to_owned());
            std::thread::spawn(move || {
                let _ = tx.send(crate::secrets::delete(&id));
                ctx.request_repaint();
            });
        }
        rx
    }

    /// Pushes `files` to `destination` one after another, noting each in its search data (see
    /// `search::record_push`); how it goes arrives on the channel, and `ctx` repaints as it does.
    pub fn push_files(&self, ctx: &eframe::egui::Context, destination: &Destination, files: Vec<PathBuf>) -> Receiver<PushEvent> {
        let (tx, rx) = channel();
        match &self.sandbox {
            Some(s) => {
                for (i, file) in files.into_iter().enumerate() {
                    let _ = tx.send(PushEvent::Pushing(i));
                    s.borrow_mut().pushed.push((destination.name.clone(), file.clone()));
                    let name = file.file_name().unwrap_or_default().to_string_lossy();
                    let result = s.borrow().push_result.clone().unwrap_or_else(|| Ok(Some(format!("https://example.com/{name}"))));
                    let _ = tx.send(outcome(destination, &self.history_file, file, result));
                }
                let _ = tx.send(PushEvent::Done);
            }
            None => {
                let (ctx, destination, history) = (ctx.clone(), destination.clone(), self.history_file.clone());
                std::thread::spawn(move || {
                    // The keyring may ask to be unlocked first, here rather than on the UI thread.
                    let mut saved = saved_secret(&destination, None);
                    for (i, file) in files.into_iter().enumerate() {
                        let _ = tx.send(PushEvent::Pushing(i));
                        ctx.request_repaint();
                        // One sign-in for all of them, so a refresh token replaced by one push is the one the next uses.
                        let result = match &mut saved {
                            Ok(saved) => crate::push::push(&destination, saved, &file),
                            Err(e) => Err(e.clone()),
                        };
                        let _ = tx.send(outcome(&destination, &history, file, result));
                    }
                    let _ = tx.send(PushEvent::Done);
                    ctx.request_repaint();
                });
            }
        }
        rx
    }

    /// Whether a key for describing screenshots with `provider` is saved; asked off the UI thread
    /// (see `save_secret`), and the answer arrives on the channel.
    pub fn describe_key_saved(&self, ctx: &eframe::egui::Context, provider: Provider) -> Receiver<Result<bool, String>> {
        let (tx, rx) = channel();
        if self.sandbox.is_some() {
            let _ = tx.send(self.secret(provider.secret_id()).map(|key| key.is_some()));
        } else {
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(crate::secrets::get(provider.secret_id()).map(|key| key.is_some_and(|key| !key.trim().is_empty())));
                ctx.request_repaint();
            });
        }
        rx
    }

    /// The models `provider` offers for describing screenshots, with the key saved for it.
    pub fn describe_models(&self, ctx: &eframe::egui::Context, provider: Provider) -> Receiver<Result<Vec<Model>, String>> {
        let (tx, rx) = channel();
        match &self.sandbox {
            Some(s) => {
                let _ = tx.send(s.borrow().models.clone().unwrap_or_else(|| Ok(vec![Model { id: provider.default_model().to_owned() }])));
            }
            None => {
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(saved_service(provider).and_then(|service| describe::models(&service).map_err(|p| p.message().to_owned())));
                    ctx.request_repaint();
                });
            }
        }
        rx
    }

    /// Describes a sample picture with `model`, to see that the key and model work.
    pub fn describe_sample(&self, ctx: &eframe::egui::Context, provider: Provider, model: &str) -> Receiver<Result<Described, String>> {
        let (tx, rx) = channel();
        match &self.sandbox {
            Some(s) => {
                let sample = s.borrow().sample_description.clone();
                let _ = tx.send(sample.unwrap_or_else(|| Ok(Described { text: format!("A sample, described by {model}"), model: model.to_owned() })));
            }
            None => {
                let (ctx, model) = (ctx.clone(), model.to_owned());
                std::thread::spawn(move || {
                    let _ = tx.send(saved_service(provider).and_then(|service| describe::describe_sample(&service, &model).map_err(|p| p.message().to_owned())));
                    ctx.request_repaint();
                });
            }
        }
        rx
    }

    /// Describes `files` one after another (see `describer::describe_all`) until they're done or
    /// `stop` is set; how it goes arrives on the channel, and `ctx` repaints as it does. In the
    /// sandbox it waits for the test to send the events (`Sandbox::describing`).
    pub fn describe_library(&self, ctx: &eframe::egui::Context, provider: Provider, model: &str, files: Vec<PathBuf>, stop: Arc<AtomicBool>) -> Receiver<Event> {
        let (tx, rx) = channel();
        match &self.sandbox {
            Some(s) => s.borrow_mut().describing = Some(Describing { provider, model: model.to_owned(), files, stop, events: tx }),
            None => {
                let (ctx, model) = (ctx.clone(), model.to_owned());
                std::thread::spawn(move || {
                    let service = match saved_service(provider) {
                        Ok(service) => service,
                        Err(e) => {
                            let _ = tx.send(Event::Finished(Some(e)));
                            ctx.request_repaint();
                            return;
                        }
                    };
                    let report = |event| {
                        let _ = tx.send(event);
                        ctx.request_repaint();
                    };
                    describer::describe_all(&files, &|file| describe::describe(&service, &model, file), &stop, &report);
                });
            }
        }
        rx
    }

    /// The SHA-256 fingerprint of an FTPS server's certificate (see `push::ftps_certificate`); the
    /// answer arrives on the channel, and `ctx` repaints when it does.
    pub fn fetch_certificate(&self, ctx: &eframe::egui::Context, host: &str, port: u16) -> Receiver<Result<String, String>> {
        let (tx, rx) = channel();
        match &self.sandbox {
            Some(s) => {
                let _ = tx.send(s.borrow().certificate.clone().unwrap_or_else(|| Ok("AB:CD:EF".to_owned())));
            }
            None => {
                let (ctx, host) = (ctx.clone(), host.to_owned());
                std::thread::spawn(move || {
                    let _ = tx.send(crate::push::ftps_certificate(&host, port));
                    ctx.request_repaint();
                });
            }
        }
        rx
    }

    /// Trades the code the service showed at the end of signing in, pasted back, for the refresh
    /// token (Dropbox's, see `push::Pkce`); the answer arrives on the channel, and `ctx` repaints
    /// when it does.
    pub fn finish_pasted_sign_in(&self, ctx: &eframe::egui::Context, pkce: &crate::push::Pkce, app_key: &str, code: &str) -> Receiver<Result<String, String>> {
        let (tx, rx) = channel();
        match &self.sandbox {
            Some(s) => {
                let _ = tx.send(s.borrow().pasted_code_token.clone().unwrap_or_else(|| Ok(format!("refresh-for-{}", code.trim()))));
            }
            None => {
                let (ctx, pkce, app_key, code) = (ctx.clone(), pkce.clone(), app_key.to_owned(), code.to_owned());
                std::thread::spawn(move || {
                    let _ = tx.send(pkce.exchange(&app_key, &code));
                    ctx.request_repaint();
                });
            }
        }
        rx
    }

    /// Whether this build can sign in to `kind` (see `push::can_sign_in`).
    pub fn can_sign_in(&self, kind: &Kind) -> bool {
        match &self.sandbox {
            Some(s) => !s.borrow().without_sign_in_apps,
            None => crate::push::can_sign_in(kind),
        }
    }

    /// Signs in to `kind` with a code shown in Settings (see `push::sign_in_with_code`), until it's
    /// done or `cancel` is set; how it's going arrives on the channel, and `ctx` repaints as it
    /// does. In the sandbox the test says how it's going (`Sandbox::code_sign_ins`).
    pub fn sign_in_with_code(&self, ctx: &eframe::egui::Context, kind: &Kind, cancel: Arc<AtomicBool>) -> Receiver<CodeEvent> {
        let (tx, rx) = channel();
        match &self.sandbox {
            Some(s) => s.borrow_mut().code_sign_ins.push((kind.clone(), tx, cancel)),
            None => {
                let (ctx, kind) = (ctx.clone(), kind.clone());
                std::thread::spawn(move || {
                    crate::push::sign_in_with_code(&kind, &cancel, |event| {
                        let _ = tx.send(event);
                        ctx.request_repaint();
                    });
                });
            }
        }
        rx
    }

    /// Checks that `destination` can be reached (see `push::test`), with `secret` if given, else the
    /// one saved for it; the answer arrives on the channel, and `ctx` repaints when it does.
    pub fn test_destination(&self, ctx: &eframe::egui::Context, destination: &Destination, secret: Option<String>) -> Receiver<Result<String, String>> {
        let (tx, rx) = channel();
        match &self.sandbox {
            Some(s) => {
                let _ = tx.send(s.borrow().test_result.clone().unwrap_or_else(|| Ok("Connected".to_owned())));
            }
            None => {
                let (ctx, destination) = (ctx.clone(), destination.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(saved_secret(&destination, secret).and_then(|mut saved| crate::push::test(&destination, &mut saved)));
                    ctx.request_repaint();
                });
            }
        }
        rx
    }

    /// This copy came from the installer, so it can update itself.
    pub fn installed(&self) -> bool {
        self.sandbox.is_none() && platform::installed()
    }

    /// An update can be installed from Settings: by the tray app, for a copy from the installer
    /// (Windows), or with the package manager, for one from the .deb or .rpm (Linux).
    pub fn can_install_update(&self) -> bool {
        match &self.sandbox {
            Some(s) => s.borrow().can_install,
            #[cfg(windows)]
            None => self.tray_app && platform::installed() && platform::tray_running(),
            #[cfg(not(windows))]
            None => platform::package().is_some(),
        }
    }

    /// Installs `release`; the answer arrives on the channel once it's installed (Linux) or handed
    /// to the tray app, which installs it and restarts RSnip (Windows).
    pub fn install_update(&self, ctx: &eframe::egui::Context, release: &Release) -> Receiver<Result<(), String>> {
        let (tx, rx) = channel();
        if self.sandbox.is_some() || cfg!(windows) {
            self.tell_tray(TrayRequest::InstallUpdate);
            let _ = tx.send(Ok(()));
            return rx;
        }
        #[cfg(not(windows))]
        if let Some(package) = platform::package() {
            let (ctx, release) = (ctx.clone(), release.clone());
            std::thread::spawn(move || {
                let _ = tx.send(crate::update::package::install(&release, package));
                ctx.request_repaint();
            });
        }
        #[cfg(windows)]
        let _ = (ctx, release);
        rx
    }

    /// Restarts the library into the update just installed (Linux; on Windows the installer does).
    pub fn restart_after_update(&self) {
        match &self.sandbox {
            Some(s) => s.borrow_mut().restarts += 1,
            #[cfg(not(windows))]
            None if platform::instance_running("Gallery") => {
                let _ = std::fs::write(&self.request_file, "--restart");
            }
            #[cfg(not(windows))]
            None => platform::start_updated_library(),
            #[cfg(windows)]
            None => {}
        }
    }
}

/// What pushing `file` to `destination` came to, noted in its search data if it went, and in the
/// push history either way.
fn outcome(destination: &Destination, history: &Path, file: PathBuf, result: Result<Option<String>, String>) -> PushEvent {
    let result = result.and_then(|link| super::search::record_push(&file, &destination.id, link.clone()).map(|()| link));
    crate::push::history::record(history, &crate::push::history::Entry::new(destination, &file, false, result.clone().err()));
    match result {
        Ok(link) => PushEvent::Pushed(file, link),
        Err(e) => PushEvent::Failed(file, e),
    }
}

/// The secret to use with `destination`: the one given (typed in and not saved yet, so nor is
/// what replaces it), else the one saved for it (which may wait for the keyring), or an error if
/// it needs one and has none.
fn saved_secret(destination: &Destination, given: Option<String>) -> Result<Saved, String> {
    match given {
        Some(secret) if destination.kind.secret_label().is_some() => Ok(Saved::unsaved(Some(secret))),
        _ => crate::push::saved_secret(destination).map(|secret| kept(destination, secret)),
    }
}

/// `secret`, read from the credential store for `destination`, with what replaces it kept there,
/// and what a push learns about the place kept in the list of destinations.
pub fn kept(destination: &Destination, secret: Option<String>) -> Saved {
    let (id, label) = (destination.id.clone(), destination.name.clone());
    let place = id.clone();
    Saved::new(secret, move |new| crate::secrets::set(&id, &label, new)).remembering(move |learned| {
        // Read again here, as Settings may have changed the list since.
        if let Some(json) = platform::text_setting(crate::options::DESTINATIONS).and_then(|json| crate::push::remember(&json, &place, learned)) {
            platform::set_text_setting(crate::options::DESTINATIONS, &json);
        }
    })
}

/// The provider, with the key saved for it in the credential store (which may wait for the keyring
/// to be unlocked), or what to do about there not being one.
fn saved_service(provider: Provider) -> Result<Service, String> {
    match crate::secrets::get(provider.secret_id())?.filter(|key| !key.trim().is_empty()) {
        Some(key) => Ok(Service::new(provider, &key)),
        None => Err(format!("Save your {} key first", provider.name())),
    }
}

/// `gsettings`, where there is one (see `platform::gsettings`).
fn real_gsettings(args: &[&str]) -> Result<String, String> {
    #[cfg(not(windows))]
    return platform::gsettings(args);
    #[cfg(windows)]
    return Err(format!("There's no gsettings on Windows to {}", args.join(" ")));
}

/// Where a second `--gallery` launch leaves its request for the running library: on Linux in this
/// user's own folder (see `runtime_dir`), as /tmp is shared.
pub fn request_file() -> PathBuf {
    #[cfg(not(windows))]
    return platform::runtime_dir().join("gallery-request.txt");
    #[cfg(windows)]
    return std::env::temp_dir().join("rsnip-gallery-request.txt");
}

/// Whether a window can be scrolled and captured here: on Windows, and on an X11 desktop (on
/// Wayland an app can't scroll another's windows).
fn scrolls() -> bool {
    #[cfg(not(windows))]
    return super::screenshot::x11();
    #[cfg(windows)]
    return true;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_keeps_secrets_in_memory() {
        let env = Env::sandbox(std::env::temp_dir());
        assert_eq!(env.secret("a"), Ok(None));
        env.set_secret("a", "Home server", "one").unwrap();
        env.set_secret("b", "Discord", "two").unwrap();
        env.set_secret("a", "Home server", "three").unwrap();
        assert_eq!(env.secret("a"), Ok(Some("three".to_owned())), "replaced");
        assert_eq!(env.secret("b"), Ok(Some("two".to_owned())));
        env.delete_secret("a").unwrap();
        env.delete_secret("a").unwrap();
        assert_eq!(env.secret("a"), Ok(None));
        assert_eq!(env.secret("b"), Ok(Some("two".to_owned())), "others stay");
    }
}
