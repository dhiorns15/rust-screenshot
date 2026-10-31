//! Playing recordings in the library, and trimming them. MP4s (and Linux's WebM) are decoded by the
//! system, GStreamer on Linux (`gst`) and Media Foundation on Windows (`mf`), and GIFs by RSnip, on a
//! thread of their own a few frames ahead; the player shows each frame when its time comes.

#[cfg(not(windows))]
mod gst;
#[cfg(windows)]
mod mf;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};

use super::env::Env;
use crate::mp4::Movie;
use crate::options::PLAYER_MUTED;

/// Frames are decoded no bigger than this, which is plenty for the viewer.
pub const MAX_SIZE: (u32, u32) = (1920, 1080);

/// A decoded frame, and when it's shown.
pub struct Frame {
    pub at: Duration,
    pub image: ColorImage,
}

/// What kind of recording a file is, by its extension.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Mp4,
    Gif,
    WebM,
}

impl Kind {
    pub fn of(path: &Path) -> Option<Kind> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "mp4" => Some(Kind::Mp4),
            "gif" => Some(Kind::Gif),
            "webm" => Some(Kind::WebM),
            _ => None,
        }
    }

    /// Trimming copies frames as they are, which RSnip can do for these (see `mp4` and `trim_gif`).
    pub fn can_trim(self) -> bool {
        self != Kind::WebM
    }
}

/// How long a recording is, how big, and whether it has sound, when that can be told without
/// decoding it all.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Info {
    pub duration: Option<Duration>,
    pub size: Option<(u32, u32)>,
    /// An MP4 or WebM with a sound track. (GIFs have none.)
    pub sound: bool,
}

pub fn info(path: &Path) -> Info {
    match Kind::of(path) {
        Some(Kind::Mp4) => match Movie::open(path) {
            Ok(m) => Info { duration: Some(m.duration()), size: Some((m.width, m.height)), sound: m.has_sound() },
            Err(_) => Info::default(),
        },
        Some(Kind::Gif) => gif_info(path).unwrap_or_default(),
        // Nothing else is known about a WebM without reading it all.
        Some(Kind::WebM) => Info { sound: webm_has_sound(path), ..Info::default() },
        None => Info::default(),
    }
}

/// Whether a WebM has a sound track: its codec (Opus, or Vorbis) is named in the track headers
/// at the start of the file.
fn webm_has_sound(path: &Path) -> bool {
    let mut head = Vec::new();
    let read = std::fs::File::open(path).and_then(|f| f.take(16 * 1024).read_to_end(&mut head));
    read.is_ok() && [&b"A_OPUS"[..], b"A_VORBIS"].iter().any(|codec| head.windows(codec.len()).any(|w| w == *codec))
}

/// A recording's sound, playing through the speakers; it stops when dropped.
pub struct Sound {
    /// Set once the sound is audible, or given up on: there's nothing to wait for any more.
    ready: Arc<AtomicBool>,
    /// Why it can't be heard, once that's known (a decoder that isn't installed, say).
    failed: Arc<Mutex<Option<String>>>,
    /// What stops it, when dropped.
    _playing: Box<dyn Send>,
}

impl Sound {
    pub fn new(ready: Arc<AtomicBool>, playing: impl Send + 'static) -> Sound {
        Sound { ready, failed: Arc::default(), _playing: Box::new(playing) }
    }

    /// It says why it can't be heard in `failed`.
    pub fn failing(self, failed: Arc<Mutex<Option<String>>>) -> Sound {
        Sound { failed, ..self }
    }

    pub fn ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }

    pub fn failed(&self) -> Option<String> {
        self.failed.lock().unwrap().clone()
    }
}

/// Starts a recording's sound from a point in it, in step with a clock that's at that point at
/// `due`, or when the sound is ready if that's sooner: sound that would come after it's due is left
/// out, so it plays in step even when it's slow to start.
type StartSound = fn(&Path, Duration, Instant) -> Option<Sound>;

/// Plays a recording's sound from `from` through the system's speakers, if it can (see `StartSound`).
pub fn play_sound(path: &Path, from: Duration, due: Instant) -> Option<Sound> {
    #[cfg(not(windows))]
    return gst::sound(path, from, due);
    #[cfg(windows)]
    return mf::sound(path, from, due);
}

/// How long the clock waits for the sound to start playing, so a sound that never does (no device)
/// doesn't hold up the picture.
const SOUND_START: Duration = Duration::from_secs(1);

/// `size` shrunk to fit `max`, keeping its shape, in even numbers (as video wants).
pub fn fit(size: (u32, u32), max: (u32, u32)) -> (u32, u32) {
    let scale = (max.0 as f64 / size.0.max(1) as f64).min(max.1 as f64 / size.1.max(1) as f64).min(1.0);
    let even = |n: f64| ((n.round() as u32) & !1).max(2);
    (even(size.0 as f64 * scale), even(size.1 as f64 * scale))
}

/// Decoding a recording on a thread of its own, from a point in it; it stops when dropped.
pub struct Decoding {
    pub frames: Receiver<Frame>,
    /// Why it stopped early, if it did.
    pub failed: Arc<Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
}

