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
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// Shares a screen that's still: a new frame every few seconds, as GNOME sends one only when
    /// something on it changes.
    still: bool,
    asked: Vec<HashMap<String, OwnedValue>>,
}

/// The stand-in for GNOME's screen-cast picker: it shares a test picture, or cancels.
struct ScreenCastBackend {
    picker: Arc<Mutex<Picker>>,
    /// Asks for a new test picture (still or not), and gets back its PipeWire node.
    screens: Mutex<Sender<(Sender<u32>, bool)>>,
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
        let mut picker = self.picker.lock().unwrap();
        if std::mem::take(&mut picker.cancel) {
            return (1, HashMap::new());
        }
        let still = std::mem::take(&mut picker.still);
        drop(picker);
        // A stream of its own each time, as GNOME starts one for each recording.
        let (tx, rx) = channel();
        self.screens.lock().unwrap().send((tx, still)).unwrap();
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
    /// Takes its time answering, as that GNOME can before refusing.
    slow: bool,
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
        if std::mem::take(&mut shutter.slow) {
            drop(shutter);
            std::thread::sleep(Duration::from_secs(6));
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
        // Where recordings (and `pactl`) find the sound server the services start.
        std::env::set_var("PULSE_SERVER", sound_server(&runtime));
        // The services live as long as this thread: until the tests end.
        let (screens, requests) = channel::<(Sender<u32>, bool)>();
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

/// The sound server's address, for `PULSE_SERVER`.
fn sound_server(runtime: &Path) -> String {
    format!("unix:{}/pulse/native", runtime.display())
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
fn run_services(runtime: &Path, requests: Receiver<(Sender<u32>, bool)>) {
    let mut running = vec![service("pipewire", &[], runtime, "pipewire.log")];
    wait_for(|| runtime.join("pipewire-0").exists(), "PipeWire's socket");
    running.push(service("wireplumber", &[], runtime, "wireplumber.log"));
    // The sound server (PulseAudio's protocol on PipeWire), with a tone playing on its default
    // output, which is what a recording of the computer's sound hears.
    running.push(service("pipewire-pulse", &[], runtime, "pipewire-pulse.log"));
    wait_for(|| runtime.join("pulse/native").exists(), "the sound server's socket");
    running.push(service("gst-launch-1.0", &["-q", "audiotestsrc", "is-live=true", "volume=0.5", "!", "pulsesink"], runtime, "tone.log"));
    let mut screen: Vec<Child> = Vec::new();
    for (n, (reply, still)) in requests.into_iter().enumerate() {
        for mut old in screen.drain(..) {
            let _ = old.kill();
            let _ = old.wait();
        }
        // BGRx, as GNOME shares the screen (a source without a format can't be negotiated).
        let caps = format!("video/x-raw,format=BGRx,width={},height={},framerate={}", SCREEN.0, SCREEN.1, if still { "1/5" } else { "30/1" });
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
    let format = Format { gif: false, hevc: false, fps: 30, sound: None, microphone: None, camera: None };
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

/// The loudest the sound in an MP4 gets, in dB (0 is full scale), by decoding it; `None` if it has
/// none.
fn sound_level(path: &Path) -> Option<f64> {
    let out = Command::new("gst-launch-1.0")
        .args(["-m", "filesrc"])
        .arg(format!("location={}", path.display()))
        .args(["!", "qtdemux", "name=d", "d.audio_0", "!", "decodebin", "!", "audioconvert", "!", "level", "!", "fakesink"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let levels = text.split("rms=(GValueArray)< ").skip(1).filter_map(|rest| rest.split([',', ' ']).next()?.parse::<f64>().ok());
    levels.reduce(f64::max)
}

#[test]
#[ignore = "needs desktop services: run ci/desktop-test.sh"]
fn desktop_records_the_computers_sound() {
    desktop();
    let cast = ScreenCast::start(Sources::Screen).unwrap().unwrap();
    let dir = temp_dir("sound");
    let format = Format { gif: false, hevc: false, fps: 30, sound: Some(None), microphone: None, camera: None };
    let recording = recorder::start(&cast, SCREEN, None, format, &dir).expect("recording starts");
    std::thread::sleep(Duration::from_secs(3));
    let recorded = recording.stop().expect("recording finishes");
    assert_eq!(recorded.note, None);
    let file = std::fs::read(&recorded.path).unwrap();
    assert!((2.0..5.0).contains(&mp4_seconds(&file).expect("a finished MP4")), "about 3 s long");
    // A video track and an AAC one, with the tone the test played (about -9 dB, for 0.5 volume).
    let has = |tag: &[u8]| file.windows(tag.len()).any(|w| w == tag);
    assert!(has(b"vide") && has(b"soun") && has(b"mp4a"), "a video and a sound track");
    let level = sound_level(&recorded.path).expect("sound that plays");
    assert!((-30.0..0.0).contains(&level), "the tone, at {level} dB");
}

/// How much sound an MP4 has, in seconds, by decoding it: sound that was lost (a gap in its times)
/// doesn't count.
fn sound_seconds(path: &Path) -> f64 {
    let raw = path.with_extension("raw");
    let status = Command::new("gst-launch-1.0")
        .args(["-q", "filesrc"])
        .arg(format!("location={}", path.display()))
        .args(["!", "qtdemux", "name=d", "d.audio_0", "!", "decodebin", "!", "audioconvert", "!", "audioresample", "!", "audio/x-raw,format=S16LE,rate=48000,channels=2", "!", "filesink"])
        .arg(format!("location={}", raw.display()))
        .status()
        .unwrap();
    assert!(status.success(), "the sound decodes");
    let bytes = std::fs::metadata(&raw).unwrap().len();
    let _ = std::fs::remove_file(raw);
    bytes as f64 / (48_000.0 * 4.0)
}

#[test]
#[ignore = "needs desktop services: run ci/desktop-test.sh"]
fn desktop_records_sound_over_a_still_screen() {
    let desktop = desktop();
    // Narration over a slide, say: the screen sends a frame every 5 s, the sound plays throughout.
    desktop.picker.lock().unwrap().still = true;
    let cast = ScreenCast::start(Sources::Screen).unwrap().unwrap();
    let dir = temp_dir("still");
    let format = Format { gif: false, hevc: false, fps: 30, sound: Some(None), microphone: None, camera: None };
    let recording = recorder::start(&cast, SCREEN, None, format, &dir).expect("recording starts");
    let started = Instant::now();
    std::thread::sleep(Duration::from_secs(12));
    let recorded = recording.stop().expect("recording finishes");
    let length = started.elapsed().as_secs_f64();
    assert_eq!(recorded.note, None);
    let file = std::fs::read(&recorded.path).unwrap();
    let seconds = mp4_seconds(&file).expect("a finished MP4");
    let sound = sound_seconds(&recorded.path);
    eprintln!("recorded for {length:.1} s: the file is {seconds:.1} s long, with {sound:.1} s of sound");
    assert!(sound > length - 1.5, "{sound:.1} s of sound for {length:.1} s of recording");
    assert!((-30.0..0.0).contains(&sound_level(&recorded.path).expect("sound that plays")));
}

#[test]
#[ignore = "needs desktop services: run ci/desktop-test.sh"]
fn desktop_a_trimmed_recording_keeps_its_sound_in_step() {
    desktop();
    let cast = ScreenCast::start(Sources::Screen).unwrap().unwrap();
    let dir = temp_dir("trim-sound");
    let format = Format { gif: false, hevc: false, fps: 30, sound: Some(None), microphone: None, camera: None };
    let recording = recorder::start(&cast, SCREEN, None, format, &dir).expect("recording starts");
    std::thread::sleep(Duration::from_secs(3));
    let recorded = recording.stop().expect("recording finishes");
    // When each track's samples are shown, by its edit list. mp4mux starts the sound a little
    // after the picture (50 ms here), with an empty edit.
    let shown = crate::mp4::Movie::open(&recorded.path).unwrap().shown();
    assert_eq!(shown.len(), 2, "a picture and a sound track");
    eprintln!("the picture starts at {} s, the sound at {} s", shown[0][0], shown[1][0]);
    let close = |a: f64, b: f64| (a - b).abs() < 0.001;

    // Trimmed with nothing cut, each starts when it did.
    let all = dir.join("all.mp4");
    let (from, _) = super::player::trim(&recorded.path, &all, Duration::ZERO, Duration::MAX).unwrap();
    assert_eq!(from, Duration::ZERO);
    let kept = crate::mp4::Movie::open(&all).unwrap().shown();
    assert!(close(kept[0][0], shown[0][0]) && close(kept[1][0], shown[1][0]), "{:?} against {:?}", (kept[0][0], kept[1][0]), (shown[0][0], shown[1][0]));
    // All of the picture, and the sound that starts before it ends: the sound can run on a few
    // audio frames past the last picture frame, more so on a busy machine.
    assert_eq!(kept[0].len(), shown[0].len());
    let ends = crate::mp4::Movie::open(&recorded.path).unwrap().duration().as_secs_f64();
    let before = |end: f64| shown[1].iter().filter(|&&t| t < end).count();
    assert!((before(ends - 1e-6)..=before(ends + 1e-6)).contains(&kept[1].len()), "{} of {} audio frames, {} before the picture ends at {ends} s", kept[1].len(), shown[1].len(), before(ends));

    // Trimmed from a keyframe, each audio frame is shown as long after the start as it was.
    let part = dir.join("part.mp4");
    let (from, _) = super::player::trim(&recorded.path, &part, Duration::from_millis(1500), Duration::from_millis(2500)).unwrap();
    let from = from.as_secs_f64();
    let kept = crate::mp4::Movie::open(&part).unwrap().shown();
    let first = shown[1].iter().position(|&t| t >= from - 1e-9).unwrap();
    assert!(kept[1].len() > 10);
    for (n, t) in kept[1].iter().enumerate() {
        assert!(close(t + from, shown[1][first + n]), "audio frame {n} at {t} s, from {from} s; it was at {} s", shown[1][first + n]);
    }
    super::player::can_play(&part).unwrap();
}

/// How many programs are playing sound on the test sound server (the test tone is one).
fn sound_players() -> usize {
    let out = Command::new("pactl").args(["list", "short", "sink-inputs"]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.trim().is_empty()).count()
}

#[test]
#[ignore = "needs desktop services: run ci/desktop-test.sh"]
fn desktop_plays_a_recordings_sound() {
    desktop();
    let cast = ScreenCast::start(Sources::Screen).unwrap().unwrap();
    let dir = temp_dir("play-sound");
    let format = Format { gif: false, hevc: false, fps: 30, sound: Some(None), microphone: None, camera: None };
    let recording = recorder::start(&cast, SCREEN, None, format, &dir).expect("recording starts");
    std::thread::sleep(Duration::from_secs(4));
    let recorded = recording.stop().expect("recording finishes");
    assert!(super::player::info(&recorded.path).sound);

    let before = sound_players();
    let begun = Instant::now();
    let sound = super::player::play_sound(&recorded.path, Duration::from_secs(1), begun + Duration::from_secs(1)).expect("the sound starts");
    // How long the player's clock waits for the sound to begin.
    while !sound.ready() && begun.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(sound.ready() && sound.failed().is_none(), "{:?}", sound.failed());
    eprintln!("the sound began after {:?}", begun.elapsed());
    wait_for(|| sound_players() == before + 1, "the sound server to hear the recording");
    // Dropping it silences it at once.
    drop(sound);
    wait_for(|| sound_players() == before, "the sound server to hear no more of it");
}

#[test]
#[ignore = "needs desktop services: run ci/desktop-test.sh"]
fn desktop_a_missing_microphone_is_left_out() {
    desktop();
    let cast = ScreenCast::start(Sources::Screen).unwrap().unwrap();
    let dir = temp_dir("no-microphone");
    // The test sound server has an output and no microphone.
    let format = Format { gif: false, hevc: false, fps: 30, sound: Some(None), microphone: Some(None), camera: None };
    let recorded = {
        let recording = recorder::start(&cast, SCREEN, None, format, &dir).expect("recording starts");
        std::thread::sleep(Duration::from_secs(2));
        recording.stop().expect("recording finishes")
    };
    assert_eq!(recorded.note.as_deref(), Some("No microphone was found, so it was recorded without one."));
    assert!(sound_level(&recorded.path).is_some(), "the computer's sound is there");
    assert!(recorder::microphones().is_empty(), "monitors aren't microphones");
}

#[test]
#[ignore = "needs desktop services: run ci/desktop-test.sh"]
fn desktop_records_without_sound_when_the_sound_server_is_gone() {
    desktop();
    let cast = ScreenCast::start(Sources::Screen).unwrap().unwrap();
    let dir = temp_dir("no-sound");
    // Tests run one at a time, so nothing else reads this meanwhile.
    let server = std::env::var("PULSE_SERVER").unwrap();
    std::env::set_var("PULSE_SERVER", "unix:/nonexistent/pulse/native");
    let format = Format { gif: false, hevc: false, fps: 30, sound: Some(None), microphone: None, camera: None };
    let recorded = recorder::start(&cast, SCREEN, None, format, &dir).map(|recording| {
        std::thread::sleep(Duration::from_secs(2));
        recording.stop()
    });
    std::env::set_var("PULSE_SERVER", server);
    let recorded = recorded.expect("recording starts").expect("recording finishes");
    assert_eq!(recorded.note.as_deref(), Some("Couldn't reach the sound server, so it was recorded without sound."));
    let file = std::fs::read(&recorded.path).unwrap();
    assert!(mp4_seconds(&file).is_some() && !file.windows(4).any(|w| w == b"soun"), "a finished MP4 with just the video");
}

#[test]
#[ignore = "needs desktop services: run ci/desktop-test.sh"]
fn desktop_records_an_area_as_a_gif() {
    desktop();
    let cast = ScreenCast::start(Sources::Screen).unwrap().unwrap();
    let area = Area { x: 100, y: 60, width: 200, height: 120 };
    let dir = temp_dir("gif");
    let format = Format { gif: true, hevc: false, fps: 30, sound: Some(None), microphone: Some(None), camera: None };
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
    assert!(menu(&conn, &item).iter().any(|(_, l)| l == "Scrolling capture…"), "on X11");

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

#[test]
#[ignore = "needs desktop services: run ci/desktop-test.sh"]
fn desktop_icon_waits_for_a_panel_that_comes_later() {
    // A runtime folder of its own: an icon that ran earlier in this process keeps its lock.
    let runtime = temp_dir("late-panel");
    std::fs::set_permissions(&runtime, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    let old_runtime = std::env::var_os("XDG_RUNTIME_DIR");
    std::env::set_var("XDG_RUNTIME_DIR", &runtime);
    let panel_on_bus = |panel: &Panel| connection::Builder::session().unwrap().name("org.kde.StatusNotifierWatcher").unwrap().serve_at("/StatusNotifierWatcher", panel.clone()).unwrap().build().unwrap();

    // As at sign-in, when RSnip may start before GNOME's AppIndicator extension.
    let icon = std::thread::spawn(|| super::background::run(None));
    std::thread::sleep(Duration::from_secs(3));
    assert!(!icon.is_finished(), "the icon gave up without a panel");
    let panel = Panel::default();
    let shell = panel_on_bus(&panel);
    wait_for(|| !panel.items.lock().unwrap().is_empty(), "the icon to register with the panel that came later");

    // The panel restarts (GNOME Shell on X11 does, say), and the icon goes in the new one.
    drop(shell);
    let panel = Panel::default();
    let _shell = panel_on_bus(&panel);
    wait_for(|| !panel.items.lock().unwrap().is_empty(), "the icon to register with the restarted panel");
    let item = panel.items.lock().unwrap()[0].clone();
    let conn = Connection::session().unwrap();
    click(&conn, &item, "Quit RSnip");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !icon.is_finished() {
        assert!(Instant::now() < deadline, "the icon didn't quit");
        std::thread::sleep(Duration::from_millis(100));
    }
    match old_runtime {
        Some(dir) => std::env::set_var("XDG_RUNTIME_DIR", dir),
        None => std::env::remove_var("XDG_RUNTIME_DIR"),
    }
}

/// An X server for the clipboard to live in, until dropped; DISPLAY points at it meanwhile.
pub(super) struct XServer(Child);

impl XServer {
    pub(super) fn start(display: u32) -> XServer {
        let server = Command::new("Xvfb").args([format!(":{display}"), "-nolisten".into(), "tcp".into()]).stderr(Stdio::null()).spawn().expect("couldn't start Xvfb");
        std::env::set_var("DISPLAY", format!(":{display}"));
        std::env::remove_var("WAYLAND_DISPLAY");
        wait_for(|| Path::new(&format!("/tmp/.X11-unix/X{display}")).exists(), "the X server");
        XServer(server)
    }
}

impl Drop for XServer {
    fn drop(&mut self) {
        // Gone before the next test starts one, which may be on the same display.
        let _ = self.0.kill();
        let _ = self.0.wait();
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
    let x_screen = (root.width_in_pixels.into(), root.height_in_pixels.into());
    assert_eq!(saved(&body).dimensions(), x_screen, "all of the X screen");

    // And it isn't asked again.
    let asked = desktop.shutter.lock().unwrap().asked.len();
    assert!(screenshot::take(Capture { mode: Mode::Full, delay: 0 }));
    assert_eq!(desktop.shutter.lock().unwrap().asked.len(), asked);
    assert_eq!(saved(&latest().1).dimensions(), x_screen);

    // An answer that's slow in coming isn't waited for; the screenshot it brings after all goes.
    screenshot::forget_refusal();
    desktop.shutter.lock().unwrap().slow = true;
    let started = Instant::now();
    assert!(screenshot::take(Capture { mode: Mode::Full, delay: 0 }));
    assert!(started.elapsed() < Duration::from_secs(5), "waited {:?}", started.elapsed());
    assert_eq!(saved(&latest().1).dimensions(), x_screen);
    let empty = || std::fs::read_dir(&desktops).unwrap().count() == 0;
    wait_for(|| started.elapsed() > Duration::from_secs(7) && empty(), "the portal's late screenshot to be deleted");
    screenshot::forget_refusal();
}

/// The tall test page that `TallWindow` scrolls: its width and height, how high the window on it
/// is and where it is on the screen, and how far one notch of the mouse wheel scrolls it.
const PAGE: (u16, u16) = (200, 900);
const VIEW: u16 = 200;
const VIEW_AT: (i16, i16) = (120, 80);
const NOTCH: u16 = 40;

/// The tall test page as a picture, every row of it different.
fn tall_page() -> image::RgbaImage {
    image::RgbaImage::from_fn(PAGE.0.into(), PAGE.1.into(), |x, y| image::Rgba([(y % 256) as u8, (y / 256 * 60 + x / 4) as u8, (x * 5 % 256) as u8, 255]))
}

/// A window on the X server showing part of the tall test page, which the mouse wheel scrolls as a
/// browser's would, until dropped. After `esc_after` notches it presses Esc, as someone stopping a
/// scrolling capture would.
struct TallWindow {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TallWindow {
    fn show(esc_after: Option<usize>) -> TallWindow {
        use x11rb::connection::Connection as _;
        use x11rb::protocol::xproto::{ConnectionExt as _, CreateGCAux, CreateWindowAux, EventMask, ImageFormat, WindowClass, KEY_PRESS_EVENT, KEY_RELEASE_EVENT};
        use x11rb::protocol::xtest::ConnectionExt as _;
        use x11rb::protocol::Event;
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let (shown, drawn) = channel();
        let thread = std::thread::spawn(move || {
            let (conn, number) = x11rb::connect(None).unwrap();
            let screen = &conn.setup().roots[number];
            let (root, depth) = (screen.root, screen.root_depth);
            let window = conn.generate_id().unwrap();
            let events = CreateWindowAux::new().event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS).override_redirect(1);
            conn.create_window(depth, window, root, VIEW_AT.0, VIEW_AT.1, PAGE.0, VIEW, 0, WindowClass::INPUT_OUTPUT, screen.root_visual, &events).unwrap();
            let gc = conn.generate_id().unwrap();
            conn.create_gc(gc, window, &CreateGCAux::new()).unwrap();
            conn.map_window(window).unwrap();
            conn.flush().unwrap();
            // As the X server has pixels: blue first, four bytes each.
            let pixels: Vec<u8> = tall_page().pixels().flat_map(|p| [p[2], p[1], p[0], 0]).collect();
            let row = usize::from(PAGE.0) * 4;
            let draw = |offset: u16| {
                let rows = &pixels[usize::from(offset) * row..usize::from(offset + VIEW) * row];
                conn.put_image(ImageFormat::Z_PIXMAP, window, gc, PAGE.0, VIEW, 0, 0, 0, depth, rows).unwrap();
                // Drawn before anyone looks.
                conn.get_input_focus().unwrap().reply().unwrap();
            };
            let (mut offset, mut notches) = (0, 0);
            while !stopped.load(Ordering::Relaxed) {
                while let Some(event) = conn.poll_for_event().unwrap() {
                    match event {
                        Event::Expose(_) => {
                            draw(offset);
                            let _ = shown.send(());
                        }
                        Event::ButtonPress(press) if press.detail == 5 => {
                            offset = (offset + NOTCH).min(PAGE.1 - VIEW);
                            draw(offset);
                            notches += 1;
                            if esc_after == Some(notches) {
                                let esc = super::scroll::escape_key(&conn).expect("a key for Esc");
                                for kind in [KEY_PRESS_EVENT, KEY_RELEASE_EVENT] {
                                    conn.xtest_fake_input(kind, esc, x11rb::CURRENT_TIME, root, 0, 0, 0).unwrap();
                                }
                                conn.flush().unwrap();
                            }
                        }
                        _ => {}
                    }
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        drawn.recv_timeout(Duration::from_secs(20)).expect("the tall window to show");
        TallWindow { stop, thread: Some(thread) }
    }
}

impl Drop for TallWindow {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Where the X server's pointer is.
fn pointer() -> (i16, i16) {
    use x11rb::protocol::xproto::ConnectionExt as _;
    let (conn, number) = x11rb::connect(None).unwrap();
    let root = x11rb::connection::Connection::setup(&conn).roots[number].root;
    let at = conn.query_pointer(root).unwrap().reply().unwrap();
    (at.root_x, at.root_y)
}

#[test]
#[ignore = "needs desktop services: run ci/desktop-test.sh"]
fn desktop_scrolls_a_window_to_its_end_and_stitches_it() {
    let x = XServer::start(77);
    let area = Area { x: VIEW_AT.0 as u32, y: VIEW_AT.1 as u32, width: PAGE.0.into(), height: VIEW.into() };
    let was = pointer();
    {
        let _window = TallWindow::show(None);
        let image = super::scroll::capture(area).expect("a scrolling capture");
        assert_eq!(image.dimensions(), (PAGE.0.into(), PAGE.1.into()), "all of the page");
        assert!(image == tall_page(), "the page as it is, each part once");
        assert_eq!(pointer(), was, "the pointer is put back");
    }

    // Esc stops it, keeping what it has: a notch, then two.
    {
        let _window = TallWindow::show(Some(3));
        let image = super::scroll::capture(area).expect("a scrolling capture");
        let height = u32::from(VIEW + 3 * NOTCH);
        assert_eq!(image.dimensions(), (PAGE.0.into(), height));
        assert!(image == image::imageops::crop_imm(&tall_page(), 0, 0, PAGE.0.into(), height).to_image());
        assert_eq!(pointer(), was);
    }

    // On Wayland it says why not.
    drop(x);
    let (notifications, _shell) = notifications();
    assert!(!screenshot::take(Capture { mode: Mode::Scroll, delay: 0 }));
    let shown = notifications.shown.lock().unwrap().clone();
    assert_eq!(shown.iter().map(|(title, _)| title.as_str()).collect::<Vec<_>>(), ["Scrolling capture needs an X11 session"]);
}

/// GNOME's schemas for keyboard shortcuts, as far as RSnip uses them: CI's image has `gsettings`
/// but not GNOME.
const GNOME_SCHEMAS: &str = r#"<schemalist>
  <schema id="org.gnome.settings-daemon.plugins.media-keys" path="/org/gnome/settings-daemon/plugins/media-keys/">
    <key name="custom-keybindings" type="as"><default>[]</default></key>
  </schema>
  <schema id="org.gnome.settings-daemon.plugins.media-keys.custom-keybinding">
    <key name="name" type="s"><default>''</default></key>
    <key name="command" type="s"><default>''</default></key>
    <key name="binding" type="s"><default>''</default></key>
  </schema>
  <schema id="org.gnome.shell.keybindings" path="/org/gnome/shell/keybindings/">
    <key name="show-screenshot-ui" type="as"><default>['Print']</default></key>
  </schema>
</schemalist>"#;

#[test]
#[ignore = "needs desktop services: run ci/desktop-test.sh"]
fn desktop_gnome_shortcuts_go_through_the_real_gsettings() {
    use super::platform::{self, gnome_keys};
    use crate::shortcuts::Action;
    let dir = temp_dir("gsettings");
    let schemas = dir.join("schemas");
    std::fs::create_dir_all(&schemas).unwrap();
    std::fs::write(schemas.join("rsnip-test.gschema.xml"), GNOME_SCHEMAS).unwrap();
    assert!(Command::new("glib-compile-schemas").arg(&schemas).status().unwrap().success());
    // The keyfile backend keeps what's set between gsettings' runs, in the test's own folder (the
    // memory one forgets it as each ends).
    let run = |args: &[&str]| {
        let mut command = Command::new("gsettings");
        command.env("GSETTINGS_SCHEMA_DIR", &schemas).env("GSETTINGS_BACKEND", "keyfile").env("XDG_CONFIG_HOME", &dir);
        platform::run_gsettings(command, args)
    };
    let get = |schema: &str, key: &str| run(&["get", schema, key]).unwrap().trim().to_owned();
    let at = |name: &str| format!("{}:{}{name}/", gnome_keys::CUSTOM, gnome_keys::BASE);
    // Someone else's.
    run(&["set", &at("custom0"), "command", "'gnome-terminal'"]).unwrap();
    run(&["set", &at("custom0"), "binding", "'<Control><Alt>t'"]).unwrap();
    run(&["set", gnome_keys::MEDIA_KEYS, gnome_keys::LIST, &format!("['{}custom0/']", gnome_keys::BASE)]).unwrap();

    let program = "/opt/RSnip's App/rsnip";
    assert_eq!(gnome_keys::set(&run, program, Action::Region, "<Shift><Control>4"), Ok(None));
    let list = format!("['{0}custom0/', '{0}rsnip0/']", gnome_keys::BASE);
    assert_eq!(get(gnome_keys::MEDIA_KEYS, gnome_keys::LIST), list);
    assert_eq!(get(&at("rsnip0"), "binding"), "'<Shift><Control>4'");
    assert_eq!(get(&at("rsnip0"), "name"), "'RSnip: Capture region'");
    let state = gnome_keys::read(&run, program).unwrap();
    assert_eq!(state.keys[Action::Region.index()].as_deref(), Some("<Shift><Control>4"), "its command, quoted, read back as RSnip's");
    assert!(state.gnome_has_print);

    let saved = gnome_keys::take_print_screen(&run, program, Action::Full).unwrap();
    assert_eq!(saved.as_deref(), Some("['Print']"));
    assert_eq!(get(gnome_keys::SHELL, gnome_keys::SCREENSHOT_UI), "@as []");
    assert_eq!(gnome_keys::read(&run, program).unwrap().print_screen(), Some(Action::Full));
    gnome_keys::give_back_print_screen(&run, program, saved.as_deref()).unwrap();
    assert_eq!(get(gnome_keys::SHELL, gnome_keys::SCREENSHOT_UI), "['Print']");

    gnome_keys::clear(&run, program, Action::Region).unwrap();
    assert_eq!(get(gnome_keys::MEDIA_KEYS, gnome_keys::LIST), format!("['{}custom0/']", gnome_keys::BASE));
    assert_eq!(get(&at("rsnip0"), "binding"), "''", "reset");
    assert_eq!(get(&at("custom0"), "command"), "'gnome-terminal'", "someone else's is left as it was");
    assert_eq!(get(&at("custom0"), "binding"), "'<Control><Alt>t'");
}

/// The top-level window on the X server named `title`, if there is one. With no window manager,
/// each is a child of the root.
fn x_window(conn: &impl x11rb::connection::Connection, root: u32, title: &str) -> Option<u32> {
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};
    let name = conn.intern_atom(false, b"_NET_WM_NAME").ok()?.reply().ok()?.atom;
    let children = conn.query_tree(root).ok()?.reply().ok()?.children;
    children.into_iter().find(|&w| conn.get_property(false, w, name, AtomEnum::ANY, 0, 256).ok().and_then(|c| c.reply().ok()).is_some_and(|p| p.value == title.as_bytes()))
}

/// Recording with the camera on: its bubble opens in a process of its own (this test binary, see
/// `camera::tests::bubble_child`), by the area's bottom right, on top and out of the taskbar,
/// showing the camera (a red test picture here, in place of `v4l2src`); stopping closes it. The
/// stand-in picker shares a test picture rather than this X server's screen, so the bubble isn't in
/// the recording itself.
#[test]
#[ignore = "needs desktop services: run ci/desktop-test.sh"]
fn desktop_records_with_the_camera_on_top() {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, ImageFormat, MapState};
    desktop();
    let _x = XServer::start(78);
    std::env::set_var(super::camera::gst::TEST_SOURCE, "videotestsrc is-live=true pattern=red");
    let cast = ScreenCast::start(Sources::Screen).unwrap().unwrap();
    let dir = temp_dir("camera");
    let area = Area { x: 100, y: 60, width: 400, height: 300 };
    let format = Format { gif: false, hevc: false, fps: 30, sound: None, microphone: None, camera: Some(None) };
    let recording = recorder::start(&cast, SCREEN, Some(area), format, &dir).expect("recording starts");
    wait_for(|| recording.camera().is_some_and(|c| c.shown()), "the camera's bubble to show a picture");

    let (conn, screen) = x11rb::connect(None).unwrap();
    let root = conn.setup().roots[screen].root;
    let window = x_window(&conn, root, super::camera::TITLE).expect("the bubble's window");
    let geometry = conn.get_geometry(window).unwrap().reply().unwrap();
    // Medium, 16 points in from the corner at 500, 360.
    let side = super::camera::window_size(super::camera::Size::Medium).x as i16;
    assert_eq!((geometry.x, geometry.y, geometry.width as i16, geometry.height as i16), (500 - side - 16, 360 - side - 16, side, side));
    // eframe shows it once it's drawn.
    wait_for(|| conn.get_window_attributes(window).unwrap().reply().unwrap().map_state == MapState::VIEWABLE, "the bubble's window to show");
    let atom = |name: &str| conn.intern_atom(false, name.as_bytes()).unwrap().reply().unwrap().atom;
    let state: Vec<u32> = conn.get_property(false, window, atom("_NET_WM_STATE"), AtomEnum::ATOM, 0, 16).unwrap().reply().unwrap().value32().unwrap().collect();
    assert!(state.contains(&atom("_NET_WM_STATE_SKIP_TASKBAR")) && state.contains(&atom("_NET_WM_STATE_ABOVE")), "on top, out of the taskbar: {state:?}");
    // Its middle shows the camera.
    let middle = |conn: &x11rb::rust_connection::RustConnection| conn.get_image(ImageFormat::Z_PIXMAP, root, geometry.x + side / 2, geometry.y + side / 2, 1, 1, !0).unwrap().reply().unwrap().data;
    wait_for(|| matches!(middle(&conn)[..3], [b, g, r] if r > 200 && g < 60 && b < 60), "the bubble to show the red test picture");
    let camera_running = || std::fs::read_dir("/proc").unwrap().flatten().any(|p| std::fs::read(p.path().join("cmdline")).is_ok_and(|c| c.windows(11).any(|w| w == b"pattern=red")));
    assert!(camera_running());

    let recorded = recording.stop().expect("recording finishes");
    assert_eq!(recorded.note, None, "the camera was there all along");
    wait_for(|| x_window(&conn, root, super::camera::TITLE).is_none(), "the bubble to close");
    // And the camera with it: its GStreamer goes with the bubble, which is killed.
    wait_for(|| !camera_running(), "the camera's pipeline to stop");
    std::env::remove_var(super::camera::gst::TEST_SOURCE);
}
