//! RSnip in the background on Linux: an icon in the top bar or panel (a StatusNotifierItem, which
//! Ubuntu's GNOME, KDE and most panels show) for opening the library, Settings and the screenshots
//! folder, that stays when the library is closed. `rsnip --tray` runs it, and the library starts
//! it (see `platform::start_tray`); `rsnip --tray-quit` or Settings stops it.

use std::path::PathBuf;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::time::Duration;

use ksni::blocking::TrayMethods;
use ksni::menu::{MenuItem, StandardItem};

use super::platform;

/// What the icon's menu asks for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Command {
    OpenLibrary,
    OpenFolder,
    Settings,
    Quit,
}

/// The file other RSnip processes (and installer/linux/install.sh) write a request to ("quit"),
/// checked twice a second.
pub fn request_file() -> PathBuf {
    let user = std::env::var("USER").unwrap_or_default();
    std::env::temp_dir().join(format!("rsnip-tray-{user}.request"))
}

/// Shows the icon until it's told to quit. Returns at once if it's already showing, or if this
/// desktop has nowhere to show it.
pub fn run() {
    // Turned off in Settings (the autostart entry may still start it).
    if !crate::options::RUN_IN_BACKGROUND.read(platform::setting) || !platform::claim_single_instance("Tray", "") {
        return;
    }
    let requests = request_file();
    let _ = std::fs::remove_file(&requests);
    let (tx, rx) = channel();
    let handle = match (Icon { tx }).spawn() {
        Ok(handle) => handle,
        Err(e) => {
            eprintln!("RSnip can't show an icon on this desktop ({e}). On GNOME, the AppIndicator extension adds them.");
            return;
        }
    };
    loop {
        let command = match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(command) => Some(command),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Command::Quit),
        };
        let requested = std::fs::read_to_string(&requests).ok().inspect(|_| {
            let _ = std::fs::remove_file(&requests);
        });
        let command = command.or(match requested.as_deref().map(str::trim) {
            Some("quit") => Some(Command::Quit),
            _ => None,
        });
        match command {
            Some(Command::Quit) => break,
            Some(Command::OpenLibrary) => platform::spawn_self(&[]),
            Some(Command::OpenFolder) => platform::open(&platform::screenshots_dir()),
            Some(Command::Settings) => platform::open_settings(false),
            None if handle.is_closed() => break,
            None => {}
        }
    }
    // Quit may have come from the menu: let its reply reach the panel first.
    std::thread::sleep(Duration::from_millis(200));
    handle.shutdown().wait();
}

/// Asks the running icon, if any, to go.
pub fn quit() {
    let _ = std::fs::write(request_file(), "quit");
}

struct Icon {
    tx: Sender<Command>,
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
        "RSnip".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        // ARGB, from the BGRA the art is drawn in.
        [22, 32, 48, 64]
            .into_iter()
            .map(|size| {
                let data = crate::icon_art::render(size).chunks_exact(4).flat_map(|p| [p[3], p[2], p[1], p[0]]).collect();
                ksni::Icon { width: size, height: size, data }
            })
            .collect()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: "RSnip".into(), description: "Click to open the library".into(), ..Default::default() }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send(Command::OpenLibrary);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            Icon::item("Open library", Command::OpenLibrary),
            Icon::item("Open screenshots folder", Command::OpenFolder),
            Icon::item("Settings", Command::Settings),
            MenuItem::Separator,
            Icon::item("Quit RSnip", Command::Quit),
        ]
    }
}
