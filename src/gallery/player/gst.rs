//! Decoding recordings with GStreamer (`gst-launch-1.0`), which sends raw frames on its standard
//! output at a steady 30 per second, so each frame's time is known from how many came before.
//! `gst-launch` can't seek, so playing from somewhere in an MP4 decodes a copy cut there (see mp4.rs),
//! from the keyframe before it. The sound is decoded and played by GStreamers of their own (`sound`).

use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::ColorImage;

use super::{fit, Frame, Kind, Sink, Sound};
use crate::mp4::Movie;

const FPS: u32 = 30;

/// What decoding needs that may not be installed, and where it comes from.
pub const DECODER_MISSING: &str = "Playing MP4s needs an H.264 decoder for GStreamer: gstreamer1.0-libav on Debian and Ubuntu, gstreamer1-plugin-openh264 on Fedora, gst-libav on Arch";
/// The same for their sound.
pub const SOUND_DECODER_MISSING: &str = "Playing the sound needs an AAC decoder for GStreamer: gstreamer1.0-libav on Debian and Ubuntu, gstreamer1-plugin-libav on Fedora, gst-libav on Arch";

/// `gst-launch-1.0`, saying what it says in English (not the desktop's language), as RSnip reads it.
fn gst_launch() -> Command {
    let mut command = Command::new("gst-launch-1.0");
    command.env("LC_ALL", "C").env_remove("LANGUAGE");
    command
}

/// Whether what GStreamer said is that a decoder it needs isn't installed.
fn missing_decoder(said: &str) -> bool {
    said.contains("Missing decoder") || said.contains("no suitable plugins")
}

/// A cut copy to decode from, removed when done.
struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub fn decode(path: &Path, from: Duration, max: (u32, u32), sink: &Sink) -> Result<(), String> {
    let movie = (Kind::of(path) == Some(Kind::Mp4)).then(|| Movie::open(path).ok()).flatten();
    // WebMs (and anything unreadable) have no size to go by: frames are fitted into the box, with
    // borders.
    let (width, height) = fit(movie.as_ref().map_or(max, |m| (m.width, m.height)), max);
    let mut start = Duration::ZERO;
    let mut temp = None;
    if let (Some(movie), false) = (&movie, from.is_zero()) {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let cut = std::env::temp_dir().join(format!("rsnip-play-{}-{}.mp4", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        if let Ok((keyframe, _)) = movie.cut(path, &cut, from, Duration::MAX) {
            start = keyframe;
            temp = Some(Temp(cut));
        }
    }
    let source = temp.as_ref().map_or(path, |t| t.0.as_path());
    let caps = format!("video/x-raw,format=RGBA,width={width},height={height},pixel-aspect-ratio=1/1,framerate={FPS}/1");
    let mut child = gst_launch()
        .args(["-q", "filesrc"])
        .arg(format!("location={}", source.display()))
        .args(["!", "decodebin", "!", "videoconvert", "!", "videoscale", "add-borders=true", "!", "videorate", "!", &caps, "!", "fdsink", "fd=1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Playing recordings needs GStreamer ({e})"))?;
    let mut stdout = child.stdout.take().expect("piped");
    let mut buf = vec![0u8; width as usize * height as usize * 4];
    let half = Duration::from_secs(1) / FPS / 2;
    let mut sent = 0;
    for n in 0u32.. {
        if sink.stopped() || stdout.read_exact(&mut buf).is_err() {
            break;
        }
        let at = start + Duration::from_secs(n as u64) / FPS;
        // Frames before the one on screen at `from` are only decoded to get to it.
        if at + half < from {
            continue;
        }
        let image = ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &buf);
        if !sink.send(Frame { at, image }) {
            break;
        }
        sent += 1;
    }
    let _ = child.kill();
    let status = child.wait().map_err(|e| e.to_string())?;
    if sent == 0 && !sink.stopped() && !status.success() {
        let mut err = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut err);
        }
        return Err(if missing_decoder(&err) {
            DECODER_MISSING.into()
        } else {
            let line = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it stopped").trim();
            format!("Couldn't play it: {line}")
        });
    }
    drop(temp);
    Ok(())
}

/// The sound as it passes from one GStreamer to the other: 48 kHz stereo, 16 bits.
const PCM: &str = "audio/x-raw,format=S16LE,layout=interleaved,rate=48000,channels=2";
/// Bytes of it per second.
const PCM_PER_SECOND: u64 = 48_000 * 4;

