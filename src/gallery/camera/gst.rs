//! The camera on Linux: GStreamer's Video4Linux source (`v4l2src`, in gstreamer1.0-plugins-good)
//! in a `gst-launch-1.0` of its own, which sends square RGBA pictures on its standard output, as
//! the player's frames come (see `player::gst`). The cameras are listed by GStreamer's device
//! monitor, or else from the kernel's list.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use eframe::egui::ColorImage;

use super::{Sink, SIDE};
use crate::gallery::platform::SoundDevice;
use crate::gallery::recorder;

/// How long `gst-device-monitor-1.0` has to list the cameras.
const MONITOR_TIMEOUT: Duration = Duration::from_secs(5);
/// What the kernel lists Video4Linux devices in.
const SYS_VIDEO: &str = "/sys/class/video4linux";
/// In tests, the GStreamer source to use instead of the camera (`videotestsrc`, say).
#[cfg(test)]
pub const TEST_SOURCE: &str = "RSNIP_CAMERA_SOURCE";

/// The cameras there are: as GStreamer's device monitor lists them (in gstreamer1.0-plugins-base-apps
/// on Debian and Ubuntu), or else as the kernel does.
pub fn cameras() -> Vec<SoundDevice> {
    let mut monitor = Command::new("gst-device-monitor-1.0");
    monitor.arg("Video/Source").env("LC_ALL", "C").env_remove("LANGUAGE");
    let listed = recorder::output_of(monitor, MONITOR_TIMEOUT).map(|out| parse_monitor(&out)).unwrap_or_default();
    if listed.is_empty() {
        from_sys(Path::new(SYS_VIDEO))
    } else {
        listed
    }
}

/// `gst-device-monitor-1.0 Video/Source`: each camera that has a Video4Linux device (`v4l2src`
/// opens), named as the monitor names it, once (PipeWire lists the same ones again).
fn parse_monitor(text: &str) -> Vec<SoundDevice> {
    let mut cameras: Vec<SoundDevice> = Vec::new();
    for block in text.split("Device found:").skip(1) {
        let field = |key: &str| block.lines().filter_map(|l| l.trim().split_once(':')).find(|(k, _)| k.trim() == key).map(|(_, v)| v.trim().to_string());
        if field("class").as_deref() != Some("Video/Source") {
            continue;
        }
        let property = |key: &str| block.lines().filter_map(|l| l.trim().split_once(" = ")).find(|(k, _)| *k == key).map(|(_, v)| v.trim().to_string());
        // The v4l2 monitor's own, PipeWire's, or the pipeline it suggests.
        let path = property("device.path")
            .or_else(|| property("api.v4l2.path"))
            .or_else(|| property("object.path").and_then(|p| p.strip_prefix("v4l2:").map(String::from)))
            .or_else(|| block.lines().find_map(|l| l.split_whitespace().find_map(|w| w.strip_prefix("device=").map(String::from))));
        let (Some(path), Some(name)) = (path.filter(|p| p.starts_with("/dev/")), field("name")) else { continue };
        if !cameras.iter().any(|c| c.id == path) {
            cameras.push(SoundDevice { id: path, name });
        }
    }
    cameras
}

/// The cameras in the kernel's list (`dir`), by their device's name: only each one's first node
/// (index 0), which has the pictures, not its metadata.
fn from_sys(dir: &Path) -> Vec<SoundDevice> {
    let mut found: Vec<(u32, SoundDevice)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let node = entry.file_name().to_string_lossy().into_owned();
            let number: u32 = node.strip_prefix("video")?.parse().ok()?;
            let read = |file: &str| std::fs::read_to_string(entry.path().join(file)).ok().map(|t| t.trim().to_string());
            if read("index").is_some_and(|index| index != "0") {
                return None;
            }
            let name = read("name").filter(|n| !n.is_empty()).unwrap_or_else(|| node.clone());
            Some((number, SoundDevice { id: format!("/dev/{node}"), name }))
        })
        .collect();
    found.sort_by_key(|(number, _)| *number);
    found.into_iter().map(|(_, camera)| camera).collect()
}

