//! Decoding recordings with GStreamer (`gst-launch-1.0`), which sends raw frames on its standard
//! output at a steady 30 per second, so each frame's time is known from how many came before.
//! `gst-launch` can't seek, so playing from somewhere in an MP4 decodes a copy cut there (see mp4.rs),
//! from the keyframe before it.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use eframe::egui::ColorImage;

use super::{fit, Frame, Kind, Sink};
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
