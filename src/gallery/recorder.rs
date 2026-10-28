//! Recording the screen on Linux: GStreamer (`gst-launch-1.0`) reads the stream the screen-cast
//! portal shares (see `portal`) and encodes it with the best encoder installed for the codec chosen
//! in Settings, or as WebM, which every GStreamer install can write. GIFs come out of RSnip's own
//! encoder, fed raw frames. Sound comes from PulseAudio (which PipeWire speaks too): what the
//! computer plays, as the default output's monitor, and a microphone, mixed into one AAC track in
//! MP4s or Opus in WebMs.

use std::io::Read;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::camera::{self, Bubble};
use super::portal::ScreenCast;

/// The file descriptor the pipeline reads PipeWire from.
const PIPEWIRE_FD: i32 = 3;
/// GIFs are bigger and slower to encode per frame, so they keep to this.
const GIF_FPS: u32 = 12;
/// Recordings stop on their own after this long, as on Windows.
pub const MAX_LENGTH: Duration = Duration::from_secs(30 * 60);

/// What recordings are saved as (see Settings → Recording).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Format {
    pub gif: bool,
    pub hevc: bool,
    pub fps: u32,
    /// Record what the computer plays: none, the default output's, or the output whose monitor has
    /// this name in PulseAudio.
    pub sound: Option<Option<String>>,
    /// Record a microphone: none, the default one, or the one with this name in PulseAudio.
    pub microphone: Option<Option<String>>,
    /// Show the camera on top while recording (see `camera`): none, the first one, or the one with
    /// this device path.
    pub camera: Option<Option<String>>,
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
    /// The encoder's settings, given the bitrate in kbit/s and the frame rate (a keyframe every
    /// second, so trimming and seeking land within a second; see mp4.rs).
    settings: fn(u32, u32) -> String,
    /// The parser and muxer after it.
    tail: &'static str,
    pub extension: &'static str,
}

/// Software encoders first: they work everywhere they're installed, where the graphics card's
/// (VA-API) may be listed but fail.
const H264: [Encoder; 4] = [
    Encoder { element: "x264enc", codec: "H.264", settings: |k, fps| format!("x264enc speed-preset=veryfast tune=zerolatency bitrate={k} key-int-max={fps}"), tail: "h264parse ! mp4mux", extension: "mp4" },
    Encoder { element: "openh264enc", codec: "H.264", settings: |k, fps| format!("openh264enc bitrate={} gop-size={fps}", k * 1000), tail: "h264parse ! mp4mux", extension: "mp4" },
    Encoder { element: "vah264lpenc", codec: "H.264", settings: |k, fps| format!("vah264lpenc bitrate={k} key-int-max={fps}"), tail: "h264parse ! mp4mux", extension: "mp4" },
    Encoder { element: "vah264enc", codec: "H.264", settings: |k, fps| format!("vah264enc bitrate={k} key-int-max={fps}"), tail: "h264parse ! mp4mux", extension: "mp4" },
];
const H265: [Encoder; 3] = [
    Encoder { element: "x265enc", codec: "H.265", settings: |k, fps| format!("x265enc speed-preset=veryfast tune=zerolatency bitrate={k} key-int-max={fps}"), tail: "h265parse ! mp4mux", extension: "mp4" },
    Encoder { element: "vah265lpenc", codec: "H.265", settings: |k, fps| format!("vah265lpenc bitrate={k} key-int-max={fps}"), tail: "h265parse ! mp4mux", extension: "mp4" },
    Encoder { element: "vah265enc", codec: "H.265", settings: |k, fps| format!("vah265enc bitrate={k} key-int-max={fps}"), tail: "h265parse ! mp4mux", extension: "mp4" },
];
const WEBM: Encoder =
    Encoder { element: "vp8enc", codec: "VP8", settings: |k, fps| format!("vp8enc deadline=1 cpu-used=8 target-bitrate={} keyframe-max-dist={fps}", k * 1000), tail: "webmmux", extension: "webm" };

impl Encoder {
    /// The GStreamer elements it needs: itself, then the parser and muxer after it. They're often in
    /// different packages (x264 in plugins-ugly, its parser in plugins-bad).
    fn parts(&self) -> impl Iterator<Item = &'static str> {
        std::iter::once(self.element).chain(self.tail.split(" ! "))
    }
}

/// A GStreamer audio encoder, for the sound in a recording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioEncoder {
    /// The element, as `gst-inspect-1.0` knows it.
    element: &'static str,
    /// What it's called in a message.
    codec: &'static str,
    /// The element with its settings: 160 kbit/s, which all of them take in bit/s.
    settings: &'static str,
}

/// AAC for MP4s, best first: Fraunhofer's is the best but not in Debian and Ubuntu; FFmpeg's comes
/// with the H.264 decoder (gstreamer1.0-libav); VisualOn's is in plugins-bad.
const AAC: [AudioEncoder; 3] = [
    AudioEncoder { element: "fdkaacenc", codec: "AAC", settings: "fdkaacenc bitrate=160000" },
    AudioEncoder { element: "avenc_aac", codec: "AAC", settings: "avenc_aac bitrate=160000" },
    AudioEncoder { element: "voaacenc", codec: "AAC", settings: "voaacenc bitrate=160000" },
];
/// WebM takes Opus, which plugins-base writes.
const OPUS: AudioEncoder = AudioEncoder { element: "opusenc", codec: "Opus", settings: "opusenc bitrate=160000" };

/// The sound of a recording: its sources, mixed.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Sound {
    /// GStreamer source elements with their settings.
    sources: Vec<String>,
    encoder: AudioEncoder,
}

/// The audio encoder for an MP4 (else WebM), or the note saying there is none.
fn audio_encoder(mp4: bool, has: &impl Fn(&str) -> bool) -> Result<AudioEncoder, String> {
    if !mp4 {
        return if has(OPUS.element) { Ok(OPUS) } else { Err("No Opus encoder is installed, so it was recorded without sound. Install gstreamer1.0-plugins-base.".into()) };
    }
    AAC.iter().find(|e| has(e.element)).copied().ok_or_else(|| "No AAC encoder is installed, so it was recorded without sound. Install gstreamer1.0-libav.".into())
}

/// What to install for GStreamer's PulseAudio source, and for `pactl`, which lists the sources.
const NO_PULSESRC: &str = "Recording sound needs GStreamer's PulseAudio plugin (gstreamer1.0-pulseaudio on Debian and Ubuntu, gstreamer1-plugins-good on Fedora, gst-plugins-good on Arch)";
const NO_PACTL: &str = "Recording sound needs pactl (pulseaudio-utils on Debian, Ubuntu and Fedora, libpulse on Arch)";

/// A source of sound that PulseAudio (or PipeWire's server for it) lists.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Source {
    name: String,
    description: String,
    /// What an output plays, rather than a microphone.
    monitor: bool,
    /// The server's default source, which plain `pulsesrc` records.
    default: bool,
}

/// How long `pactl` has to answer: a sound server that's stuck shouldn't hold up a recording, or
/// Settings' lists, for longer.
const PACTL_TIMEOUT: Duration = Duration::from_secs(5);

/// `pactl`'s output, if it ran and the sound server answered in time.
fn pactl(args: &[&str]) -> Option<String> {
    let mut command = Command::new("pactl");
    command.args(args);
    output_of(command, PACTL_TIMEOUT)
}