impl Drop for Decoding {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// What a decoder thread gets: where to send frames, where to say why it failed, and when to stop.
pub struct Sink {
    pub tx: SyncSender<Frame>,
    pub failed: Arc<Mutex<Option<String>>>,
    pub stop: Arc<AtomicBool>,
    pub ctx: Option<egui::Context>,
}

impl Sink {
    /// Sends a frame, waiting while the player is ahead; false once it's no longer wanted.
    pub fn send(&self, frame: Frame) -> bool {
        if self.stop.load(Ordering::Relaxed) || self.tx.send(frame).is_err() {
            return false;
        }
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
        true
    }

    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    pub fn fail(&self, why: String) {
        *self.failed.lock().unwrap() = Some(why);
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }
}

/// Starts decoding `path` from `from`, frames no bigger than `max`.
pub fn decode(path: &Path, from: Duration, max: (u32, u32), ctx: Option<egui::Context>) -> Decoding {
    let (tx, frames) = sync_channel(4);
    let failed = Arc::new(Mutex::new(None));
    let stop = Arc::new(AtomicBool::new(false));
    let sink = Sink { tx, failed: failed.clone(), stop: stop.clone(), ctx };
    let path = path.to_owned();
    std::thread::spawn(move || {
        let result = match Kind::of(&path) {
            Some(Kind::Gif) => decode_gif(&path, from, max, &sink),
            #[cfg(not(windows))]
            _ => gst::decode(&path, from, max, &sink),
            #[cfg(windows)]
            _ => mf::decode(&path, from, max, &sink),
        };
        if let Err(e) = result {
            sink.fail(e);
        }
    });
    Decoding { frames, failed, stop }
}

/// Whether a recording plays: its first frame decodes (for `rsnip --check-recording`).
#[cfg(not(windows))]
pub fn can_play(path: &Path) -> Result<(), String> {
    let decoding = decode(path, Duration::ZERO, (320, 240), None);
    match decoding.frames.recv_timeout(Duration::from_secs(10)) {
        Ok(_) => Ok(()),
        Err(_) => Err(decoding.failed.lock().unwrap().take().unwrap_or_else(|| "no picture came out".into())),
    }
}

/// The first frame of a recording, no bigger than `max`, for its thumbnail.
pub fn first_frame(path: &Path, max: (u32, u32)) -> Option<ColorImage> {
    let decoding = decode(path, Duration::ZERO, max, None);
    decoding.frames.recv_timeout(Duration::from_secs(10)).ok().map(|f| f.image)
}

fn gif_info(path: &Path) -> Result<Info, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut decoder = gif::DecodeOptions::new().read_info(std::io::BufReader::new(file)).map_err(|e| e.to_string())?;
    let size = (decoder.width() as u32, decoder.height() as u32);
    let mut hundredths = 0u64;
    while let Some(frame) = decoder.next_frame_info().map_err(|e| e.to_string())? {
        hundredths += frame.delay as u64;
    }
    Ok(Info { duration: Some(Duration::from_millis(hundredths * 10)), size: Some(size), sound: false })
}

/// GIFs, decoded whole frame by whole frame (GIF frames can be patches over the one before).
fn decode_gif(path: &Path, from: Duration, max: (u32, u32), sink: &Sink) -> Result<(), String> {
    use image::AnimationDecoder;
    let file = std::io::BufReader::new(std::fs::File::open(path).map_err(|e| e.to_string())?);
    let frames = image::codecs::gif::GifDecoder::new(file).map_err(|e| e.to_string())?.into_frames();
    let mut at = Duration::ZERO;
    for frame in frames {
        let frame = frame.map_err(|e| e.to_string())?;
        let (n, d) = frame.delay().numer_denom_ms();
        let shown_for = Duration::from_secs_f64(n as f64 / d.max(1) as f64 / 1000.0);
        // The frame on screen at `from` is the first wanted.
        if at + shown_for > from || shown_for.is_zero() && at >= from {
            let image = frame.into_buffer();
            let (w, h) = fit(image.dimensions(), max);
            let image = if (w, h) == image.dimensions() { image } else { image::imageops::thumbnail(&image, w, h) };
            let color = ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw());
            if !sink.send(Frame { at, image: color }) {
                return Ok(());
            }
        }
        at += shown_for;
    }
    Ok(())
}

/// Copies `from`..`to` of a GIF to `dst`, each frame as it's shown.
pub fn trim_gif(src: &Path, dst: &Path, from: Duration, to: Duration) -> Result<(), String> {
    let result = copy_gif(src, dst, from, to);
    if result.is_err() {
        // Not a half-written one, which the library would show.
        let _ = std::fs::remove_file(dst);
    }
    result
}

fn copy_gif(src: &Path, dst: &Path, from: Duration, to: Duration) -> Result<(), String> {
    use image::AnimationDecoder;
    let file = std::io::BufReader::new(std::fs::File::open(src).map_err(|e| e.to_string())?);
    let frames = image::codecs::gif::GifDecoder::new(file).map_err(|e| e.to_string())?.into_frames();
    let out = std::fs::File::create(dst).map_err(|e| format!("Couldn't write {}: {e}", dst.display()))?;
    let mut encoder: Option<gif::Encoder<std::io::BufWriter<std::fs::File>>> = None;
    let mut out = Some(std::io::BufWriter::new(out));
    let mut at = Duration::ZERO;
    for frame in frames {
        let frame = frame.map_err(|e| e.to_string())?;
        let (n, d) = frame.delay().numer_denom_ms();
        let shown_for = Duration::from_secs_f64(n as f64 / d.max(1) as f64 / 1000.0);
        let (start, end) = (at, at + shown_for);
        at = end;
        if end <= from || start >= to {
            continue;
        }
        // Only the part of its time inside the trim.
        let kept = end.min(to) - start.max(from);
        let mut rgba = frame.into_buffer();
        let (w, h) = (rgba.width() as u16, rgba.height() as u16);
        if encoder.is_none() {
            let mut e = gif::Encoder::new(out.take().unwrap(), w, h, &[]).map_err(|e| e.to_string())?;
            e.set_repeat(gif::Repeat::Infinite).map_err(|e| e.to_string())?;
            encoder = Some(e);
        }
        let mut gif_frame = gif::Frame::from_rgba_speed(w, h, &mut rgba, 10);
        gif_frame.delay = ((kept.as_millis() + 5) / 10).clamp(2, u16::MAX as u128) as u16;
        encoder.as_mut().unwrap().write_frame(&gif_frame).map_err(|e| e.to_string())?;
    }
    if encoder.is_none() {
        return Err("Nothing is left between those points".into());
    }
    Ok(())
}

