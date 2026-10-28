//! Asking the desktop for a screen or window to record, through the screen-cast portal
//! (xdg-desktop-portal), which every Wayland desktop and most X11 ones have. It shows the desktop's
//! own picker (the one browsers use for screen sharing), and gives back a PipeWire stream of what
//! was picked, which GStreamer records (see `recorder`). Screenshots come through the screenshot
//! portal, and files and folders through the file chooser.

use std::collections::HashMap;
use std::os::fd::OwnedFd;

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{self, ObjectPath, OwnedObjectPath, OwnedValue, Value};

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SCREEN_CAST: &str = "org.freedesktop.portal.ScreenCast";

/// What the picker offers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sources {
    Screen,
    ScreenOrWindow,
}

/// A screen or window being shared with RSnip, until dropped.
pub struct ScreenCast {
    conn: Connection,
    session: OwnedObjectPath,
    /// The PipeWire stream to record.
    pub node: u32,
    /// Where the shared screen's top left is on the desktop (0, 0 if the desktop doesn't say).
    pub origin: (i32, i32),
    /// It's a window rather than a screen.
    pub window: bool,
}

/// What the portal answers a request with: whether it went ahead, and its results.
enum Response {
    Done(HashMap<String, OwnedValue>),
    Cancelled,
}

impl ScreenCast {
    /// Shows the desktop's picker; `None` if it was cancelled.
    pub fn start(sources: Sources) -> Result<Option<ScreenCast>, String> {
        let conn = Connection::session().map_err(|e| format!("Couldn't reach the desktop's session bus: {e}"))?;
        let cast = Proxy::new(&conn, PORTAL, PORTAL_PATH, SCREEN_CAST).map_err(portal_missing)?;
        let available: u32 = cast.get_property("AvailableSourceTypes").map_err(portal_missing)?;
        let cursor_modes: u32 = cast.get_property("AvailableCursorModes").unwrap_or(1);

        let mut options = HashMap::from([("session_handle_token", Value::from(token()))]);
        let Response::Done(results) = request(&conn, &cast, "CreateSession", &mut options, |c, o| c.call_method("CreateSession", &(o,)))? else {
            return Ok(None);
        };
        let session: String = take(results, "session_handle")?;
        let session = OwnedObjectPath::try_from(session).map_err(|e| e.to_string())?;
        let screen_cast = ScreenCast { conn, session, node: 0, origin: (0, 0), window: false };
        let path = ObjectPath::from(&screen_cast.session);

        let mut options = source_options(sources, available, cursor_modes);
        let selected = request(&screen_cast.conn, &cast, "SelectSources", &mut options, |c, o| c.call_method("SelectSources", &(&path, o)))?;
        if matches!(selected, Response::Cancelled) {
            return Ok(None);
        }
        let mut options = HashMap::new();
        let Response::Done(results) = request(&screen_cast.conn, &cast, "Start", &mut options, |c, o| c.call_method("Start", &(&path, "", o)))? else {
            return Ok(None);
        };
        let streams: Vec<(u32, HashMap<String, OwnedValue>)> = take(results, "streams")?;
        let mut screen_cast = screen_cast;
        let (node, properties) = streams.into_iter().next().ok_or("The desktop shared nothing to record")?;
        screen_cast.node = node;
        (screen_cast.origin, screen_cast.window) = placed(properties);
        Ok(Some(screen_cast))
    }

    /// A new connection to PipeWire for a pipeline to read the stream with (each GStreamer
    /// process needs its own).
    pub fn connect(&self) -> Result<OwnedFd, String> {
        let cast = Proxy::new(&self.conn, PORTAL, PORTAL_PATH, SCREEN_CAST).map_err(|e| e.to_string())?;
        let options: HashMap<&str, Value> = HashMap::new();
        let fd: zvariant::OwnedFd = cast
            .call("OpenPipeWireRemote", &(ObjectPath::from(&self.session), options))
            .map_err(|e| format!("Couldn't open the screen recording: {e}"))?;
        Ok(fd.into())
    }
}

impl Drop for ScreenCast {
    /// Ends the sharing (the desktop's "sharing your screen" indicator goes away).
    fn drop(&mut self) {
        if let Ok(session) = Proxy::new(&self.conn, PORTAL, self.session.as_str(), "org.freedesktop.portal.Session") {
            let _ = session.call_method("Close", &());
        }
    }
}

