//! Recording the screen on Linux: GStreamer (`gst-launch-1.0`) reads the stream the screen-cast
//! portal shares (see `portal`) and encodes it with the best encoder installed for the codec chosen
//! in Settings, or as WebM, which every GStreamer install can write. GIFs come out of RSnip's own
//! encoder, fed raw frames.

use std::io::Read;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::portal::ScreenCast;

/// The file descriptor the pipeline reads PipeWire from.
const PIPEWIRE_FD: i32 = 3;
/// GIFs are bigger and slower to encode per frame, so they keep to this.
const GIF_FPS: u32 = 12;
/// Recordings stop on their own after this long, as on Windows.
pub const MAX_LENGTH: Duration = Duration::from_secs(30 * 60);

/// What recordings are saved as (see Settings → Recording).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub gif: bool,
    pub hevc: bool,
    pub fps: u32,
}

/// A part of the frame to record, in its pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// A GStreamer video encoder, and what goes after it.
#[derive(Clone, Copy, Debug)]
pub struct Encoder {
    /// The element, as `gst-inspect-1.0` knows it.
    pub element: &'static str,
    pub codec: &'static str,
    /// The encoder's settings, given the bitrate in kbit/s.
    settings: fn(u32) -> String,
    /// The parser and muxer after it.
    tail: &'static str,
    pub extension: &'static str,
}

/// Software encoders first: they work everywhere they're installed, where the graphics card's
/// (VA-API) may be listed but fail.
const H264: [Encoder; 4] = [
    Encoder { element: "x264enc", codec: "H.264", settings: |k| format!("x264enc speed-preset=veryfast tune=zerolatency bitrate={k} key-int-max=120"), tail: "h264parse ! mp4mux", extension: "mp4" },
    Encoder { element: "openh264enc", codec: "H.264", settings: |k| format!("openh264enc bitrate={}", k * 1000), tail: "h264parse ! mp4mux", extension: "mp4" },
    Encoder { element: "vah264lpenc", codec: "H.264", settings: |k| format!("vah264lpenc bitrate={k}"), tail: "h264parse ! mp4mux", extension: "mp4" },
    Encoder { element: "vah264enc", codec: "H.264", settings: |k| format!("vah264enc bitrate={k}"), tail: "h264parse ! mp4mux", extension: "mp4" },
];
const H265: [Encoder; 3] = [
    Encoder { element: "x265enc", codec: "H.265", settings: |k| format!("x265enc speed-preset=veryfast tune=zerolatency bitrate={k} key-int-max=120"), tail: "h265parse ! mp4mux", extension: "mp4" },
    Encoder { element: "vah265lpenc", codec: "H.265", settings: |k| format!("vah265lpenc bitrate={k}"), tail: "h265parse ! mp4mux", extension: "mp4" },
    Encoder { element: "vah265enc", codec: "H.265", settings: |k| format!("vah265enc bitrate={k}"), tail: "h265parse ! mp4mux", extension: "mp4" },
];
const WEBM: Encoder =
    Encoder { element: "vp8enc", codec: "VP8", settings: |k| format!("vp8enc deadline=1 cpu-used=8 target-bitrate={}", k * 1000), tail: "webmmux", extension: "webm" };

