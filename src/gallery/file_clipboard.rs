//! Copying files on Linux the way GNOME's Files does, so they paste into a file manager, the desktop
//! or a chat. Each app reads its own format: GNOME's Files, the desktop, Nemo and Caja
//! `x-special/gnome-copied-files`; Dolphin and most others `text/uri-list`; sandboxed apps (a snap
//! or Flatpak Firefox) a key for files handed over through the document portal; text boxes the
//! paths. RSnip owns the X11 clipboard with all of them, from a thread that serves pastes until
//! something else is copied (GNOME passes X11's clipboard on to Wayland apps).

use std::os::fd::{AsFd, OwnedFd};
use std::path::{Path, PathBuf};

use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, SelectionNotifyEvent, WindowClass, SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::Event;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::CURRENT_TIME;
use zbus::blocking::Connection;
use zbus::zvariant::{Fd, Value};

const GNOME: &str = "x-special/gnome-copied-files";
const URI_LIST: &str = "text/uri-list";
const PORTAL_TRANSFER: &str = "application/vnd.portal.filetransfer";
const PORTAL_FILES: &str = "application/vnd.portal.files";
const TEXT: [&str; 4] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "STRING"];

/// Whether copying through X11 reaches the apps: in an X11 session, and on GNOME, which passes X11's
/// clipboard on to Wayland apps. Elsewhere on Wayland (KDE, wlroots) arboard's `text/uri-list` is
/// used, which their file managers read.
pub fn usable() -> bool {
    let var = |name| std::env::var(name).unwrap_or_default();
    !var("DISPLAY").is_empty() && (var("WAYLAND_DISPLAY").is_empty() || var("XDG_CURRENT_DESKTOP").split(':').any(|d| d.eq_ignore_ascii_case("GNOME")))
}

/// Puts `paths` on the clipboard in every format, serving pastes from a thread from then on.
pub fn copy(paths: &[PathBuf]) -> Result<(), String> {
    let (portal, key) = match portal_transfer(paths) {
        Ok((conn, key)) => (Some(conn), Some(key)),
        Err(_) => (None, None),
    };
    let formats = formats(paths, key.as_deref());
    let s = |e: &dyn std::fmt::Display| e.to_string();
    let (conn, screen) = x11rb::connect(None).map_err(|e| format!("Couldn't reach the X server: {e}"))?;
    let root = conn.setup().roots[screen].root;
    let window = conn.generate_id().map_err(|e| s(&e))?;
    conn.create_window(0, window, root, 0, 0, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new()).map_err(|e| s(&e))?;
    let atom = |name: &str| -> Result<Atom, String> { Ok(conn.intern_atom(false, name.as_bytes()).map_err(|e| s(&e))?.reply().map_err(|e| s(&e))?.atom) };
    let (clipboard, targets) = (atom("CLIPBOARD")?, atom("TARGETS")?);
    let offered = formats.iter().map(|(name, data)| Ok((atom(name)?, data.clone()))).collect::<Result<Vec<_>, String>>()?;
    conn.set_selection_owner(window, clipboard, CURRENT_TIME).map_err(|e| s(&e))?;
    if conn.get_selection_owner(clipboard).map_err(|e| s(&e))?.reply().map_err(|e| s(&e))?.owner != window {
        return Err("Couldn't take over the clipboard".into());
    }
    conn.flush().map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        // Kept until the thread ends: the portal's transfer lasts as long as this connection.
        let _portal = portal;
        while let Ok(event) = conn.wait_for_event() {
            match event {
                Event::SelectionClear(e) if e.selection == clipboard => break,
                Event::SelectionRequest(e) => {
                    // Old clients ask with no property: the answer goes in one named after the target.
                    let property = if e.property == u32::from(AtomEnum::NONE) { e.target } else { e.property };
                    let answered = if e.target == targets {
                        let atoms: Vec<Atom> = std::iter::once(targets).chain(offered.iter().map(|(a, _)| *a)).collect();
                        conn.change_property32(PropMode::REPLACE, e.requestor, property, AtomEnum::ATOM, &atoms).is_ok()
                    } else if let Some((_, data)) = offered.iter().find(|(a, _)| *a == e.target) {
                        conn.change_property8(PropMode::REPLACE, e.requestor, property, e.target, data).is_ok()
                    } else {
                        false
                    };
                    let notify = SelectionNotifyEvent {
                        response_type: SELECTION_NOTIFY_EVENT,
                        sequence: 0,
                        time: e.time,
                        requestor: e.requestor,
                        selection: e.selection,
                        target: e.target,
                        property: if answered { property } else { AtomEnum::NONE.into() },
                    };
                    let _ = conn.send_event(false, e.requestor, EventMask::NO_EVENT, notify);
                    let _ = conn.flush();
                }
                _ => {}
            }
        }
    });
    Ok(())
}

