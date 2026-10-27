//! Screenshots on Linux: `rsnip --full`, `--region`, `--text` and `--last`, as on Windows. The
//! desktop's screenshot portal takes the screenshot (see `portal::screenshot`; `grab` for when it
//! won't), RSnip's own picker
//! (see `pick_area`) chooses the part of it, and the result is copied, saved and opened in the
//! editor as Settings says. The background icon's process does it all (see `background`): on Linux
//! the clipboard only holds what was copied for as long as the process that copied it lives.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use image::RgbaImage;

use super::background::{notify, pick_area, tell_library};
use super::env::ReadWords;
use super::pick_area::Purpose;
use super::platform;
use super::portal::{self, ScreenCast, Sources};
use super::recorder::{self, Area};
use super::words;
use crate::options;

/// Where the area picked last is kept (a text setting), for `--last`: "x y width height", then the
/// size of the screenshot it was picked on, like "100 50 600 400 1920x1080".
const LAST_AREA: &str = "LastArea";

/// Which part of the screen to capture.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// All of it.
    Full,
    Region,
    /// A region, whose text is copied.
    Text,
    /// The region picked last time.
    Last,
}

impl Mode {
    const ALL: [Mode; 4] = [Mode::Full, Mode::Region, Mode::Text, Mode::Last];

    fn name(self) -> &'static str {
        match self {
            Mode::Full => "full",
            Mode::Region => "region",
            Mode::Text => "text",
            Mode::Last => "last",
        }
    }
}

/// A screenshot to take, after `delay` seconds (0 for none).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Capture {
    pub mode: Mode,
    pub delay: u32,
}

impl Capture {
    /// As written to the background icon's request file: the mode, and the delay if there is one.
    pub fn name(self) -> String {
        match self.delay {
            0 => self.mode.name().into(),
            delay => format!("{} {delay}", self.mode.name()),
        }
    }

    pub fn parse(name: &str) -> Option<Capture> {
        let mut words = name.split_whitespace();
        let first = words.next()?;
        let mode = Mode::ALL.into_iter().find(|m| m.name() == first)?;
        let delay = words.next().map_or(Some(0), |d| d.parse().ok())?;
        words.next().is_none().then_some(Capture { mode, delay })
    }
}

/// Takes the screenshot and delivers it, saying so, or why not. True if it left something on the
/// clipboard, which the process has to stay for.
pub fn take(capture: Capture) -> bool {
    let read = match capture.mode {
        Mode::Text => match platform::read_words() {
            Some(read) => Some(read),
            None => {
                notify("Couldn't read the text", "Copying text needs Tesseract: install the tesseract-ocr package");
                return false;
            }
        },
        _ => None,
    };
    std::thread::sleep(Duration::from_secs(capture.delay.into()));
    let hide = options::HIDE_WHILE_CAPTURING.read(platform::setting) && platform::instance_running("Gallery");
    if hide {
        tell_library("--hide");
        // It checks twice a second, then the desktop animates it away.
        std::thread::sleep(Duration::from_millis(900));
    }
    let image = shoot(capture.mode);
    if hide {
        tell_library("--show");
    }
    match (image, read) {
        (Ok(Some(image)), Some(read)) => copy_text(&image, &read),
        (Ok(Some(image)), None) => deliver(&image),
        (Ok(None), _) => false,
        (Err(e), _) => {
            notify("Couldn't take the screenshot", &e);
            false
        }
    }
}

/// The screenshot, and the part of it `mode` wants, picked if need be; `None` if the desktop
/// declined or the picker was cancelled.
fn shoot(mode: Mode) -> Result<Option<RgbaImage>, String> {
    let Some(screen) = grab()? else { return Ok(None) };
    let size = (screen.width(), screen.height());
    let last = if mode == Mode::Last { last_area(platform::text_setting(LAST_AREA).as_deref(), size) } else { None };
    let area = match (mode, last) {
        (Mode::Full, _) => return Ok(Some(screen)),
        (_, Some(area)) => area,
        // Nothing picked yet, or on another screen setup: pick one as usual.
        _ => {
            let purpose = if mode == Mode::Text { Purpose::Text } else { Purpose::Capture };
            let Some(area) = pick(&screen, purpose)?.filter(|a| inside(*a, size)) else { return Ok(None) };
            platform::set_text_setting(LAST_AREA, &format!("{} {} {} {} {}x{}", area.x, area.y, area.width, area.height, size.0, size.1));
            area
        }
    };
    Ok(Some(image::imageops::crop_imm(&screen, area.x, area.y, area.width, area.height).to_image()))
}

