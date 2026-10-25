//! The library on Windows, sharing the tray app's clipboard, settings and capture code.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::time::SystemTime;

use image::RgbaImage;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, FILETIME, SYSTEMTIME};
use windows_sys::Win32::Storage::FileSystem::{GetFileAttributesW, SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, INVALID_FILE_ATTRIBUTES};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows_sys::Win32::UI::Shell::{SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW};
use windows_sys::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, FindWindowW, PostMessageW, SetForegroundWindow, ShowWindow, ASFW_ANY, SW_RESTORE};

use super::super::env::TrayRequest;
use super::super::system_info::{self, Host};
use crate::capture::Image;
use crate::gallery::doc::flatten;
use crate::util::{wide, wide_os};

/// Pinning, OCR, copying as files and dragging files out are available.
pub const EXTRAS: bool = true;

/// True if this is the only library window; otherwise brings the existing one to the front.
/// True if this is the only window of its kind (`name`, e.g. "Gallery"); otherwise brings the
/// existing one (found by `title`) to the front.
pub fn claim_single_instance(name: &str, title: &str) -> bool {
    let name = wide(&format!("Local\\RSnip.{name}"));
    // Held (never closed) for the life of the process.
    let _mutex = unsafe { CreateMutexW(null(), 0, name.as_ptr()) };
    if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
        return true;
    }
    let title = wide(title);
    unsafe {
        let hwnd = FindWindowW(null(), title.as_ptr());
        if !hwnd.is_null() {
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
        }
    }
    false
}

/// True if no other RSnip process holds the lock `name` (with no window, unlike
/// `claim_single_instance`); this one then holds it until it exits.
pub fn claim_lock(name: &str) -> bool {
    let name = wide(&format!("Local\\RSnip.{name}"));
    let mutex = unsafe { CreateMutexW(null(), 0, name.as_ptr()) };
    if mutex.is_null() {
        return false;
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        // Not kept: it would keep the lock there after its holder ends.
        unsafe { CloseHandle(mutex) };
        return false;
    }
    true
}

pub fn start_with_windows() -> bool {
    crate::settings::startup_enabled()
}

pub fn set_start_with_windows(on: bool) -> bool {
    crate::settings::set_startup(on)
}

fn tray_window() -> windows_sys::Win32::Foundation::HWND {
    let class = wide(crate::MAIN_CLASS);
    unsafe { FindWindowW(class.as_ptr(), null()) }
}

pub fn tray_running() -> bool {
    !tray_window().is_null()
}

pub fn tell_tray(request: TrayRequest) {
    let (msg, wparam) = match request {
        TrayRequest::SettingsChanged => (crate::WM_APP_SETTINGS_CHANGED, 0),
        TrayRequest::RecordShortcut(action) => (crate::WM_APP_RECORD_SHORTCUT, action.index()),
        TrayRequest::InstallUpdate => (crate::WM_APP_UPDATE_NOW, 0),
        // Only Linux's background icon is stopped from Settings.
        TrayRequest::Quit => return,
    };
    let tray = tray_window();
    if !tray.is_null() {
        unsafe { PostMessageW(tray, msg, wparam, 0) };
    }
}

fn spawn_self(args: &[&str]) {
    if let Ok(exe) = std::env::current_exe() {
        // Let the new window come to the front.
        unsafe { AllowSetForegroundWindow(ASFW_ANY) };
        let _ = std::process::Command::new(exe).args(args).spawn();
    }
}

pub fn start_tray() {
    spawn_self(&[]);
}

/// Opens the Settings window, on System info if `system_info`.
pub fn open_settings(system_info: bool) {
    spawn_self(if system_info { &["--settings", "--system-info"] } else { &["--settings"] });
}

/// Windows' own "Use the Print screen key to open screen capture" (Settings → Accessibility →
/// Keyboard).
pub fn snipping_key() -> bool {
    crate::settings::read_dword("Control Panel\\Keyboard", "PrintScreenKeyForSnippingEnabled") == Some(1)
}

pub fn installed() -> bool {
    crate::update::installed()
}

pub fn shortcut_name(key: u32) -> String {
    crate::hook::Hotkey::unpack(key).name()
}

pub fn screenshots_dir() -> PathBuf {
    crate::output::screenshots_dir()
}

