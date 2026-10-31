//! Saving a recording in the other format: an MP4 as a GIF, a GIF as an MP4. Frames come from the
//! player's own decoders (`decode`), are taken at a steady rate, and go to RSnip's GIF writer or to
//! an H.264 encoder: Media Foundation's on Windows (`encode::Mp4`, as recordings are made), and on
//! Linux GStreamer's, through the pipeline recordings use (`recorder::mp4_from_frames`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, ColorImage};

use super::{decode, fit, info, Decoding, Frame, GifOut, Kind};

/// GIFs are taken at this many frames a second, as Linux records them: smooth enough for a screen,
/// and each frame is costly in a GIF.
pub const GIF_FPS: u32 = 12;
/// No wider than this: a GIF of a whole screen would be huge.
const GIF_MAX: (u32, u32) = (1280, 4096);
/// MP4s are made at a steady 30 frames a second, as recordings are by default.
pub const MP4_FPS: u32 = 30;
/// About the biggest H.264 encoders all take.
const MP4_MAX: (u32, u32) = (3840, 2160);

/// What a recording of `kind` is saved as: an MP4 as a GIF, and a GIF as an MP4.
pub fn target(kind: Kind) -> Option<Kind> {
    match kind {
        Kind::Mp4 => Some(Kind::Gif),
        Kind::Gif => Some(Kind::Mp4),
        // Linux's WebMs have no size known without decoding them all.
        Kind::WebM => None,
    }
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Mp4 => "MP4",
            Kind::Gif => "GIF",
            Kind::WebM => "WebM",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Kind::Mp4 => "mp4",
            Kind::Gif => "gif",
            Kind::WebM => "webm",
        }
    }
}

/// Where `src` saved as `kind` goes: beside it, under its name with the new extension, or
/// "name (2).gif" and so on if that's taken.
pub fn converted_path(src: &Path, kind: Kind) -> PathBuf {
    super::super::free_name(src.parent().unwrap_or(Path::new(".")), &src.with_extension(kind.extension()))
}

/// Saves `from`..`to` of the recording at `src` as `dst`, a GIF or an MP4 (by its extension),
/// telling `progress` how far it has got (0 to 1, by the frames' times). Ok(false) if `stop` was
/// set first. Nothing is left at `dst` unless it finishes.
pub fn convert(src: &Path, dst: &Path, from: Duration, to: Duration, stop: &AtomicBool, progress: &mut dyn FnMut(f32)) -> Result<bool, String> {
    // Without its size, GStreamer would decode it to the size asked for, with borders.
    let size = info(src).size.ok_or("Couldn't read the recording")?;
    let result = match Kind::of(dst) {
        Some(Kind::Gif) => to_gif(src, dst, (from, to), stop, progress),
        Some(Kind::Mp4) => to_mp4(src, dst, size, (from, to), stop, progress),
        _ => Err("Recordings can be saved as GIF or MP4".into()),
    };
    if !matches!(result, Ok(true)) {
        let _ = std::fs::remove_file(dst);
    }
    result
}

fn to_gif(src: &Path, dst: &Path, (from, to): (Duration, Duration), stop: &AtomicBool, progress: &mut dyn FnMut(f32)) -> Result<bool, String> {
    let decoding = decode(src, from, GIF_MAX, None);
    let mut out = GifOut::create(dst)?;
    // In hundredths of a second from `from`, as GIFs count, rounded at each frame rather than for
    // each frame's length, so the frames' lengths (8 or 9) add up to the time it takes.
    let hundredths = |tick: u64| (tick * 100 + GIF_FPS as u64 / 2) / GIF_FPS as u64;
    // Each frame waits for the next, which says how long it's shown.
    let mut pending: Option<(Vec<u8>, (u16, u16), u64)> = None;
    let write = |out: &mut GifOut, pending: Option<(Vec<u8>, (u16, u16), u64)>, until: u64| match pending {
        Some((mut rgba, size, at)) => out.frame(&mut rgba, size, until.saturating_sub(at)),
        None => Ok(()),
    };
    let done = sample(&decoding, (from, to), GIF_FPS, stop, progress, &mut |tick, frame, same| {
        // A frame that's still on screen is shown for longer instead.
        if same {
            return Ok(());
        }
        let at = hundredths(tick);
        write(&mut out, pending.take(), at)?;
        let [w, h] = frame.image.size;
        pending = Some((frame.image.as_raw().to_vec(), (w as u16, h as u16), at));
        Ok(())
    })?;
    if !done {
        return Ok(false);
    }
    write(&mut out, pending, (to.saturating_sub(from).as_millis() as u64 + 5) / 10)?;
    out.finish()?;
    Ok(true)
}