/// Starts `path`'s sound playing from `from`, in step with a clock that's there at `due` (see
/// `StartSound`). One GStreamer decodes its sound track (not its video) to raw sound, and RSnip
/// drops what comes before `from` (it can't seek, and a cut copy would start at a keyframe, up to a
/// second early) and what comes too late, on its way to a second, which plays it on the default
/// output. Both are killed when the `Sound` is dropped.
pub fn sound(path: &Path, from: Duration, due: Instant) -> Option<Sound> {
    start(path, from, due).map(|(playing, ready, failed)| Sound::new(ready, playing).failing(failed))
}

/// Why a sound plays no more, once that's known.
type Failed = Arc<Mutex<Option<String>>>;

/// The GStreamer that decodes `path`'s sound to raw sound on its standard output: just its sound
/// track, so starting far into a long recording doesn't wait for its video to be decoded too.
fn decoder(path: &Path) -> Option<Command> {
    let demuxer = match Kind::of(path) {
        Some(Kind::Mp4) => "qtdemux",
        Some(Kind::WebM) => "matroskademux",
        _ => return None,
    };
    let mut command = gst_launch();
    command.args(["-q", "filesrc"]).arg(format!("location={}", path.display()));
    command.args(["!", demuxer, "name=d", "d.audio_0", "!", "queue", "!", "decodebin", "!", "audioconvert", "!", "audioresample", "!", PCM, "!", "fdsink", "fd=1"]);
    Some(command)
}