/// All of the screen, from the screenshot portal; `None` if it was declined. The portal can refuse
/// without asking (Ubuntu 24.04's GNOME does, for an app started from its menu, and can take a
/// minute to), so then it's read from the X server on an X11 desktop, or taken from a screen the
/// desktop's picker shares, as for recording. Once it has refused it isn't asked again.
fn grab() -> Result<Option<RgbaImage>, String> {
    let x11 = std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_some();
    let refused = if REFUSED.load(Ordering::Relaxed) {
        "it refused before".to_string()
    } else {
        match ask_portal(x11) {
            Ok(Some(file)) => return load(&file).map(Some),
            Ok(None) => return Ok(None),
            Err(e) => {
                REFUSED.store(true, Ordering::Relaxed);
                e
            }
        }
    };
    if x11 {
        return from_x_server().map(Some).map_err(|e| format!("The desktop didn't allow it ({refused}), and {e}"));
    }
    let Some(cast) = ScreenCast::start(Sources::Screen).map_err(|e| format!("The desktop didn't allow it ({refused}), nor sharing the screen: {e}"))? else {
        return Ok(None);
    };
    recorder::snapshot(&cast).map(Some)
}

/// The portal refused a screenshot (or took too long to answer), so it isn't asked again.
static REFUSED: AtomicBool = AtomicBool::new(false);

/// How long the portal is waited for on an X11 desktop, where the screen can be read without it.
const PORTAL_WAIT: Duration = Duration::from_secs(3);

/// Asks the portal for a screenshot (see `portal::screenshot`). On an X11 desktop no answer within
/// `PORTAL_WAIT` counts as a refusal; a screenshot that comes after all is deleted.
fn ask_portal(x11: bool) -> Result<Option<PathBuf>, String> {
    if !x11 {
        return portal::screenshot();
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if let Err(std::sync::mpsc::SendError(Ok(Some(file)))) = tx.send(portal::screenshot()) {
            let _ = std::fs::remove_file(file);
        }
    });
    rx.recv_timeout(PORTAL_WAIT).unwrap_or_else(|_| Err(format!("no answer in {} s", PORTAL_WAIT.as_secs())))
}

/// Forgets that the portal refused, as a new process would.
#[cfg(test)]
pub fn forget_refusal() {
    REFUSED.store(false, Ordering::Relaxed);
}

/// All of the screen, as the X server has it (only on an X11 desktop: on Wayland its windows
/// aren't there).
fn from_x_server() -> Result<RgbaImage, String> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, ImageOrder};
    let failed = |e: &dyn std::fmt::Display| format!("couldn't read the screen: {e}");
    let (conn, number) = x11rb::connect(None).map_err(|e| failed(&e))?;
    let screen = &conn.setup().roots[number];
    let (width, height) = (screen.width_in_pixels, screen.height_in_pixels);
    let image = conn.get_image(ImageFormat::Z_PIXMAP, screen.root, 0, 0, width, height, !0).map_err(|e| failed(&e))?.reply().map_err(|e| failed(&e))?;
    // As every desktop's X server has it: 24-bit colour, four bytes a pixel, blue first.
    let pixels = usize::from(width) * usize::from(height);
    if !matches!(image.depth, 24 | 32) || conn.setup().image_byte_order != ImageOrder::LSB_FIRST || image.data.len() != pixels * 4 {
        return Err(failed(&format!("{}-bit colour isn't supported", image.depth)));
    }
    let mut rgba = image.data;
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        pixel[3] = 255;
    }
    RgbaImage::from_raw(width.into(), height.into(), rgba).ok_or_else(|| failed(&"the wrong number of pixels"))
}

