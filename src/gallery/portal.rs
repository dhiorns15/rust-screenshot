//! Asking the desktop for a screen or window to record, through the screen-cast portal
//! (xdg-desktop-portal), which every Wayland desktop and most X11 ones have. It shows the desktop's
//! own picker (the one browsers use for screen sharing), and gives back a PipeWire stream of what
//! was picked, which GStreamer records (see `recorder`).

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
        // Monitor 1, window 2. Pointer: hidden 1, embedded in the video 2.
        let wanted = match sources {
            Sources::Screen => 1,
            Sources::ScreenOrWindow => 3,
        } & available;
        let cursor_modes: u32 = cast.get_property("AvailableCursorModes").unwrap_or(1);
        let cursor = if cursor_modes & 2 != 0 { 2 } else { 1 };

        let mut options = HashMap::from([("session_handle_token", Value::from("rsnip"))]);
        let Response::Done(results) = request(&conn, &cast, "CreateSession", &mut options, |c, o| c.call_method("CreateSession", &(o,)))? else {
            return Ok(None);
        };
        let session: String = take(results, "session_handle")?;
        let session = OwnedObjectPath::try_from(session).map_err(|e| e.to_string())?;
        let screen_cast = ScreenCast { conn, session, node: 0 };
        let path = ObjectPath::from(&screen_cast.session);

        let mut options = HashMap::from([
            ("types", Value::from(wanted.max(1))),
            ("multiple", Value::from(false)),
            ("cursor_mode", Value::from(cursor)),
        ]);
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
        screen_cast.node = streams.first().map(|s| s.0).ok_or("The desktop shared nothing to record")?;
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
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let token = format!("rsnip{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let sender = conn.unique_name().map(|n| n.trim_start_matches(':').replace('.', "_")).unwrap_or_default();
    // Listen before calling, so the answer can't be missed.
    let path = format!("{PORTAL_PATH}/request/{sender}/{token}");
    let request = Proxy::new(conn, PORTAL, path.as_str(), "org.freedesktop.portal.Request").map_err(|e| e.to_string())?;
    let mut responses = request.receive_signal("Response").map_err(|e| e.to_string())?;
    options.insert("handle_token", Value::from(token));
    call(cast, options).map_err(|e| format!("The desktop refused to record ({method}): {e}"))?;
    let message = responses.next().ok_or("The desktop stopped answering")?;
    let (code, results): (u32, HashMap<String, OwnedValue>) = message.body().deserialize().map_err(|e| e.to_string())?;
    match code {
        0 => Ok(Response::Done(results)),
        1 => Ok(Response::Cancelled),
        _ => Err(format!("The desktop couldn't start recording ({method})")),
    }
}

/// One of a request's results.
fn take<T: TryFrom<OwnedValue>>(mut results: HashMap<String, OwnedValue>, key: &str) -> Result<T, String> {
    results.remove(key).and_then(|v| T::try_from(v).ok()).ok_or_else(|| format!("The desktop's answer had no {key}"))
}
