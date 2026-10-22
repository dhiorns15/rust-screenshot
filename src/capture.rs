use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::util::*;

/// 32-bit BGRA pixels, top-down rows, no padding.
pub struct Image {
    pub width: i32,
    pub height: i32,
    pub data: Vec<u8>,
}

impl Image {
    pub fn crop(&self, r: RECT) -> Image {
        let r = intersect(&r, &rect(0, 0, self.width, self.height));
        let (w, h) = ((r.right - r.left).max(0), (r.bottom - r.top).max(0));
        let mut data = Vec::with_capacity((w * h * 4) as usize);
        for y in r.top..r.top + h {
            let start = ((y * self.width + r.left) * 4) as usize;
            data.extend_from_slice(&self.data[start..start + (w * 4) as usize]);
        }
        Image { width: w, height: h, data }
    }
}

/// A capture of the whole virtual desktop. `x`/`y` is the screen position of pixel (0, 0).
pub struct Screenshot {
    pub x: i32,
    pub y: i32,
    pub image: Image,
}

impl Screenshot {
    /// The part of `r` (virtual-screen coordinates) that's on screen, in image coordinates.
    pub fn to_image_rect(&self, r: RECT) -> Option<RECT> {
        let bounds = rect(0, 0, self.image.width, self.image.height);
        let r = intersect(&offset(&r, -self.x, -self.y), &bounds);
        (!is_empty(&r)).then_some(r)
    }

    pub fn to_screen_rect(&self, r: RECT) -> RECT {
        offset(&r, self.x, self.y)
    }
}

fn bitmap_info(width: i32, height: i32) -> BITMAPINFO {
    let mut bmi: BITMAPINFO = unsafe { zeroed() };
    bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = width;
    bmi.bmiHeader.biHeight = -height; // top-down
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;
    bmi
}

unsafe fn create_dib(hdc: HDC, width: i32, height: i32) -> (HBITMAP, *mut u8) {
    let bmi = bitmap_info(width, height);
    let mut bits: *mut c_void = null_mut();
    let bitmap = CreateDIBSection(hdc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    (bitmap, bits as *mut u8)
}

/// Creates a GDI bitmap holding a copy of `data` (BGRA, top-down).
pub unsafe fn dib_from_pixels(hdc: HDC, width: i32, height: i32, data: &[u8]) -> HBITMAP {
    let (bitmap, bits) = create_dib(hdc, width, height);
    if !bitmap.is_null() {
        std::ptr::copy_nonoverlapping(data.as_ptr(), bits, data.len());
    }
    bitmap
}

/// Captures every monitor at physical resolution.
pub fn capture_screen() -> Option<Screenshot> {
    let (x, y, w, h) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    capture_rect(rect(x, y, x + w, y + h)).map(|image| Screenshot { x, y, image })
}

/// Captures part of the screen, given in virtual-screen coordinates.
pub fn capture_rect(r: RECT) -> Option<Image> {
    grab(r, false)
}

/// Like `capture_rect`, with the mouse pointer drawn in (for recordings).
pub fn capture_rect_with_cursor(r: RECT) -> Option<Image> {
    grab(r, true)
}

fn grab(r: RECT, cursor: bool) -> Option<Image> {
    let (x, y, w, h) = (r.left, r.top, r.right - r.left, r.bottom - r.top);
    if w <= 0 || h <= 0 {
        return None;
    }
    unsafe {
        let screen = GetDC(null_mut());
        let mem = CreateCompatibleDC(screen);
        let (bitmap, bits) = create_dib(screen, w, h);
        let mut image = None;
        if !bitmap.is_null() {
            let old = SelectObject(mem, bitmap);
            if BitBlt(mem, 0, 0, w, h, screen, x, y, SRCCOPY | CAPTUREBLT) != 0 {
                if cursor {
                    draw_cursor(mem, x, y);
                }
                GdiFlush();
                let mut data = std::slice::from_raw_parts(bits, (w * h * 4) as usize).to_vec();
                for px in data.chunks_exact_mut(4) {
                    px[3] = 255;
                }
                image = Some(Image { width: w, height: h, data });
            }
            SelectObject(mem, old);
            DeleteObject(bitmap);
        }
        DeleteDC(mem);
        ReleaseDC(null_mut(), screen);
        image
    }
}

/// Draws the current mouse pointer into `dc`, whose top-left is at screen (`x`, `y`).
unsafe fn draw_cursor(dc: HDC, x: i32, y: i32) {
    let mut ci: CURSORINFO = zeroed();
    ci.cbSize = size_of::<CURSORINFO>() as u32;
    if GetCursorInfo(&mut ci) == 0 || ci.flags & CURSOR_SHOWING == 0 {
        return;
    }
    let mut info: ICONINFO = zeroed();
    if GetIconInfo(ci.hCursor, &mut info) == 0 {
        return;
    }
    let (px, py) = (ci.ptScreenPos.x - x - info.xHotspot as i32, ci.ptScreenPos.y - y - info.yHotspot as i32);
    DrawIconEx(dc, px, py, ci.hCursor, 0, 0, 0, null_mut(), DI_NORMAL);
    for bitmap in [info.hbmMask, info.hbmColor] {
        if !bitmap.is_null() {
            DeleteObject(bitmap);
        }
    }
}

/// Something the user can click to capture: a window, or a whole monitor (`window` is null).
#[derive(Clone, Copy)]
pub struct Target {
    pub rect: RECT,
    pub window: HWND,
}

struct Targets {
    list: Vec<Target>,
    pid: u32,
}

/// Click targets in image coordinates, top-most first: visible top-level windows, then monitors.
pub fn window_targets(shot: &Screenshot) -> Vec<Target> {
    let mut ctx = Targets { list: Vec::new(), pid: unsafe { GetCurrentProcessId() } };
    unsafe {
        EnumWindows(Some(enum_window), &mut ctx as *mut Targets as LPARAM);
    }
    let mut targets = to_image_space(shot, ctx.list);
    targets.extend(monitor_rects(shot).into_iter().map(|rect| Target { rect, window: null_mut() }));
    targets
}

/// Each monitor's bounds, in image coordinates.
pub fn monitor_rects(shot: &Screenshot) -> Vec<RECT> {
    let mut ctx = Targets { list: Vec::new(), pid: 0 };
    unsafe {
        EnumDisplayMonitors(null_mut(), null(), Some(enum_monitor), &mut ctx as *mut Targets as LPARAM);
    }
    to_image_space(shot, ctx.list).into_iter().map(|t| t.rect).collect()
}

fn to_image_space(shot: &Screenshot, targets: Vec<Target>) -> Vec<Target> {
    let bounds = rect(0, 0, shot.image.width, shot.image.height);
    targets
        .into_iter()
        .map(|t| Target { rect: intersect(&offset(&t.rect, -shot.x, -shot.y), &bounds), ..t })
        .filter(|t| !is_empty(&t.rect))
        .collect()
}

/// Visible bounds of a window in screen coordinates, without the invisible resize border.
pub fn window_rect(hwnd: HWND) -> Option<RECT> {
    unsafe {
        let mut r: RECT = zeroed();
        let hr = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            &mut r as *mut RECT as *mut c_void,
            size_of::<RECT>() as u32,
        );
        if hr < 0 && GetWindowRect(hwnd, &mut r) == 0 {
            return None;
        }
        Some(r)
    }
}