/// Where a shared stream is on the desktop, and whether it's a window, from its properties: a
/// screen's `position` (which only screens have), and its `source_type` (window 2).
fn placed(mut properties: HashMap<String, OwnedValue>) -> ((i32, i32), bool) {
    let origin = properties.remove("position").and_then(|p| <(i32, i32)>::try_from(p).ok()).unwrap_or((0, 0));
    let window = properties.remove("source_type").and_then(|t| u32::try_from(t).ok()) == Some(2);
    (origin, window)
}

/// SelectSources' options: what to offer of what the desktop can (`available`), and the pointer
/// drawn into the video where it can be (`cursor_modes`). The portal checks their types, which are
/// all unsigned (`u`) but `multiple`.
fn source_options(sources: Sources, available: u32, cursor_modes: u32) -> HashMap<&'static str, Value<'static>> {
    // Monitor 1, window 2. Pointer: hidden 1, embedded in the video 2.
    let wanted: u32 = match sources {
        Sources::Screen => 1,
        Sources::ScreenOrWindow => 3,
    } & available;
    let cursor: u32 = if cursor_modes & 2 != 0 { 2 } else { 1 };
    HashMap::from([("types", Value::from(wanted.max(1))), ("multiple", Value::from(false)), ("cursor_mode", Value::from(cursor))])
}

/// Asks the desktop to pick a folder, starting in `current`, with its own file chooser; `None` if
/// it was cancelled.
pub fn pick_folder(title: &str, current: &std::path::Path) -> Result<Option<std::path::PathBuf>, String> {
    use std::os::unix::ffi::OsStrExt;
    let conn = Connection::session().map_err(|e| format!("Couldn't reach the desktop's session bus: {e}"))?;
    let chooser = Proxy::new(&conn, PORTAL, PORTAL_PATH, "org.freedesktop.portal.FileChooser").map_err(|e| e.to_string())?;
    // The current folder as the portal takes it: bytes, ending in a zero.
    let mut start = current.as_os_str().as_bytes().to_vec();
    start.push(0);
    let options = HashMap::from([("directory", Value::from(true)), ("current_folder", Value::from(start))]);
    open_file(&conn, &chooser, title, options)
}

/// Asks the desktop to pick a file, offering those matching `filter` (a name and a pattern like
/// "*.sxcu"); `None` if it was cancelled.
pub fn pick_file(title: &str, (name, pattern): (&str, &str)) -> Result<Option<std::path::PathBuf>, String> {
    let conn = Connection::session().map_err(|e| format!("Couldn't reach the desktop's session bus: {e}"))?;
    let chooser = Proxy::new(&conn, PORTAL, PORTAL_PATH, "org.freedesktop.portal.FileChooser").map_err(|e| e.to_string())?;
    // a(sa(us)): each filter's name and its patterns, 0 meaning a glob.
    let filters = vec![(name.to_owned(), vec![(0u32, pattern.to_owned())])];
    open_file(&conn, &chooser, title, HashMap::from([("filters", Value::from(filters))]))
}

/// The file chooser's OpenFile, with `options`; the first thing picked.
fn open_file(conn: &Connection, chooser: &Proxy, title: &str, mut options: HashMap<&str, Value>) -> Result<Option<std::path::PathBuf>, String> {
    let Response::Done(results) = request(conn, chooser, "OpenFile", &mut options, |c, o| c.call_method("OpenFile", &("", title, o)))? else {
        return Ok(None);
    };
    let uris: Vec<String> = take(results, "uris")?;
    Ok(uris.first().and_then(|uri| file_path(uri)))
}

/// Asks the desktop to take a screenshot of everything on screen, through the screenshot portal,
/// and gives the file it saved it in (RSnip's to delete); `None` if it was declined. The first
/// time, GNOME asks whether RSnip may take screenshots.
pub fn screenshot() -> Result<Option<std::path::PathBuf>, String> {
    let conn = Connection::session().map_err(|e| format!("Couldn't reach the desktop's session bus: {e}"))?;
    let portal = Proxy::new(&conn, PORTAL, PORTAL_PATH, "org.freedesktop.portal.Screenshot").map_err(|e| e.to_string())?;
    // No picker of its own: RSnip's area picker comes next, on the screenshot.
    let mut options = HashMap::from([("interactive", Value::from(false)), ("modal", Value::from(false))]);
    let Response::Done(results) = request(&conn, &portal, "Screenshot", &mut options, |c, o| c.call_method("Screenshot", &("", o)))? else {
        return Ok(None);
    };
    let uri: String = take(results, "uri")?;
    file_path(&uri).map(Some).ok_or_else(|| format!("The desktop saved the screenshot somewhere RSnip can't read ({uri})"))
}

