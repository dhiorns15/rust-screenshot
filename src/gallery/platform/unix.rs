//! The library on Linux and other Unix desktops. Capturing is up to another tool (e.g. the
//! desktop's own, Flameshot or grim); RSnip opens and annotates the results.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::SystemTime;

use image::RgbaImage;

use super::super::system_info::{self, Host};

/// Pinning, copying as files and dragging files out are Windows-only.
pub const EXTRAS: bool = false;
const NOT_HERE: &str = "Not available on this system";

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

/// True if this is the only window of its kind (held with a lock file until the process exits).
pub fn claim_single_instance(name: &str, _title: &str) -> bool {
    static LOCK: OnceLock<File> = OnceLock::new();
    let Ok(file) = File::options().create(true).truncate(false).write(true).open(lock_path(name)) else { return true };
    if file.try_lock().is_err() {
        return false;
    }
    let _ = LOCK.set(file);
    true
}

// There's no tray app here: no startup entry, global shortcuts or installer.
/// The autostart entry that starts RSnip's background icon when you sign in (freedesktop's
/// autostart, which GNOME, KDE and the rest follow).
fn autostart_file() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config"));
    config.join("autostart").join("rsnip.desktop")
}

/// RSnip starts in the background when you sign in.
pub fn start_with_windows() -> bool {
    autostart_file().is_file()
}

/// False if it couldn't be changed.
pub fn set_start_with_windows(on: bool) -> bool {
    let file = autostart_file();
    if !on {
        return std::fs::remove_file(&file).is_ok() || !file.exists();
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=RSnip\nComment=RSnip in the background, for screenshots and recordings\nExec={} --tray\nIcon=rsnip\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n",
        desktop_quote(&exe.to_string_lossy())
    );
    file.parent().is_some_and(|dir| std::fs::create_dir_all(dir).is_ok()) && std::fs::write(&file, entry).is_ok()
}

/// `arg` quoted for a desktop entry's `Exec` line.
fn desktop_quote(arg: &str) -> String {
    let mut out = String::from('"');
    for c in arg.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// The background icon is running (it holds its lock; see `background::run`).
pub fn tray_running() -> bool {
    instance_running("Tray")
}

/// Another process holds `claim_single_instance(name)`.
pub fn instance_running(name: &str) -> bool {
    File::options().write(true).open(lock_path(name)).is_ok_and(|f| f.try_lock().is_err())
}

fn lock_path(name: &str) -> PathBuf {
    let user = std::env::var("USER").unwrap_or_default();
    std::env::temp_dir().join(format!("rsnip-{}-{user}.lock", name.to_lowercase()))
}

pub fn tell_tray(request: super::super::env::TrayRequest) {
    if request == super::super::env::TrayRequest::Quit {
        super::super::background::quit();
    }
}

/// Starts the background icon, unless it's running already.
pub fn start_tray() {
    spawn_self(&["--tray"]);
}

/// Runs RSnip again with `args`, on its own, so it outlives this process and the terminal.
pub fn spawn_self(args: &[&str]) {
    use std::os::unix::process::CommandExt;
    if let Ok(exe) = std::env::current_exe() {
        let _ = Command::new(exe).args(args).process_group(0).spawn();
    }
}

/// Opens the Settings window, on System info if `system_info`.
pub fn open_settings(system_info: bool) {
    if let Ok(exe) = std::env::current_exe() {
        let args: &[&str] = if system_info { &["--settings", "--system-info"] } else { &["--settings"] };
        let _ = Command::new(exe).args(args).spawn();
    }
}

pub fn snipping_key() -> bool {
    false
}

pub fn installed() -> bool {
    false
}

pub fn shortcut_name(key: u32) -> String {
    crate::shortcuts::fallback_name(key)
}

/// `~/Pictures/Screenshots`, or wherever the desktop's Pictures folder is.
pub fn screenshots_dir() -> PathBuf {
    let pictures = Command::new("xdg-user-dir")
        .arg("PICTURES")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join("Pictures"));
    pictures.join("Screenshots")
}

fn settings_path() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config"));
    config.join("rsnip").join("settings.json")
}

fn settings() -> BTreeMap<String, u32> {
    std::fs::read(settings_path()).map(|b| parse_settings(&b)).unwrap_or_default()
}