/// The screenshot the portal saved. It's deleted: it was made for RSnip, and where the desktop
/// puts it can be the folder its own tool saves to, where `bring_in` would take it for a second
/// screenshot.
fn load(file: &Path) -> Result<RgbaImage, String> {
    let unreadable = |e: &dyn std::fmt::Display| format!("Couldn't read the screenshot: {e}");
    // Read and deleted at once, before `bring_in` (in the library's process too) finds it.
    let bytes = std::fs::read(file).map_err(|e| unreadable(&e))?;
    let _ = std::fs::remove_file(file);
    image::load_from_memory(&bytes).map(|i| i.to_rgba8()).map_err(|e| unreadable(&e))
}

/// Shows `screen` to pick an area on; `None` if that was cancelled.
fn pick(screen: &RgbaImage, purpose: Purpose) -> Result<Option<Area>, String> {
    let file = platform::runtime_dir().join(format!("area-{}.png", std::process::id()));
    write_png(screen, &file)?;
    let area = pick_area(&file, purpose);
    let _ = std::fs::remove_file(&file);
    Ok(area)
}

/// The area kept in `saved` (see `LAST_AREA`), if it was picked on a screenshot of this size.
fn last_area(saved: Option<&str>, size: (u32, u32)) -> Option<Area> {
    let fields: Vec<&str> = saved?.split_whitespace().collect();
    let [x, y, width, height, picked_on] = fields[..] else { return None };
    let (picked_width, picked_height) = picked_on.split_once('x')?;
    let picked_size = (picked_width.parse().ok()?, picked_height.parse().ok()?);
    let area = Area { x: x.parse().ok()?, y: y.parse().ok()?, width: width.parse().ok()?, height: height.parse().ok()? };
    (picked_size == size && inside(area, size)).then_some(area)
}

/// Whether `area` has some size and is in a screenshot of `size`.
fn inside(area: Area, size: (u32, u32)) -> bool {
    let fits = |start: u32, length: u32, whole: u32| length > 0 && start.checked_add(length).is_some_and(|end| end <= whole);
    fits(area.x, area.width, size.0) && fits(area.y, area.height, size.1)
}

/// Copies the screenshot and saves it, or opens it in the editor, as Settings says; as on Windows,
/// the editor works on files, so opening it saves it too. True if it was copied.
fn deliver(image: &RgbaImage) -> bool {
    let get = platform::setting;
    let copied = match platform::copy_image(image) {
        Ok(()) => true,
        Err(e) => {
            notify("Couldn't copy screenshot", &e);
            false
        }
    };
    let (save, edit, tell) = (options::SAVE_TO_FOLDER.read(get), options::OPEN_EDITOR.read(get), options::NOTIFY.read(get));
    if !save && !edit {
        if copied && tell {
            notify("Screenshot copied", "Paste it anywhere with Ctrl+V.");
        }
        return copied;
    }
    let path = match save_png(image, &platform::screenshots_dir()) {
        Ok(path) => path,
        Err(e) => {
            notify("Couldn't save screenshot", &e);
            return copied;
        }
    };
    if edit {
        platform::spawn_self(&[&path.to_string_lossy()]);
    } else if tell {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let title = if copied { "Screenshot copied to clipboard" } else { "Screenshot saved" };
        notify(title, &format!("{name} is in your screenshots folder."));
    }
    copied
}

/// Saves `image` in `dir`, named as on Windows ("Screenshot 2026-10-24 091500.png").
fn save_png(image: &RgbaImage, dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Can't create {}: {e}", dir.display()))?;
    let path = platform::new_path(dir, "Screenshot", "png");
    write_png(image, &path)?;
    Ok(path)
}