/// Copies `from`..`to` of a recording to `dst`, without re-encoding it; returns the part kept
/// (an MP4's start moves back to a keyframe).
pub fn trim(src: &Path, dst: &Path, from: Duration, to: Duration) -> Result<(Duration, Duration), String> {
    match Kind::of(src) {
        Some(Kind::Mp4) => Movie::open(src)?.cut(src, dst, from, to),
        Some(Kind::Gif) => trim_gif(src, dst, from, to).map(|_| (from, to)),
        _ => Err("Only MP4 and GIF recordings can be trimmed".into()),
    }
}

/// A new path beside `path` for its trimmed copy: "Recording … (trimmed).mp4", numbered if taken.
pub fn trimmed_path(path: &Path) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ext = path.extension().unwrap_or_default().to_string_lossy();
    let mut candidate = path.with_file_name(format!("{stem} (trimmed).{ext}"));
    let mut n = 2;
    while candidate.exists() {
        candidate = path.with_file_name(format!("{stem} (trimmed {n}).{ext}"));
        n += 1;
    }
    candidate
}

/// What a press on the timeline moves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Grab {
    Playhead,
    Start,
    End,
}

/// What the player asks of the library.
pub enum Request {
    /// Save `from`..`to` as a new recording.
    Trim(Duration, Duration),
    /// The sound was muted (true) or unmuted, to be remembered.
    Muted(bool),
}

/// A recording open in the library: its frames shown in time with its sound, play and pause,
/// seeking, and choosing a part to trim.
pub struct Player {
    pub path: PathBuf,
    pub kind: Kind,
    pub info: Info,
    /// Where it's at while paused; while playing, from `playing_since`.
    position: Duration,
    /// When playing started, and from where.
    playing_since: Option<(Instant, Duration)>,
    /// The sound is off (see `mute`).
    pub muted: bool,
    /// How to play the sound; none where there's nothing to play it through (the tests).
    speakers: Option<StartSound>,
    /// The sound, while it plays.
    sound: Option<Sound>,
    /// While the sound starts, the clock waits at where it begins, until the sound is playing or
    /// this time, so the picture and the sound set off together.
    sound_wait: Option<Instant>,
    /// Why the sound can't be heard, once it has failed to play.
    pub sound_failed: Option<String>,
    decoding: Option<Decoding>,
    /// When the decoder was last started, so dragging the scrubber doesn't restart it every frame.
    decoding_since: Instant,
    /// The frame on screen, and the next one once it has arrived.
    shown: Option<(Duration, TextureHandle)>,
    next: Option<Frame>,
    /// The decoder has sent its last frame.
    ended: bool,
    /// Show the next frame whenever it's due: the decoder was just started somewhere new.
    awaiting_first: bool,
    pub error: Option<String>,
    /// The part to keep, while trimming.
    pub trim: Option<(Duration, Duration)>,
    /// What the pointer pressed on the timeline is moving, until it's let go.
    grab: Option<Grab>,
    /// Where the timeline was drawn last (for tests to drag on).
    pub timeline: egui::Rect,
}

impl Player {
    pub fn open(path: &Path, env: &Env) -> Option<Player> {
        let kind = Kind::of(path)?;
        Some(Player {
            path: path.to_owned(),
            kind,
            info: info(path),
            position: Duration::ZERO,
            playing_since: None,
            muted: PLAYER_MUTED.read(|name| env.setting(name)),
            speakers: env.play_sound.then_some(play_sound as StartSound),
            sound: None,
            sound_wait: None,
            sound_failed: None,
            decoding: None,
            decoding_since: Instant::now(),
            shown: None,
            next: None,
            ended: false,
            awaiting_first: true,
            error: None,
            trim: None,
            grab: None,
            timeline: egui::Rect::NOTHING,
        })
    }

    /// A frame is on screen.
    #[cfg(test)]
    pub fn showing(&self) -> bool {
        self.shown.is_some()
    }

    pub fn playing(&self) -> bool {
        self.playing_since.is_some()
    }

    pub fn position(&self) -> Duration {
        let at = match self.playing_since {
            // Held at where it begins, while the sound starts.
            Some((_, from)) if self.sound_wait.is_some() => from,
            Some((since, from)) => from + since.elapsed(),
            None => self.position,
        };
        self.info.duration.map_or(at, |d| at.min(d))
    }

    /// Where playing stops: the end of the part kept, while trimming.
    fn end(&self) -> Option<Duration> {
        self.trim.map(|t| t.1).or(self.info.duration)
    }

    pub fn play(&mut self) {
        // From the start again (of the part kept, while trimming), at the end.
        let at_end = self.ended && self.next.is_none() || self.end().is_some_and(|e| self.position() >= e);
        let from = if at_end { self.trim.map_or(Duration::ZERO, |t| t.0) } else { self.position() };
        if at_end {
            self.seek(from);
        }
        self.playing_since = Some((Instant::now(), from));
        self.start_sound(from, SOUND_START);
    }

    pub fn pause(&mut self) {
        self.position = self.position();
        self.playing_since = None;
        self.sound = None;
        self.sound_wait = None;
    }