/// What `command` writes on its standard output, if it ran, succeeded and finished within
/// `timeout` (it's killed if it doesn't).
pub(super) fn output_of(mut command: Command, timeout: Duration) -> Option<String> {
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    // Read as it comes, so a long list can't fill the pipe and stop it finishing.
    let mut stdout = child.stdout.take()?;
    let reading = std::thread::spawn(move || {
        let mut out = Vec::new();
        stdout.read_to_end(&mut out).map(|_| out)
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let out = reading.join().ok()?.ok()?;
    status.success().then(|| String::from_utf8_lossy(&out).into_owned())
}

/// The sound server's sources, or None if `pactl` isn't installed or it can't be reached. Older
/// `pactl`s have no JSON output, so there are only the names.
fn sources() -> Option<Vec<Source>> {
    pactl(&["-f", "json", "list", "sources"]).and_then(|json| parse_sources(&json)).or_else(|| pactl(&["list", "short", "sources"]).map(|list| parse_short_sources(&list)))
}

/// `pactl -f json list sources`.
fn parse_sources(json: &str) -> Option<Vec<Source>> {
    let list: Vec<serde_json::Value> = serde_json::from_str(json).ok()?;
    let text = |source: &serde_json::Value, key: &str| source[key].as_str().filter(|t| !t.is_empty()).map(String::from);
    Some(
        list.iter()
            .filter_map(|s| {
                let name = text(s, "name")?;
                let monitor = name.ends_with(".monitor") || text(s, "monitor_source").is_some();
                Some(Source { description: text(s, "description").unwrap_or_else(|| name.clone()), name, monitor, default: false })
            })
            .collect(),
    )
}

/// `pactl list short sources`: tab-separated, the name second.
fn parse_short_sources(list: &str) -> Vec<Source> {
    let names = list.lines().filter_map(|line| line.split('\t').nth(1)).filter(|n| !n.is_empty());
    names.map(|name| Source { name: name.into(), description: name.into(), monitor: name.ends_with(".monitor"), default: false }).collect()
}

/// `sources`, with the server's default one marked, for a recording (`pactl get-default-source`;
/// older `pactl`s don't say, so none is).
fn sources_with_default() -> Option<Vec<Source>> {
    let sources = sources()?;
    Some(mark_default(sources, pactl(&["get-default-source"]).as_deref().map(str::trim)))
}

/// `sources`, with the one named `default` marked as the default.
fn mark_default(sources: Vec<Source>, default: Option<&str>) -> Vec<Source> {
    sources.into_iter().map(|s| Source { default: Some(s.name.as_str()) == default, ..s }).collect()
}

/// The source to record for "Default microphone": the server's default (plain `pulsesrc`, `None`),
/// unless that's an output's monitor (as when there's no microphone to default to), which would
/// record the computer's sound again, when it's the first microphone there is. None at all if
/// there's no microphone.
fn default_microphone(sources: &[Source]) -> Option<Option<&str>> {
    let first = sources.iter().find(|s| !s.monitor)?;
    Some(sources.iter().any(|s| s.default && s.monitor).then_some(first.name.as_str()))
}

/// A microphone to record from.
pub fn microphones() -> Vec<super::platform::SoundDevice> {
    let microphones = sources().unwrap_or_default().into_iter().filter(|s| !s.monitor);
    microphones.map(|s| super::platform::SoundDevice { id: s.name, name: s.description }).collect()
}

/// An output whose sound can be recorded: its monitor.
pub fn outputs() -> Vec<super::platform::SoundDevice> {
    let monitors = sources().unwrap_or_default().into_iter().filter(|s| s.monitor);
    monitors.map(|s| super::platform::SoundDevice { id: s.name, name: s.description }).collect()
}

/// The sound for a recording as `format` asks, given the encoder for it (`audio`, else why there
/// is none), what GStreamer has installed (`has`), and the sound server's sources (`sources`, none
/// if it can't be reached) — and a note on anything asked for that couldn't be had.
fn plan_sound(format: &Format, audio: Result<AudioEncoder, String>, has: &impl Fn(&str) -> bool, sources: &impl Fn() -> Option<Vec<Source>>) -> (Option<Sound>, Option<String>) {
    if format.sound.is_none() && format.microphone.is_none() {
        return (None, None);
    }
    let none = |note: String| (None, Some(note));
    let encoder = match audio {
        Ok(encoder) => encoder,
        Err(note) => return none(note),
    };
    if !has("pulsesrc") {
        return none(format!("{NO_PULSESRC}, so it was recorded without sound."));
    }
    let Some(sources) = sources() else {
        return none("Couldn't reach the sound server, so it was recorded without sound.".into());
    };
    let mut wanted = Vec::new();
    let mut notes = Vec::new();
    match &format.sound {
        None => {}
        Some(Some(id)) if sources.iter().any(|s| s.monitor && s.name == *id) => wanted.push(format!("pulsesrc device={id} do-timestamp=true")),
        Some(chosen) => {
            if chosen.is_some() {
                notes.push("The chosen output isn't connected, so the default one's sound was recorded.");
            }
            wanted.push("pulsesrc device=@DEFAULT_MONITOR@ do-timestamp=true".to_string());
        }
    }
    match &format.microphone {
        None => {}
        Some(None) => match default_microphone(&sources) {
            Some(None) => wanted.push("pulsesrc do-timestamp=true".into()),
            Some(Some(id)) => wanted.push(format!("pulsesrc device={id} do-timestamp=true")),
            None => notes.push("No microphone was found, so it was recorded without one."),
        },
        Some(Some(id)) if sources.iter().any(|s| !s.monitor && s.name == *id) => wanted.push(format!("pulsesrc device={id} do-timestamp=true")),
        Some(Some(_)) => notes.push("The chosen microphone isn't connected, so it was recorded without one."),
    }
    if !wanted.is_empty() && !(has("audiomixer") && has("audiotestsrc")) {
        // Both are in plugins-base, so this is a broken install more than anything.
        return none("Mixing sound needs GStreamer's audiomixer and audiotestsrc (gstreamer1.0-plugins-base on Debian and Ubuntu), so it was recorded without sound.".into());
    }
    let note = (!notes.is_empty()).then(|| notes.join(" "));
    ((!wanted.is_empty()).then_some(Sound { sources: wanted, encoder }), note)
}

/// Silence, mixed in with the sound: the mixer is live, so it goes on without a source that sends
/// nothing (a microphone xrdp doesn't redirect, say), and keeps the sound in step meanwhile, as on
/// Windows.
const SILENCE: &str = "audiotestsrc wave=silence is-live=true";

/// The pipeline for `sound`, ending in the muxer `mux`: each source as 48 kHz stereo, mixed with
/// silence.
fn audio_pipeline(sound: &Sound) -> String {
    let chain = |source: &str| format!("{source} ! audioconvert ! audioresample ! audio/x-raw,rate=48000,channels=2 ! queue ! mix.");
    let sources: Vec<String> = sound.sources.iter().map(String::as_str).chain([SILENCE]).map(chain).collect();
    // The muxer waits for the video, which comes only when the screen changes (see `source`), so
    // the encoded sound waits in a queue with no limit (it's small) rather than holding up the
    // sources, which would lose what they hear meanwhile.
    format!("{} audiomixer name=mix ! audioconvert ! {} ! queue max-size-buffers=0 max-size-bytes=0 max-size-time=0 ! mux.", sources.join(" "), sound.encoder.settings)
}

/// What's missing for recording sound, for Settings: `mp4` if recordings are MP4s, `pactl` if
/// that's installed, and what GStreamer has installed (`has`).
fn sound_problem(mp4: bool, pactl: bool, has: &impl Fn(&str) -> bool) -> Option<String> {
    let mut problems = Vec::new();
    match audio_encoder(mp4, has) {
        Ok(_) => {}
        Err(_) if mp4 => problems.push("No AAC encoder is installed, so MP4s will have no sound. Install gstreamer1.0-libav on Debian and Ubuntu, gstreamer1-plugin-libav on Fedora, gst-libav on Arch.".to_string()),
        Err(_) => problems.push("No Opus encoder is installed, so WebMs will have no sound. Install gstreamer1.0-plugins-base.".to_string()),
    }
    if !has("pulsesrc") {
        problems.push(format!("{NO_PULSESRC}, so recordings will have no sound."));
    }
    if !pactl {
        problems.push(format!("{NO_PACTL}, so recordings will have no sound."));
    }
    (!problems.is_empty()).then(|| problems.join(" "))
}

/// Where a parser or muxer comes from, for saying what to install.
fn package(element: &str) -> &'static str {
    match element {
        "h264parse" | "h265parse" => "gstreamer1.0-plugins-bad on Debian and Ubuntu, gstreamer1-plugins-bad-free on Fedora, gst-plugins-bad on Arch",
        _ => "gstreamer1.0-plugins-good on Debian and Ubuntu, gstreamer1-plugins-good on Fedora, gst-plugins-good on Arch",
    }
}

/// The first of `encoders` with everything it needs installed (`has`).
fn usable(encoders: &[Encoder], has: &impl Fn(&str) -> bool) -> Option<Encoder> {
    encoders.iter().find(|e| e.parts().all(has)).copied()
}

/// Why none of `encoders` (for `codec`) can be used: what the first one installed lacks, or that
/// none is.
fn why_not(codec: &str, encoders: &[Encoder], has: &impl Fn(&str) -> bool) -> String {
    let Some(encoder) = encoders.iter().find(|e| has(e.element)) else {
        return format!("There's no {codec} encoder installed");
    };
    let missing: Vec<&str> = encoder.parts().filter(|p| !has(p)).collect();
    format!("{codec} needs GStreamer's {} ({})", missing.join(" and "), package(missing[0]))
}

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
    choose(hevc, &|e| installed(e))
}

