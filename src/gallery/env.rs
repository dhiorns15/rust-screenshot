//! What the library and Settings windows read and change outside themselves: the screenshots
//! folder, the settings, the running tray app, and so on. Tests get a sandbox instead: a temporary
//! folder, in-memory settings, and a record of what would have been done, so they never touch the
//! real ones.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{channel, Receiver};

use super::platform;
use super::system_info::Host;
use crate::shortcuts::Action;
use crate::update::{Release, Version};

/// Reads the text in an image file (for search).
pub type Ocr = Arc<dyn Fn(&Path) -> Result<String, String> + Send + Sync>;

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
    /// Text recognition for search, where the system has it (Windows).
    pub ocr: Option<Ocr>,
    settings: Store,
    #[cfg_attr(not(test), allow(dead_code))]
    sandbox: Option<RefCell<Sandbox>>,
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
}

/// The outside world, as far as tests are concerned.
#[derive(Default)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct Sandbox {
    pub tray_running: bool,
    pub requests: Vec<TrayRequest>,
    pub start_with_windows: bool,
    pub settings_opened: usize,
    pub trays_started: usize,
    /// Windows' own "Use the Print screen key to open screen capture".
    pub snipping_key: bool,
    pub latest: Option<Release>,
    /// The text "read" from each file name, for search.
    pub ocr_text: Arc<std::sync::Mutex<BTreeMap<String, String>>>,
    /// What System info reports about the machine.
    pub host: Host,
    /// The text last put on the clipboard.
    pub copied: Option<String>,
    /// Links opened in the browser.
    pub opened: Vec<String>,
}

enum Store {
    /// The registry on Windows, `settings.json` elsewhere.
    Platform,
    #[cfg_attr(not(test), allow(dead_code))]
    Memory(RefCell<BTreeMap<String, u32>>),
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
            settings: Store::Platform,
            sandbox: None,
        }
    }

    /// A sandbox in `dir`, with settings kept in memory, no network access, and a running tray app.
    #[cfg(test)]
    pub fn sandbox(dir: PathBuf) -> Env {
        let sandbox = Sandbox { tray_running: true, host: super::system_info::example_host(&dir), ..Default::default() };
        let texts = sandbox.ocr_text.clone();
        let ocr: Ocr = Arc::new(move |path| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            Ok(texts.lock().unwrap().get(name.as_ref()).cloned().unwrap_or_default())
        });
        Env {
            request_file: dir.join("request.txt"),
            dir,
            check_updates: false,
            tray_app: cfg!(windows),
            version: Version(1, 2, 3),
            ocr: Some(ocr),
            settings: Store::Memory(RefCell::default()),
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
            Store::Memory(values) => values.borrow().get(name).copied(),
        }
    }

    pub fn set_setting(&self, name: &str, value: u32) {
        match &self.settings {
            Store::Platform => platform::set_setting(name, value),
            Store::Memory(values) => {
                values.borrow_mut().insert(name.to_owned(), value);
            }
        }
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
            None => platform::open_settings(),
        }
    }

    /// Windows itself opens its screen capture on Print Screen, which can get in the way.
    pub fn snipping_key(&self) -> bool {
        match &self.sandbox {
            Some(s) => s.borrow().snipping_key,
            None => platform::snipping_key(),
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
                std::thread::spawn(move || {
                    let _ = tx.send(crate::update::check());
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

    pub fn open_link(&self, url: &str) {
        match &self.sandbox {
            Some(s) => s.borrow_mut().opened.push(url.to_owned()),
            None => platform::open(Path::new(url)),
        }
    }

    /// This copy came from the installer, so it can update itself.
    pub fn installed(&self) -> bool {
        self.sandbox.is_none() && platform::installed()
    }
}

/// Where a second `--gallery` launch leaves its request for the running library.
pub fn request_file() -> PathBuf {
    std::env::temp_dir().join("rsnip-gallery-request.txt")
}