    /// Mutes or unmutes the sound, at once. The picture goes on: unmuted, the sound joins it where
    /// it has got to.
    pub fn mute(&mut self, muted: bool) {
        self.muted = muted;
        if self.playing() {
            let at = self.position();
            self.playing_since = Some((Instant::now(), at));
            self.start_sound(at, Duration::ZERO);
        }
    }

    /// (Re)starts the sound from `from` while playing, unless it's muted, there's none or nothing to
    /// play it through. The clock waits up to `wait` for it to begin; sound later than that is left
    /// out.
    fn start_sound(&mut self, from: Duration, wait: Duration) {
        // The one before stops first.
        self.sound = None;
        self.sound_wait = None;
        let audible = self.playing() && self.info.sound && !self.muted;
        if let (true, Some(start)) = (audible, self.speakers) {
            let due = Instant::now() + wait;
            self.sound = start(&self.path, from, due);
            self.sound_wait = self.sound.as_ref().filter(|_| !wait.is_zero()).map(|_| due);
        }
    }

    pub fn toggle(&mut self) {
        if self.playing() {
            self.pause();
        } else {
            self.play();
        }
    }

    /// Goes to `to`, and starts the decoder there.
    pub fn seek(&mut self, to: Duration) {
        let to = self.info.duration.map_or(to, |d| to.min(d));
        self.position = to;
        if self.playing() {
            self.playing_since = Some((Instant::now(), to));
        }
        self.restart(to);
        self.start_sound(to, SOUND_START);
    }

    fn restart(&mut self, from: Duration) {
        self.decoding = None;
        self.next = None;
        self.ended = false;
        self.awaiting_first = true;
        self.error = None;
        // The frame on screen stays until the new one comes, so seeking doesn't flash.
        self.decoding_since = Instant::now();
        self.decoding = Some(decode(&self.path, from, MAX_SIZE, None));
    }