/// `encoder`, given what GStreamer has installed (`has`).
fn choose(hevc: bool, has: &impl Fn(&str) -> bool) -> (Encoder, Option<String>) {
    if let Some(e) = usable(if hevc { &H265 } else { &H264 }, has) {
        return (e, None);
    }
    // H.265 falls back to H.264 before WebM.
    if hevc {
        if let Some(e) = usable(&H264, has) {
            return (e, Some(format!("{}, so it was recorded as H.264.", why_not("H.265", &H265, has))));
        }
    }
    (WEBM, Some(format!("{}, so it was recorded as WebM.", why_not("H.264", &H264, has))))
}

/// What's missing for recording as `hevc` (H.265, else H.264) would have it, for Settings.
pub fn problem(hevc: bool) -> Option<String> {
    if let Err(e) = available() {
        return Some(e);
    }
    let pactl = Command::new("pactl").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok();
    problems(hevc, pactl, &|e| installed(e))
}

/// `problem`, past GStreamer itself, given whether `pactl` is installed and what GStreamer has
/// installed (`has`): the video's, then the sound's.
fn problems(hevc: bool, pactl: bool, has: &impl Fn(&str) -> bool) -> Option<String> {
    let video = problem_with(hevc, has);
    let sound = sound_problem(choose(hevc, has).0.extension == "mp4", pactl, has);
    match (video, sound) {
        (Some(video), Some(sound)) => Some(format!("{video} {sound}")),
        (video, sound) => video.or(sound),
    }
}

/// The video part of `problems`.
fn problem_with(hevc: bool, has: &impl Fn(&str) -> bool) -> Option<String> {
    let h264 = "x264 (gstreamer1.0-plugins-ugly on Debian and Ubuntu, gstreamer1-plugins-ugly from RPM Fusion on Fedora, gst-plugins-ugly on Arch)";
    match (hevc, usable(&H265, has).is_some(), usable(&H264, has).is_some()) {
        (true, true, _) | (false, _, true) => None,
        (true, false, true) => Some(format!("{}, so MP4s will be H.264.", why_not("H.265", &H265, has))),
        _ if !H264.iter().any(|e| has(e.element)) => {
            Some(format!("No H.264 encoder is installed, so recordings will be WebM (VP8) instead of MP4. Install {h264}."))
        }
        _ => Some(format!("{}, so recordings will be WebM (VP8) instead of MP4.", why_not("H.264", &H264, has))),
    }
}

/// Whether there's an H.265 encoder installed, with what it needs.
pub fn hevc_available() -> bool {
    usable(&H265, &|e| installed(e)).is_some()
}

