//! Recording and the background icon against real desktop services: xdg-desktop-portal (which
//! checks everything an app asks of the desktop, and refused 0.5.0's recordings for a mistyped
//! option), PipeWire with WirePlumber, and GStreamer. Only the desktop's own pieces are stand-ins:
//! GNOME's screen-cast picker (a portal backend that shares a test picture through PipeWire), the
//! panel the icon goes in, and notifications.
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

use super::portal::{ScreenCast, Sources};
use super::recorder::{self, Area, Format};

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

/// The real services, started once for all these tests.
struct Desktop {
    picker: Arc<Mutex<Picker>>,
    /// Keeps the stand-in picker on the bus.
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
        let backend = connection::Builder::session()
            .unwrap()
            .name(BACKEND)
            .unwrap()
            .serve_at("/org/freedesktop/portal/desktop", ScreenCastBackend { picker: picker.clone(), screens: Mutex::new(screens) })
            .unwrap()
            .build()
            .unwrap();
        start_portal(&runtime);
        Desktop { picker, _backend: backend }
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
/// end.
fn run_services(runtime: &Path, requests: Receiver<Sender<u32>>) {
    let mut running = vec![service("pipewire", &[], runtime, "pipewire.log")];
    wait_for(|| runtime.join("pipewire-0").exists(), "PipeWire's socket");
    running.push(service("wireplumber", &[], runtime, "wireplumber.log"));
    let mut screen: Option<Child> = None;
    for (n, reply) in requests.into_iter().enumerate() {
        if let Some(mut old) = screen.take() {
            let _ = old.kill();
            let _ = old.wait();
        }
        // BGRx, as GNOME shares the screen (a source without a format can't be negotiated).
        let caps = format!("video/x-raw,format=BGRx,width={},height={},framerate=30/1", SCREEN.0, SCREEN.1);
        let name = format!("rsnip-test-screen-{n}");
        let props = format!("stream-properties=props,media.class=Video/Source,node.name={name}");
        let source = ["-q", "videotestsrc", "is-live=true", "pattern=ball", "!", &caps, "!", "pipewiresink", "mode=provide", &props];
        screen = Some(service("gst-launch-1.0", &source, runtime, &format!("{name}.log")));
        let mut node = None;
        wait_for(
            || {
                node = test_screen_node(runtime, &name);
                node.is_some()
            },
            "the test picture in PipeWire",
        );
        let _ = reply.send(node.unwrap());
    }
    for mut child in running.into_iter().chain(screen) {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// The test picture's PipeWire node, from `pw-dump`.
fn test_screen_node(runtime: &Path, name: &str) -> Option<u32> {
    let out = Command::new("pw-dump").env("XDG_RUNTIME_DIR", runtime).stderr(Stdio::null()).output().ok()?;
    let objects: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).ok()?;
    objects.iter().find(|o| o["info"]["props"]["node.name"] == name).and_then(|o| o["id"].as_u64()).map(|id| id as u32)
}

/// xdg-desktop-portal, told about the stand-in picker the way a desktop's portal settings do.
fn start_portal(runtime: &Path) {
    let portals = runtime.join("portals");
    std::fs::create_dir_all(&portals).unwrap();
    std::fs::write(
        portals.join("rsnip-test.portal"),
        format!("[portal]\nDBusName={BACKEND}\nInterfaces=org.freedesktop.impl.portal.ScreenCast;\nUseIn={DESKTOP}\n"),
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
fn desktop_icon_records_from_its_menu() {
    desktop();
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

    click(&conn, &item, "Quit RSnip");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !icon.is_finished() {
        assert!(Instant::now() < deadline, "the icon didn't quit");
        std::thread::sleep(Duration::from_millis(100));
    }
}
