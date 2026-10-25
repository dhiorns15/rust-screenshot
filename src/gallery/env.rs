//! What the library and Settings windows read and change outside themselves: the screenshots
//! folder, the settings, the running tray app, and so on. Tests get a sandbox instead: a temporary
//! folder, in-memory settings, and a record of what would have been done, so they never touch the
//! real ones.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{channel, Receiver};

use image::RgbaImage;

use super::platform;
use super::system_info::Host;
use super::words::Word;
use crate::push::Destination;
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
    /// This build's version (a fixed one in the sandbox, so snapshots survive version bumps).
    pub version: Version,
    /// Text recognition for search, where the system has it (Windows, or Tesseract elsewhere).
    pub ocr: Option<Ocr>,
    /// The same, word by word, for copying text out of an image.
    pub read_words: Option<ReadWords>,
    /// Where crash reports are written (see `crash`).
    pub crash_dir: PathBuf,
    /// What was pushed where (see `push::history`).
    pub history_file: PathBuf,
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
    pub trays_started: usize,
    /// Windows' own "Use the Print screen key to open screen capture".
    pub snipping_key: bool,
    /// There's an H.265 encoder for recordings.
    pub hevc: bool,
    /// What's missing for recording (Linux's GStreamer, say).
    pub recording_problem: Option<String>,
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
    /// Files, folders and links opened in other apps.
    pub opened: Vec<PathBuf>,
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
    /// Files pushed, with the destination's name.
    pub pushed: Vec<(String, PathBuf)>,
    /// What pushing a file gives (`None`: a link ending in its name).
    pub push_result: Option<Result<Option<String>, String>>,
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
            version: Version::current(),
            ocr: platform::ocr(),
            read_words: platform::read_words(),
            crash_dir: crate::crash::dir(),
            history_file: crate::push::history::file(),
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
            dir,
            check_updates: false,
            tray_app: cfg!(windows),
            version: Version::new(1, 2, 3),
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

    pub fn open_system_info(&self) {
        match &self.sandbox {
            Some(s) => s.borrow_mut().system_info_opened += 1,
            None => platform::open_settings(true),
        }
    }

    /// Windows itself opens its screen capture on Print Screen, which can get in the way.
    pub fn snipping_key(&self) -> bool {
        match &self.sandbox {
            Some(s) => s.borrow().snipping_key,
            None => platform::snipping_key(),
        }
    }

    /// Recordings can be H.265 here (see `encode::hevc_available`).
    pub fn hevc_available(&self) -> bool {
        match &self.sandbox {
            Some(s) => s.borrow().hevc,
            None => platform::hevc_available(),
        }
    }

    /// What's missing for recording as `hevc` (H.265, else H.264) would have it, if anything.
    pub fn recording_problem(&self, hevc: bool) -> Option<String> {
        match &self.sandbox {
            Some(s) => s.borrow().recording_problem.clone(),
            None => platform::recording_problem(hevc),
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

    /// Opens a file or folder in its app, or a link in the browser.
    pub fn open(&self, path: &Path) {
        match &self.sandbox {
            Some(s) => s.borrow_mut().opened.push(path.to_owned()),
            None => platform::open(path),
        }
    }

    /// Opens the file manager on `path`'s folder, with it selected.
    pub fn show_in_folder(&self, path: &Path) {
        match &self.sandbox {
            Some(s) => s.borrow_mut().shown_in_folder.push(path.to_owned()),
            None => platform::show_in_folder(path),
        }
    }

    /// The secret saved in the system's credential store for the destination `id`.
    #[cfg_attr(not(test), allow(dead_code))]
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
                    let secret = saved_secret(&destination, None);
                    for (i, file) in files.into_iter().enumerate() {
                        let _ = tx.send(PushEvent::Pushing(i));
                        ctx.request_repaint();
                        let result = secret.clone().and_then(|secret| crate::push::push(&destination, secret.as_deref(), &file));
                        let _ = tx.send(outcome(&destination, &history, file, result));
                    }
                    let _ = tx.send(PushEvent::Done);
                    ctx.request_repaint();
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
                    let _ = tx.send(saved_secret(&destination, secret).and_then(|secret| crate::push::test(&destination, secret.as_deref())));
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

/// The secret to use with `destination`: the one given, else the one saved for it (which may
/// wait for the keyring), or an error if it needs one and has none.
fn saved_secret(destination: &Destination, given: Option<String>) -> Result<Option<String>, String> {
    match given {
        Some(secret) if destination.kind.secret_label().is_some() => Ok(Some(secret)),
        _ => crate::push::saved_secret(destination),
    }
}

/// Where a second `--gallery` launch leaves its request for the running library.
pub fn request_file() -> PathBuf {
    std::env::temp_dir().join("rsnip-gallery-request.txt")
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