/// Records a moment of a test picture as a recording would be made now (`hevc` as in Settings),
/// for `rsnip --check-recording`: what it came out as, or why it couldn't be recorded.
pub fn check(hevc: bool) -> Result<String, String> {
    available()?;
    let (encoder, note) = encoder(hevc);
    // Sound is tried with a test tone, which needs no sound server; whether the server is there
    // comes from planning a recording with sound.
    let audio = audio_encoder(encoder.extension == "mp4", &|e| installed(e));
    let asked = Format { gif: false, hevc, fps: 30, sound: Some(None), microphone: None, camera: None };
    let sound_note = plan_sound(&asked, audio.clone(), &|e| installed(e), &sources).1;
    let dir = super::platform::runtime_dir().join(format!("check-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let path = dir.join(format!("check.{}", encoder.extension));
    let recorded = record_test_picture(&encoder, audio.as_ref().ok(), &path, Duration::from_millis(1500));
    // And plays back in the library, which needs a decoder as well.
    let played = recorded.is_ok().then(|| super::player::can_play(&path));
    let _ = std::fs::remove_dir_all(&dir);
    recorded?;
    let mut made = format!("Recording works: {} {} with {}", encoder.codec, encoder.extension.to_uppercase(), encoder.element);
    if let (Ok(audio), None) = (&audio, &sound_note) {
        made.push_str(&format!(" and {} sound ({})", audio.codec, audio.element));
    }
    made.push('.');
    let reachable = sound_note.is_none();
    for note in note.into_iter().chain(sound_note) {
        made = format!("{made}\n{note}");
    }
    if reachable {
        let names: Vec<String> = super::platform::microphones().into_iter().map(|m| m.name).collect();
        made.push_str(&if names.is_empty() { "\nNo microphone was found.".to_string() } else { format!("\nMicrophones: {}.", names.join(", ")) });
    }
    match played {
        Some(Err(e)) => made.push_str(&format!("\nPlayback doesn't work: {e}")),
        _ => made.push_str("\nPlayback works."),
    }
    Ok(made)
}

/// Records a test picture to `path` through `encoder`, with a test tone as the sound if there's an
/// `audio` encoder, for about `length` (GStreamer's start-up included), with the pipeline a
/// recording uses.
fn record_test_picture(encoder: &Encoder, audio: Option<&AudioEncoder>, path: &Path, length: Duration) -> Result<(), String> {
    let area = Some(Area { x: 8, y: 6, width: 201, height: 151 });
    let sound = audio.map(|&encoder| Sound { sources: vec!["audiotestsrc is-live=true wave=sine".into()], encoder });
    let args = video_pipeline("videotestsrc is-live=true ! videoconvert", (320, 240), area, 30, encoder, sound.as_ref(), path);
    let mut child = Command::new("gst-launch-1.0").args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| format!("Couldn't start GStreamer: {e}"))?;
    let said = said(&mut child);
    std::thread::sleep(length);
    unsafe { libc::kill(child.id() as i32, libc::SIGINT) };
    if !child.wait().map_err(|e| e.to_string())?.success() {
        return Err(failure(said));
    }
    match std::fs::metadata(path) {
        Ok(m) if m.len() > 1000 => Ok(()),
        _ => Err(format!("GStreamer wrote no {} file", encoder.extension.to_uppercase())),
    }
}

/// The pipeline's source: the shared stream, as RGB.
fn source(node: u32) -> String {
    // Not keepalive-time or resend-last (which repeat the last frame while the screen is still, as
    // GNOME's recorder has them): with either, Ctrl+C's end of stream never reaches the file, so a
    // recording can't be finished.
    format!("pipewiresrc fd={PIPEWIRE_FD} path={node} do-timestamp=true ! videoconvert")
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

/// The `gst-launch-1.0` arguments recording the video from `source` (a `frame` of it) to `path`,
/// with `sound` if any: its pipeline comes first, since the file's location has to be the last
/// argument, and ends in the muxer, which the video's pipeline names. With -e, Ctrl+C (how it's
/// stopped) finishes the file properly.
fn video_pipeline(source: &str, frame: (u32, u32), area: Option<Area>, fps: u32, encoder: &Encoder, sound: Option<&Sound>, path: &Path) -> Vec<String> {
    let (w, h) = output_size(area, frame.0, frame.1);
    let pipeline = format!(
        "{}{}{} ! videoscale add-borders=true ! videorate ! video/x-raw,format=I420,width={w},height={h},framerate={fps}/1 ! queue ! {} ! {} name=mux ! filesink",
        sound.map_or(String::new(), |s| audio_pipeline(s) + " "),
        source,
        crop(area, frame.0, frame.1),
        (encoder.settings)(bitrate(encoder, w, h, fps), fps),
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

/// Runs `gst-launch-1.0` with `args`, reading PipeWire from `pipewire`, and what it says (see
/// `said`).
fn launch(args: &[String], pipewire: OwnedFd, stdout: Stdio) -> Result<(Child, Option<JoinHandle<String>>), String> {
    let fd = pipewire.as_raw_fd();
    let mut command = Command::new("gst-launch-1.0");
    command.args(args).stdin(Stdio::null()).stdout(stdout).stderr(Stdio::piped());
    // Hand the connection over as fd 3 (dup2 also clears close-on-exec).
    unsafe {
        command.pre_exec(move || if libc::dup2(fd, PIPEWIRE_FD) < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) });
    }
    let mut child = command.spawn().map_err(|e| format!("Couldn't start GStreamer: {e}"))?;
    drop(pipewire);
    let said = said(&mut child);
    Ok((child, said))
}

/// Reads what a pipeline says on its standard error as it comes, so it never fills the pipe and
/// holds the pipeline up (with GST_DEBUG set it says a lot), keeping the end of it.
pub(super) fn said(child: &mut Child) -> Option<JoinHandle<String>> {
    let mut stderr = child.stderr.take()?;
    Some(std::thread::spawn(move || {
        let (mut kept, mut chunk) = (Vec::new(), [0; 4096]);
        while let Ok(n @ 1..) = stderr.read(&mut chunk) {
            kept.extend_from_slice(&chunk[..n]);
            kept.drain(..kept.len().saturating_sub(16 * 1024));
        }
        String::from_utf8_lossy(&kept).into_owned()
    }))
}

/// The last thing a pipeline that has ended said (see `said`), if anything.
fn last_line(said: Option<JoinHandle<String>>) -> Option<String> {
    let err = said.and_then(|s| s.join().ok())?;
    err.lines().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_string())
}

/// What a failed pipeline said, last line first.
fn failure(said: Option<JoinHandle<String>>) -> String {
    format!("GStreamer couldn't record: {}", last_line(said).as_deref().unwrap_or("it stopped"))
}

/// One frame of what's shared: to know its size, and to pick an area on.
pub fn snapshot(cast: &ScreenCast) -> Result<image::RgbaImage, String> {
    let file = super::platform::runtime_dir().join(format!("frame-{}.png", std::process::id()));
    let pipeline = format!("{} ! video/x-raw,format=RGBA ! pngenc snapshot=true ! filesink", source(cast.node));
    let (mut child, said) = launch(&args(&["-q"], &pipeline, Some(&file)), cast.connect()?, Stdio::null())?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if Instant::now() > deadline {
            // Waited for too, or it stays behind in the background process until that quits.
            let _ = child.kill();
            let _ = child.wait();
            return Err("The screen recording sent no picture".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    if !status.success() {
        return Err(failure(said));
    }
    let frame = image::open(&file).map(|i| i.to_rgba8()).map_err(|e| format!("Couldn't read the first frame: {e}"));
    let _ = std::fs::remove_file(&file);
    frame
}

pub struct Recording {
    child: Child,
    /// What the pipeline says (see `said`).
    said: Option<JoinHandle<String>>,
    pub path: PathBuf,
    /// Where it's written until it's finished (see `working_path`).
    working: PathBuf,
    pub started: Instant,
    /// Writes the GIF from the pipeline's frames.
    gif: Option<JoinHandle<Result<(), String>>>,
    note: Option<String>,
    /// The camera's bubble, if it's shown.
    camera: Option<Bubble>,
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
    if format.gif {
        let path = name("gif");
        let working = super::working_path(&path)?;
        let (args, size) = gif_pipeline(cast.node, frame, area);
        let (mut child, said) = launch(&args, cast.connect()?, Stdio::piped())?;
        let frames = child.stdout.take().expect("piped");
        let file = std::fs::File::create(&working).map_err(|e| format!("Couldn't create {}: {e}", working.display()))?;
        let gif = std::thread::spawn(move || write_gif(frames, file, size));
        let mut recording = Recording { child, said, path, working, started: Instant::now(), gif: Some(gif), note: None, camera: None };
        running(&mut recording)?;
        recording.camera = bubble(&format, cast, frame, area);
        return Ok(recording);
    }
    let (encoder, video_note) = encoder(format.hevc);
    let (mut sound, sound_note) = plan_sound(&format, audio_encoder(encoder.extension == "mp4", &|e| installed(e)), &|e| installed(e), &sources_with_default);
    let mut notes: Vec<String> = [video_note, sound_note].into_iter().flatten().collect();
    let path = name(encoder.extension);
    let working = super::working_path(&path)?;
    let mut sound_error = None;
    loop {
        let args = video_pipeline(&source(cast.node), frame, area, format.fps.clamp(1, 60), &encoder, sound.as_ref(), &working);
        let (child, said) = launch(&args, cast.connect()?, Stdio::null())?;
        let mut recording = Recording { child, said, path: path.clone(), working: working.clone(), started: Instant::now(), gif: None, note: None, camera: None };
        match running(&mut recording) {
            Ok(()) => {
                // It was the sound only if it records without.
                notes.extend(sound_error.map(|error| format!("Sound couldn't be recorded, so it was recorded without it. ({error})")));
                recording.note = (!notes.is_empty()).then(|| notes.join(" "));
                recording.camera = bubble(&format, cast, frame, area);
                return Ok(recording);
            }
            // The sound is the likeliest thing to stop it (the sound server went away, a device
            // is busy), and shouldn't stop the recording: it starts again without.
            Err(error) if sound.is_some() => {
                sound = None;
                sound_error = Some(error);
            }
            Err(error) => return Err(error),
        }
    }
}

/// The camera's bubble for a recording of `cast` (a `frame` of it, or the `area` of that), if
/// `format` has it: where it was last dragged to in what's recorded, or by its bottom right. A
/// window that's shared is recorded by itself, without what's on top of it, so there's no bubble
/// for one.
fn bubble(format: &Format, cast: &ScreenCast, frame: (u32, u32), area: Option<Area>) -> Option<Bubble> {
    let device = format.camera.clone()?;
    if cast.window {
        return Some(Bubble::left_out("The camera is only recorded with a screen or an area, not a window, so it wasn't shown.".into()));
    }
    Some(Bubble::start(&camera::Launch::in_area(device, bounds(cast.origin, frame, area))))
}

/// The left, top, right and bottom of `area` of a `frame` (all of it, without one) of the screen
/// whose top left is at `origin` on the desktop.
fn bounds(origin: (i32, i32), frame: (u32, u32), area: Option<Area>) -> [i32; 4] {
    let area = area.unwrap_or(Area { x: 0, y: 0, width: frame.0, height: frame.1 });
    let (left, top) = (origin.0 + area.x as i32, origin.1 + area.y as i32);
    [left, top, left + area.width as i32, top + area.height as i32]
}

/// Gives a pipeline a moment to start, and fails if it stopped straight away (a missing element,
/// say).
fn running(recording: &mut Recording) -> Result<(), String> {
    std::thread::sleep(Duration::from_millis(800));
    if recording.child.try_wait().map_err(|e| e.to_string())?.is_some() {
        let error = failure(recording.said.take());
        let _ = std::fs::remove_file(&recording.working);
        return Err(error);
    }
    Ok(())
}

/// Whether a video whose pipeline didn't end properly (it failed, or had to be killed) can be
/// kept: mp4mux writes an MP4's index (moov) last, and nothing plays one without it (a reserved
/// index at the start would mean megabytes more in every file); a WebM plays without its end.
fn playable(path: &Path) -> bool {
    if path.extension().is_some_and(|e| e == "mp4") {
        crate::mp4::Movie::open(path).is_ok()
    } else {
        std::fs::metadata(path).is_ok_and(|m| m.len() > 1000)
    }
}

impl Recording {
    /// Stops it and finishes the file.
    pub fn stop(mut self) -> Result<Recorded, String> {
        unsafe { libc::kill(self.child.id() as i32, libc::SIGINT) };
        // Closed once the recording has stopped, so it's there to the end.
        let camera = self.camera.take().and_then(Bubble::stop);
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut late = false;
        let status = loop {
            if let Some(status) = self.child.try_wait().map_err(|e| e.to_string())? {
                break status;
            }
            if Instant::now() > deadline {
                late = true;
                let _ = self.child.kill();
                break self.child.wait().map_err(|e| e.to_string())?;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let gif = self.gif.take();
        let is_gif = gif.is_some();
        if let Some(gif) = gif {
            gif.join().unwrap_or_else(|_| Err("The GIF encoder stopped unexpectedly".into()))?;
        }
        let mut note = [self.note.take(), camera].into_iter().flatten().reduce(|a, b| format!("{a} {b}"));
        if !status.success() {
            let why = if late { Some("it took more than 20 seconds".to_string()) } else { last_line(self.said.take()) };
            let why = why.map_or(String::new(), |w| format!(" ({w})"));
            // RSnip's own encoder finishes a GIF, whatever the pipeline did.
            let kept = if is_gif { std::fs::metadata(&self.working).is_ok_and(|m| m.len() > 0) } else { playable(&self.working) };
            if !kept {
                let _ = std::fs::remove_file(&self.working);
                return Err(format!("GStreamer stopped before it finished the recording{why}, so it can't be played"));
            }
            let stopped = format!("GStreamer stopped before it finished the recording{why}, so its end may be missing.");
            note = Some(note.map_or(stopped.clone(), |n| format!("{n} {stopped}")));
        }
        super::finish_recording(&self.working, &self.path)?;
        Ok(Recorded { path: self.path, note })
    }

    /// The pipeline ended by itself (the shared window closed, say).
    pub fn ended(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_some()
    }

    #[cfg(test)]
    pub fn camera(&self) -> Option<&Bubble> {
        self.camera.as_ref()
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
    fn the_camera_sits_by_the_corner_of_what_is_recorded() {
        assert_eq!(bounds((0, 0), (1920, 1080), None), [0, 0, 1920, 1080], "the whole screen");
        // An area of the screen to the right of the first.
        assert_eq!(bounds((1920, -200), (1280, 1024), Some(Area { x: 100, y: 50, width: 400, height: 300 })), [2020, -150, 2420, 150]);
    }

    #[test]
    fn keeps_the_path_in_one_argument() {
        let args = video_pipeline(&source(42), (1920, 1080), None, 30, &H264[0], None, Path::new("/home/a b/Recording \"1\".mp4"));
        assert_eq!(&args[..2], ["-q", "-e"]);
        assert!(args.contains(&"path=42".to_string()));
        assert!(args.contains(&"x264enc".to_string()) && args.contains(&"bitrate=6220".to_string()), "{args:?}");
        assert_eq!(args.last().unwrap(), r#"location=/home/a b/Recording "1".mp4"#);
    }

    /// A sound server with a speaker's monitor and a headset's microphone.
    fn server() -> Option<Vec<Source>> {
        let source = |name: &str, description: &str, monitor| Source { name: name.into(), description: description.into(), monitor, default: false };
        Some(vec![source("alsa_output.pci-0000_00_1f.3.analog-stereo.monitor", "Monitor of Built-in Audio", true), source("alsa_input.usb-Headset.mono", "Headset", false)])
    }

    fn everything(_: &str) -> bool {
        true
    }

    fn asked(sound: bool, microphone: Option<Option<&str>>) -> Format {
        Format { gif: false, hevc: false, fps: 30, sound: sound.then_some(None), microphone: microphone.map(|m| m.map(String::from)), camera: None }
    }

    /// The sound a recording of `format` gets with everything installed, in an MP4 or a WebM.
    fn sound_for(format: &Format, mp4: bool) -> Sound {
        let (sound, note) = plan_sound(format, audio_encoder(mp4, &everything), &everything, &server);
        assert_eq!(note, None);
        sound.expect("sound")
    }

    #[test]
    fn sound_comes_from_the_monitor_and_the_microphone() {
        let lane = "audioconvert ! audioresample ! audio/x-raw,rate=48000,channels=2 ! queue ! mix.";
        let computer = format!("pulsesrc device=@DEFAULT_MONITOR@ do-timestamp=true ! {lane}");
        let default_mic = format!("pulsesrc do-timestamp=true ! {lane}");
        let chosen_mic = format!("pulsesrc device=alsa_input.usb-Headset.mono do-timestamp=true ! {lane}");
        // Silence, so neither the mixer nor the muxer waits for a source that sends nothing.
        let silence = format!("audiotestsrc wave=silence is-live=true ! {lane}");
        // The encoded sound waits for the video as long as it takes.
        let unbounded = "queue max-size-buffers=0 max-size-bytes=0 max-size-time=0 ! mux.";
        let aac = format!("audiomixer name=mix ! audioconvert ! fdkaacenc bitrate=160000 ! {unbounded}");
        let pipeline = |format: Format, mp4| audio_pipeline(&sound_for(&format, mp4));
        // Every source is mixed, with the silence.
        assert_eq!(pipeline(asked(true, None), true), format!("{computer} {silence} {aac}"));
        assert_eq!(pipeline(asked(false, Some(None)), true), format!("{default_mic} {silence} {aac}"));
        assert_eq!(pipeline(asked(false, Some(Some("alsa_input.usb-Headset.mono"))), true), format!("{chosen_mic} {silence} {aac}"));
        assert_eq!(pipeline(asked(true, Some(None)), true), format!("{computer} {default_mic} {silence} {aac}"));
        assert_eq!(pipeline(asked(true, Some(Some("alsa_input.usb-Headset.mono"))), true), format!("{computer} {chosen_mic} {silence} {aac}"));
        // WebM has Opus.
        assert_eq!(pipeline(asked(true, None), false), format!("{computer} {silence} audiomixer name=mix ! audioconvert ! opusenc bitrate=160000 ! {unbounded}"));
    }

    #[test]
    fn the_computers_sound_can_come_from_a_chosen_output() {
        let monitor = "alsa_output.pci-0000_00_1f.3.analog-stereo.monitor";
        let chosen = Format { sound: Some(Some(monitor.into())), ..asked(false, None) };
        assert!(audio_pipeline(&sound_for(&chosen, true)).starts_with(&format!("pulsesrc device={monitor} do-timestamp=true ")));
        // One that's gone gives the default output's, and says so.
        let gone = Format { sound: Some(Some("unplugged.monitor".into())), ..asked(false, None) };
        let (sound, note) = plan_sound(&gone, audio_encoder(true, &everything), &everything, &server);
        assert!(audio_pipeline(&sound.expect("sound")).starts_with("pulsesrc device=@DEFAULT_MONITOR@ "));
        assert_eq!(note.as_deref(), Some("The chosen output isn't connected, so the default one's sound was recorded."));
        // A microphone isn't an output.
        let mic = Format { sound: Some(Some("alsa_input.usb-Headset.mono".into())), ..asked(false, None) };
        assert!(plan_sound(&mic, audio_encoder(true, &everything), &everything, &server).0.is_some_and(|s| s.sources[0].contains("@DEFAULT_MONITOR@")));
    }

    #[test]
    fn the_sound_joins_the_video_in_a_named_muxer() {
        let path = Path::new("/home/a b/Recording.mp4");
        let video = |encoder: &Encoder, sound: Option<&Sound>| video_pipeline("pipewiresrc path=7 ! videoconvert", (1920, 1080), None, 30, encoder, sound, path);
        let silent = video(&H264[0], None);
        assert!(!silent.iter().any(|a| a == "mux."), "{silent:?}");
        assert_eq!(&silent[silent.len() - 5..], ["mp4mux", "name=mux", "!", "filesink", "location=/home/a b/Recording.mp4"]);

        let sound = sound_for(&asked(true, Some(None)), true);
        let loud = video(&H264[0], Some(&sound));
        // The sound's pipeline first, then the video's, ending in the muxer it names, then the file.
        let at = |token: &str| loud.iter().position(|a| a == token).unwrap_or_else(|| panic!("{token} in {loud:?}"));
        assert_eq!(loud[2], "pulsesrc");
        assert!(at("audiomixer") < at("pipewiresrc") && at("pipewiresrc") < at("x264enc") && at("x264enc") < at("mp4mux"), "{loud:?}");
        assert_eq!(loud.iter().filter(|a| *a == "mux.").count(), 1);
        assert_eq!(loud[at("mp4mux") + 1], "name=mux");
        assert_eq!(&loud[loud.len() - 2..], ["filesink".to_string(), format!("location={}", path.display())]);

        let webm = video(&WEBM, Some(&sound_for(&asked(true, None), false)));
        assert!(webm.contains(&"opusenc".to_string()) && webm.contains(&"webmmux".to_string()) && webm.contains(&"name=mux".to_string()), "{webm:?}");
        assert!(webm.iter().all(|a| !a.contains("aac")));
    }

    #[test]
    fn the_best_installed_audio_encoder_is_used() {
        let has = |list: &'static [&'static str]| move |e: &str| list.contains(&e);
        let element = |mp4, installed| audio_encoder(mp4, &has(installed)).map(|e| e.element);
        assert_eq!(element(true, &["fdkaacenc", "avenc_aac", "voaacenc", "opusenc"]), Ok("fdkaacenc"));
        assert_eq!(element(true, &["avenc_aac", "voaacenc"]), Ok("avenc_aac"));
        assert_eq!(element(true, &["voaacenc", "opusenc"]), Ok("voaacenc"));
        assert_eq!(element(false, &["avenc_aac", "opusenc"]), Ok("opusenc"));
        // Ubuntu without gstreamer1.0-libav or plugins-bad.
        let none = element(true, &["opusenc", "x264enc"]).unwrap_err();
        assert_eq!(none, "No AAC encoder is installed, so it was recorded without sound. Install gstreamer1.0-libav.");
        assert!(element(false, &["avenc_aac"]).unwrap_err().starts_with("No Opus encoder"));
        // All three take bits per second.
        assert!(AAC.iter().chain([&OPUS]).all(|e| e.settings.starts_with(e.element) && e.settings.ends_with(" bitrate=160000")));
    }

    #[test]
    fn sound_that_cant_be_had_is_left_out_with_a_note() {
        let plan = |format: &Format, installed: &'static [&'static str], server: Option<Vec<Source>>| {
            let has = move |e: &str| installed.contains(&e);
            let (sound, note) = plan_sound(format, audio_encoder(true, &has), &has, &|| server.clone());
            (sound.map(|s| s.sources.len()), note)
        };
        let installed = &["avenc_aac", "pulsesrc", "audiomixer", "audiotestsrc"];
        // Nothing asked for, nothing to say.
        assert_eq!(plan(&asked(false, None), &[], None), (None, None));
        assert_eq!(plan(&asked(true, Some(None)), installed, server()), (Some(2), None));
        let note = |installed, server| plan(&asked(true, None), installed, server).1.unwrap();
        assert_eq!(note(&["pulsesrc"], server()), "No AAC encoder is installed, so it was recorded without sound. Install gstreamer1.0-libav.");
        assert!(note(&["avenc_aac"], server()).starts_with("Recording sound needs GStreamer's PulseAudio plugin (gstreamer1.0-pulseaudio on Debian and Ubuntu"));
        assert_eq!(note(installed, None), "Couldn't reach the sound server, so it was recorded without sound.");
        // The microphone alone being unavailable leaves the computer's sound.
        let no_microphones = Some(server().unwrap().into_iter().filter(|s| s.monitor).collect());
        assert_eq!(plan(&asked(true, Some(None)), installed, no_microphones.clone()), (Some(1), Some("No microphone was found, so it was recorded without one.".into())));
        assert_eq!(plan(&asked(false, Some(None)), installed, no_microphones), (None, Some("No microphone was found, so it was recorded without one.".into())));
        assert_eq!(
            plan(&asked(true, Some(Some("alsa_input.gone"))), installed, server()),
            (Some(1), Some("The chosen microphone isn't connected, so it was recorded without one.".into()))
        );
        // A speaker's monitor isn't a microphone.
        assert_eq!(plan(&asked(false, Some(Some("alsa_output.pci-0000_00_1f.3.analog-stereo.monitor"))), installed, server()).0, None);
        // Even one source is mixed (with silence), which needs the mixer and the silence.
        assert!(plan(&asked(true, Some(None)), &["avenc_aac", "pulsesrc", "audiotestsrc"], server()).1.unwrap().starts_with("Mixing sound needs GStreamer's audiomixer and audiotestsrc"));
        assert_eq!(plan(&asked(true, None), &["avenc_aac", "pulsesrc", "audiomixer"], server()).0, None);
    }

    #[test]
    fn microphones_are_the_sources_that_are_not_monitors() {
        let json = r#"[
            {"index":1,"name":"alsa_output.pci.analog-stereo.monitor","description":"Monitor of Built-in Audio","monitor_source":"alsa_output.pci.analog-stereo"},
            {"index":2,"name":"alsa_input.usb-Headset.mono","description":"Headset Microphone","monitor_source":"","state":"SUSPENDED"},
            {"index":3,"name":"virtual_monitor","description":"Odd","monitor_source":"sink"},
            {"index":4,"name":"bluez_input.00:11:22:33:44:55","monitor_source":""}
        ]"#;
        let sources = parse_sources(json).unwrap();
        let names: Vec<(&str, &str, bool)> = sources.iter().map(|s| (s.name.as_str(), s.description.as_str(), s.monitor)).collect();
        assert_eq!(
            names,
            [
                ("alsa_output.pci.analog-stereo.monitor", "Monitor of Built-in Audio", true),
                ("alsa_input.usb-Headset.mono", "Headset Microphone", false),
                ("virtual_monitor", "Odd", true),
                ("bluez_input.00:11:22:33:44:55", "bluez_input.00:11:22:33:44:55", false),
            ]
        );
        assert_eq!(parse_sources("[]"), Some(vec![]));
        assert_eq!(parse_sources("not json"), None);
        // Without JSON output: index, name, driver, format, state, tab-separated.
        let short = "1\talsa_output.pci.analog-stereo.monitor\tPipeWire\ts16le 2ch 48000Hz\tSUSPENDED\n2\talsa_input.usb-Headset.mono\tPipeWire\ts16le 1ch 48000Hz\tRUNNING\n";
        let short = parse_short_sources(short);
        let names: Vec<(&str, bool)> = short.iter().map(|s| (s.name.as_str(), s.monitor)).collect();
        assert_eq!(names,[("alsa_output.pci.analog-stereo.monitor", true), ("alsa_input.usb-Headset.mono", false)]);
    }

    #[test]
    fn the_default_microphone_is_never_a_monitor() {
        // As PipeWire lists them: the monitor names its output, a microphone has none.
        let json = r#"[
            {"index":55,"name":"alsa_output.pci-0000_00_1f.3.analog-stereo.monitor","description":"Monitor of Built-in Audio Analog Stereo","monitor_source":"alsa_output.pci-0000_00_1f.3.analog-stereo","monitor_of_sink":null},
            {"index":56,"name":"alsa_input.usb-Headset.mono","description":"Headset Mono","monitor_source":"","monitor_of_sink":null},
            {"index":57,"name":"alsa_input.pci-0000_00_1f.3.analog-stereo","description":"Built-in Audio Analog Stereo","monitor_source":"","monitor_of_sink":null}
        ]"#;
        let with_default = |default: Option<&str>| mark_default(parse_sources(json).unwrap(), default);
        let defaults = |sources: &[Source]| sources.iter().filter(|s| s.default).map(|s| s.name.clone()).collect::<Vec<_>>();
        assert_eq!(defaults(&with_default(Some("alsa_input.usb-Headset.mono"))), ["alsa_input.usb-Headset.mono"]);
        assert!(defaults(&with_default(None)).is_empty() && defaults(&with_default(Some("gone"))).is_empty());
        // A microphone as the default is recorded as the server's default, which follows it.
        assert_eq!(default_microphone(&with_default(Some("alsa_input.pci-0000_00_1f.3.analog-stereo"))), Some(None));
        // So is it when the default isn't known (an older pactl).
        assert_eq!(default_microphone(&with_default(None)), Some(None));
        // A monitor as the default would record the computer's sound twice: the first microphone instead.
        let monitor = with_default(Some("alsa_output.pci-0000_00_1f.3.analog-stereo.monitor"));
        assert_eq!(default_microphone(&monitor), Some(Some("alsa_input.usb-Headset.mono")));
        let format = asked(true, Some(None));
        let (sound, note) = plan_sound(&format, audio_encoder(true, &everything), &everything, &|| Some(monitor.clone()));
        assert_eq!(note, None);
        assert_eq!(sound.unwrap().sources, ["pulsesrc device=@DEFAULT_MONITOR@ do-timestamp=true", "pulsesrc device=alsa_input.usb-Headset.mono do-timestamp=true"]);
        // With no microphone at all, there's none to record, whatever the default.
        let monitors: Vec<Source> = monitor.into_iter().filter(|s| s.monitor).collect();
        assert_eq!(default_microphone(&monitors), None);
        let (sound, note) = plan_sound(&format, audio_encoder(true, &everything), &everything, &|| Some(monitors.clone()));
        assert_eq!(sound.unwrap().sources.len(), 1);
        assert_eq!(note.as_deref(), Some("No microphone was found, so it was recorded without one."));
    }

    #[test]
    fn settings_warn_about_missing_sound_pieces() {
        let has = |list: &'static [&'static str]| move |e: &str| list.contains(&e);
        let mp4 = ["x264enc", "h264parse", "mp4mux", "vp8enc", "webmmux", "opusenc", "pulsesrc"];
        let with = |extra: &'static [&'static str]| {
            let all: Vec<&'static str> = mp4.iter().chain(extra).copied().collect();
            move |e: &str| all.contains(&e)
        };
        // MP4s without an AAC encoder: Debian's advice.
        let problem = problems(false, true, &with(&[])).unwrap();
        assert!(problem.starts_with("No AAC encoder is installed, so MP4s will have no sound. Install gstreamer1.0-libav"), "{problem}");
        assert_eq!(problems(false, true, &with(&["avenc_aac"])), None);
        // WebM (no H.264 parser) has Opus, so no AAC to ask for.
        let webm = problems(false, true, &has(&["x264enc", "mp4mux", "vp8enc", "webmmux", "opusenc", "pulsesrc"])).unwrap();
        assert!(webm.starts_with("H.264 needs GStreamer's h264parse") && !webm.contains("AAC"), "{webm}");
        // The video's problem and the sound's, together.
        let both = problems(true, true, &with(&[])).unwrap();
        assert!(both.starts_with("H.265 needs") || both.starts_with("There's no H.265"), "{both}");
        assert!(both.ends_with("Install gstreamer1.0-libav on Debian and Ubuntu, gstreamer1-plugin-libav on Fedora, gst-libav on Arch."), "{both}");
        // The player and pactl.
        let problem = problems(false, false, &with(&["avenc_aac"])).unwrap();
        assert!(problem.starts_with("Recording sound needs pactl (pulseaudio-utils"), "{problem}");
        let no_pulse = has(&["x264enc", "h264parse", "mp4mux", "avenc_aac", "vp8enc", "webmmux", "opusenc"]);
        assert!(problems(false, true, &no_pulse).unwrap().starts_with("Recording sound needs GStreamer's PulseAudio plugin (gstreamer1.0-pulseaudio"));
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
        let mut encoders: Vec<Encoder> = [&H264[..], &H265[..], &[WEBM]].concat().into_iter().filter(|e| e.parts().all(installed)).collect();
        encoders.dedup_by_key(|e| e.codec);
        for encoder in encoders {
            let path = dir.join(format!("test-{}.{}", encoder.element, encoder.extension));
            // With sound where there's an encoder for it: a track in the file, not just a pipeline that runs.
            let audio = audio_encoder(encoder.extension == "mp4", &installed).ok();
            record_test_picture(&encoder, audio.as_ref(), &path, Duration::from_millis(1500)).unwrap_or_else(|e| panic!("{}: {e}", encoder.element));
            let marker = if encoder.extension == "mp4" { &b"mp4a"[..] } else { b"A_OPUS" };
            let file = std::fs::read(&path).unwrap();
            assert_eq!(file.windows(marker.len()).any(|w| w == marker), audio.is_some(), "{} has sound if it can", encoder.element);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A sound source that sends nothing (a microphone xrdp doesn't redirect, say; here an appsrc
    /// never fed) doesn't hold the recording up: the silence mixed in keeps it going, and it
    /// finishes when it's stopped.
    #[test]
    fn a_silent_sound_source_doesnt_hold_up_the_recording() {
        let (encoder, _) = encoder(false);
        let audio = audio_encoder(encoder.extension == "mp4", &installed);
        if available().is_err() || !installed("videotestsrc") || encoder.extension != "mp4" || audio.is_err() {
            return eprintln!("skipped: no GStreamer MP4 with sound");
        }
        let dir = std::env::temp_dir().join(format!("rsnip-silent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (path, working) = (dir.join("Recording.mp4"), dir.join("working.mp4"));
        let nothing = "appsrc is-live=true format=time caps=audio/x-raw,format=S16LE,rate=48000,channels=2,layout=interleaved";
        let sound = Sound { sources: vec![nothing.into()], encoder: audio.unwrap() };
        let args = video_pipeline("videotestsrc is-live=true ! videoconvert", (320, 240), None, 30, &encoder, Some(&sound), &working);
        let mut child = Command::new("gst-launch-1.0").args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
        let said = said(&mut child);
        std::thread::sleep(Duration::from_secs(3));
        let recording = Recording { child, said, path: path.clone(), working, started: Instant::now(), gif: None, note: None, camera: None };
        let recorded = recording.stop().expect("it finishes");
        assert_eq!(recorded.note, None, "it ended properly");
        let movie = crate::mp4::Movie::open(&path).unwrap();
        // Held up, it wrote nothing and never stopped. GStreamer may be slow to start on a busy
        // machine, so it's not held to the whole 3 s.
        assert!(movie.has_sound() && movie.duration() > Duration::from_millis(500), "the picture, with a sound track: {:?}", movie.duration());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A recording whose pipeline dies part way (killed here, as a crash would) isn't said to be
    /// saved when it can't be played: an MP4 has no index then. A WebM plays, so it's kept, with a
    /// note.
    #[test]
    fn a_pipeline_that_dies_doesnt_save_a_broken_file() {
        if available().is_err() || !installed("videotestsrc") {
            return eprintln!("skipped: no GStreamer");
        }
        let dir = std::env::temp_dir().join(format!("rsnip-died-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (mp4, _) = encoder(false);
        for encoder in [mp4, WEBM].into_iter().filter(|e| e.parts().all(installed)) {
            let path = dir.join(format!("Recording.{}", encoder.extension));
            let working = dir.join(format!("working.{}", encoder.extension));
            let args = video_pipeline("videotestsrc is-live=true ! videoconvert", (320, 240), None, 30, &encoder, None, &working);
            let mut child = Command::new("gst-launch-1.0").args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
            let said = said(&mut child);
            std::thread::sleep(Duration::from_millis(1500));
            child.kill().unwrap();
            let recording = Recording { child, said, path: path.clone(), working: working.clone(), started: Instant::now(), gif: None, note: Some("Noted.".into()), camera: None };
            let stopped = recording.stop();
            if encoder.extension == "mp4" {
                assert_eq!(stopped.err().as_deref(), Some("GStreamer stopped before it finished the recording, so it can't be played"));
                assert!(!path.exists() && !working.exists(), "nothing left behind");
            } else {
                let note = stopped.expect("a WebM plays unfinished").note.unwrap();
                assert_eq!(note, "Noted. GStreamer stopped before it finished the recording, so its end may be missing.");
                if installed("vp8dec") {
                    super::super::player::can_play(&path).unwrap();
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A recording as RSnip makes it trims without re-encoding and plays back from anywhere: from
    /// the keyframe before (one a second), decoded up to the point asked for.
    #[test]
    fn recordings_trim_and_play_from_anywhere() {
        let (encoder, _) = encoder(false);
        if available().is_err() || encoder.extension != "mp4" || !["avdec_h264", "openh264dec"].into_iter().any(installed) {
            return eprintln!("skipped: no GStreamer H.264");
        }
        let dir = std::env::temp_dir().join(format!("rsnip-trim-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Recording.mp4");
        // Long enough for a second keyframe however slowly GStreamer starts.
        record_test_picture(&encoder, None, &path, Duration::from_secs(3)).unwrap();
        let movie = crate::mp4::Movie::open(&path).unwrap();
        let keys = movie.keyframes();
        assert!(keys.len() >= 2 && keys[1] <= Duration::from_millis(1100), "a keyframe every second: {keys:?}");

        let trimmed = super::super::player::trimmed_path(&path);
        let (from, to) = super::super::player::trim(&path, &trimmed, Duration::from_millis(1200), Duration::from_millis(1400)).unwrap();
        assert!(from <= Duration::from_millis(1200) && from >= Duration::from_millis(900), "from the keyframe at about 1 s: {from:?}");
        assert!(to.abs_diff(Duration::from_millis(1400)) < Duration::from_millis(50));
        super::super::player::can_play(&trimmed).unwrap();

        let decoding = super::super::player::decode(&path, Duration::from_millis(1200), (160, 120), None);
        let first = decoding.frames.recv_timeout(Duration::from_secs(10)).expect("a frame");
        assert!(first.at.abs_diff(Duration::from_millis(1200)) < Duration::from_millis(40), "starts where asked: {:?}", first.at);
        let (w, h) = super::super::player::fit((movie.width, movie.height), (160, 120));
        assert_eq!(first.image.size, [w as usize, h as usize], "shrunk to fit");
        drop(decoding);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An encoder without its parser or muxer can't record; RSnip says which package has them
    /// rather than failing when a recording starts.
    #[test]
    fn an_encoder_needs_its_parser_and_muxer() {
        let has = |list: &'static [&'static str]| move |e: &str| list.contains(&e);
        // Ubuntu with plugins-ugly (x264) but not plugins-bad (its parser).
        let ugly_only = has(&["x264enc", "mp4mux", "vp8enc", "webmmux"]);
        let (encoder, note) = choose(false, &ugly_only);
        assert_eq!(encoder.element, "vp8enc");
        assert_eq!(note.unwrap(), format!("H.264 needs GStreamer's h264parse ({}), so it was recorded as WebM.", package("h264parse")));
        let problem = problem_with(false, &ugly_only).unwrap();
        assert!(problem.starts_with("H.264 needs GStreamer's h264parse (gstreamer1.0-plugins-bad on Debian and Ubuntu"), "{problem}");

        let full = has(&["x264enc", "h264parse", "x265enc", "h265parse", "mp4mux", "vp8enc", "webmmux"]);
        let (encoder, note) = choose(true, &full);
        assert_eq!((encoder.element, note), ("x265enc", None));
        assert_eq!(problem_with(true, &full), None);
        // H.265's encoder and the H.264 parser, but not H.265's parser: H.264 it is.
        let no_h265_parser = has(&["x264enc", "h264parse", "x265enc", "mp4mux", "vp8enc", "webmmux"]);
        let (encoder, note) = choose(true, &no_h265_parser);
        assert_eq!(encoder.element, "x264enc");
        assert!(note.unwrap().starts_with("H.265 needs GStreamer's h265parse"));
        // Nothing for H.264 at all: the old advice, to install x264.
        let problem = problem_with(false, &has(&["mp4mux", "vp8enc", "webmmux"])).unwrap();
        assert!(problem.starts_with("No H.264 encoder is installed"), "{problem}");
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
