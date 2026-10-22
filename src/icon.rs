//! Turns the drawn app icon into an HICON for the tray and windows.

use std::ptr::null_mut;

use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::capture::dib_from_pixels;
pub use crate::icon_art::render;

pub fn create_icon(size: i32) -> HICON {
    let data = render(size);
    unsafe {
        let screen = GetDC(null_mut());
        let color = dib_from_pixels(screen, size, size, &data);
        ReleaseDC(null_mut(), screen);
        let mask_bits = vec![0u8; ((size + 15) / 16 * 2 * size) as usize];
        let mask = CreateBitmap(size, size, 1, 1, mask_bits.as_ptr() as *const _);
        let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info);
        DeleteObject(color);
        DeleteObject(mask);
        icon
    }
}