/// The settings in a `settings.json`; anything unreadable means defaults.
fn parse_settings(bytes: &[u8]) -> BTreeMap<String, u32> {
    serde_json::from_slice(bytes).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_paths_for_the_autostart_entry() {
        assert_eq!(desktop_quote("/home/a b/.local/bin/rsnip"), r#""/home/a b/.local/bin/rsnip""#);
        assert_eq!(desktop_quote(r#"/x/$y"z\w"#), r#""/x/\$y\"z\\w""#);
    }

    #[test]
    fn damaged_settings_mean_defaults() {
        assert_eq!(parse_settings(br#"{"LibraryDock": 1, "CheckUpdates": 0}"#).get("LibraryDock"), Some(&1));
        for junk in [&b""[..], b"{", b"null", b"[1,2]", br#"{"LibraryDock": -1}"#, br#"{"LibraryDock": "bottom"}"#, b"\xff\xfe"] {
            assert!(parse_settings(junk).is_empty(), "{junk:?}");
        }
    }
}

pub fn setting(name: &str) -> Option<u32> {
    settings().get(name).copied()
}

pub fn set_setting(name: &str, value: u32) {
    let mut all = settings();
    all.insert(name.to_owned(), value);
    let path = settings_path();
    if let (Some(dir), Ok(json)) = (path.parent(), serde_json::to_vec_pretty(&all)) {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(path, json);
    }
}

thread_local! {
    // On X11 and Wayland the copied data is served by this process, so the clipboard stays open.
    static CLIPBOARD: RefCell<Option<arboard::Clipboard>> = const { RefCell::new(None) };
}

fn with_clipboard(f: impl FnOnce(&mut arboard::Clipboard) -> Result<(), arboard::Error>) -> Result<(), String> {
    CLIPBOARD.with(|cell| {
        let mut cell = cell.borrow_mut();
        if cell.is_none() {
            *cell = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
        }
        f(cell.as_mut().unwrap()).map_err(|e| e.to_string())
    })
}

pub fn copy_image(img: &RgbaImage) -> Result<(), String> {
    let data = arboard::ImageData { width: img.width() as usize, height: img.height() as usize, bytes: img.as_raw().into() };
    with_clipboard(|c| c.set_image(data))
}

pub fn copy_text(text: &str) -> Result<(), String> {
    with_clipboard(|c| c.set_text(text))
}

pub fn copy_files(_paths: &[PathBuf]) -> Result<(), String> {
    Err(NOT_HERE.into())
}

pub fn open(path: &Path) {
    let _ = Command::new("xdg-open").arg(path).spawn();
}

pub fn show_in_folder(path: &Path) {
    open(path.parent().unwrap_or(path));
}

pub fn recycle(path: &Path) -> bool {
    trash::delete(path).is_ok()
}

/// A folder starting with a dot is already hidden.
pub fn hide(_dir: &Path) {}

/// The font for text annotations.
pub fn font_bytes() -> Option<Vec<u8>> {
    std::fs::read(font_path()?).ok()
}

/// The desktop's sans-serif, or a common one.
fn font_path() -> Option<PathBuf> {
    let matched = Command::new("fc-match")
        .args(["-f", "%{file}", "sans-serif:semibold"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .filter(|f| f.ends_with(".ttf") || f.ends_with(".otf"));
    let common = [
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    ];
    matched.into_iter().chain(common.map(String::from)).map(PathBuf::from).find(|f| f.is_file())
}

/// The programs RSnip runs: opening files and links, finding Pictures and the annotation font, and
/// checking for updates (curl or wget).
const HELPERS: [&str; 7] = ["xdg-open", "xdg-user-dir", "fc-match", "curl", "wget", "tesseract", "gst-launch-1.0"];

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// What System info reports about this machine, from `/etc/os-release`, `/proc` and the session.
pub fn host() -> Host {
    let read = |path: &str| std::fs::read_to_string(path).ok();
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let os = read("/etc/os-release").or_else(|| read("/usr/lib/os-release")).and_then(|t| system_info::os_release_name(&t));
    let desktop = match (var("XDG_CURRENT_DESKTOP"), var("XDG_SESSION_TYPE")) {
        (Some(d), Some(t)) => Some(format!("{d}, {t}")),
        (d, t) => d.or(t),
    };
    let meminfo = read("/proc/meminfo").unwrap_or_default();
    let home = home();
    let program = std::env::current_exe().ok();
    let install = match program.as_deref() {
        // installer/linux/install.sh copies it here.
        Some(p) if p.parent() == Some(home.join(".local/bin").as_path()) => "install.sh",
        // The .deb or .rpm (see installer/linux/build-packages.sh).
        Some(p) if p == Path::new("/usr/bin/rsnip") && Path::new("/var/lib/dpkg/info/rsnip.list").exists() => ".deb package",
        Some(p) if p == Path::new("/usr/bin/rsnip") => ".rpm package",
        _ => "Standalone",
    };
    Host {
        os: os.unwrap_or_else(|| std::env::consts::OS.to_string()),
        kernel: read("/proc/sys/kernel/osrelease").map(|k| k.trim().to_string()).filter(|k| !k.is_empty()),
        desktop,
        cpu: read("/proc/cpuinfo").and_then(|t| system_info::cpuinfo_model(&t)),
        threads: std::thread::available_parallelism().map_or(1, |n| n.get()),
        memory: system_info::meminfo(&meminfo, "MemTotal"),
        memory_available: system_info::meminfo(&meminfo, "MemAvailable"),
        install: install.into(),
        program,
        settings: system_info::tidy_path(&settings_path(), Some(&home)),
        font: font_path(),
        helpers: HELPERS.map(|h| (h, on_path(h))).to_vec(),
        home: Some(home),
    }
}

/// A new file in `dir` named like Windows' captures: "Recording 2026-10-23 181500.mp4", with
/// " (2)" and so on if that's taken.
pub fn new_path(dir: &Path, kind: &str, ext: &str) -> PathBuf {
    let secs = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs() as libc::time_t);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    let base = format!("{kind} {:04}-{:02}-{:02} {:02}{:02}{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec);
    let mut path = dir.join(format!("{base}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{base} ({n}).{ext}"));
        n += 1;
    }
    path
}

/// The local calendar date of `t` as (year, month, day).
pub fn local_date(t: SystemTime) -> (i32, u32, u32) {
    let secs = t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs() as libc::time_t);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    (tm.tm_year + 1900, tm.tm_mon as u32 + 1, tm.tm_mday as u32)
}

pub fn pin(_img: &RgbaImage) -> Result<(), String> {
    Err(NOT_HERE.into())
}

pub fn pin_file(_path: &Path) {}

/// Why H.265 can't be chosen.
pub const HEVC_MISSING: &str = "H.265 needs GStreamer's x265 encoder (gstreamer1.0-plugins-bad on Debian and Ubuntu, gst-plugins-bad on Arch)";

/// What's missing for recording, if anything (see `recorder::problem`).
pub fn recording_problem(hevc: bool) -> Option<String> {
    super::super::recorder::problem(hevc)
}

/// Whether recordings can be H.265 (an encoder for it is installed in GStreamer).
pub fn hevc_available() -> bool {
    super::super::recorder::hevc_available()
}

/// What reads the text in screenshots, for Settings.
pub const TEXT_RECOGNITION: &str = "Tesseract";

/// Tesseract's text recognition on an image file, for search, if it's installed.
pub fn ocr() -> Option<super::super::env::Ocr> {
    let read = read_words()?;
    Some(std::sync::Arc::new(move |path: &Path| {
        let img = image::open(path).map_err(|e| e.to_string())?.to_rgba8();
        read(&img).map(|words| super::super::words::text(&words))
    }))
}

/// Tesseract's text recognition word by word, for copying text out of an image, if it's installed
/// (the `tesseract-ocr` package, or `tesseract` on some distributions).
pub fn read_words() -> Option<super::super::env::ReadWords> {
    on_path("tesseract").then(|| std::sync::Arc::new(tesseract) as super::super::env::ReadWords)
}

fn tesseract(img: &RgbaImage) -> Result<Vec<super::super::words::Word>, String> {
    // It reads small UI text far better enlarged; the boxes are scaled back afterwards.
    let scale = if img.width().max(img.height()) <= 2000 { 2 } else { 1 };
    let big;
    let img = if scale == 1 {
        img
    } else {
        big = image::imageops::resize(img, img.width() * scale, img.height() * scale, image::imageops::FilterType::CatmullRom);
        &big
    };
    let mut png = Vec::new();
    let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
    rgb.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| e.to_string())?;
    let out = Command::new("tesseract")
        .args(["stdin", "-", "tsv"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            // It reads the whole image before writing anything, so this can't block on its output.
            child.stdin.take().expect("piped").write_all(&png)?;
            child.wait_with_output()
        });
    let out = out.map_err(|e| format!("Couldn't run Tesseract: {e}"))?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        return Err(format!("Tesseract couldn't read the text: {}", why.lines().last().unwrap_or("").trim()));
    }
    Ok(super::super::words::from_tesseract_tsv(&String::from_utf8_lossy(&out.stdout), 1.0 / scale as f32))
}

pub fn drag_files(_paths: &[PathBuf]) -> Result<(), String> {
    Err(NOT_HERE.into())
}
