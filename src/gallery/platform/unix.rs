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

/// Pinning, OCR, copying as files and dragging files out are Windows-only.
pub const EXTRAS: bool = false;
const NOT_HERE: &str = "Not available on this system";

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

/// True if this is the only window of its kind (held with a lock file until the process exits).
pub fn claim_single_instance(name: &str, _title: &str) -> bool {
    static LOCK: OnceLock<File> = OnceLock::new();
    let user = std::env::var("USER").unwrap_or_default();
    let path = std::env::temp_dir().join(format!("rsnip-{}-{user}.lock", name.to_lowercase()));
    let Ok(file) = File::options().create(true).truncate(false).write(true).open(&path) else { return true };
    if file.try_lock().is_err() {
        return false;
    }
    let _ = LOCK.set(file);
    true
}

// There's no tray app here: no startup entry, global shortcuts or installer.
pub fn start_with_windows() -> bool {
    false
}

pub fn set_start_with_windows(_on: bool) -> bool {
    false
}

pub fn tray_running() -> bool {
    false
}

pub fn tell_tray(_request: super::super::env::TrayRequest) {}

pub fn start_tray() {}

pub fn open_settings() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = Command::new(exe).arg("--settings").spawn();
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
const HELPERS: [&str; 5] = ["xdg-open", "xdg-user-dir", "fc-match", "curl", "wget"];

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
    // installer/linux/install.sh copies it here.
    let installed = program.as_deref().and_then(Path::parent) == Some(home.join(".local/bin").as_path());
    Host {
        os: os.unwrap_or_else(|| std::env::consts::OS.to_string()),
        kernel: read("/proc/sys/kernel/osrelease").map(|k| k.trim().to_string()).filter(|k| !k.is_empty()),
        desktop,
        cpu: read("/proc/cpuinfo").and_then(|t| system_info::cpuinfo_model(&t)),
        threads: std::thread::available_parallelism().map_or(1, |n| n.get()),
        memory: system_info::meminfo(&meminfo, "MemTotal"),
        memory_available: system_info::meminfo(&meminfo, "MemAvailable"),
        install: if installed { "install.sh" } else { "Standalone" }.into(),
        program,
        settings: system_info::tidy_path(&settings_path(), Some(&home)),
        font: font_path(),
        helpers: HELPERS.map(|h| (h, on_path(h))).to_vec(),
        home: Some(home),
    }
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

/// No text recognition here, so search uses names only.
pub fn ocr() -> Option<super::super::env::Ocr> {
    None
}

pub fn read_text(_img: &RgbaImage) -> Result<String, String> {
    Err(NOT_HERE.into())
}

pub fn drag_files(_paths: &[PathBuf]) -> Result<(), String> {
    Err(NOT_HERE.into())
}