/// Writes `image` as a PNG, fast rather than small: a screenshot is big, and someone is waiting.
fn write_png(image: &RgbaImage, path: &Path) -> Result<(), String> {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::ImageEncoder;
    let file = std::fs::File::create(path).map_err(|e| format!("Can't write {}: {e}", path.display()))?;
    let rgb = image::DynamicImage::ImageRgba8(image.clone()).to_rgb8();
    PngEncoder::new_with_quality(std::io::BufWriter::new(file), CompressionType::Fast, FilterType::Sub)
        .write_image(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
        .map_err(|e| format!("Can't encode PNG: {e}"))
}

/// Recognises the text in `image` and copies it. True if it was copied.
fn copy_text(image: &RgbaImage, read: &ReadWords) -> bool {
    let text = match read(image).map(|found| words::text(&found)) {
        Ok(text) if text.trim().is_empty() => {
            notify("No text found", "Try selecting the text more closely.");
            return false;
        }
        Ok(text) => text,
        Err(e) => {
            notify("Couldn't read the text", &e);
            return false;
        }
    };
    if let Err(e) = platform::copy_text(&text) {
        notify("Couldn't copy the text", &e);
        return false;
    }
    let lines = text.lines().count();
    let first: String = text.lines().next().unwrap_or_default().chars().take(60).collect();
    let title = if lines == 1 { "Copied 1 line of text".to_string() } else { format!("Copied {lines} lines of text") };
    notify(&title, &first);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_area_for_a_screen_of_the_same_size() {
        let area = Area { x: 100, y: 50, width: 600, height: 400 };
        assert_eq!(last_area(Some("100 50 600 400 1920x1080"), (1920, 1080)), Some(area));
        assert_eq!(last_area(Some("100 50 600 400 1920x1080"), (2560, 1440)), None, "another screen");
        assert_eq!(last_area(Some("100 50 600 400 1920x1080"), (1080, 1920)), None, "turned round");
        assert_eq!(last_area(None, (1920, 1080)), None, "never picked");
    }

    #[test]
    fn ignores_what_is_damaged_or_not_on_the_screen() {
        for saved in [
            "",
            "100 50 600 400",
            "100 50 600 400 1920",
            "100 50 600 400 1920x",
            "100 50 600 400 1920x1080 1",
            "100 50 600 four 1920x1080",
            "-1 50 600 400 1920x1080",
            "100 50 0 400 1920x1080",
            "1500 50 600 400 1920x1080",
            "100 800 600 400 1920x1080",
            "4294967295 0 2 1 1920x1080",
        ] {
            assert_eq!(last_area(Some(saved), (1920, 1080)), None, "{saved:?}");
        }
        assert!(inside(Area { x: 1320, y: 680, width: 600, height: 400 }, (1920, 1080)), "up to the edge");
    }

    #[test]
    fn saves_a_png_named_like_windows_captures() {
        let dir = super::super::tests::sandbox_dir("screenshot-save").join("RSnip");
        let image = RgbaImage::from_fn(40, 30, |x, y| image::Rgba([x as u8 * 6, y as u8 * 8, 99, 255]));
        let path = save_png(&image, &dir).unwrap();
        assert!(crate::options::saved_to_by_rsnip(&dir), "{} is named as RSnip names screenshots", path.display());
        assert_eq!(image::open(&path).unwrap().to_rgba8(), image, "as it was taken");
        assert_ne!(save_png(&image, &dir).unwrap(), path, "the next one is another file");
    }

    #[test]
    fn deletes_the_portals_file_once_loaded() {
        let dir = super::super::tests::sandbox_dir("screenshot-load");
        let (file, junk) = (dir.join("Screenshot from 2026-10-24 09-15-00.png"), dir.join("junk.png"));
        let image = RgbaImage::from_pixel(8, 6, image::Rgba([1, 2, 3, 255]));
        write_png(&image, &file).unwrap();
        std::fs::write(&junk, b"not a picture").unwrap();
        assert_eq!(load(&file).unwrap(), image);
        assert!(!file.exists(), "it was only made for RSnip");
        assert!(load(&junk).unwrap_err().starts_with("Couldn't read the screenshot"));
        assert!(load(&file).is_err(), "nothing there now");
    }
}