/// The path in a `file://` URI.
fn file_path(uri: &str) -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let encoded = uri.strip_prefix("file://")?.as_bytes();
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut i = 0;
    while i < encoded.len() {
        let hex = encoded.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (encoded[i], hex) {
            (b'%', Some(byte)) => {
                bytes.push(byte);
                i += 3;
            }
            (byte, _) => {
                bytes.push(byte);
                i += 1;
            }
        }
    }
    Some(std::ffi::OsString::from_vec(bytes).into())
}

fn portal_missing(e: zbus::Error) -> String {
    format!("This desktop has no screen-recording portal (xdg-desktop-portal), which recording needs ({e})")
}

/// Calls a portal method that answers later, on a Request object, and waits for the answer (the
/// picker waits for the user). `options` gets the token that names the Request.
fn request(
    conn: &Connection,
    cast: &Proxy,
    method: &str,
    options: &mut HashMap<&str, Value>,
    call: impl FnOnce(&Proxy, &HashMap<&str, Value>) -> zbus::Result<zbus::message::Message>,
) -> Result<Response, String> {
    let token = token();
    let sender = conn.unique_name().map(|n| n.trim_start_matches(':').replace('.', "_")).unwrap_or_default();
    // Listen before calling, so the answer can't be missed.
    let path = format!("{PORTAL_PATH}/request/{sender}/{token}");
    let request = Proxy::new(conn, PORTAL, path.as_str(), "org.freedesktop.portal.Request").map_err(|e| e.to_string())?;
    let mut responses = request.receive_signal("Response").map_err(|e| e.to_string())?;
    options.insert("handle_token", Value::from(token));
    call(cast, options).map_err(|e| format!("The desktop refused the request ({method}): {e}"))?;
    let message = responses.next().ok_or("The desktop stopped answering")?;
    let (code, results): (u32, HashMap<String, OwnedValue>) = message.body().deserialize().map_err(|e| e.to_string())?;
    match code {
        0 => Ok(Response::Done(results)),
        1 => Ok(Response::Cancelled),
        _ => Err(format!("The desktop couldn't do it ({method})")),
    }
}

/// A name for a request or session, different each time (the portal makes it part of their paths).
fn token() -> String {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    format!("rsnip{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

/// One of a request's results.
fn take<T: TryFrom<OwnedValue>>(mut results: HashMap<String, OwnedValue>, key: &str) -> Result<T, String> {
    results.remove(key).and_then(|v| T::try_from(v).ok()).ok_or_else(|| format!("The desktop's answer had no {key}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_where_the_shared_screen_is() {
        let value = |v: Value| OwnedValue::try_from(v).unwrap();
        let screen = HashMap::from([("position".to_string(), value(Value::from((1920, -200)))), ("size".to_string(), value(Value::from((1280, 1024)))), ("source_type".to_string(), value(Value::from(1u32)))]);
        assert_eq!(placed(screen), ((1920, -200), false));
        assert_eq!(placed(HashMap::from([("source_type".to_string(), value(Value::from(2u32)))])), ((0, 0), true), "a window, which has no place");
        assert_eq!(placed(HashMap::new()), ((0, 0), false));
    }

    /// The types the screen-cast portal's documentation gives, which GNOME's checks.
    #[test]
    fn options_have_the_types_the_portal_expects() {
        let options = source_options(Sources::ScreenOrWindow, 7, 7);
        let types: Vec<(&str, String)> = ["types", "multiple", "cursor_mode"].map(|k| (k, options[k].value_signature().to_string())).into();
        assert_eq!(types, [("types", "u".to_string()), ("multiple", "b".to_string()), ("cursor_mode", "u".to_string())]);
    }

    #[test]
    fn reads_the_folder_from_the_choosers_uri() {
        assert_eq!(file_path("file:///home/a%20b/RSnip%C3%A9").unwrap(), std::path::Path::new("/home/a b/RSnipé"));
        assert_eq!(file_path("file:///tmp/100%"), Some("/tmp/100%".into()), "a stray % stays");
        assert_eq!(file_path("https://example.com/x"), None);
    }

    #[test]
    fn offers_what_the_desktop_can() {
        let get = |o: &HashMap<&str, Value>, k: &str| u32::try_from(&o[k]).unwrap();
        let options = source_options(Sources::ScreenOrWindow, 1, 1);
        assert_eq!((get(&options, "types"), get(&options, "cursor_mode")), (1, 1), "screens only, pointer hidden");
        let options = source_options(Sources::Screen, 3, 7);
        assert_eq!((get(&options, "types"), get(&options, "cursor_mode")), (1, 2), "a screen, pointer drawn in");
    }
}