pub fn legacy_save_folder() -> PathBuf {
    crate::output::legacy_save_folder()
}

pub fn setting(name: &str) -> Option<u32> {
    crate::settings::get(name)
}

pub fn set_setting(name: &str, value: u32) {
    crate::settings::set(name, value);
}

pub fn text_setting(name: &str) -> Option<String> {
    crate::settings::get_text(name)
}

pub fn set_text_setting(name: &str, value: &str) {
    crate::settings::set_text(name, value);
}

/// BGRA pixels for the Windows APIs, composited over white since they ignore transparency.
fn to_bgra(img: &RgbaImage) -> Image {
    let img = flatten(img);
    let (width, height) = (img.width() as i32, img.height() as i32);
    let data = img.into_raw().chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], 255]).collect();
    Image { width, height, data }
}

pub fn copy_image(img: &RgbaImage) -> Result<(), String> {
    crate::output::copy_to_clipboard(null_mut(), &to_bgra(img))
}

pub fn copy_text(text: &str) -> Result<(), String> {
    crate::output::copy_text_to_clipboard(null_mut(), text)
}

pub fn copy_files(paths: &[PathBuf]) -> Result<(), String> {
    crate::output::copy_files_to_clipboard(null_mut(), paths)
}

pub fn open(path: &Path) {
    crate::shell_open(path);
}

pub fn show_in_folder(path: &Path) {
    let _ = std::process::Command::new("explorer").raw_arg(format!("/select,\"{}\"", path.display())).spawn();
}

/// Windows' folder picker, starting in `current`; `None` if it was cancelled.
pub fn pick_folder(title: &str, current: &Path) -> Result<Option<PathBuf>, String> {
    pick(title, Some(current), None)
}

/// Windows' file picker, offering the files matching `filter` (a name and a pattern like
/// "*.sxcu"); `None` if it was cancelled.
pub fn pick_file(title: &str, filter: (&str, &str)) -> Result<Option<PathBuf>, String> {
    pick(title, None, Some(filter))
}

/// A folder, or with `filter` a file.
fn pick(title: &str, current: Option<&Path>, filter: Option<(&str, &str)>) -> Result<Option<PathBuf>, String> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, SIGDN_FILESYSPATH};
    unsafe {
        // Run on a thread of its own (see `Env::pick_folder`), so this is its COM setup.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).map_err(|e| e.to_string())?;
        let options = dialog.GetOptions().map_err(|e| e.to_string())? | FOS_FORCEFILESYSTEM;
        dialog.SetOptions(if filter.is_some() { options } else { options | FOS_PICKFOLDERS }).map_err(|e| e.to_string())?;
        let _ = dialog.SetTitle(&HSTRING::from(title));
        if let Some((name, pattern)) = filter {
            let (name, pattern) = (HSTRING::from(name), HSTRING::from(pattern));
            let _ = dialog.SetFileTypes(&[COMDLG_FILTERSPEC { pszName: PCWSTR(name.as_ptr()), pszSpec: PCWSTR(pattern.as_ptr()) }]);
        }
        if let Some(start) = current.and_then(|c| SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(c.as_os_str()), None).ok()) {
            let _ = dialog.SetFolder(&start);
        }
        // Fails when it's cancelled.
        if dialog.Show(None).is_err() {
            return Ok(None);
        }
        let name = dialog.GetResult().and_then(|item| item.GetDisplayName(SIGDN_FILESYSPATH)).map_err(|e| e.to_string())?;
        let path = name.to_string().map_err(|e| e.to_string());
        CoTaskMemFree(Some(name.0 as *const _));
        path.map(|p| Some(PathBuf::from(p)))
    }
}

/// Moves a file to the Recycle Bin.
pub fn recycle(path: &Path) -> bool {
    // pFrom must be double-null-terminated.
    let mut from = wide_os(path.as_os_str());
    from.push(0);
    let mut op = SHFILEOPSTRUCTW {
        hwnd: null_mut(),
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        pTo: null(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI) as u16,
        fAnyOperationsAborted: 0,
        hNameMappings: null_mut(),
        lpszProgressTitle: null(),
    };
    unsafe { SHFileOperationW(&mut op) == 0 && op.fAnyOperationsAborted == 0 }
}

