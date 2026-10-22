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

/// Puts the image on the clipboard as a 24-bit DIB, which every app that accepts images understands.
pub fn copy_to_clipboard(owner: HWND, img: &Image) -> Result<(), String> {
    let (w, h) = (img.width as usize, img.height as usize);
    let stride = (w * 3 + 3) & !3;
    let header = size_of::<BITMAPINFOHEADER>();
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, header + stride * h);
        if mem.is_null() {
            return Err("Out of memory".into());
        }
        let p = GlobalLock(mem) as *mut u8;
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
        std::ptr::write_unaligned(p as *mut BITMAPINFOHEADER, info);
        let pixels = std::slice::from_raw_parts_mut(p.add(header), stride * h);
        for (y, src) in img.data.chunks_exact(w * 4).enumerate() {
            let row = &mut pixels[(h - 1 - y) * stride..][..stride];
            for (dst, px) in row.chunks_exact_mut(3).zip(src.chunks_exact(4)) {
                dst.copy_from_slice(&px[..3]);
            }
        }
        GlobalUnlock(mem);

        // Another app may briefly hold the clipboard open.
        let mut opened = false;
        for _ in 0..10 {
            if OpenClipboard(owner) != 0 {
                opened = true;
                break;
            }
            Sleep(30);
        }
        if !opened {
            GlobalFree(mem);
            return Err("The clipboard is in use by another app".into());
        }
        EmptyClipboard();
        let ok = !SetClipboardData(CF_DIB, mem).is_null();
        CloseClipboard();
        if !ok {
            GlobalFree(mem);
            return Err("Couldn't place the image on the clipboard".into());
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

/// The same folder Windows uses for Win+PrtScn captures.
pub fn screenshots_dir() -> PathBuf {
    unsafe {
        known_folder(&FOLDERID_Screenshots)
            .or_else(|| known_folder(&FOLDERID_Pictures).map(|p| p.join("Screenshots")))
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default()).join("Pictures\\Screenshots")
            })
    }
}

pub fn next_path() -> Result<PathBuf, String> {
    let dir = screenshots_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Can't create {}: {e}", dir.display()))?;
    let mut t = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    let base = format!(
        "Screenshot {:04}-{:02}-{:02} {:02}{:02}{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    );
    let mut path = dir.join(format!("{base}.png"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{base} ({n}).png"));
        n += 1;
    }
    Ok(path)
}

pub fn save_png(img: &Image, path: &Path) -> Result<(), String> {
    let file = File::create(path).map_err(|e| format!("Can't write {}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), img.width as u32, img.height as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let rgb: Vec<u8> = img.data.chunks_exact(4).flat_map(|px| [px[2], px[1], px[0]]).collect();
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(&rgb))
        .map_err(|e| format!("Can't encode PNG: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_png() {
        let img = Image { width: 3, height: 2, data: vec![200; 3 * 2 * 4] };
        let path = std::env::temp_dir().join(format!("rsnip-test-{}.png", std::process::id()));
        save_png(&img, &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(&bytes[1..4], b"PNG");
    }
}
