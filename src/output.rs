//! Delivering a capture: clipboard and PNG files.

use std::fs::File;
use std::io::BufWriter;
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::ptr::null_mut;

use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::{BITMAPINFOHEADER, BI_RGB};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::DataExchange::*;
use windows_sys::Win32::System::Memory::*;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::Sleep;
use windows_sys::Win32::UI::Shell::*;

use crate::capture::Image;
use crate::util::from_wide_ptr;

const CF_DIB: u32 = 8;
const CF_HDROP: u32 = 15;
const CF_UNICODETEXT: u32 = 13;
const CF_DIBV5: u32 = 17;

/// Puts the image on the clipboard as a 24-bit DIB, which every app that accepts images understands.
pub fn copy_to_clipboard(owner: HWND, img: &Image) -> Result<(), String> {
    set_clipboard(owner, CF_DIB, &dib(img))
}

/// `img` as a 24-bit device-independent bitmap: a `BITMAPINFOHEADER`, then the rows bottom-up.
fn dib(img: &Image) -> Vec<u8> {
    let (w, h) = (img.width as usize, img.height as usize);
    let stride = (w * 3 + 3) & !3;
    let header = size_of::<BITMAPINFOHEADER>();
    let mut data = vec![0u8; header + stride * h];
    let info = BITMAPINFOHEADER {
        biSize: header as u32,
        biWidth: img.width,
        biHeight: img.height, // bottom-up
        biPlanes: 1,
        biBitCount: 24,
        biCompression: BI_RGB,
        biSizeImage: (stride * h) as u32,
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: 0,
        biClrImportant: 0,
    };
    unsafe { std::ptr::write_unaligned(data.as_mut_ptr() as *mut BITMAPINFOHEADER, info) };
    let pixels = &mut data[header..];
    for (y, src) in img.data.chunks_exact(w * 4).enumerate() {
        let row = &mut pixels[(h - 1 - y) * stride..][..stride];
        for (dst, px) in row.chunks_exact_mut(3).zip(src.chunks_exact(4)) {
            dst.copy_from_slice(&px[..3]);
        }
    }
    data
}

/// The picture on the clipboard as the bytes of an image file, if it has one: a PNG where the app
/// that copied it gave one (browsers and Office do), which keeps its transparency, else a BMP made
/// from its bitmap (which Windows makes for any picture copied).
pub fn clipboard_picture(owner: HWND) -> Result<Option<Vec<u8>>, String> {
    let png = unsafe { RegisterClipboardFormatW(crate::util::wide("PNG").as_ptr()) };
    with_clipboard(owner, || unsafe {
        let png = (png != 0).then(|| clipboard_bytes(png)).flatten();
        png.or_else(|| [CF_DIB, CF_DIBV5].into_iter().find_map(|format| clipboard_bytes(format).and_then(|dib| bmp_file(&dib))))
    })
}

/// A device-independent bitmap (a `BITMAPINFOHEADER` or a later one, any colour table, then the
/// pixels) as a BMP file: the file header in front, saying where the pixels start.
fn bmp_file(dib: &[u8]) -> Option<Vec<u8>> {
    let u32_at = |at: usize| Some(u32::from_le_bytes(dib.get(at..at + 4)?.try_into().ok()?));
    let header = u32_at(0)?;
    if header < 40 || dib.len() < header as usize {
        return None;
    }
    let bits = u32::from(u16::from_le_bytes(dib.get(14..16)?.try_into().ok()?));
    let (compression, colours_used) = (u32_at(16)?, u32_at(32)?);
    // A plain info header is followed by the colour masks, for BI_BITFIELDS and BI_ALPHABITFIELDS;
    // later headers have them inside.
    let masks = match (header, compression) {
        (40, 3) => 12,
        (40, 6) => 16,
        _ => 0,
    };
    let colours = if bits <= 8 && colours_used == 0 { 1 << bits } else { colours_used.min(256) };
    let start = 14 + header + masks + colours * 4;
    let mut file = Vec::with_capacity(14 + dib.len());
    file.extend_from_slice(b"BM");
    file.extend_from_slice(&(14 + dib.len() as u32).to_le_bytes());
    file.extend_from_slice(&[0; 4]);
    file.extend_from_slice(&start.to_le_bytes());
    file.extend_from_slice(dib);
    Some(file)
}

/// The files on the clipboard (copied in Explorer), if any.
pub fn clipboard_files(owner: HWND) -> Result<Vec<PathBuf>, String> {
    with_clipboard(owner, || unsafe { clipboard_bytes(CF_HDROP) }.map(|bytes| dropped_files(&bytes)).unwrap_or_default())
}