/// Each format's name and what's pasted for it.
fn formats(paths: &[PathBuf], portal_key: Option<&str>) -> Vec<(String, Vec<u8>)> {
    let uris: Vec<String> = paths.iter().map(|p| super::platform::file_uri(p)).collect();
    let text = paths.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>().join("\n");
    let mut formats = vec![
        (GNOME.to_owned(), format!("copy\n{}", uris.join("\n")).into_bytes()),
        (URI_LIST.to_owned(), uris.iter().map(|u| format!("{u}\r\n")).collect::<String>().into_bytes()),
    ];
    if let Some(key) = portal_key {
        // As GTK writes it: the key with its terminating NUL.
        let key = [key.as_bytes(), &[0]].concat();
        formats.extend([(PORTAL_TRANSFER.to_owned(), key.clone()), (PORTAL_FILES.to_owned(), key)]);
    }
    formats.extend(TEXT.iter().map(|name| (name.to_string(), text.clone().into_bytes())));
    formats
}

/// Hands the files to the document portal for sandboxed apps: the transfer's key, and the
/// connection that keeps it open.
fn portal_transfer(paths: &[PathBuf]) -> Result<(Connection, String), String> {
    let conn = Connection::session().map_err(|e| e.to_string())?;
    let proxy = zbus::blocking::Proxy::new(&conn, "org.freedesktop.portal.Documents", "/org/freedesktop/portal/documents", "org.freedesktop.portal.FileTransfer")
        .map_err(|e| e.to_string())?;
    // Not stopped after the first paste, so pasting twice works.
    let options = std::collections::HashMap::from([("writable", Value::from(false)), ("autostop", Value::from(false))]);
    let key: String = proxy.call("StartTransfer", &(options,)).map_err(|e| e.to_string())?;
    let files = paths.iter().map(|p| open_path(p)).collect::<Result<Vec<OwnedFd>, String>>()?;
    let fds: Vec<Fd> = files.iter().map(|f| Fd::from(f.as_fd())).collect();
    let none = std::collections::HashMap::<&str, Value>::new();
    proxy.call::<_, _, ()>("AddFiles", &(key.as_str(), fds, none)).map_err(|e| e.to_string())?;
    Ok((conn, key))
}

/// The file opened only to name it (`O_PATH`), as the portal wants.
fn open_path(path: &Path) -> Result<OwnedFd, String> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::File::options().read(true).custom_flags(libc::O_PATH).open(path).map(OwnedFd::from).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offers_files_the_way_gnome_files_does() {
        let paths = [PathBuf::from("/home/me/RSnip/Shot 1.png"), PathBuf::from("/home/me/RSnip/b.mp4")];
        let formats = formats(&paths, Some("1234"));
        let get = |name: &str| String::from_utf8(formats.iter().find(|(n, _)| n == name).unwrap().1.clone()).unwrap();
        assert_eq!(get(GNOME), "copy\nfile:///home/me/RSnip/Shot%201.png\nfile:///home/me/RSnip/b.mp4");
        assert_eq!(get(URI_LIST), "file:///home/me/RSnip/Shot%201.png\r\nfile:///home/me/RSnip/b.mp4\r\n");
        assert_eq!(get(PORTAL_TRANSFER), "1234\0");
        assert_eq!(get("UTF8_STRING"), "/home/me/RSnip/Shot 1.png\n/home/me/RSnip/b.mp4");
        assert!(super::formats(&paths, None).iter().all(|(n, _)| !n.contains("portal")), "no key, no portal formats");
    }
}