fn to_mp4(src: &Path, dst: &Path, size: (u32, u32), (from, to): (Duration, Duration), stop: &AtomicBool, progress: &mut dyn FnMut(f32)) -> Result<bool, String> {
    // Fitted by the decoder as here, so the frames come out this size.
    let decoding = decode(src, from, MP4_MAX, None);
    let mut out = Mp4Out::create(dst, fit(size, MP4_MAX))?;
    let frame_time = |tick: u64| Duration::from_nanos(tick * 1_000_000_000 / MP4_FPS as u64);
    let done = sample(&decoding, (from, to), MP4_FPS, stop, progress, &mut |tick, frame, same| out.frame(&frame.image, frame_time(tick), same))?;
    if !done {
        out.abandon();
        return Ok(false);
    }
    out.finish()?;
    Ok(true)
}

/// Takes the frames `decoding` sends (in time order) as they're on screen at `fps` steady ticks a
/// second from `from` until `to`: `out` gets each tick's number (from 0), the frame, and whether
/// that's the frame it had at the tick before. Ok(false) if `stop` was set first.
fn sample(decoding: &Decoding, (from, to): (Duration, Duration), fps: u32, stop: &AtomicBool, progress: &mut dyn FnMut(f32), out: &mut dyn FnMut(u64, &Frame, bool) -> Result<(), String>) -> Result<bool, String> {
    let at = |tick: u64| from + Duration::from_nanos(tick * 1_000_000_000 / fps as u64);
    let length = to.saturating_sub(from).as_secs_f32().max(f32::EPSILON);
    let (mut shown, mut tick, mut fresh) = (None::<Frame>, 0u64, false);
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let next = match decoding.frames.recv_timeout(Duration::from_millis(100)) {
            Ok(frame) => Some(frame),
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => None,
        };
        // Until the next frame (or the end), the ticks show the one before it. The first frame
        // shows from the start, even if it's a moment after it.
        let until = next.as_ref().map_or(to, |f| f.at.min(to));
        if let Some(frame) = &shown {
            while at(tick) < until {
                if stop.load(Ordering::Relaxed) {
                    return Ok(false);
                }
                out(tick, frame, !fresh)?;
                fresh = false;
                tick += 1;
                progress((at(tick).saturating_sub(from).as_secs_f32() / length).min(1.0));
            }
        }
        match next {
            Some(frame) if frame.at < to => {
                shown = Some(frame);
                fresh = true;
            }
            _ => break,
        }
    }
    if let Some(why) = decoding.failed.lock().unwrap().take() {
        return Err(why);
    }
    if tick == 0 {
        return Err("Nothing is left between those points".into());
    }
    Ok(true)
}

/// An MP4 written frame by frame, through Media Foundation's H.264 encoder.
#[cfg(windows)]
struct Mp4Out {
    encoder: Box<crate::encode::Mp4>,
    /// The last frame, as the encoder takes it (BGRA), for when it's shown again.
    last: Option<crate::capture::Image>,
}

#[cfg(windows)]
impl Mp4Out {
    fn create(dst: &Path, (width, height): (u32, u32)) -> Result<Mp4Out, String> {
        let encoder = crate::encode::Mp4::with_sound(dst, width as i32, height as i32, MP4_FPS, false, false)?;
        Ok(Mp4Out { encoder: Box::new(encoder), last: None })
    }