/// GStreamer has `element` installed.
fn installed(element: &str) -> bool {
    Command::new("gst-inspect-1.0").args(["--exists", element]).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

/// GStreamer and its PipeWire source are installed, which any recording needs.
pub fn available() -> Result<(), String> {
    if Command::new("gst-launch-1.0").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_err() {
        return Err("Recording needs GStreamer's tools (gstreamer1.0-tools on Debian and Ubuntu, gstreamer1-tools on Fedora, gstreamer on Arch)".into());
    }
    if !installed("pipewiresrc") {
        return Err("Recording needs GStreamer's PipeWire plugin (gstreamer1.0-pipewire on Debian and Ubuntu, pipewire-gstreamer on Fedora, gst-plugin-pipewire on Arch)".into());
    }
    Ok(())
}

/// The encoder for `hevc` (H.265, else H.264), and why it's not that one if it isn't.
pub fn encoder(hevc: bool) -> (Encoder, Option<String>) {
    let (wanted, codec) = if hevc { (&H265[..], "H.265") } else { (&H264[..], "H.264") };
    if let Some(&e) = wanted.iter().find(|e| installed(e.element)) {
        return (e, None);
    }
    // H.265 falls back to H.264 before WebM.
    if hevc {
        if let Some(&e) = H264.iter().find(|e| installed(e.element)) {
            return (e, Some("There's no H.265 encoder installed, so it was recorded as H.264.".into()));
        }
    }
    (WEBM, Some(format!("There's no {codec} encoder installed, so it was recorded as WebM.")))
}

/// What's missing for recording as `hevc` (H.265, else H.264) would have it, for Settings.
pub fn problem(hevc: bool) -> Option<String> {
    if let Err(e) = available() {
        return Some(e);
    }
    let has = |encoders: &[Encoder]| encoders.iter().any(|e| installed(e.element));
    let h264 = "x264 (gstreamer1.0-plugins-ugly on Debian and Ubuntu, gstreamer1-plugins-ugly from RPM Fusion on Fedora, gst-plugins-ugly on Arch)";
    match (hevc, has(&H265), has(&H264)) {
        (true, true, _) | (false, _, true) => None,
        (true, false, true) => Some("No H.265 encoder is installed, so MP4s will be H.264.".into()),
        _ => Some(format!("No H.264 encoder is installed, so recordings will be WebM (VP8) instead of MP4. Install {h264}.")),
    }
}

/// Whether there's an H.265 encoder installed.
pub fn hevc_available() -> bool {
    H265.iter().any(|e| installed(e.element))
}

/// The pipeline's source: the shared stream, as RGB.
fn source(node: u32) -> String {
    format!("pipewiresrc fd={PIPEWIRE_FD} path={node} do-timestamp=true keepalive-time=1000 resend-last=true ! videoconvert")
}

/// `area` of a `frame_width` × `frame_height` frame, for `videocrop`.
fn crop(area: Option<Area>, frame_width: u32, frame_height: u32) -> String {
    let Some(a) = area else { return String::new() };
    let right = frame_width.saturating_sub(a.x + a.width);
    let bottom = frame_height.saturating_sub(a.y + a.height);
    format!(" ! videocrop left={} top={} right={right} bottom={bottom}", a.x, a.y)
}

/// The output size: the area (or frame), even, as encoders need.
fn output_size(area: Option<Area>, frame_width: u32, frame_height: u32) -> (u32, u32) {
    let (w, h) = area.map_or((frame_width, frame_height), |a| (a.width, a.height));
    ((w & !1).max(16), (h & !1).max(16))
}

/// About 0.1 bit per pixel per frame looks clean for screen content in H.264; the rest need
/// about two thirds of that. In kbit/s.
fn bitrate(encoder: &Encoder, width: u32, height: u32, fps: u32) -> u32 {
    let bits = width as u64 * height as u64 * fps as u64 / 10;
    let bits = if encoder.codec == "H.264" { bits } else { bits * 2 / 3 };
    (bits / 1000).clamp(1000, 20_000) as u32
}

/// `gst-launch-1.0` arguments: one per token, so a path needs no quoting (it escapes each one).
fn args(flags: &[&str], pipeline: &str, location: Option<&Path>) -> Vec<String> {
    let tokens = flags.iter().copied().chain(pipeline.split_whitespace()).map(String::from);
    tokens.chain(location.map(|path| format!("location={}", path.to_string_lossy()))).collect()
}

/// The `gst-launch-1.0` arguments recording the stream `node` to `path`. With -e, Ctrl+C (how it's
/// stopped) finishes the file properly.
fn video_pipeline(node: u32, frame: (u32, u32), area: Option<Area>, fps: u32, encoder: &Encoder, path: &Path) -> Vec<String> {
    let (w, h) = output_size(area, frame.0, frame.1);
    let pipeline = format!(
        "{}{} ! videoscale add-borders=true ! videorate ! video/x-raw,format=I420,width={w},height={h},framerate={fps}/1 ! queue ! {} ! {} ! filesink",
        source(node),
        crop(area, frame.0, frame.1),
        (encoder.settings)(bitrate(encoder, w, h, fps)),
        encoder.tail,
    );
    args(&["-q", "-e"], &pipeline, Some(path))
}

/// The arguments for raw RGBA frames on standard output, for a GIF.
fn gif_pipeline(node: u32, frame: (u32, u32), area: Option<Area>) -> (Vec<String>, (u32, u32)) {
    let (w, h) = output_size(area, frame.0, frame.1);
    let pipeline = format!(
        "{}{} ! videoscale add-borders=true ! videorate ! videoconvert ! video/x-raw,format=RGBA,width={w},height={h},framerate={GIF_FPS}/1 ! fdsink fd=1",
        source(node),
        crop(area, frame.0, frame.1),
    );
    (args(&["-q", "-e"], &pipeline, None), (w, h))
}

/// Runs `gst-launch-1.0` with `args`, reading PipeWire from `pipewire`.
fn launch(args: &[String], pipewire: OwnedFd, stdout: Stdio) -> Result<Child, String> {
    let fd = pipewire.as_raw_fd();
    let mut command = Command::new("gst-launch-1.0");
    command.args(args).stdin(Stdio::null()).stdout(stdout).stderr(Stdio::piped());
    // Hand the connection over as fd 3 (dup2 also clears close-on-exec).
    unsafe {
        command.pre_exec(move || if libc::dup2(fd, PIPEWIRE_FD) < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) });
    }
    let child = command.spawn().map_err(|e| format!("Couldn't start GStreamer: {e}"))?;
    drop(pipewire);
    Ok(child)
}