/// The paths in a `DROPFILES` (see `drop_files`).
fn dropped_files(bytes: &[u8]) -> Vec<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let u32_at = |at: usize| Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    let (Some(start), Some(wide)) = (u32_at(0), u32_at(16)) else { return Vec::new() };
    let list = bytes.get(start as usize..).unwrap_or_default();
    if wide == 0 {
        // In the ANSI code page, from a very old app; ASCII paths at least come through.
        return list.split(|&b| b == 0).take_while(|p| !p.is_empty()).map(|p| PathBuf::from(String::from_utf8_lossy(p).into_owned())).collect();
    }
    let units: Vec<u16> = list.chunks_exact(2).map(|u| u16::from_le_bytes([u[0], u[1]])).collect();
    units.split(|&u| u == 0).take_while(|p| !p.is_empty()).map(|p| PathBuf::from(std::ffi::OsString::from_wide(p))).collect()
}

pub fn copy_text_to_clipboard(owner: HWND, text: &str) -> Result<(), String> {
    let bytes: Vec<u8> = text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
    set_clipboard(owner, CF_UNICODETEXT, &bytes)
}

/// The text on the clipboard, if it has some.
pub fn clipboard_text(owner: HWND) -> Result<Option<String>, String> {
    with_clipboard(owner, || unsafe { clipboard_bytes(CF_UNICODETEXT) }.map(|bytes| text_of(&bytes)))
}

/// UTF-16 clipboard text, up to its terminating NUL.
fn text_of(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).take_while(|&u| u != 0).collect();
    String::from_utf16_lossy(&units)
}

/// Opens the clipboard for `f`, then closes it. Another app may briefly hold it open, so this
/// tries a few times.
fn with_clipboard<T>(owner: HWND, f: impl FnOnce() -> T) -> Result<T, String> {
    unsafe {
        for _ in 0..10 {
            if OpenClipboard(owner) != 0 {
                let result = f();
                CloseClipboard();
                return Ok(result);
            }
            Sleep(30);
        }
    }
    Err("The clipboard is in use by another app".into())
}

/// A copy of what the open clipboard holds in `format`, if it has that.
unsafe fn clipboard_bytes(format: u32) -> Option<Vec<u8>> {
    let handle = GetClipboardData(format);
    if handle.is_null() {
        return None;
    }
    let size = GlobalSize(handle);
    let data = GlobalLock(handle) as *const u8;
    if data.is_null() {
        return None;
    }
    let bytes = std::slice::from_raw_parts(data, size).to_vec();
    GlobalUnlock(handle);
    Some(bytes)
}

/// Puts files on the clipboard the way Explorer's Copy does, so pasting attaches or copies them.
pub fn copy_files_to_clipboard(owner: HWND, paths: &[PathBuf]) -> Result<(), String> {
    set_clipboard(owner, CF_HDROP, &drop_files(paths))
}

/// A `DROPFILES` header followed by the double-null-terminated list of UTF-16 paths.
fn drop_files(paths: &[PathBuf]) -> Vec<u8> {
    const HEADER: u32 = 20; // pFiles, pt.x, pt.y, fNC, fWide
    let mut out = Vec::new();
    for field in [HEADER, 0, 0, 0, 1] {
        out.extend_from_slice(&field.to_le_bytes());
    }
    for path in paths {
        for unit in crate::util::wide_os(path.as_os_str()) {
            out.extend_from_slice(&unit.to_le_bytes());
        }
    }
    out.extend_from_slice(&[0, 0]);
    out
}

/// Replaces the clipboard contents with `data` in `format`.
fn set_clipboard(owner: HWND, format: u32, data: &[u8]) -> Result<(), String> {
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, data.len());
        if mem.is_null() {
            return Err("Out of memory".into());
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), GlobalLock(mem) as *mut u8, data.len());
        GlobalUnlock(mem);
        let set = with_clipboard(owner, || {
            EmptyClipboard();
            !SetClipboardData(format, mem).is_null()
        });
        if set != Ok(true) {
            GlobalFree(mem);
            return Err(set.err().unwrap_or_else(|| "Couldn't write to the clipboard".into()));
        }
    }
    Ok(())
}

unsafe fn known_folder(id: &GUID) -> Option<PathBuf> {
    let mut p = null_mut();
    let hr = SHGetKnownFolderPath(id, KF_FLAG_CREATE as _, null_mut(), &mut p);
    let path = (hr >= 0 && !p.is_null()).then(|| PathBuf::from(from_wide_ptr(p)));
    CoTaskMemFree(p as *const _);
    path
}

/// Where screenshots and recordings are saved: the folder chosen in Settings, else `RSnip` in the
/// user's folder (see `options::save_folder`).
pub fn screenshots_dir() -> PathBuf {
    crate::options::save_folder(crate::settings::get_text(crate::options::SAVE_FOLDER), legacy_save_folder(), &home(), |dir| {
        crate::settings::set_text(crate::options::SAVE_FOLDER, &dir.to_string_lossy())
    })
}

/// The folder Windows uses for Win+PrtScn captures, where RSnip saved before there was a choice.
pub fn legacy_save_folder() -> PathBuf {
    unsafe { known_folder(&FOLDERID_Screenshots).or_else(|| known_folder(&FOLDERID_Pictures).map(|p| p.join("Screenshots"))) }.unwrap_or_else(|| home().join("Pictures\\Screenshots"))
}