    fn frame(&mut self, image: &ColorImage, at: Duration, same: bool) -> Result<(), String> {
        use crate::encode::Encoder;
        if !same || self.last.is_none() {
            let data = image.pixels.iter().flat_map(|p| [p.b(), p.g(), p.r(), 255]).collect();
            self.last = Some(crate::capture::Image { width: image.size[0] as i32, height: image.size[1] as i32, data });
        }
        self.encoder.frame(self.last.as_ref().unwrap(), at)
    }

    fn finish(self) -> Result<(), String> {
        use crate::encode::Encoder;
        self.encoder.finish()
    }

    /// Stops without a finished file (which `convert` removes).
    fn abandon(self) {
        // Finished all the same, so Media Foundation lets go of the file.
        let _ = self.finish();
    }
}

/// An MP4 written frame by frame: raw frames into a GStreamer that encodes them as a recording is.
#[cfg(not(windows))]
struct Mp4Out {
    child: std::process::Child,
    frames: Option<std::process::ChildStdin>,
    said: Option<std::thread::JoinHandle<String>>,
    dst: PathBuf,
    size: (u32, u32),
}

#[cfg(not(windows))]
impl Mp4Out {
    fn create(dst: &Path, size: (u32, u32)) -> Result<Mp4Out, String> {
        use std::process::{Command, Stdio};
        let args = super::super::recorder::mp4_from_frames(size, MP4_FPS, dst)?;
        let mut child = Command::new("gst-launch-1.0")
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Saving an MP4 needs GStreamer ({e})"))?;
        let said = super::super::recorder::said(&mut child);
        let frames = child.stdin.take();
        Ok(Mp4Out { child, frames, said, dst: dst.to_owned(), size })
    }

    fn frame(&mut self, image: &ColorImage, _at: Duration, _same: bool) -> Result<(), String> {
        use std::io::Write;
        if image.size != [self.size.0 as usize, self.size.1 as usize] {
            return Err(format!("A frame came out {} × {}, not {} × {}", image.size[0], image.size[1], self.size.0, self.size.1));
        }
        // Every tick's frame goes in, in turn: GStreamer times them by the frame rate.
        let written = self.frames.as_mut().map(|frames| frames.write_all(image.as_raw()));
        match written {
            Some(Ok(())) => Ok(()),
            // It stopped taking them: what it said is why.
            _ => Err(self.failure()),
        }
    }

    /// Why GStreamer stopped, once it has.
    fn failure(&mut self) -> String {
        self.frames = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
        let why = super::super::recorder::last_line(self.said.take());
        format!("GStreamer couldn't save the MP4: {}", why.as_deref().unwrap_or("it stopped"))
    }

    fn finish(mut self) -> Result<(), String> {
        // The end of its input ends the file.
        self.frames = None;
        let status = self.child.wait().map_err(|e| e.to_string())?;
        if !status.success() || crate::mp4::Movie::open(&self.dst).is_err() {
            return Err(self.failure());
        }
        Ok(())
    }

    /// Stops without a finished file (which `convert` removes).
    fn abandon(self) {}
}