    /// Takes the frames that are due, and shows the latest; `ctx` repaints when the next is due.
    fn update(&mut self, ctx: &egui::Context) {
        if self.decoding.is_none() && !self.ended {
            self.restart(self.position);
        }
        if let Some(deadline) = self.sound_wait {
            if self.sound.as_ref().is_none_or(Sound::ready) || Instant::now() >= deadline {
                // The sound is playing: the clock starts, and the picture follows it. Past the
                // deadline, it started then, as the sound (which leaves out what's late) has it.
                self.sound_wait = None;
                if let Some((_, from)) = self.playing_since {
                    self.playing_since = Some((Instant::now().min(deadline), from));
                }
            } else {
                ctx.request_repaint_after(Duration::from_millis(5));
            }
        }
        if let Some(why) = self.sound.as_ref().and_then(Sound::failed) {
            self.sound_failed = Some(why);
        }
        let now = self.position();
        let mut newest = None;
        loop {
            if self.next.is_none() {
                let Some(decoding) = &self.decoding else { break };
                match decoding.frames.try_recv() {
                    Ok(f) => self.next = Some(f),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.ended = true;
                        if let Some(e) = decoding.failed.lock().unwrap().take() {
                            self.error = Some(e);
                        }
                        break;
                    }
                }
            }
            let due = self.next.as_ref().is_some_and(|f| f.at <= now);
            // The first frame after a seek shows straight away, even when paused.
            if !due && !self.awaiting_first {
                break;
            }
            newest = self.next.take();
            self.awaiting_first = false;
            if !due {
                break;
            }
        }
        if let Some(frame) = newest {
            let name = format!("player-{}", self.path.display());
            match &mut self.shown {
                Some((at, tex)) => {
                    tex.set(frame.image, TextureOptions::LINEAR);
                    *at = frame.at;
                }
                None => self.shown = Some((frame.at, ctx.load_texture(name, frame.image, TextureOptions::LINEAR))),
            }
        }
        if self.playing() {
            let end = self.end();
            let finished = end.is_some_and(|e| self.position() >= e) || self.ended && self.next.is_none();
            if finished {
                self.pause();
                if let Some(end) = end {
                    self.position = end;
                }
            } else {
                let wait = self.next.as_ref().map_or(Duration::from_millis(15), |f| f.at.saturating_sub(self.position()));
                ctx.request_repaint_after(wait.min(Duration::from_millis(30)));
            }
        }
    }

    /// The player, filling `ui`; what it asks the library to do.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<Request> {
        self.update(ui.ctx());
        if !ui.ctx().egui_wants_keyboard_input() {
            if ui.input(|i| i.key_pressed(egui::Key::Space)) {
                self.toggle();
            }
            let step = ui.input(|i| (i.key_pressed(egui::Key::ArrowRight) as i32 - i.key_pressed(egui::Key::ArrowLeft) as i32) as f64 * 5.0);
            if step != 0.0 {
                self.seek(Duration::from_secs_f64((self.position().as_secs_f64() + step).max(0.0)));
            }
        }
        let controls = if self.trim.is_some() { 96.0 } else { 56.0 };
        let area = ui.available_rect_before_wrap();
        let picture = egui::Rect::from_min_max(area.min, egui::pos2(area.max.x, (area.max.y - controls).max(area.min.y)));
        let response = ui.allocate_rect(picture, egui::Sense::click());
        if response.clicked() {
            self.toggle();
        }
        match (&self.shown, &self.error) {
            (_, Some(e)) => {
                ui.painter().text(picture.center(), egui::Align2::CENTER_CENTER, e, egui::FontId::proportional(14.0), ui.visuals().text_color());
            }
            (Some((_, tex)), _) => {
                let size = tex.size_vec2();
                let scale = (picture.width() / size.x).min(picture.height() / size.y).min(4.0);
                let rect = egui::Rect::from_center_size(picture.center(), size * scale);
                ui.painter().image(tex.id(), rect, egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
            }
            (None, None) => {
                ui.painter().text(picture.center(), egui::Align2::CENTER_CENTER, "Loading…", egui::FontId::proportional(14.0), ui.visuals().weak_text_color());
            }
        }
        let bar = egui::Rect::from_min_max(egui::pos2(area.min.x + 12.0, picture.max.y + 8.0), egui::pos2(area.max.x - 12.0, area.max.y - 8.0));
        let mut request = None;
        ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| request = self.controls(ui));
        request
    }

    fn controls(&mut self, ui: &mut egui::Ui) -> Option<Request> {
        let mut request = None;
        ui.horizontal(|ui| {
            let icon = if self.playing() { super::theme::icon::PAUSE } else { super::theme::icon::PLAY };
            let label = if self.playing() { "Pause" } else { "Play" };
            if ui.add(egui::Button::new(egui::RichText::new(icon).size(18.0))).on_hover_text(format!("{label} (Space)")).clicked() {
                self.toggle();
            }
            if self.info.sound {
                let (icon, label) = if self.muted { (super::theme::icon::SPEAKER_SLASH, "Unmute") } else { (super::theme::icon::SPEAKER_HIGH, "Mute") };
                if ui.add(egui::Button::new(egui::RichText::new(icon).size(18.0))).on_hover_text(label).clicked() {
                    self.mute(!self.muted);
                    request = Some(Request::Muted(self.muted));
                }
            }
            let total = self.info.duration.map_or("–:––".into(), clock);
            ui.label(egui::RichText::new(format!("{} / {total}", clock(self.position()))).monospace());
            if let Some(why) = &self.sound_failed {
                ui.weak("No sound").on_hover_text(why);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.trim.is_none() {
                    let trim = ui.add_enabled(self.kind.can_trim() && self.info.duration.is_some(), egui::Button::new(format!("{} Trim", super::theme::icon::SCISSORS)));
                    let trim = if self.kind.can_trim() { trim.on_hover_text("Keep part of this recording, as a new file") } else { trim.on_disabled_hover_text("RSnip can trim MP4 and GIF recordings") };
                    if trim.clicked() {
                        self.trim = self.info.duration.map(|d| (Duration::ZERO, d));
                    }
                }
                self.scrubber(ui);
            });
        });
        if let Some((from, to)) = self.trim {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(format!("Keep {} to {} ({:.1} s)", fine_clock(from), fine_clock(to), to.saturating_sub(from).as_secs_f64()));
                if ui.small_button("Start here").on_hover_text("Keep from where the player is").clicked() {
                    let at = self.position().min(to);
                    self.trim = Some((at, to));
                }
                if ui.small_button("End here").on_hover_text("Keep up to where the player is").clicked() {
                    let at = self.position().max(from);
                    self.trim = Some((from, at));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Cancel").clicked() {
                        self.trim = None;
                    }
                    let note = if self.kind == Kind::Mp4 { "Starts at the keyframe at or before the start (one every second)" } else { "Saves the kept part as a new GIF" };
                    if ui.add(super::theme::primary_button("Save trimmed copy")).on_hover_text(note).clicked() {
                        request = Some(Request::Trim(from, to));
                    }
                });
            });
        }
        request
    }

    /// The timeline: where playback is, and the part kept while trimming. Pressing on a trim handle
    /// drags it; anywhere else seeks, and dragging scrubs.
    fn scrubber(&mut self, ui: &mut egui::Ui) {
        let Some(total) = self.info.duration.filter(|d| !d.is_zero()) else {
            ui.add_space(ui.available_width());
            return;
        };
        let (outer, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.0), egui::Sense::click_and_drag());
        // Room for the knob at either end.
        let rect = outer.shrink2(egui::vec2(10.0, 0.0));
        self.timeline = rect;
        let x = |t: Duration| rect.left() + rect.width() * (t.as_secs_f32() / total.as_secs_f32()).clamp(0.0, 1.0);
        let time = |px: f32| Duration::from_secs_f32(((px - rect.left()) / rect.width()).clamp(0.0, 1.0) * total.as_secs_f32());
        // The trim handle within reach of `px`, the nearer if both are.
        let handle_at = |px: f32| {
            let (from, to) = self.trim?;
            let (d_from, d_to) = ((x(from) - px).abs(), (x(to) - px).abs());
            let reach = 12.0;
            match (d_from <= reach, d_to <= reach) {
                (true, true) if d_to < d_from => Some(Grab::End),
                (true, _) => Some(Grab::Start),
                (_, true) => Some(Grab::End),
                _ => None,
            }
        };

        // What a press moves is settled where it starts, and kept until it's let go.
        let down = response.is_pointer_button_down_on();
        if down && self.grab.is_none() {
            let origin = ui.input(|i| i.pointer.press_origin()).or(response.interact_pointer_pos());
            self.grab = origin.map(|p| handle_at(p.x).unwrap_or(Grab::Playhead));
        }
        let hovered = response.hover_pos().and_then(|p| handle_at(p.x));
        if hovered.is_some() || matches!(self.grab, Some(Grab::Start | Grab::End)) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if let (Some(grab), Some(pointer)) = (self.grab, response.interact_pointer_pos().filter(|_| down)) {
            let at = time(pointer.x);
            match (grab, self.trim) {
                (Grab::Start, Some((_, to))) => {
                    self.trim = Some((at.min(to), to));
                    self.preview(at.min(to));
                }
                (Grab::End, Some((from, _))) => {
                    self.trim = Some((from, at.max(from)));
                    self.preview(at.max(from));
                }
                _ => self.preview(at),
            }
        }
        if !down && self.grab.take().is_some() {
            // Let go: decode from where it ended up.
            self.seek(self.position);
        }

        let palette = super::theme::palette(ui);
        let track = egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), 6.0));
        let painter = ui.painter();
        painter.rect_filled(track, 3.0, palette.raised);
        if let Some((from, to)) = self.trim {
            let kept = egui::Rect::from_min_max(egui::pos2(x(from), track.top() - 4.0), egui::pos2(x(to), track.bottom() + 4.0));
            painter.rect_filled(kept, 3.0, super::theme::ACCENT.gamma_multiply(0.35));
            for (t, which) in [(from, Grab::Start), (to, Grab::End)] {
                let active = self.grab == Some(which) || self.grab.is_none() && hovered == Some(which);
                let size = if active { egui::vec2(9.0, 24.0) } else { egui::vec2(7.0, 22.0) };
                painter.rect_filled(egui::Rect::from_center_size(egui::pos2(x(t), rect.center().y), size), 2.0, super::theme::ACCENT);
            }
        }
        let played = egui::Rect::from_min_max(track.min, egui::pos2(x(self.position()), track.max.y));
        painter.rect_filled(played, 3.0, palette.muted);
        painter.circle_filled(egui::pos2(x(self.position()), rect.center().y), 6.0, ui.visuals().strong_text_color());
    }

    /// Shows `at` while the scrubber is dragged: the decoder starts there at most a few times a second.
    fn preview(&mut self, at: Duration) {
        self.position = at;
        if self.playing() {
            self.playing_since = Some((Instant::now(), at));
            // Silent while dragging; letting go seeks, which starts it again.
            self.sound = None;
            self.sound_wait = None;
        }
        if self.decoding_since.elapsed() > Duration::from_millis(250) {
            self.restart(at);
        }
    }
}