fn home() -> PathBuf {
    unsafe { known_folder(&FOLDERID_Profile) }.unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default()))
}

pub fn next_path() -> Result<PathBuf, String> {
    new_path("Screenshot", "png")
}

/// A new, unused "`kind` YYYY-MM-DD HHMMSS.`ext`" path in the screenshots folder.
pub fn new_path(kind: &str, ext: &str) -> Result<PathBuf, String> {
    let dir = screenshots_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Can't create {}: {e}", dir.display()))?;
    Ok(new_path_in(&dir, kind, ext))
}

/// A new, unused "`kind` YYYY-MM-DD HHMMSS.`ext`" path in `dir`, with " (2)" and so on if that's
/// taken.
pub fn new_path_in(dir: &Path, kind: &str, ext: &str) -> PathBuf {
    let mut t = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    let base = format!(
        "{kind} {:04}-{:02}-{:02} {:02}{:02}{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    );
    let mut path = dir.join(format!("{base}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{base} ({n}).{ext}"));
        n += 1;
    }
    path
}

pub fn save_png(img: &Image, path: &Path) -> Result<(), String> {
    let file = File::create(path).map_err(|e| format!("Can't write {}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), img.width as u32, img.height as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let rgb: Vec<u8> = img.data.chunks_exact(4).flat_map(|px| [px[2], px[1], px[0]]).collect();
    let result = encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(&rgb))
        .map_err(|e| format!("Can't encode PNG: {e}"));
    if result.is_err() {
        // Not a picture, so don't leave it for the library to try to show.
        let _ = std::fs::remove_file(path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_files_lists_wide_paths() {
        let data = drop_files(&[PathBuf::from(r"C:\a.png"), PathBuf::from("b")]);
        assert_eq!(&data[..4], &20u32.to_le_bytes(), "paths start after the header");
        assert_eq!(&data[16..20], &1u32.to_le_bytes(), "wide strings");
        let units: Vec<u16> = data[20..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let expected: Vec<u16> = "C:\\a.png\0b\0\0".encode_utf16().collect();
        assert_eq!(units, expected);
        assert_eq!(dropped_files(&data), [PathBuf::from(r"C:\a.png"), PathBuf::from("b")], "and read back");
        assert!(dropped_files(&[1, 2]).is_empty(), "nothing from what isn't one");
    }

    #[test]
    fn a_copied_bitmap_reads_back_as_a_bmp_file() {
        // BGRA, so the first pixel is red, the second green: 3 by 2, padded rows.
        let mut data = Vec::new();
        for i in 0..6u8 {
            data.extend_from_slice(&[i * 10, 200 - i * 10, 50 + i, 255]);
        }
        let img = Image { width: 3, height: 2, data: data.clone() };
        let read = image::load_from_memory(&bmp_file(&dib(&img)).unwrap()).unwrap().to_rgba8();
        assert_eq!(read.dimensions(), (3, 2));
        let pixels: Vec<u8> = read.pixels().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
        assert_eq!(pixels, data, "top row first, as copied");

        // 32 bits with colour masks after a plain header: the pixels start after the masks.
        let mut masked = vec![0u8; 40 + 12 + 4];
        masked[..4].copy_from_slice(&40u32.to_le_bytes());
        masked[4..8].copy_from_slice(&1u32.to_le_bytes());
        masked[8..12].copy_from_slice(&1u32.to_le_bytes());
        masked[12..14].copy_from_slice(&1u16.to_le_bytes());
        masked[14..16].copy_from_slice(&32u16.to_le_bytes());
        masked[16..20].copy_from_slice(&3u32.to_le_bytes());
        for (at, mask) in [(40, 0x00ff_0000u32), (44, 0x0000_ff00), (48, 0x0000_00ff)] {
            masked[at..at + 4].copy_from_slice(&mask.to_le_bytes());
        }
        masked[52..56].copy_from_slice(&[30, 20, 10, 0]);
        let read = image::load_from_memory(&bmp_file(&masked).unwrap()).unwrap().to_rgba8();
        assert_eq!(read.get_pixel(0, 0).0[..3], [10, 20, 30]);
        assert!(bmp_file(&[0; 12]).is_none(), "not a header it knows");
    }

    #[test]
    fn writes_png() {
        let img = Image { width: 3, height: 2, data: vec![200; 3 * 2 * 4] };
        let path = std::env::temp_dir().join(format!("rsnip-test-{}.png", std::process::id()));
        save_png(&img, &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(&bytes[1..4], b"PNG");
    }

    #[test]
    fn a_failed_save_leaves_no_file() {
        // Far fewer pixels than the size says, which the encoder refuses.
        let img = Image { width: 3, height: 2, data: vec![200; 4] };
        let path = std::env::temp_dir().join(format!("rsnip-test-bad-{}.png", std::process::id()));
        assert!(save_png(&img, &path).is_err());
        assert!(!path.exists(), "nothing half-written is left behind");
    }
}