/// Stopped before its input ends, which would finish the file after `convert` has removed it.
#[cfg(not(windows))]
impl Drop for Mp4Out {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// What a `Converting` comes to: where the new file was saved, None if it was stopped first, or
/// why it couldn't be.
pub type Converted = Result<Option<PathBuf>, String>;

/// A recording being saved in the other format (see `target`) on a thread of its own; stopped if
/// it's dropped before it's done. It's written where the library doesn't look (see
/// `working_path`), and moved beside the recording once it's finished.
pub struct Converting {
    /// What it's saved as, and where.
    pub kind: Kind,
    pub path: PathBuf,
    /// How far it has got, in thousandths.
    done: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    result: Receiver<Converted>,
}

impl Converting {
    /// Starts saving `from`..`to` of `src` in the other format; `ctx` repaints as it goes.
    pub fn start(src: &Path, from: Duration, to: Duration, ctx: Option<egui::Context>) -> Result<Converting, String> {
        let kind = Kind::of(src).and_then(target).ok_or("Only MP4s and GIFs can be saved in the other format")?;
        let path = converted_path(src, kind);
        let working = super::super::working_path(&path)?;
        let (done, stop) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicBool::new(false)));
        let (tx, result) = channel();
        let (src, dst, thread_done, thread_stop) = (src.to_owned(), path.clone(), done.clone(), stop.clone());
        std::thread::spawn(move || {
            let repaint = || {
                if let Some(ctx) = &ctx {
                    ctx.request_repaint();
                }
            };
            let mut progress = |p: f32| {
                let thousandths = (p * 1000.0) as u32;
                // A repaint for each step the window shows.
                if thread_done.swap(thousandths, Ordering::Relaxed) / 10 != thousandths / 10 {
                    repaint();
                }
            };
            let converted = convert(&src, &working, from, to, &thread_stop, &mut progress);
            let result = match converted {
                Ok(true) => {
                    // Under another name if something has taken this one meanwhile.
                    let dst = if dst.exists() { converted_path(&src, kind) } else { dst };
                    super::super::move_into_place(&working, &dst).map(|()| Some(dst))
                }
                Ok(false) => Ok(None),
                Err(e) => Err(e),
            };
            if result.is_err() {
                let _ = std::fs::remove_file(&working);
            }
            let _ = tx.send(result);
            repaint();
        });
        Ok(Converting { kind, path, done, stop, result })
    }

    /// How far it has got, from 0 to 1.
    pub fn progress(&self) -> f32 {
        self.done.load(Ordering::Relaxed) as f32 / 1000.0
    }

    /// Stops it; `finished` says so once it has.
    pub fn cancel(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// What it came to, once it's done.
    pub fn finished(&self) -> Option<Converted> {
        match self.result.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("It stopped unexpectedly".into())),
        }
    }
}

impl Drop for Converting {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rsnip-convert-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A GIF of `n` frames of 64 × 48, each shown for `hundredths` of a second and filled with
    /// its own colour.
    fn gif(path: &Path, n: u8, hundredths: u16) {
        let mut out = gif::Encoder::new(std::fs::File::create(path).unwrap(), 64, 48, &[]).unwrap();
        for i in 0..n {
            let mut rgba: Vec<u8> = std::iter::repeat_n([i.wrapping_mul(25), 255 - i.wrapping_mul(25), 90, 255], 64 * 48).flatten().collect();
            let mut frame = gif::Frame::from_rgba_speed(64, 48, &mut rgba, 10);
            frame.delay = hundredths;
            out.write_frame(&frame).unwrap();
        }
    }

    /// `seconds` of a 30-frames-a-second MP4 of `size`, every frame different, made as recordings
    /// are; why not, where there's no encoder.
    fn mp4(path: &Path, (width, height): (u32, u32), seconds: u32) -> Result<(), String> {
        #[cfg(windows)]
        {
            use crate::encode::{Encoder, Mp4};
            let mut mp4: Box<dyn Encoder> = Box::new(Mp4::with_sound(path, width as i32, height as i32, 30, false, false)?);
            for i in 0..seconds * 30 {
                let data = (0..width * height).flat_map(|p| [(p % 251) as u8, (i * 2) as u8, 90, 255]).collect();
                let frame = crate::capture::Image { width: width as i32, height: height as i32, data };
                mp4.frame(&frame, Duration::from_millis(i as u64 * 1000 / 30))?;
            }
            mp4.finish()
        }
        #[cfg(not(windows))]
        {
            let pipeline = format!(
                "videotestsrc pattern=ball num-buffers={} ! video/x-raw,width={width},height={height},framerate=30/1 ! videoconvert ! x264enc key-int-max=30 ! h264parse ! mp4mux ! filesink",
                seconds * 30
            );
            let made = std::process::Command::new("gst-launch-1.0")
                .arg("-q")
                .args(pipeline.split_whitespace())
                .arg(format!("location={}", path.display()))
                .output()
                .map_err(|e| format!("no GStreamer: {e}"))?;
            if !made.status.success() {
                return Err(format!("GStreamer couldn't make an MP4: {}", String::from_utf8_lossy(&made.stderr)));
            }
            // Decoding it needs an H.264 decoder too.
            super::super::can_play(path)
        }
    }