/// The GStreamer source for `device` (a /dev/video path), else the first camera, and a note if the
/// one chosen isn't there.
fn source(device: Option<&str>) -> (String, Option<&'static str>) {
    #[cfg(test)]
    if let Ok(source) = std::env::var(TEST_SOURCE) {
        return (source, None);
    }
    let first = || from_sys(Path::new(SYS_VIDEO)).into_iter().next().map_or("v4l2src".to_string(), |c| format!("v4l2src device={}", c.id));
    match device {
        Some(path) if Path::new(path).exists() => (format!("v4l2src device={path}"), None),
        Some(_) => (first(), Some("The chosen camera isn't connected, so the first one was shown.")),
        None => (first(), None),
    }
}

/// The `gst-launch-1.0` arguments for square pictures from `source`, `SIDE` pixels across, cut from
/// the middle of the camera's. `decodebin` takes cameras that send JPEGs as well as raw ones.
pub fn pipeline(source: &str) -> Vec<String> {
    let caps = format!("video/x-raw,format=RGBA,width={SIDE},height={SIDE},pixel-aspect-ratio=1/1");
    let pipeline = format!("{source} ! decodebin ! videoconvert ! aspectratiocrop aspect-ratio=1/1 ! videoscale ! {caps} ! fdsink fd=1");
    std::iter::once("-q").chain(pipeline.split_whitespace()).map(String::from).collect()
}

/// The pipeline, killed when dropped.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Sends `device`'s pictures to `sink` until nothing's looking (`Ok`), or why it can't.
pub fn capture(device: Option<&str>, sink: &Sink) -> Result<(), String> {
    let (source, note) = source(device);
    if let Some(note) = note {
        sink.note(note);
    }
    run(&source, sink)
}

/// `capture`, from the GStreamer element `source`.
fn run(source: &str, sink: &Sink) -> Result<(), String> {
    let mut command = Command::new("gst-launch-1.0");
    command.args(pipeline(source)).env("LC_ALL", "C").env_remove("LANGUAGE").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // It goes with the bubble, which the recorder kills: else it would keep the camera on.
    unsafe {
        command.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            Ok(())
        });
    }
    let mut running = Running(command.spawn().map_err(|e| format!("GStreamer isn't installed ({e})"))?);
    let said = recorder::said(&mut running.0);
    let mut out = running.0.stdout.take().expect("piped");
    let mut buf = vec![0u8; SIDE * SIDE * 4];
    while out.read_exact(&mut buf).is_ok() {
        if !sink.picture(ColorImage::from_rgba_unmultiplied([SIDE, SIDE], &buf)) {
            return Ok(());
        }
    }
    drop(running);
    Err(failure(&said.and_then(|s| s.join().ok()).unwrap_or_default()))
}