/// Brings `hwnd` in front of whatever covers it, waits for it to repaint, and captures just it.
pub fn capture_window_in_front(hwnd: HWND) -> Option<Image> {
    unsafe {
        // We received the click that closed the overlay, so Windows lets us change the foreground.
        if SetForegroundWindow(hwnd) == 0 {
            SetWindowPos(hwnd, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
        pump_messages_for(std::time::Duration::from_millis(200));
        DwmFlush();
        let shot = capture_screen()?;
        let r = to_image_space(&shot, vec![Target { rect: window_rect(hwnd)?, window: hwnd }]).pop()?;
        Some(shot.image.crop(r.rect))
    }
}

/// Waits while still dispatching messages, so the keyboard hook never stalls.
pub unsafe fn pump_messages_for(duration: std::time::Duration) {
    let start = std::time::Instant::now();
    let mut msg: MSG = zeroed();
    while start.elapsed() < duration {
        while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        windows_sys::Win32::System::Threading::Sleep(10);
    }
}

unsafe extern "system" fn enum_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let ctx = &mut *(lparam as *mut Targets);
    if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
        return 1;
    }
    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid == ctx.pid {
        return 1;
    }
    // Click-through overlays (game bars, GPU overlays, ...) shouldn't be pickable.
    if GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT != 0 {
        return 1;
    }
    let mut cloaked: u32 = 0;
    let hr = DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED as u32,
        &mut cloaked as *mut u32 as *mut c_void,
        size_of::<u32>() as u32,
    );
    if hr >= 0 && cloaked != 0 {
        return 1;
    }
    // The desktop is covered by the monitor targets instead.
    let mut class = [0u16; 32];
    let len = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) as usize;
    let class = String::from_utf16_lossy(&class[..len]);
    if class == "Progman" || class == "WorkerW" {
        return 1;
    }

    if let Some(r) = window_rect(hwnd) {
        if r.right - r.left > 1 && r.bottom - r.top > 1 {
            ctx.list.push(Target { rect: r, window: hwnd });
        }
    }
    1
}

unsafe extern "system" fn enum_monitor(_: HMONITOR, _: HDC, r: *mut RECT, lparam: LPARAM) -> BOOL {
    (*(lparam as *mut Targets)).list.push(Target { rect: *r, window: null_mut() });
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_clamps_to_bounds() {
        let data = (0..4 * 3 * 4).map(|i| i as u8).collect();
        let img = Image { width: 4, height: 3, data };
        let c = img.crop(rect(2, 1, 10, 10));
        assert_eq!((c.width, c.height), (2, 2));
        assert_eq!(&c.data[..4], &img.data[(4 + 2) * 4..(4 + 2) * 4 + 4]);
    }

    #[test]
    fn maps_regions_between_screen_and_image() {
        // A desktop whose left monitor starts at x = -1920.
        let shot = Screenshot { x: -1920, y: 0, image: Image { width: 3840, height: 1080, data: Vec::new() } };
        let r = shot.to_image_rect(rect(-100, 10, 100, 60)).unwrap();
        assert!(same(&r, &rect(1820, 10, 2020, 60)));
        assert!(same(&shot.to_screen_rect(r), &rect(-100, 10, 100, 60)));
        let clipped = shot.to_image_rect(rect(1900, 1000, 2100, 1200)).unwrap();
        assert!(same(&clipped, &rect(3820, 1000, 3840, 1080)), "clipped to the screen");
        assert!(shot.to_image_rect(rect(5000, 0, 5100, 50)).is_none(), "entirely off screen");
    }

    #[test]
    fn captures_screen() {
        let shot = capture_screen().expect("capture");
        assert!(shot.image.width > 0 && shot.image.height > 0);
        assert_eq!(shot.image.data.len(), (shot.image.width * shot.image.height * 4) as usize);
        assert!(!window_targets(&shot).is_empty());
    }
}
