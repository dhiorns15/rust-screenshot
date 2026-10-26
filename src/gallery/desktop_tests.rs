//! Recording, screenshots and the background icon against real desktop services: xdg-desktop-portal
//! (which checks everything an app asks of the desktop, and refused 0.5.0's recordings for a
//! mistyped option), PipeWire with WirePlumber, and GStreamer. Only the desktop's own pieces are
//! stand-ins: GNOME's screen-cast picker (a portal backend that shares a test picture through
//! PipeWire) and screenshot service (one that saves the test picture), the panel the icon goes in,
//! and notifications.
//!
//! They need a session bus and those services installed, so plain `cargo test` skips them;
//! ci/desktop-test.sh runs them.

use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use zbus::blocking::{connection, Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use super::portal::{self, ScreenCast, Sources};
use super::recorder::{self, Area, Format};
use super::screenshot::{self, Capture, Mode};

/// The stand-in picker's portal backend name, and the "desktop" it's for.
const BACKEND: &str = "org.freedesktop.impl.portal.desktop.rsnipTest";
const DESKTOP: &str = "rsnip-test";
/// The test picture's size.
const SCREEN: (u32, u32) = (640, 480);

/// What the stand-in picker does with the next request, and what it was asked.
#[derive(Default)]
struct Picker {
    cancel: bool,
    asked: Vec<HashMap<String, OwnedValue>>,
}

/// The stand-in for GNOME's screen-cast picker: it shares a test picture, or cancels.
struct ScreenCastBackend {
    picker: Arc<Mutex<Picker>>,
    /// Asks for a new test picture, and gets back its PipeWire node.
    screens: Mutex<Sender<Sender<u32>>>,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.ScreenCast")]
impl ScreenCastBackend {
    fn create_session(&self, _handle: OwnedObjectPath, _session: OwnedObjectPath, _app_id: String, _options: HashMap<String, OwnedValue>) -> (u32, HashMap<String, OwnedValue>) {
        (0, HashMap::new())
    }

    fn select_sources(&self, _handle: OwnedObjectPath, _session: OwnedObjectPath, _app_id: String, options: HashMap<String, OwnedValue>) -> (u32, HashMap<String, OwnedValue>) {
        self.picker.lock().unwrap().asked.push(options);
        (0, HashMap::new())
    }

    fn start(
        &self,
        _handle: OwnedObjectPath,
        _session: OwnedObjectPath,
        _app_id: String,
        _parent_window: String,
        _options: HashMap<String, OwnedValue>,
    ) -> (u32, HashMap<String, OwnedValue>) {
        if std::mem::take(&mut self.picker.lock().unwrap().cancel) {
            return (1, HashMap::new());
        }
        // A stream of its own each time, as GNOME starts one for each recording.
        let (tx, rx) = channel();
        self.screens.lock().unwrap().send(tx).unwrap();
        let node = rx.recv().unwrap();
        let props = HashMap::from([("size", Value::from((SCREEN.0 as i32, SCREEN.1 as i32))), ("source_type", Value::from(1u32))]);
        let streams = Value::from(vec![(node, props)]);
        (0, HashMap::from([("streams".to_string(), OwnedValue::try_from(streams).unwrap())]))
    }

    #[zbus(property)]
    fn available_source_types(&self) -> u32 {
        3
    }

    #[zbus(property)]
    fn available_cursor_modes(&self) -> u32 {
        3
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        5
    }
}

/// What the stand-in screenshot service does with the next request, and what it was asked.
#[derive(Default)]
struct Shutter {
    cancel: bool,
    /// Refuses without asking, as Ubuntu 24.04's GNOME does for an app started from its menu.
    refuse: bool,
    asked: Vec<HashMap<String, OwnedValue>>,
}

/// The stand-in for GNOME's screenshot service: it saves the test picture, cancels or refuses.
struct ScreenshotBackend {
    shutter: Arc<Mutex<Shutter>>,
}

/// The test screen as a picture, every part of it different, to tell where a crop came from.
fn screen_picture() -> image::RgbaImage {
    image::RgbaImage::from_fn(SCREEN.0, SCREEN.1, |x, y| image::Rgba([(x / 3) as u8, (y / 2) as u8, ((x + y) % 251) as u8, 255]))
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Screenshot")]
impl ScreenshotBackend {
    fn screenshot(&self, _handle: OwnedObjectPath, _app_id: String, _parent_window: String, options: HashMap<String, OwnedValue>) -> (u32, HashMap<String, OwnedValue>) {
        let mut shutter = self.shutter.lock().unwrap();
        shutter.asked.push(options);
        if std::mem::take(&mut shutter.cancel) {
            return (1, HashMap::new());
        }
        if std::mem::take(&mut shutter.refuse) {
            return (2, HashMap::new());
        }
        // Where GNOME's own tool saves: the folder `bring_in` watches.
        let folder = super::platform::legacy_save_folder();
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("Screenshot from 2026-10-24 09-15-00.png");
        screen_picture().save(&file).unwrap();
        let uri = Value::from(super::platform::file_uri(&file));
        (0, HashMap::from([("uri".to_string(), OwnedValue::try_from(uri).unwrap())]))
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        2
    }
}

/// The stand-in for the dialog that asks whether an app may take screenshots, which GNOME shows
/// the first time: the portal needs a service for it to offer screenshots at all. It's allowed.
struct AccessBackend;

#[zbus::interface(name = "org.freedesktop.impl.portal.Access")]
impl AccessBackend {
    #[allow(clippy::too_many_arguments)]
    fn access_dialog(
        &self,
        _handle: OwnedObjectPath,
        _app_id: String,
        _parent_window: String,
        _title: String,
        _subtitle: String,
        _body: String,
        _options: HashMap<String, OwnedValue>,
    ) -> (u32, HashMap<String, OwnedValue>) {
        (0, HashMap::new())
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }
}

/// The real services, started once for all these tests.
struct Desktop {
    picker: Arc<Mutex<Picker>>,
    shutter: Arc<Mutex<Shutter>>,
    /// Keeps the stand-in picker and screenshot service on the bus.
    _backend: Connection,
}

/// Starts PipeWire, WirePlumber, the test picture and xdg-desktop-portal with the stand-in picker.
fn desktop() -> &'static Desktop {
    static DESKTOP_SERVICES: OnceLock<Desktop> = OnceLock::new();
    DESKTOP_SERVICES.get_or_init(|| {
        assert!(std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some(), "run these with ci/desktop-test.sh, which starts a session bus");
        // A short path: PipeWire's socket path is limited to 108 bytes.
        let runtime = PathBuf::from(format!("/tmp/rsnip-desktop-{}", std::process::id()));
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::set_permissions(&runtime, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        // The services live as long as this thread: until the tests end.
        let (screens, requests) = channel::<Sender<u32>>();
        let dir = runtime.clone();
        std::thread::spawn(move || run_services(&dir, requests));
        let picker = Arc::new(Mutex::new(Picker::default()));
        let shutter = Arc::new(Mutex::new(Shutter::default()));
        let backend = connection::Builder::session()
            .unwrap()
            .name(BACKEND)
            .unwrap()
            .serve_at("/org/freedesktop/portal/desktop", ScreenCastBackend { picker: picker.clone(), screens: Mutex::new(screens) })
            .unwrap()
            .serve_at("/org/freedesktop/portal/desktop", ScreenshotBackend { shutter: shutter.clone() })
            .unwrap()
            .serve_at("/org/freedesktop/portal/desktop", AccessBackend)
            .unwrap()
            .build()
            .unwrap();
        start_portal(&runtime);
        Desktop { picker, shutter, _backend: backend }
    })
}

/// Runs a service that stops when the thread that started it does.
fn service(program: &str, args: &[&str], runtime: &Path, log: &str) -> Child {
    let log = std::fs::File::create(runtime.join(log)).unwrap();
    let mut command = Command::new(program);
    command.args(args).env("XDG_RUNTIME_DIR", runtime).stdout(log.try_clone().unwrap()).stderr(log);
    unsafe {
        command.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    command.spawn().unwrap_or_else(|e| panic!("couldn't start {program}: {e}"))
}

/// Runs PipeWire and WirePlumber, and shares a new test picture for each request, until the tests
/// end. Each picture has a viewer of its own as long as it's shared: RSnip connects to it more than
/// once (a frame first, then the recording), and GStreamer's PipeWire output, unlike GNOME's, stops
/// when its last viewer leaves.
fn run_services(runtime: &Path, requests: Receiver<Sender<u32>>) {
    let mut running = vec![service("pipewire", &[], runtime, "pipewire.log")];
    wait_for(|| runtime.join("pipewire-0").exists(), "PipeWire's socket");
    running.push(service("wireplumber", &[], runtime, "wireplumber.log"));
    let mut screen: Vec<Child> = Vec::new();
    for (n, reply) in requests.into_iter().enumerate() {
        for mut old in screen.drain(..) {
            let _ = old.kill();
            let _ = old.wait();
        }
        // BGRx, as GNOME shares the screen (a source without a format can't be negotiated).
        let caps = format!("video/x-raw,format=BGRx,width={},height={},framerate=30/1", SCREEN.0, SCREEN.1);
        let name = format!("rsnip-test-screen-{n}");
        let props = format!("stream-properties=props,media.class=Video/Source,node.name={name}");
        let source = ["-q", "videotestsrc", "is-live=true", "pattern=ball", "!", &caps, "!", "pipewiresink", "mode=provide", &props];
        screen.push(service("gst-launch-1.0", &source, runtime, &format!("{name}.log")));
        let mut node = None;
        wait_for(
            || {
                node = test_screen_node(runtime, &name).map(|n| n.0);
                node.is_some()
            },
            "the test picture in PipeWire",
        );
        let node = node.unwrap();
        // Through videoconvert, so it asks for video (PipeWire won't link a stream of unknown type).
        let path = format!("path={node}");
        let viewer = ["-q", "pipewiresrc", &path, "!", "videoconvert", "!", "fakesink", "sync=false"];
        screen.push(service("gst-launch-1.0", &viewer, runtime, &format!("{name}-viewer.log")));
        wait_for(|| test_screen_node(runtime, &name).is_some_and(|n| n.1 == "running"), "the test picture to be viewed");
        let _ = reply.send(node);
    }
    for mut child in running.into_iter().chain(screen) {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// The test picture's PipeWire node and its state ("running" while it's viewed), from `pw-dump`.
fn test_screen_node(runtime: &Path, name: &str) -> Option<(u32, String)> {
    let out = Command::new("pw-dump").env("XDG_RUNTIME_DIR", runtime).stderr(Stdio::null()).output().ok()?;
    let objects: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).ok()?;
    let node = objects.iter().find(|o| o["info"]["props"]["node.name"] == name)?;
    Some((node["id"].as_u64()? as u32, node["info"]["state"].as_str().unwrap_or_default().to_string()))
}

/// xdg-desktop-portal, told about the stand-in picker the way a desktop's portal settings do.
fn start_portal(runtime: &Path) {
    let portals = runtime.join("portals");
    std::fs::create_dir_all(&portals).unwrap();
    std::fs::write(
        portals.join("rsnip-test.portal"),
        format!("[portal]\nDBusName={BACKEND}\nInterfaces=org.freedesktop.impl.portal.ScreenCast;org.freedesktop.impl.portal.Screenshot;org.freedesktop.impl.portal.Access;\nUseIn={DESKTOP}\n"),
    )
    .unwrap();
    let config = runtime.join("config");
    std::fs::create_dir_all(config.join("xdg-desktop-portal")).unwrap();
    std::fs::write(config.join("xdg-desktop-portal/portals.conf"), format!("[preferred]\ndefault={DESKTOP}\n")).unwrap();
    let log = std::fs::File::create(runtime.join("portal.log")).unwrap();
    let mut command = Command::new("/usr/libexec/xdg-desktop-portal");
    command
        .arg("--replace")
        .env("XDG_RUNTIME_DIR", runtime)
        .env("XDG_DESKTOP_PORTAL_DIR", &portals)
        .env("XDG_CONFIG_HOME", &config)
        .env("XDG_CURRENT_DESKTOP", DESKTOP)
        // RSnip runs unsandboxed: no Flatpak or Snap to find its app ID from.
        .env("XDG_DESKTOP_PORTAL_TEST_APP_INFO_KIND", "host")
        .stdout(log.try_clone().unwrap())
        .stderr(log);
    unsafe {
        command.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    // Started from a thread that lives as long as the tests (see `desktop`).
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(command.spawn().map(|_| ()));
        std::thread::park();
    });
    rx.recv().unwrap().expect("couldn't start xdg-desktop-portal");
    let conn = Connection::session().unwrap();
    let dbus = zbus::blocking::fdo::DBusProxy::new(&conn).unwrap();
    wait_for(|| dbus.name_has_owner("org.freedesktop.portal.Desktop".try_into().unwrap()).unwrap_or(false), "xdg-desktop-portal");
}

fn wait_for(mut done: impl FnMut() -> bool, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rsnip-desktop-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
#[ignore = "needs desktop services: run ci/desktop-test.sh"]
fn desktop_records_through_the_real_portal() {
    let desktop = desktop();
    let cast = ScreenCast::start(Sources::ScreenOrWindow).expect("the portal accepted the request").expect("the picker shared something");

    // What reached the picker got past the portal's checks; it asked for screens and windows, with the pointer drawn in.
    let asked = desktop.picker.lock().unwrap().asked.pop().unwrap();
    assert_eq!(u32::try_from(&asked["types"]).unwrap(), 3);
    assert_eq!(u32::try_from(&asked["cursor_mode"]).unwrap(), 2);

    let frame = recorder::snapshot(&cast).expect("a frame of the shared screen");
    assert_eq!((frame.width(), frame.height()), SCREEN);

    let dir = temp_dir("mp4");
    let format = Format { gif: false, hevc: false, fps: 30 };
    let recording = recorder::start(&cast, SCREEN, None, format, &dir).expect("recording starts");
    std::thread::sleep(Duration::from_secs(2));
    let recorded = recording.stop().expect("recording finishes");
    let seconds = mp4_seconds(&std::fs::read(&recorded.path).unwrap()).expect("a finished MP4, with its index");
    assert!((1.5..4.0).contains(&seconds), "{seconds} s long, for about 2 s of recording");
}

/// How long an MP4 says it is: from its movie header (`moov`/`mvhd`), which is written when it's
/// finished properly. `None` if it has none.
fn mp4_seconds(file: &[u8]) -> Option<f64> {
    let at = file.windows(4).position(|w| w == b"mvhd")?;
    let header = &file[at + 4..];
    let word = |i: usize| u32::from_be_bytes(header[i..i + 4].try_into().unwrap()) as u64;
    // Version 1 has 64-bit times.
    let (timescale, duration) = if header[0] == 1 {
        (word(20), word(24) << 32 | word(28))
    } else {
        (word(12), word(16))
    };
    (timescale > 0).then(|| duration as f64 / timescale as f64)
}

#[test]
#[ignore = "needs desktop services: run ci/desktop-test.sh"]
fn desktop_records_an_area_as_a_gif() {
    desktop();
    let cast = ScreenCast::start(Sources::Screen).unwrap().unwrap();
    let area = Area { x: 100, y: 60, width: 200, height: 120 };
    let dir = temp_dir("gif");
    let format = Format { gif: true, hevc: false, fps: 30 };
    let recording = recorder::start(&cast, SCREEN, Some(area), format, &dir).unwrap();
    std::thread::sleep(Duration::from_secs(2));
    let recorded = recording.stop().unwrap();
    let mut decoder = gif::DecodeOptions::new().read_info(std::fs::File::open(&recorded.path).unwrap()).unwrap();
    assert_eq!((decoder.width(), decoder.height()), (200, 120), "just the area");
    // Each frame shows for as long as it was on screen: they add up to the time recorded.
    let mut hundredths = 0;
    while let Some(frame) = decoder.read_next_frame().unwrap() {
        hundredths += frame.delay as u32;
    }
    assert!((150..400).contains(&hundredths), "{} s long, for about 2 s of recording", hundredths as f32 / 100.0);
}

#[test]
#[ignore = "needs desktop services: run ci/desktop-test.sh"]
fn desktop_a_cancelled_picker_records_nothing() {
    let desktop = desktop();
    desktop.picker.lock().unwrap().cancel = true;
    assert!(ScreenCast::start(Sources::ScreenOrWindow).unwrap().is_none());
}

/// The stand-in panel: it shows any icon that registers.
#[derive(Clone, Default)]
struct Panel {
    items: Arc<Mutex<Vec<String>>>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Panel {
    fn register_status_notifier_item(&self, service: String, #[zbus(header)] header: zbus::message::Header<'_>) {
        // An icon names its bus name, or just its object path.
        let item = if service.starts_with('/') { header.sender().map(|s| s.to_string()).unwrap_or_default() } else { service };
        self.items.lock().unwrap().push(item);
    }

    fn register_status_notifier_host(&self, _service: String) {}

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.items.lock().unwrap().clone()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }
}

/// The stand-in notifications: it keeps them.
#[derive(Clone, Default)]
struct Notifications {
    shown: Arc<Mutex<Vec<(String, String)>>>,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        _app: String,
        _replaces: u32,
        _icon: String,
        summary: String,
        body: String,
        _actions: Vec<String>,
        _hints: HashMap<String, OwnedValue>,
        _timeout: i32,
    ) -> u32 {
        let mut shown = self.shown.lock().unwrap();
        shown.push((summary, body));
        shown.len() as u32
    }
}

/// The stand-in notifications, on the bus until the connection is dropped.
fn notifications() -> (Notifications, Connection) {
    let notifications = Notifications::default();
    let shell = connection::Builder::session()
        .unwrap()
        .name("org.freedesktop.Notifications")
        .unwrap()
        .serve_at("/org/freedesktop/Notifications", notifications.clone())
        .unwrap()
        .build()
        .unwrap();
    (notifications, shell)
}

/// The icon's menu, as the panel sees it: (id, label) for each item.
fn menu(conn: &Connection, item: &str) -> Vec<(i32, String)> {
    let icon = Proxy::new(conn, item, "/StatusNotifierItem", "org.kde.StatusNotifierItem").unwrap();
    let path: OwnedObjectPath = icon.get_property("Menu").unwrap();
    let menu = Proxy::new(conn, item, path.as_str(), "com.canonical.dbusmenu").unwrap();
    type Layout = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);
    let (_, (_, _, children)): (u32, Layout) = menu.call("GetLayout", &(0i32, -1i32, Vec::<&str>::new())).unwrap();
    children
        .into_iter()
        .filter_map(|child| {
            let (id, props, _): Layout = child.try_into().ok()?;
            Some((id, props.get("label").and_then(|l| String::try_from(l.try_clone().ok()?).ok())?))
        })
        .collect()
}

/// Clicks the menu item labelled `label`.
fn click(conn: &Connection, item: &str, label: &str) {
    let (id, _) = menu(conn, item).into_iter().find(|(_, l)| l == label).unwrap_or_else(|| panic!("no {label} in the menu"));
    let icon = Proxy::new(conn, item, "/StatusNotifierItem", "org.kde.StatusNotifierItem").unwrap();
    let path: OwnedObjectPath = icon.get_property("Menu").unwrap();
    let menu = Proxy::new(conn, item, path.as_str(), "com.canonical.dbusmenu").unwrap();
    let _: () = menu.call("Event", &(id, "clicked", Value::from(0i32), 0u32)).unwrap();
}

#[test]
#[ignore = "needs desktop services: run ci/desktop-test.sh"]
fn desktop_icon_records_and_takes_screenshots() {
    desktop();
    // For the clipboard, which the icon's thread opens when it first copies something.
    let _x = XServer::start(73);
    let panel = Panel::default();
    let notifications = Notifications::default();
    let _desktop_shell = connection::Builder::session()
        .unwrap()
        .name("org.kde.StatusNotifierWatcher")
        .unwrap()
        .name("org.freedesktop.Notifications")
        .unwrap()
        .serve_at("/StatusNotifierWatcher", panel.clone())
        .unwrap()
        .serve_at("/org/freedesktop/Notifications", notifications.clone())
        .unwrap()
        .build()
        .unwrap();

    let icon = std::thread::spawn(|| super::background::run(None));
    wait_for(|| !panel.items.lock().unwrap().is_empty(), "the icon to register with the panel");
    let item = panel.items.lock().unwrap()[0].clone();
    let conn = Connection::session().unwrap();
    let title = |conn: &Connection| -> String { Proxy::new(conn, item.as_str(), "/StatusNotifierItem", "org.kde.StatusNotifierItem").unwrap().get_property("Title").unwrap() };
    assert_eq!(title(&conn), "RSnip");

    click(&conn, &item, "Record screen or window…");
    wait_for(|| menu(&conn, &item).iter().any(|(_, l)| l == "Stop recording"), "the menu to offer Stop recording");
    assert_eq!(title(&conn), "RSnip: recording");
    std::thread::sleep(Duration::from_secs(2));
    click(&conn, &item, "Stop recording");
    wait_for(|| notifications.shown.lock().unwrap().iter().any(|(s, _)| s == "Recording saved"), "the Recording saved notification");
    let shown = notifications.shown.lock().unwrap().clone();
    assert!(!shown.iter().any(|(s, _)| s.starts_with("Couldn't")), "{shown:?}");
    let name = shown.iter().find(|(s, _)| s == "Recording saved").unwrap().1.split(" is in").next().unwrap().to_string();
    let file = super::platform::screenshots_dir().join(&name);
    let seconds = std::fs::read(&file).ok().and_then(|bytes| mp4_seconds(&bytes));
    assert!(seconds.is_some_and(|s| (1.5..4.0).contains(&s)), "{} was saved and finished, about 2 s long: {seconds:?} s", file.display());
    assert!(menu(&conn, &item).iter().any(|(_, l)| l == "Record screen or window…"), "back to recording");

    // A screenshot, asked for as `rsnip --last` does while the icon runs. The icon copies it, so
    // it stays on the clipboard for as long as the icon lives.
    let picked = Area { x: 100, y: 60, width: 200, height: 120 };
    super::platform::set_text_setting("LastArea", "100 60 200 120 640x480");
    super::background::capture(Capture { mode: Mode::Last, delay: 0 });
    wait_for(|| notifications.shown.lock().unwrap().iter().any(|(s, _)| s == "Screenshot copied to clipboard"), "the Screenshot copied notification");
    let copied = image::load_from_memory(&paste("image/png")).expect("a picture on the clipboard").to_rgba8();
    assert_eq!(copied, image::imageops::crop_imm(&screen_picture(), picked.x, picked.y, picked.width, picked.height).to_image());

    click(&conn, &item, "Quit RSnip");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !icon.is_finished() {
        assert!(Instant::now() < deadline, "the icon didn't quit");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// An X server for the clipboard to live in, until dropped; DISPLAY points at it meanwhile.
struct XServer(Child);

impl XServer {
    fn start(display: u32) -> XServer {
        let server = Command::new("Xvfb").args([format!(":{display}"), "-nolisten".into(), "tcp".into()]).stderr(Stdio::null()).spawn().expect("couldn't start Xvfb");
        std::env::set_var("DISPLAY", format!(":{display}"));
        std::env::remove_var("WAYLAND_DISPLAY");
        wait_for(|| Path::new(&format!("/tmp/.X11-unix/X{display}")).exists(), "the X server");
        XServer(server)
    }
}

impl Drop for XServer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        std::env::remove_var("DISPLAY");
    }
}

/// What's on the clipboard as `target` (a MIME type, or TARGETS for the list).
fn paste(target: &str) -> Vec<u8> {
    Command::new("xclip").args(["-selection", "clipboard", "-t", target, "-o"]).output().expect("couldn't run xclip").stdout
}

#[test]
#[ignore = "needs desktop services: run ci/desktop-test.sh"]
fn desktop_copies_files_for_file_managers() {
    let _server = XServer::start(71);
    let dir = temp_dir("copy-files");
    let files = [dir.join("Shot 1.png"), dir.join("b.mp4")];
    for f in &files {
        std::fs::write(f, b"x").unwrap();
    }
    super::platform::copy_files(&files).unwrap();
    let paste = |target: &str| String::from_utf8(paste(target)).unwrap();
    let uri = |f: &PathBuf| format!("file://{}", f.display()).replace(' ', "%20");
    let targets = paste("TARGETS");
    assert!(targets.lines().any(|t| t == "x-special/gnome-copied-files") && targets.lines().any(|t| t == "text/uri-list"), "{targets}");
    assert_eq!(paste("x-special/gnome-copied-files"), format!("copy\n{}\n{}", uri(&files[0]), uri(&files[1])));
    assert_eq!(paste("text/uri-list"), format!("{}\r\n{}\r\n", uri(&files[0]), uri(&files[1])));
    assert_eq!(paste("UTF8_STRING"), format!("{}\n{}", files[0].display(), files[1].display()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
#[ignore = "needs desktop services: run ci/desktop-test.sh"]
fn desktop_asks_the_real_portal_for_a_screenshot() {
    let desktop = desktop();
    let file = portal::screenshot().expect("the portal accepted the request").expect("the desktop took a screenshot");

    // What reached the screenshot service got past the portal's checks: no dialog of its own.
    let asked = desktop.shutter.lock().unwrap().asked.pop().unwrap();
    assert!(!bool::try_from(&asked["interactive"]).unwrap());
    assert_eq!(image::open(&file).unwrap().to_rgba8(), screen_picture());
    let _ = std::fs::remove_file(file);

    desktop.shutter.lock().unwrap().cancel = true;
    assert_eq!(portal::screenshot().unwrap(), None, "declined");
}

#[test]
#[ignore = "needs desktop services: run ci/desktop-test.sh"]
fn desktop_screenshots_are_saved_copied_and_told_of() {
    let desktop = desktop();
    let _x = XServer::start(72);
    let (notifications, _shell) = notifications();
    let (folder, desktops) = (super::platform::screenshots_dir(), super::platform::legacy_save_folder());
    let latest = || notifications.shown.lock().unwrap().last().cloned().unwrap();
    let saved = |body: &str| image::open(folder.join(body.split(" is in").next().unwrap())).unwrap().to_rgba8();

    assert!(screenshot::take(Capture { mode: Mode::Full, delay: 0 }), "copied");
    let (title, body) = latest();
    assert_eq!(title, "Screenshot copied to clipboard");
    assert_eq!(saved(&body), screen_picture(), "saved in RSnip's folder as it was");
    let on_clipboard = image::load_from_memory(&paste("image/png")).expect("a picture on the clipboard").to_rgba8();
    assert_eq!(on_clipboard, screen_picture());
    // The desktop's file is gone, so `bring_in` can't bring it in as a second copy.
    assert_eq!(std::fs::read_dir(&desktops).unwrap().count(), 0, "left in {}", desktops.display());

    // The same area as last time, and only that.
    super::platform::set_text_setting("LastArea", "100 60 200 120 640x480");
    assert!(screenshot::take(Capture { mode: Mode::Last, delay: 0 }));
    let cropped = image::imageops::crop_imm(&screen_picture(), 100, 60, 200, 120).to_image();
    assert_eq!(saved(&latest().1), cropped);
    assert_eq!(image::load_from_memory(&paste("image/png")).unwrap().to_rgba8(), cropped);

    // Not saved, if Settings says so.
    let files = std::fs::read_dir(&folder).unwrap().count();
    super::platform::set_setting(crate::options::SAVE_TO_FOLDER.name, 0);
    assert!(screenshot::take(Capture { mode: Mode::Full, delay: 0 }));
    super::platform::set_setting(crate::options::SAVE_TO_FOLDER.name, 1);
    assert_eq!(latest().0, "Screenshot copied");
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), files, "nothing saved");

    // Declined in the desktop's dialog: nothing happens.
    let shown = notifications.shown.lock().unwrap().len();
    desktop.shutter.lock().unwrap().cancel = true;
    assert!(!screenshot::take(Capture { mode: Mode::Full, delay: 0 }));
    assert_eq!(notifications.shown.lock().unwrap().len(), shown);
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), files);

    // Refused without asking: on X11 it's read from the X server instead.
    desktop.shutter.lock().unwrap().refuse = true;
    assert!(screenshot::take(Capture { mode: Mode::Full, delay: 0 }), "copied");
    let (title, body) = latest();
    assert_eq!(title, "Screenshot copied to clipboard");
    let (conn, number) = x11rb::connect(None).unwrap();
    let root = &x11rb::connection::Connection::setup(&conn).roots[number];
    assert_eq!(saved(&body).dimensions(), (root.width_in_pixels.into(), root.height_in_pixels.into()), "all of the X screen");
}