/// Why a camera's pipeline stopped, from what it said, in a few words.
fn failure(said: &str) -> String {
    let has = |text: &str| said.contains(text);
    if has("busy") {
        "it's in use by another app".into()
    } else if has("Cannot identify device") || has("No such file") || has("does not exist") {
        "no camera was found".into()
    } else if has("Permission denied") {
        "this user isn't allowed to use it".into()
    } else if has("no element \"v4l2src\"") {
        "it needs GStreamer's Video4Linux plugin (gstreamer1.0-plugins-good on Debian and Ubuntu, gstreamer1-plugins-good on Fedora, gst-plugins-good on Arch)".into()
    } else {
        // "ERROR: from element /GstPipeline:pipeline0/GstV4l2Src:v4l2src0: Could not read from resource.", else its last line.
        let error = said.lines().find_map(|l| l.trim().strip_prefix("ERROR: ")).map(|e| e.strip_prefix("from element ").and_then(|e| e.split_once(": ")).map_or(e, |(_, why)| why));
        let line = error.or_else(|| said.lines().rev().map(str::trim).find(|l| !l.is_empty())).unwrap_or("it stopped");
        line.trim_end_matches('.').to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_the_cameras_the_device_monitor_finds() {
        // Ubuntu 24.04 with a laptop's camera: once from GStreamer's own monitor, once from
        // PipeWire's, and a capture card that has a name of its own.
        let out = "Probing devices...\n\n\nDevice found:\n\n\tname  : Integrated Camera: Integrated C\n\tclass : Video/Source\n\tcaps  : video/x-raw, format=YUY2, width=640, height=480, framerate=30/1\n\t        image/jpeg, width=1280, height=720, framerate=30/1\n\tproperties:\n\t\tudev-probed = true\n\t\tdevice.bus_path = pci-0000:00:14.0-usb-0:8:1.0\n\t\tdevice.vendor.name = \"Chicony\\ Electronics\\ Co.\\,\\ Ltd.\"\n\t\tdevice.capabilities = :capture:\n\t\tdevice.api = v4l2\n\t\tdevice.path = /dev/video0\n\tgst-launch-1.0 v4l2src device=/dev/video0 ! ...\n\n\nDevice found:\n\n\tname  : Integrated Camera (V4L2)\n\tclass : Video/Source\n\tcaps  : video/x-raw, format=YUY2, width=640, height=480\n\tproperties:\n\t\tapi.v4l2.path = /dev/video0\n\t\tobject.path = v4l2:/dev/video0\n\t\tmedia.class = Video/Source\n\tgst-launch-1.0 pipewiresrc target-object=52 ! ...\n\n\nDevice found:\n\n\tname  : USB3 Video\n\tclass : Video/Source\n\tcaps  : video/x-raw, format=NV12, width=1920, height=1080\n\tproperties:\n\t\tobject.path = v4l2:/dev/video2\n\tgst-launch-1.0 pipewiresrc target-object=60 ! ...\n\n\nDevice found:\n\n\tname  : Monitor of Built-in Audio\n\tclass : Audio/Source\n\tproperties:\n\t\tdevice.path = /dev/snd/pcmC0D0p\n";
        let found: Vec<(String, String)> = parse_monitor(out).into_iter().map(|c| (c.id, c.name)).collect();
        assert_eq!(found, [("/dev/video0".into(), "Integrated Camera: Integrated C".into()), ("/dev/video2".into(), "USB3 Video".into())]);
        // From the suggested pipeline alone, and one with no device (libcamera's) left out.
        let pipeline_only = "Device found:\n\tname  : Cam\n\tclass : Video/Source\n\tgst-launch-1.0 v4l2src device=/dev/video4 ! ...\nDevice found:\n\tname  : Sensor\n\tclass : Video/Source\n\tgst-launch-1.0 libcamerasrc camera-name=/base/i2c ! ...\n";
        assert_eq!(parse_monitor(pipeline_only), [SoundDevice { id: "/dev/video4".into(), name: "Cam".into() }]);
        assert!(parse_monitor("Probing devices...\n").is_empty());
        assert!(parse_monitor("").is_empty());
    }

    #[test]
    fn falls_back_to_the_kernels_list() {
        let dir = crate::gallery::tests::sandbox_dir("v4l-sys");
        for (node, name, index) in [("video0", "Integrated Camera: Integrated C", "0"), ("video1", "Integrated Camera: Integrated C", "1"), ("video10", "USB3 Video", "0"), ("video2", "Loopback", "")] {
            std::fs::create_dir_all(dir.join(node)).unwrap();
            std::fs::write(dir.join(node).join("name"), format!("{name}\n")).unwrap();
            if !index.is_empty() {
                std::fs::write(dir.join(node).join("index"), format!("{index}\n")).unwrap();
            }
        }
        std::fs::create_dir_all(dir.join("v4l-subdev0")).unwrap();
        let found: Vec<(String, String)> = from_sys(&dir).into_iter().map(|c| (c.id, c.name)).collect();
        assert_eq!(found, [("/dev/video0".into(), "Integrated Camera: Integrated C".into()), ("/dev/video2".into(), "Loopback".into()), ("/dev/video10".into(), "USB3 Video".into())], "not the metadata node, in order");
        assert!(from_sys(&dir.join("missing")).is_empty());
    }

    #[test]
    fn pictures_are_square_rgba() {
        let args = pipeline("v4l2src device=/dev/video2");
        assert_eq!(&args[..4], ["-q", "v4l2src", "device=/dev/video2", "!"]);
        assert!(args.contains(&"aspect-ratio=1/1".to_string()), "cut from the middle: {args:?}");
        assert!(args.contains(&format!("video/x-raw,format=RGBA,width={SIDE},height={SIDE},pixel-aspect-ratio=1/1")));
        assert_eq!(&args[args.len() - 2..], ["fdsink", "fd=1"]);
    }

    /// The pipeline, with a test picture standing in for the camera, where GStreamer is installed
    /// (CI's Linux job has it): square pictures, as they come, until nothing's looking; and one
    /// that can't open its camera says why.
    #[test]
    fn pictures_come_from_gstreamer() {
        if !recorder::available().is_ok_and(|()| Command::new("gst-inspect-1.0").args(["--exists", "aspectratiocrop"]).status().is_ok_and(|s| s.success())) {
            return eprintln!("skipped: no GStreamer");
        }
        let (tx, rx) = std::sync::mpsc::sync_channel(2);
        let running = std::thread::spawn(move || run("videotestsrc is-live=true pattern=red", &Sink { tx, ctx: None }));
        let mut red = 0;
        while red < 3 {
            match rx.recv_timeout(Duration::from_secs(20)).expect("a picture") {
                super::super::Event::Picture(image, _) => {
                    assert_eq!(image.size, [SIDE, SIDE]);
                    let [r, g, b, a] = image.pixels[SIDE * SIDE / 2 + SIDE / 2].to_array();
                    assert!(r > 200 && g < 50 && b < 50 && a == 255, "red: {:?}", [r, g, b, a]);
                    red += 1;
                }
                _ => panic!("only pictures"),
            }
        }
        drop(rx);
        assert_eq!(running.join().unwrap(), Ok(()), "it stops once nothing's looking");

        let (tx, _rx) = std::sync::mpsc::sync_channel(2);
        assert_eq!(run("v4l2src device=/dev/rsnip-no-camera", &Sink { tx, ctx: None }), Err("no camera was found".into()));
    }

    #[test]
    fn says_why_a_camera_cant_be_used() {
        assert_eq!(failure("ERROR: from element /GstPipeline:pipeline0/GstV4l2Src:v4l2src0: Cannot identify device '/dev/video0'.\nAdditional debug info:\n..."), "no camera was found");
        assert_eq!(failure("ERROR: from element /GstPipeline:pipeline0/GstV4l2Src:v4l2src0: Device '/dev/video0' is busy\n"), "it's in use by another app");
        assert!(failure("WARNING: erroneous pipeline: no element \"v4l2src\"").starts_with("it needs GStreamer's Video4Linux plugin"));
        let unplugged = "ERROR: from element /GstPipeline:pipeline0/GstV4l2Src:v4l2src0: Could not read from resource.\nAdditional debug info:\n../sys/v4l2/gstv4l2bufferpool.c(1350): poll error 1: Success (0)\nExecution ended after 0:00:04.1\nFreeing pipeline ...\n";
        assert_eq!(failure(unplugged), "Could not read from resource");
        assert_eq!(failure("Setting pipeline to PAUSED ...\nIt stopped somehow.\n\n"), "It stopped somehow");
        assert_eq!(failure(""), "it stopped");
    }
}
