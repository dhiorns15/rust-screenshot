//! Decoding recordings with GStreamer (`gst-launch-1.0`), which sends raw frames on its standard
//! output at a steady 30 per second, so each frame's time is known from how many came before.
//! `gst-launch` can't seek, so playing from somewhere in an MP4 decodes a copy cut there (see mp4.rs),
//! from the keyframe before it. The sound is decoded and played by GStreamers of their own (`sound`).

use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::ColorImage;

use super::{fit, Frame, Kind, Sink, Sound};
use crate::mp4::Movie;

const FPS: u32 = 30;

/// What decoding needs that may not be installed, and where it comes from.
pub const DECODER_MISSING: &str = "Playing MP4s needs an H.264 decoder for GStreamer: gstreamer1.0-libav on Debian and Ubuntu, gstreamer1-plugin-openh264 on Fedora, gst-libav on Arch";

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
    let mut child = Command::new("gst-launch-1.0")
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
        return Err(if err.contains("Missing decoder") || err.contains("no suitable plugins") {
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

/// Starts `path`'s sound playing from `from`. One GStreamer decodes it to raw sound and RSnip drops
/// what comes before `from` (it can't seek, and a cut copy would start at a keyframe, up to a
/// second early) on its way to a second, which plays it on the default output. Both are killed
/// when the `Sound` is dropped.
pub fn sound(path: &Path, from: Duration) -> Option<Sound> {
    start(path, from).map(|(playing, ready)| Sound::new(ready, playing))
}

fn start(path: &Path, from: Duration) -> Option<(Playing, Arc<AtomicBool>)> {
    let mut decoder = Command::new("gst-launch-1.0")
        .args(["-q", "filesrc"])
        .arg(format!("location={}", path.display()))
        .args(["!", "decodebin", "!", "audioconvert", "!", "audioresample", "!", PCM, "!", "fdsink", "fd=1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Not quiet, as it says "New clock" once it's playing.
    let player = Command::new("gst-launch-1.0")
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
    let (mut raw, mut speaker, said) = (decoder.stdout.take().expect("piped"), player.stdin.take().expect("piped"), player.stdout.take().expect("piped"));
    let skip = (from.as_secs_f64() * PCM_PER_SECOND as f64) as u64 / 4 * 4;
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut raw.by_ref().take(skip), &mut std::io::sink()).and_then(|_| std::io::copy(&mut raw, &mut speaker));
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
    Some((Playing([decoder, player]), ready))
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

            // Ready once it plays, or has given up: there may be no sound server here.
            let begun = Instant::now();
            let (playing, ready) = start(&path, Duration::from_millis(1500)).expect("GStreamer starts");
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