    /// The frames of a GIF, and how long each is shown in hundredths of a second.
    fn gif_frames(path: &Path) -> ((u16, u16), Vec<u16>) {
        let mut decoder = gif::DecodeOptions::new().read_info(std::fs::File::open(path).unwrap()).unwrap();
        let mut delays = Vec::new();
        while let Some(frame) = decoder.read_next_frame().unwrap() {
            delays.push(frame.delay);
        }
        ((decoder.width(), decoder.height()), delays)
    }

    #[test]
    fn names_and_sizes_the_copy() {
        let dir = temp_dir("names");
        let src = dir.join("Recording.mp4");
        assert_eq!(converted_path(&src, Kind::Gif), dir.join("Recording.gif"));
        std::fs::write(dir.join("Recording.gif"), b"taken").unwrap();
        assert_eq!(converted_path(&src, Kind::Gif), dir.join("Recording (2).gif"), "never over another file");
        assert_eq!((target(Kind::Mp4), target(Kind::Gif), target(Kind::WebM)), (Some(Kind::Gif), Some(Kind::Mp4), None));
        assert_eq!(fit((2560, 1440), GIF_MAX), (1280, 720), "a GIF is at most 1280 wide");
        assert_eq!(fit((800, 600), GIF_MAX), (800, 600), "and otherwise the recording's size");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An MP4 saved as a GIF: 12 frames a second at its size, as long as it, and of the part kept
    /// when there is one.
    #[test]
    fn saves_an_mp4_as_a_gif() {
        let dir = temp_dir("mp4-gif");
        let src = dir.join("Recording.mp4");
        if let Err(e) = mp4(&src, (160, 120), 3) {
            return crate::testing::skipped(e);
        }
        let dst = dir.join("Recording.gif");
        let mut seen = Vec::new();
        assert_eq!(convert(&src, &dst, Duration::ZERO, Duration::from_secs(3), &AtomicBool::new(false), &mut |p| seen.push(p)), Ok(true));
        let (size, delays) = gif_frames(&dst);
        assert_eq!(size, (160, 120));
        assert!(delays.len().abs_diff(36) <= 1, "3 s at 12 a second: {}", delays.len());
        let length: u32 = delays.iter().map(|&d| d as u32).sum();
        assert!(length.abs_diff(300) <= 9, "as long as the recording: {length} hundredths");
        assert!(delays.iter().all(|&d| d == 8 || d == 9), "{delays:?}");
        assert!(seen.windows(2).all(|w| w[0] <= w[1]) && seen.last() == Some(&1.0), "progress goes up to 100 %: {seen:?}");

        // The part kept, 1 s to 2 s.
        let part = dir.join("Part.gif");
        assert_eq!(convert(&src, &part, Duration::from_secs(1), Duration::from_secs(2), &AtomicBool::new(false), &mut |_| {}), Ok(true));
        let (_, delays) = gif_frames(&part);
        assert!(delays.len().abs_diff(12) <= 1, "{}", delays.len());
        assert_eq!(super::super::info(&part).duration, Some(Duration::from_secs(1)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A GIF saved as an MP4: its size and length, each frame for as long as it's shown.
    #[test]
    fn saves_a_gif_as_an_mp4() {
        let dir = temp_dir("gif-mp4");
        let src = dir.join("Recording.gif");
        // 20 frames of 0.1 s: 2 s.
        gif(&src, 20, 10);
        let dst = dir.join("Recording.mp4");
        let made = convert(&src, &dst, Duration::ZERO, Duration::from_secs(2), &AtomicBool::new(false), &mut |_| {});
        if let Err(e) = &made {
            // Without an H.264 encoder (a Windows server, or GStreamer without x264).
            if !dst.exists() && (e.contains("encoder") || e.contains("GStreamer")) {
                return crate::testing::skipped(e);
            }
        }
        assert_eq!(made, Ok(true));
        let info = super::super::info(&dst);
        assert_eq!(info.size, Some((64, 48)));
        let length = info.duration.unwrap();
        assert!(length.abs_diff(Duration::from_secs(2)) <= Duration::from_secs(1) / MP4_FPS, "{length:?}");
        assert!(!info.sound);
        let first = super::super::first_frame(&dst, super::super::MAX_SIZE).expect("it plays");
        assert_eq!(first.size, [64, 48]);
        // The first frame's colour (0, 255, 90), give or take the encoding.
        let p = first.pixels[64 * 24 + 32];
        assert!(p.r() < 40 && p.g() > 215 && p.b().abs_diff(90) < 40, "{p:?}");

        // Its last half second.
        let part = dir.join("Part.mp4");
        assert_eq!(convert(&src, &part, Duration::from_millis(1500), Duration::from_secs(2), &AtomicBool::new(false), &mut |_| {}), Ok(true));
        let length = super::super::info(&part).duration.unwrap();
        assert!(length.abs_diff(Duration::from_millis(500)) <= Duration::from_secs(1) / MP4_FPS, "{length:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Stopped part way, nothing is left behind, either way.
    #[test]
    fn stopping_leaves_nothing_behind() {
        let dir = temp_dir("stop");
        let src = dir.join("Recording.gif");
        gif(&src, 40, 10);
        for dst in [dir.join("Stopped.gif"), dir.join("Stopped.mp4")] {
            let stop = AtomicBool::new(false);
            let mut last = 0.0;
            // As if the button were pressed a quarter of the way through.
            let made = convert(&src, &dst, Duration::ZERO, Duration::from_secs(4), &stop, &mut |p| {
                last = p;
                if p >= 0.25 {
                    stop.store(true, Ordering::Relaxed);
                }
            });
            if let Err(e) = &made {
                return crate::testing::skipped(e);
            }
            assert_eq!(made, Ok(false), "{}", dst.display());
            assert!(last < 0.5, "it stopped early: {last}");
            assert!(!dst.exists(), "no half-written {}", dst.display());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The library's way: on a thread, written out of sight and moved beside the recording when
    /// it's done; cancelled, it leaves nothing anywhere.
    #[test]
    fn converts_on_a_thread_beside_the_recording() {
        let dir = temp_dir("thread");
        let src = dir.join("Recording.gif");
        gif(&src, 20, 10);
        let wait = |job: &Converting| {
            for _ in 0..3000 {
                if let Some(result) = job.finished() {
                    return result;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("it never finished");
        };
        // Taken as soon as it starts.
        let job = Converting::start(&src, Duration::ZERO, Duration::from_secs(2), None).unwrap();
        job.cancel();
        assert_eq!(wait(&job), Ok(None));
        let working = dir.join(".rsnip").join("Recording.mp4");
        assert!(!dir.join("Recording.mp4").exists() && !working.exists(), "nothing left behind");

        let job = Converting::start(&src, Duration::ZERO, Duration::from_secs(2), None).unwrap();
        assert_eq!((job.kind, job.path.clone()), (Kind::Mp4, dir.join("Recording.mp4")));
        match wait(&job) {
            Ok(Some(path)) => {
                assert_eq!(path, dir.join("Recording.mp4"));
                assert!(job.progress() >= 0.99 && !working.exists());
            }
            Err(e) => crate::testing::skipped(e),
            Ok(None) => panic!("it wasn't stopped"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