/// What a failed pipeline said, last line first.
fn failure(child: &mut Child) -> String {
    let mut err = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut err);
    }
    let line = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it stopped").trim();
    format!("GStreamer couldn't record: {line}")
}

/// One frame of what's shared: to know its size, and to pick an area on.
pub fn snapshot(cast: &ScreenCast) -> Result<image::RgbaImage, String> {
    let file = std::env::temp_dir().join(format!("rsnip-frame-{}.png", std::process::id()));
    let pipeline = format!("{} ! video/x-raw,format=RGBA ! pngenc snapshot=true ! filesink", source(cast.node));
    let mut child = launch(&args(&["-q"], &pipeline, Some(&file)), cast.connect()?, Stdio::null())?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            return Err("The screen recording sent no picture".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    if !status.success() {
        return Err(failure(&mut child));
    }
    let frame = image::open(&file).map(|i| i.to_rgba8()).map_err(|e| format!("Couldn't read the first frame: {e}"));
    let _ = std::fs::remove_file(&file);
    frame
}

pub struct Recording {
    child: Child,
    pub path: PathBuf,
    pub started: Instant,
    /// Writes the GIF from the pipeline's frames.
    gif: Option<JoinHandle<Result<(), String>>>,
    note: Option<String>,
}

/// A finished recording, and anything worth saying about how it was made.
pub struct Recorded {
    pub path: PathBuf,
    pub note: Option<String>,
}

/// Starts recording what `cast` shares (`frame` is a frame of it) into the screenshots folder.
pub fn start(cast: &ScreenCast, frame: (u32, u32), area: Option<Area>, format: Format, folder: &Path) -> Result<Recording, String> {
    available()?;
    std::fs::create_dir_all(folder).map_err(|e| format!("Couldn't create {}: {e}", folder.display()))?;
    let name = |ext: &str| super::platform::new_path(folder, "Recording", ext);
    let mut recording = if format.gif {
        let path = name("gif");
        let (args, size) = gif_pipeline(cast.node, frame, area);
        let mut child = launch(&args, cast.connect()?, Stdio::piped())?;
        let frames = child.stdout.take().expect("piped");
        let file = std::fs::File::create(&path).map_err(|e| format!("Couldn't create {}: {e}", path.display()))?;
        let gif = std::thread::spawn(move || write_gif(frames, file, size));
        Recording { child, path, started: Instant::now(), gif: Some(gif), note: None }
    } else {
        let (encoder, note) = encoder(format.hevc);
        let path = name(encoder.extension);
        let args = video_pipeline(cast.node, frame, area, format.fps.clamp(1, 60), &encoder, &path);
        let child = launch(&args, cast.connect()?, Stdio::null())?;
        Recording { child, path, started: Instant::now(), gif: None, note }
    };
    // A pipeline that can't run (a missing element, say) stops straight away.
    std::thread::sleep(Duration::from_millis(800));
    if recording.child.try_wait().map_err(|e| e.to_string())?.is_some() {
        let error = failure(&mut recording.child);
        let _ = std::fs::remove_file(&recording.path);
        return Err(error);
    }
    Ok(recording)
}

impl Recording {
    /// Stops it and finishes the file.
    pub fn stop(mut self) -> Result<Recorded, String> {
        unsafe { libc::kill(self.child.id() as i32, libc::SIGINT) };
        let deadline = Instant::now() + Duration::from_secs(20);
        let status = loop {
            if let Some(status) = self.child.try_wait().map_err(|e| e.to_string())? {
                break status;
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                break self.child.wait().map_err(|e| e.to_string())?;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        if let Some(gif) = self.gif.take() {
            gif.join().unwrap_or_else(|_| Err("The GIF encoder stopped unexpectedly".into()))?;
        }
        let written = std::fs::metadata(&self.path).is_ok_and(|m| m.len() > 0);
        if !status.success() && !written {
            return Err(failure(&mut self.child));
        }
        Ok(Recorded { path: self.path, note: self.note })
    }

    /// The pipeline ended by itself (the shared window closed, say).
    pub fn ended(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_some()
    }
}

/// Encodes raw RGBA frames of `size` into a looping GIF. Encoding is slower than the frames come,
/// so it takes the newest one each time and shows each for as long as it was on screen, as on
/// Windows (see `encode::Gif`).
fn write_gif(mut frames: impl Read + Send + 'static, file: std::fs::File, size: (u32, u32)) -> Result<(), String> {
    use std::sync::{Arc, Condvar, Mutex};
    let (w, h) = (size.0 as u16, size.1 as u16);
    let mut encoder = gif::Encoder::new(std::io::BufWriter::new(file), w, h, &[]).map_err(|e| e.to_string())?;
    encoder.set_repeat(gif::Repeat::Infinite).map_err(|e| e.to_string())?;

    // The newest frame and when it came, and whether the pipeline has finished.
    let newest = Arc::new((Mutex::new((None::<(Vec<u8>, Instant)>, false)), Condvar::new()));
    let reader = newest.clone();
    let len = size.0 as usize * size.1 as usize * 4;
    std::thread::spawn(move || {
        let (slot, ready) = &*reader;
        loop {
            let mut buffer = vec![0; len];
            let read = frames.read_exact(&mut buffer).is_ok();
            let mut slot = slot.lock().unwrap();
            if read {
                slot.0 = Some((buffer, Instant::now()));
            } else {
                slot.1 = true;
            }
            ready.notify_one();
            if !read {
                break;
            }
        }
    });

    let delay = |d: Duration| ((d.as_millis() + 5) / 10).clamp(2, u16::MAX as u128) as u16;
    let mut pending: Option<(gif::Frame<'static>, Instant)> = None;
    loop {
        let (slot, ready) = &*newest;
        let next = {
            let mut slot = ready.wait_while(slot.lock().unwrap(), |s| s.0.is_none() && !s.1).unwrap();
            slot.0.take()
        };
        let Some((mut rgba, at)) = next else { break };
        if let Some((mut frame, shown)) = pending.take() {
            frame.delay = delay(at - shown);
            encoder.write_frame(&frame).map_err(|e| format!("Couldn't write the GIF: {e}"))?;
        }
        // Speed 10 of 30: much faster quantising than the default, still good for screen content.
        pending = Some((gif::Frame::from_rgba_speed(w, h, &mut rgba, 10), at));
    }
    if let Some((mut frame, _)) = pending {
        frame.delay = delay(Duration::from_secs(1) / GIF_FPS);
        encoder.write_frame(&frame).map_err(|e| format!("Couldn't write the GIF: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crops_and_evens_the_area() {
        let area = Area { x: 10, y: 20, width: 301, height: 199 };
        assert_eq!(crop(Some(area), 1920, 1080), " ! videocrop left=10 top=20 right=1609 bottom=861");
        assert_eq!(crop(None, 1920, 1080), "");
        assert_eq!(output_size(Some(area), 1920, 1080), (300, 198));
        assert_eq!(output_size(None, 1366, 767), (1366, 766));
    }

    #[test]
    fn keeps_the_path_in_one_argument() {
        let args = video_pipeline(42, (1920, 1080), None, 30, &H264[0], Path::new("/home/a b/Recording \"1\".mp4"));
        assert_eq!(&args[..2], ["-q", "-e"]);
        assert!(args.contains(&"path=42".to_string()));
        assert!(args.contains(&"x264enc".to_string()) && args.contains(&"bitrate=6220".to_string()), "{args:?}");
        assert_eq!(args.last().unwrap(), r#"location=/home/a b/Recording "1".mp4"#);
    }

    #[test]
    fn bitrates_suit_the_size_and_codec() {
        assert_eq!(bitrate(&H264[0], 1920, 1080, 30), 6220);
        assert_eq!(bitrate(&H265[0], 1920, 1080, 30), 4147);
        assert_eq!(bitrate(&H264[0], 200, 100, 15), 1000, "never below 1 Mbit/s");
        assert_eq!(bitrate(&H264[0], 3840, 2160, 60), 20_000, "nor above 20");
    }

    /// The pipelines RSnip builds, run with a test picture in place of the screen, where GStreamer
    /// is installed (CI's Linux job has it).
    #[test]
    fn pipelines_record_playable_files() {
        if available().is_err() || !installed("videotestsrc") {
            return eprintln!("skipped: no GStreamer");
        }
        let dir = std::env::temp_dir().join(format!("rsnip-gst-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let area = Some(Area { x: 8, y: 6, width: 201, height: 151 });
        let mut cases: Vec<(Encoder, &str)> = [&H264[..], &H265[..], &[WEBM]].concat().into_iter().filter(|e| installed(e.element)).map(|e| (e, e.extension)).collect();
        cases.dedup_by_key(|c| c.0.codec);
        for (encoder, ext) in cases {
            let path = dir.join(format!("test-{}.{ext}", encoder.element));
            // A test picture in place of the screen: the flags, then the source's tokens.
            let pipeline = video_pipeline(0, (320, 240), area, 30, &encoder, &path);
            let skip = 2 + source(0).split_whitespace().count();
            let test = ["-q", "-e", "videotestsrc", "is-live=true", "!", "videoconvert"].map(String::from);
            let args: Vec<String> = test.into_iter().chain(pipeline[skip..].iter().cloned()).collect();
            let mut child = Command::new("gst-launch-1.0").args(&args).stderr(Stdio::piped()).spawn().unwrap();
            std::thread::sleep(Duration::from_millis(1500));
            unsafe { libc::kill(child.id() as i32, libc::SIGINT) };
            assert!(child.wait().unwrap().success(), "{}: {}", encoder.element, failure(&mut child));
            let bytes = std::fs::read(&path).unwrap();
            assert!(bytes.len() > 1000, "{} wrote {} bytes", encoder.element, bytes.len());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gifs_get_every_frame() {
        let (w, h) = (8, 6);
        let raw: Vec<u8> = (0..3).flat_map(|i| std::iter::repeat_n([i * 80, 0, 0, 255], w * h).flatten()).collect();
        let path = std::env::temp_dir().join(format!("rsnip-gif-{}.gif", std::process::id()));
        // Frames that come slowly, so each is encoded.
        struct Slow(std::io::Cursor<Vec<u8>>);
        impl Read for Slow {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                std::thread::sleep(Duration::from_millis(30));
                self.0.read(buf)
            }
        }
        write_gif(Slow(std::io::Cursor::new(raw)), std::fs::File::create(&path).unwrap(), (w as u32, h as u32)).unwrap();
        let mut decoder = gif::DecodeOptions::new().read_info(std::fs::File::open(&path).unwrap()).unwrap();
        let mut delays = Vec::new();
        while let Some(frame) = decoder.read_next_frame().unwrap() {
            delays.push(frame.delay);
        }
        std::fs::remove_file(&path).unwrap();
        assert_eq!(delays.len(), 3);
        assert!(delays[0] >= 2 && delays[0] <= 6, "about as long as it was shown: {delays:?}");
        assert_eq!(delays[2], 8, "the last gets a frame's time");
    }
}