/// Keeps a folder out of the way in Explorer.
pub fn hide(dir: &Path) {
    let wide = wide_os(dir.as_os_str());
    unsafe {
        let attrs = GetFileAttributesW(wide.as_ptr());
        if attrs != INVALID_FILE_ATTRIBUTES && attrs & FILE_ATTRIBUTE_HIDDEN == 0 {
            SetFileAttributesW(wide.as_ptr(), attrs | FILE_ATTRIBUTE_HIDDEN);
        }
    }
}

/// The font for text annotations.
pub fn font_bytes() -> Option<Vec<u8>> {
    std::fs::read(font_path()?).ok()
}

fn font_path() -> Option<PathBuf> {
    let fonts = std::env::var_os("WINDIR").map(|w| Path::new(&w).join("Fonts"))?;
    ["seguisb.ttf", "segoeui.ttf", "arial.ttf"].iter().map(|name| fonts.join(name)).find(|f| f.is_file())
}

/// What System info reports about this machine, from the registry.
pub fn host() -> Host {
    const NT: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let text = |name: &str| crate::settings::read_machine_string(NT, name);
    let os = system_info::windows_name(
        text("ProductName").as_deref(),
        text("DisplayVersion").as_deref(),
        text("CurrentBuild").as_deref(),
        crate::settings::read_machine_dword(NT, "UBR"),
    );
    let mut memory: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    memory.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    let memory = (unsafe { GlobalMemoryStatusEx(&mut memory) } != 0).then_some(memory);
    Host {
        os,
        kernel: None,
        desktop: None,
        cpu: crate::settings::read_machine_string(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString"),
        threads: std::thread::available_parallelism().map_or(1, |n| n.get()),
        memory: memory.map(|m| m.ullTotalPhys),
        memory_available: memory.map(|m| m.ullAvailPhys),
        install: if installed() { "Installer" } else { "Standalone" }.into(),
        program: std::env::current_exe().ok(),
        home: std::env::var_os("USERPROFILE").filter(|h| !h.is_empty()).map(PathBuf::from),
        settings: r"HKCU\Software\RSnip".into(),
        font: font_path(),
        helpers: Vec::new(),
    }
}

/// The local calendar date of `t` as (year, month, day).
pub fn local_date(t: SystemTime) -> (i32, u32, u32) {
    // FILETIME counts 100 ns ticks since 1601.
    let since_1970 = t.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    let ticks = since_1970.as_nanos() as u64 / 100 + 116_444_736_000_000_000;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let (mut utc, mut local): (SYSTEMTIME, SYSTEMTIME) = unsafe { (std::mem::zeroed(), std::mem::zeroed()) };
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc) == 0 || SystemTimeToTzSpecificLocalTime(null(), &utc, &mut local) == 0 {
            local = utc;
        }
    }
    (local.wYear as i32, local.wMonth as u32, local.wDay as u32)
}

pub fn pin(img: &RgbaImage) -> Result<(), String> {
    crate::pin::pin_image(&to_bgra(img))
}

pub fn pin_file(path: &Path) {
    crate::pin::spawn(path);
}

/// Why H.265 can't be chosen.
pub const HEVC_MISSING: &str = "H.265 needs an encoder this PC doesn't have; the HEVC Video Extensions from the Microsoft Store add one";

/// What's missing for recording, if anything: nothing, Windows has it all.
pub fn recording_problem(_hevc: bool) -> Option<String> {
    None
}

/// Whether recordings can be H.265 (see `encode::hevc_available`).
pub fn hevc_available() -> bool {
    crate::encode::hevc_available()
}

/// What reads the text in screenshots, for Settings.
pub const TEXT_RECOGNITION: &str = "Windows' built-in text recognition";

/// Windows' text recognition on an image file, for search.
pub fn ocr() -> Option<super::super::env::Ocr> {
    Some(std::sync::Arc::new(|path: &Path| {
        let img = image::open(path).map_err(|e| e.to_string())?.to_rgba8();
        crate::ocr::recognize(&to_bgra(&img))
    }))
}

/// Windows' text recognition word by word, for copying text out of an image.
pub fn read_words() -> Option<super::super::env::ReadWords> {
    Some(std::sync::Arc::new(|img: &RgbaImage| crate::ocr::recognize_words(&to_bgra(img))))
}

pub fn drag_files(paths: &[PathBuf]) -> Result<(), String> {
    super::super::dragout::drag_files(paths)
}