/// "1:05.3": to a tenth of a second, for trimming.
pub fn fine_clock(d: Duration) -> String {
    let tenths = d.as_millis() / 100;
    format!("{}.{}", clock(Duration::from_secs((tenths / 10) as u64)), tenths % 10)
}

/// "1:05", or "1:02:05".
pub fn clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn fits_frames_in_the_viewer_evenly() {
        assert_eq!(fit((3840, 2160), (1920, 1080)), (1920, 1080));
        assert_eq!(fit((1000, 3000), (1920, 1080)), (360, 1080));
        assert_eq!(fit((641, 481), (1920, 1080)), (640, 480), "never bigger, and even");
        assert_eq!(clock(Duration::from_secs(65)), "1:05");
        assert_eq!(clock(Duration::from_secs(3725)), "1:02:05");
        assert_eq!(fine_clock(Duration::from_millis(65_340)), "1:05.3");
    }

    /// A GIF of `n` (up to 21) frames, each shown for a tenth of a second, the nth filled with red n × 12.
    pub fn gif(path: &Path, n: u8) {
        let mut out = gif::Encoder::new(std::fs::File::create(path).unwrap(), 8, 6, &[]).unwrap();
        for i in 0..n {
            let mut rgba: Vec<u8> = std::iter::repeat_n([i * 12, 0, 0, 255], 48).flatten().collect();
            let mut frame = gif::Frame::from_rgba_speed(8, 6, &mut rgba, 10);
            frame.delay = 10;
            out.write_frame(&frame).unwrap();
        }
    }

    /// An MP4 as RSnip records it on Windows (Media Foundation) reads, trims without re-encoding,
    /// and plays back from anywhere.
    #[cfg(windows)]
    #[test]
    fn plays_and_trims_windows_recordings() {
        use crate::encode::{Encoder, Mp4};
        let dir = std::env::temp_dir().join(format!("rsnip-player-mf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Recording.mp4");
        let mut mp4: Box<dyn Encoder> = match Mp4::new(&path, 320, 240, 30, false) {
            Ok(mp4) => Box::new(mp4),
            // Server installs (like CI's) may not include Media Foundation.
            Err(e) => return eprintln!("skipped: {e}"),
        };
        for i in 0..90u32 {
            let data = (0..320 * 240).flat_map(|p| [(p % 251) as u8, (i * 2) as u8, 90, 255]).collect();
            let frame = crate::capture::Image { width: 320, height: 240, data };
            mp4.frame(&frame, Duration::from_millis(i as u64 * 1000 / 30)).unwrap();
        }
        mp4.finish().unwrap();

        let movie = Movie::open(&path).unwrap();
        assert_eq!((movie.width, movie.height), (320, 240));
        assert!(movie.duration().abs_diff(Duration::from_secs(3)) < Duration::from_millis(100), "{:?}", movie.duration());
        let trimmed = trimmed_path(&path);
        let (from, to) = trim(&path, &trimmed, Duration::from_millis(1500), Duration::from_millis(2500)).unwrap();
        assert!(from <= Duration::from_millis(1500) && to.abs_diff(Duration::from_millis(2500)) < Duration::from_millis(50), "{from:?}..{to:?}");
        assert!(Movie::open(&trimmed).is_ok());
        // Given longer than a thumbnail is, as CI's machines can be slow to decode, and saying why
        // if it didn't.
        let decoding = decode(&trimmed, Duration::ZERO, MAX_SIZE, None);
        let frame = decoding.frames.recv_timeout(Duration::from_secs(30));
        assert!(frame.as_ref().is_ok_and(|f| f.image.size == [320, 240]), "the trimmed copy plays: {:?}", decoding.failed.lock().unwrap());
        drop(decoding);

        let decoding = decode(&path, Duration::from_millis(1500), MAX_SIZE, None);
        let first = decoding.frames.recv_timeout(Duration::from_secs(10)).expect("a frame");
        assert!(first.at.abs_diff(Duration::from_millis(1500)) < Duration::from_millis(40), "starts where asked: {:?}", first.at);
        drop(decoding);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A recording's sound reaches the default output from where it's asked to, for as long as it's
    /// let, and stops when told to or at its end.
    #[cfg(windows)]
    #[test]
    fn plays_windows_recordings_sound() {
        if let Err(e) = crate::audio::Speaker::open(48_000, 2) {
            // CI's machines may have no sound device.
            return eprintln!("skipped: {e}");
        }
        let dir = std::env::temp_dir().join(format!("rsnip-player-sound-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Recording.mp4");
        if let Err(e) = crate::encode::tests::recording_with_tone(&path, 3) {
            return eprintln!("skipped: {e}");
        }
        assert!(info(&path).sound);

        // Plays from `from` until `stop` is set after `wait`, or the sound ends first; how much was
        // played, and how long after it began playing that was.
        let play = |from: Duration, wait: Duration, due: Instant| {
            let (stop, ready) = (AtomicBool::new(false), AtomicBool::new(false));
            std::thread::scope(|s| {
                let thread = s.spawn(|| unsafe { mf::play(&path, from, due, &stop, &ready) });
                let begun = Instant::now();
                while !ready.load(Ordering::Relaxed) && begun.elapsed() < Duration::from_secs(10) {
                    std::thread::sleep(Duration::from_millis(2));
                }
                let began = Instant::now();
                while began.elapsed() < wait && !thread.is_finished() {
                    std::thread::sleep(Duration::from_millis(5));
                }
                stop.store(true, Ordering::Relaxed);
                let played = thread.join().unwrap().expect("plays");
                (played, began.elapsed())
            })
        };
        let soon = || Instant::now() + Duration::from_secs(5);
        // With the whole suite running at once, the output can go hungry for a moment and the
        // sound fall behind the clock; so each check gets a few goes before it fails.
        let in_time = |check: &dyn Fn() -> Result<(), String>| {
            let mut last = Ok(());
            for _ in 0..3 {
                last = check();
                if last.is_ok() {
                    return;
                }
            }
            panic!("{}", last.unwrap_err());
        };
        let within = |played: Duration, expected: Duration, tolerance: u64, elapsed: Duration, limit: Duration| {
            (played.abs_diff(expected) < Duration::from_millis(tolerance) && elapsed < limit).then_some(()).ok_or_else(|| format!("{played:?} played in {elapsed:?}"))
        };
        in_time(&|| {
            let (played, elapsed) = play(Duration::ZERO, Duration::from_millis(300), soon());
            within(played, elapsed, 100, elapsed, Duration::from_secs(2)).and_then(|()| (played > Duration::from_millis(200)).then_some(()).ok_or_else(|| format!("only {played:?} played")))
        });
        // 0.5 s from the end, it plays that and stops on its own.
        in_time(&|| {
            let (played, elapsed) = play(Duration::from_millis(2500), Duration::from_secs(5), soon());
            within(played, Duration::from_millis(500), 100, elapsed, Duration::from_secs(2))
        });
        // Due 2 s ago, it leaves out the 2 s it's late for and plays the last second.
        in_time(&|| {
            let (played, elapsed) = play(Duration::ZERO, Duration::from_secs(5), Instant::now() - Duration::from_secs(2));
            within(played, Duration::from_secs(1), 150, elapsed, Duration::from_secs(3))
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The sound starts and stops along with the picture, muting and unmuting take effect at once,
    /// and the clock waits for the sound to start.
    #[test]
    fn the_sound_follows_the_player() {
        use std::sync::atomic::AtomicUsize;
        static ALIVE: AtomicUsize = AtomicUsize::new(0);
        static STARTS: Mutex<Vec<Duration>> = Mutex::new(Vec::new());
        static READY: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);
        struct Heard;
        impl Drop for Heard {
            fn drop(&mut self) {
                ALIVE.fetch_sub(1, Ordering::Relaxed);
            }
        }
        static FAILED: Mutex<Option<Arc<Mutex<Option<String>>>>> = Mutex::new(None);
        fn sound(_: &Path, from: Duration, _: Instant) -> Option<Sound> {
            ALIVE.fetch_add(1, Ordering::Relaxed);
            STARTS.lock().unwrap().push(from);
            let ready = Arc::new(AtomicBool::new(false));
            *READY.lock().unwrap() = Some(ready.clone());
            let failed = Arc::new(Mutex::new(None));
            *FAILED.lock().unwrap() = Some(failed.clone());
            Some(Sound::new(ready, Heard).failing(failed))
        }
        let heard = || ALIVE.load(Ordering::Relaxed);
        let last_start = || *STARTS.lock().unwrap().last().unwrap();
        let wait = |ms| std::thread::sleep(Duration::from_millis(ms));

        let dir = std::env::temp_dir().join(format!("rsnip-player-follow-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Recording.gif");
        gif(&path, 20);
        let env = Env::sandbox(dir.clone());
        let mut player = Player::open(&path, &env).unwrap();
        assert!(player.speakers.is_none(), "the sandbox plays no sound");
        let ctx = egui::Context::default();

        // A GIF has none, whatever there is to play it through.
        player.speakers = Some(sound);
        player.play();
        assert_eq!(heard(), 0);
        player.pause();
        player.info.sound = true;

        player.play();
        assert_eq!(heard(), 1);
        let start = last_start();
        wait(60);
        player.update(&ctx);
        assert_eq!(player.position(), start, "the clock waits for the sound to start");
        READY.lock().unwrap().as_ref().unwrap().store(true, Ordering::Relaxed);
        player.update(&ctx);
        wait(60);
        assert!(player.position() >= Duration::from_millis(50), "and goes on once it has: {:?}", player.position());

        // Muting stops the sound and the picture goes on; unmuting starts it from where it's got to,
        // and the picture doesn't wait for it.
        player.mute(true);
        assert!(player.playing() && player.muted && heard() == 0);
        wait(60);
        player.mute(false);
        assert!(heard() == 1 && last_start() >= Duration::from_millis(100), "{:?}", last_start());
        let unmuted = last_start();
        wait(60);
        player.update(&ctx);
        assert!(player.position() >= unmuted + Duration::from_millis(50), "{:?} from {unmuted:?}", player.position());
        // Sound that can't play says why.
        assert_eq!(player.sound_failed, None);
        *FAILED.lock().unwrap().as_ref().unwrap().lock().unwrap() = Some("No decoder".into());
        player.update(&ctx);
        assert_eq!(player.sound_failed.as_deref(), Some("No decoder"));
        // Seeking starts it again from there, and pausing stops it.
        player.seek(Duration::from_secs(1));
        assert!(heard() == 1 && last_start() == Duration::from_secs(1));
        player.pause();
        assert_eq!(heard(), 0);

        // Muted, it stays quiet.
        player.mute(true);
        player.play();
        player.seek(Duration::from_millis(500));
        assert_eq!(heard(), 0);
        player.mute(false);
        assert_eq!(heard(), 1);
        drop(player);
        assert_eq!(heard(), 0, "closing the recording stops the sound");

        // It starts out muted if it was last time.
        env.set_setting(PLAYER_MUTED.name, 1);
        assert!(Player::open(&path, &env).unwrap().muted);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// While trimming, playing stops at the end of the part kept, and playing again from there
    /// plays it from its start.
    #[test]
    fn replays_the_part_kept_from_its_start() {
        let dir = std::env::temp_dir().join(format!("rsnip-player-replay-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Recording.gif");
        gif(&path, 20);
        let mut player = Player::open(&path, &Env::sandbox(dir.clone())).unwrap();
        let ctx = egui::Context::default();
        let (from, to) = (Duration::from_millis(500), Duration::from_millis(800));
        player.trim = Some((from, to));
        player.seek(Duration::from_millis(600));
        player.play();
        let begun = Instant::now();
        while player.playing() && begun.elapsed() < Duration::from_secs(5) {
            player.update(&ctx);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!player.playing() && player.position() == to, "stops at the end of the part kept: {:?}", player.position());
        player.play();
        assert!(player.playing() && player.position().abs_diff(from) < Duration::from_millis(50), "from its start: {:?}", player.position());
        player.update(&ctx);
        assert!(player.playing(), "and goes on");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A GIF that can't be read to the end (cut short here, after some frames) leaves no
    /// half-trimmed copy behind for the library to show.
    #[test]
    fn a_trim_that_fails_leaves_nothing_behind() {
        let dir = std::env::temp_dir().join(format!("rsnip-player-cut-short-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Recording.gif");
        gif(&path, 10);
        let whole = std::fs::read(&path).unwrap();
        std::fs::write(&path, &whole[..whole.len() * 7 / 10]).unwrap();
        let trimmed = trimmed_path(&path);
        let failed = trim(&path, &trimmed, Duration::ZERO, Duration::from_secs(1)).unwrap_err();
        assert_ne!(failed, "Nothing is left between those points", "it got some frames in before failing: {failed}");
        assert!(!trimmed.exists(), "no half-written copy");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plays_gifs_from_any_frame_and_trims_them() {
        let dir = std::env::temp_dir().join(format!("rsnip-player-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Recording.gif");
        gif(&path, 10);
        assert_eq!(info(&path), Info { duration: Some(Duration::from_secs(1)), size: Some((8, 6)), sound: false });

        // From 0.45 s: the frame shown then (the fifth, from 0.4 s) and the rest.
        let decoding = decode(&path, Duration::from_millis(450), MAX_SIZE, None);
        let frames: Vec<Frame> = decoding.frames.iter().collect();
        assert_eq!(frames.iter().map(|f| f.at.as_millis()).collect::<Vec<_>>(), [400, 500, 600, 700, 800, 900]);
        assert_eq!(frames[0].image.pixels[0].r(), 48);
        assert_eq!(first_frame(&path, (4, 4)).map(|f| f.size), Some([4, 2]), "shrunk to fit, in even sizes");

        let trimmed = trimmed_path(&path);
        assert_eq!(trimmed.file_name().unwrap(), "Recording (trimmed).gif");
        assert_eq!(trim(&path, &trimmed, Duration::from_millis(250), Duration::from_millis(600)).unwrap(), (Duration::from_millis(250), Duration::from_millis(600)));
        assert_eq!(info(&trimmed).duration, Some(Duration::from_millis(350)), "the part of each frame inside the trim");
        let first = first_frame(&trimmed, MAX_SIZE).unwrap();
        assert_eq!(first.pixels[0].r(), 24, "starts on the frame shown at 0.25 s");
        assert_eq!(trimmed_path(&path).file_name().unwrap(), "Recording (trimmed 2).gif");
        assert!(trim(&path, &dir.join("none.gif"), Duration::from_secs(5), Duration::from_secs(6)).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