fn start(path: &Path, from: Duration, due: Instant) -> Option<(Playing, Arc<AtomicBool>, Failed)> {
    let mut decoder = decoder(path)?
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    // Not quiet, as it says "New clock" once it's playing.
    let player = gst_launch()
        .args(["fdsrc", "fd=0", "!", PCM, "!", "audioconvert", "!", "audioresample", "!", "autoaudiosink"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut player) = player else {
        let _ = decoder.kill();
        let _ = decoder.wait();
        return None;
    };
    let (raw, speaker, said) = (decoder.stdout.take().expect("piped"), player.stdin.take().expect("piped"), player.stdout.take().expect("piped"));
    let mut complaints = decoder.stderr.take().expect("piped");
    let failed = Failed::default();
    let failing = failed.clone();
    std::thread::spawn(move || {
        if let Ok(0) = feed(raw, speaker, bytes(from), due) {
            // No sound came out. If that's for want of a decoder, the player says so. (The decoder
            // has stopped, or stops now nothing reads what it decodes.)
            let mut err = String::new();
            let _ = complaints.read_to_string(&mut err);
            if missing_decoder(&err) {
                *failing.lock().unwrap() = Some(SOUND_DECODER_MISSING.into());
            }
        }
    });
    let ready = Arc::new(AtomicBool::new(false));
    let waiting = ready.clone();
    std::thread::spawn(move || {
        let mut lines = std::io::BufReader::new(said).lines();
        while let Some(Ok(line)) = lines.next() {
            if line.starts_with("New clock") {
                waiting.store(true, Ordering::Relaxed);
            }
        }
        // It has ended, or failed (no sound device, say): nothing more to wait for.
        waiting.store(true, Ordering::Relaxed);
    });
    Some((Playing([decoder, player]), ready, failed))
}

/// How many bytes of raw sound play in `d`, in whole samples.
fn bytes(d: Duration) -> u64 {
    (d.as_secs_f64() * PCM_PER_SECOND as f64) as u64 / 4 * 4
}

/// Passes raw sound from `raw` to `speaker`, but for its first `skip` bytes, and the sound that
/// comes late: what was due between `due` and when the rest arrives (if that's after), so it starts
/// in step with the picture. Returns how many bytes it passed on.
fn feed(raw: impl Read, mut speaker: impl Write, skip: u64, due: Instant) -> std::io::Result<u64> {
    let mut raw = std::io::BufReader::with_capacity(64 * 1024, raw);
    std::io::copy(&mut raw.by_ref().take(skip), &mut std::io::sink())?;
    if raw.fill_buf()?.is_empty() {
        return Ok(0);
    }
    let late = bytes(Instant::now().saturating_duration_since(due));
    std::io::copy(&mut raw.by_ref().take(late), &mut std::io::sink())?;
    std::io::copy(&mut raw, &mut speaker)
}

/// The two processes making a recording's sound heard, killed when dropped.
struct Playing([Child; 2]);

impl Drop for Playing {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// A 3 s recording at `path` with a video and a sound track, made with GStreamer as `kind`
    /// (an MP4 with AAC, or a WebM with Opus); false where it can't be (no GStreamer or encoder).
    fn recording_with_sound(path: &Path, kind: Kind) -> bool {
        let (video, audio, mux) = match kind {
            Kind::WebM => ("vp8enc", "opusenc", "webmmux"),
            _ => ("x264enc key-int-max=30 ! h264parse", "avenc_aac ! aacparse", "mp4mux"),
        };
        let pipeline = format!(
            "videotestsrc num-buffers=90 ! video/x-raw,width=320,height=240,framerate=30/1 ! videoconvert ! {video} ! mux. \
             audiotestsrc num-buffers=141 samplesperbuffer=1024 freq=440 ! audioconvert ! audioresample ! audio/x-raw,rate=48000,channels=2 ! {audio} ! mux. \
             {mux} name=mux ! filesink location={}",
            path.display()
        );
        Command::new("gst-launch-1.0").arg("-q").args(pipeline.split_whitespace()).output().is_ok_and(|o| o.status.success())
    }

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rsnip-gst-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Recordings with sound still show their picture, and the sound can be played from anywhere
    /// in them: its processes are stopped, and gone, when it's dropped.
    #[test]
    fn plays_the_sound_of_recordings_beside_their_picture() {
        let dir = dir("sound");
        for (kind, name) in [(Kind::Mp4, "Recording.mp4"), (Kind::WebM, "Recording.webm")] {
            let path = dir.join(name);
            if !recording_with_sound(&path, kind) {
                eprintln!("skipped {name}: no GStreamer, or no encoders to make one with");
                continue;
            }
            assert!(super::super::info(&path).sound, "{name} has sound");

            let decoding = super::super::decode(&path, Duration::ZERO, (320, 240), None);
            let frames = decoding.frames.iter().count();
            assert!((88..=92).contains(&frames), "{name}: {frames} frames, {:?}", decoding.failed.lock().unwrap());
            // Its sound track decodes to raw sound, all 3 s of it.
            let pcm = decoder(&path).unwrap().output().unwrap();
            let seconds = pcm.stdout.len() as f64 / PCM_PER_SECOND as f64;
            assert!((2.8..3.2).contains(&seconds), "{name}: {seconds} s of sound, {}", String::from_utf8_lossy(&pcm.stderr));

            // Ready once it plays, or has given up: there may be no sound server here.
            let begun = Instant::now();
            let (playing, ready, _) = start(&path, Duration::from_millis(1500), begun + Duration::from_secs(1)).expect("GStreamer starts");
            let pids = playing.0.each_ref().map(Child::id);
            while !ready.load(Ordering::Relaxed) && begun.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(ready.load(Ordering::Relaxed), "{name}: it started or failed within 10 s");
            drop(playing);
            for pid in pids {
                assert!(!Path::new(&format!("/proc/{pid}")).exists(), "{name}: process {pid} is gone");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The sound before where it starts is left out, and so is what comes too late to play in step.
    #[test]
    fn drops_the_sound_before_the_start_and_what_comes_late() {
        /// Raw sound that takes `0` to start coming.
        struct Slow(Duration, std::io::Cursor<Vec<u8>>);
        impl Read for Slow {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                std::thread::sleep(std::mem::take(&mut self.0));
                self.1.read(buf)
            }
        }
        let second = || std::io::Cursor::new(vec![0u8; PCM_PER_SECOND as usize]);
        let later = Instant::now() + Duration::from_secs(60);
        let mut heard = Vec::new();
        assert_eq!(feed(second(), &mut heard, bytes(Duration::from_millis(250)), later).unwrap(), bytes(Duration::from_millis(750)));
        assert_eq!(heard.len() as u64, bytes(Duration::from_millis(750)));

        // Due now, but 0.2 s coming: it starts 0.2 s in.
        let passed = feed(Slow(Duration::from_millis(200), second()), std::io::sink(), 0, Instant::now()).unwrap();
        assert!((bytes(Duration::from_millis(650))..=bytes(Duration::from_millis(800))).contains(&passed), "{passed}");
        assert_eq!(passed % 4, 0, "whole samples");
        // Due later, nothing is late.
        assert_eq!(feed(Slow(Duration::from_millis(50), second()), std::io::sink(), 0, later).unwrap(), PCM_PER_SECOND);
        assert_eq!(feed(std::io::empty(), std::io::sink(), 0, Instant::now()).unwrap(), 0);
    }

    #[test]
    fn a_recording_without_sound_isnt_taken_for_one() {
        let dir = dir("silent");
        let path = dir.join("Recording.webm");
        // Just a video track.
        let made = Command::new("gst-launch-1.0")
            .args(["-q", "videotestsrc", "num-buffers=10", "!", "vp8enc", "!", "webmmux", "!", "filesink"])
            .arg(format!("location={}", path.display()))
            .output();
        if !made.is_ok_and(|o| o.status.success()) {
            return eprintln!("skipped: no GStreamer, or no VP8 encoder");
        }
        assert!(!super::super::info(&path).sound);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
