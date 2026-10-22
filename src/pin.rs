//! A capture pinned on top of other windows (`rsnip --pin <file>`), to keep for reference while
//! working. Drag to move, wheel to fade, Ctrl+wheel to zoom, double-click or Esc to close.

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::capture::{dib_from_pixels, Image};
use crate::util::wide;

const CLASS: &str = "RSnip.Pin";
const BORDER: i32 = 2;
const ACCENT: COLORREF = 0x00FF9E3B; // RGB(59, 158, 255)
const MIN_ALPHA: u8 = 51; // 20%

const CMD_COPY: usize = 1;
const CMD_SAVE: usize = 2;
const CMD_ACTUAL_SIZE: usize = 3;
const CMD_CLOSE: usize = 4;

struct Pin {
    image: Image,
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    zoom: f32,
    alpha: u8,
}

/// Where the editor and overlay put images to pin; the pin deletes these once loaded.
pub fn temp_path() -> PathBuf {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    temp_dir().join(format!("pin-{}-{stamp}.png", std::process::id()))
}

fn temp_dir() -> PathBuf {
    std::env::temp_dir().join("RSnip").join("pins")
}

/// Zoom after `notches` Ctrl+wheel steps, 10% per step, kept within 25%–400%.
pub fn zoom_after(zoom: f32, notches: f32) -> f32 {
    (zoom * 1.1f32.powf(notches)).clamp(0.25, 4.0)
}

/// Opacity after `notches` wheel steps, 10% per step, never fading out entirely.
pub fn alpha_after(alpha: u8, notches: f32) -> u8 {
    (alpha as f32 + notches * 25.5).round().clamp(MIN_ALPHA as f32, 255.0) as u8
}

/// The window position that keeps the image point under `cursor` fixed while resizing from
/// `old` to `new` (both inner sizes).
fn anchored(left: i32, cursor: i32, old: i32, new: i32) -> i32 {
    let offset = (cursor - left - BORDER) as f32 * new as f32 / old.max(1) as f32;
    cursor - BORDER - offset.round() as i32
}

pub fn run(path: PathBuf) {
    let Ok(img) = image::open(&path) else { return };
    if path.starts_with(temp_dir()) {
        let _ = std::fs::remove_file(&path);
    }
    let rgba = img.to_rgba8();
    let (width, height) = (rgba.width() as i32, rgba.height() as i32);
    let data = rgba.into_raw().chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], 255]).collect();
    unsafe { show(Image { width, height, data }) }
}

unsafe fn show(image: Image) {
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    let hinstance = GetModuleHandleW(null());
    let class = wide(CLASS);
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: CS_DROPSHADOW,
        lpfnWndProc: Some(wndproc),
        hInstance: hinstance,
        hCursor: LoadCursorW(null_mut(), IDC_SIZEALL),
        lpszClassName: class.as_ptr(),
        ..zeroed()
    };
    RegisterClassExW(&wc);

    // Centre on the monitor under the mouse, shrinking to fit if needed.
    let mut cursor: POINT = zeroed();
    GetCursorPos(&mut cursor);
    let mut info: MONITORINFO = zeroed();
    info.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST), &mut info);
    let work = info.rcWork;
    let (ww, wh) = (work.right - work.left, work.bottom - work.top);
    let fit = (ww as f32 * 0.9 / image.width as f32).min(wh as f32 * 0.9 / image.height as f32);
    let zoom = fit.min(1.0);
    let (w, h) = ((image.width as f32 * zoom) as i32 + 2 * BORDER, (image.height as f32 * zoom) as i32 + 2 * BORDER);

    let screen = GetDC(null_mut());
    let bitmap = dib_from_pixels(screen, image.width, image.height, &image.data);
    let dc = CreateCompatibleDC(screen);
    ReleaseDC(null_mut(), screen);
    let old = SelectObject(dc, bitmap);
    let pin = Box::into_raw(Box::new(Pin { image, dc, bitmap, old, zoom, alpha: 255 }));

    let title = wide("RSnip pin");
    let hwnd = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
        class.as_ptr(),
        title.as_ptr(),
        WS_POPUP,
        work.left + (ww - w) / 2,
        work.top + (wh - h) / 2,
        w,
        h,
        null_mut(),
        null_mut(),
        hinstance,
        pin as *const c_void,
    );
    if hwnd.is_null() {
        drop(Box::from_raw(pin));
        return;
    }
    SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA);
    ShowWindow(hwnd, SW_SHOW);
    SetForegroundWindow(hwnd);

    let mut msg: MSG = zeroed();
    while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    let pin = Box::from_raw(pin);
    SelectObject(pin.dc, pin.old);
    DeleteObject(pin.bitmap);
    DeleteDC(pin.dc);
}

impl Pin {
    fn inner_size(&self) -> (i32, i32) {
        ((self.image.width as f32 * self.zoom).round() as i32, (self.image.height as f32 * self.zoom).round() as i32)
    }

    /// Applies a new zoom, keeping the point under the cursor in place.
    unsafe fn set_zoom(&mut self, hwnd: HWND, zoom: f32) {
        let (old_w, old_h) = self.inner_size();
        self.zoom = zoom;
        let (w, h) = self.inner_size();
        let mut r: RECT = zeroed();
        GetWindowRect(hwnd, &mut r);
        let mut c: POINT = zeroed();
        GetCursorPos(&mut c);
        let (x, y) = (anchored(r.left, c.x, old_w, w), anchored(r.top, c.y, old_h, h));
        SetWindowPos(hwnd, null_mut(), x, y, w + 2 * BORDER, h + 2 * BORDER, SWP_NOZORDER | SWP_NOACTIVATE);
        InvalidateRect(hwnd, null(), 0);
    }

    unsafe fn paint(&self, hwnd: HWND) {
        let mut ps: PAINTSTRUCT = zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut client: RECT = zeroed();
        GetClientRect(hwnd, &mut client);
        let brush = CreateSolidBrush(ACCENT);
        FillRect(hdc, &client, brush);
        DeleteObject(brush);
        let (w, h) = self.inner_size();
        SetStretchBltMode(hdc, if self.zoom == 1.0 { COLORONCOLOR } else { HALFTONE });
        SetBrushOrgEx(hdc, 0, 0, null_mut());
        StretchBlt(hdc, BORDER, BORDER, w, h, self.dc, 0, 0, self.image.width, self.image.height, SRCCOPY);
        EndPaint(hwnd, &ps);
    }

    unsafe fn command(&mut self, hwnd: HWND, cmd: usize) {
        match cmd {
            CMD_COPY => {
                let _ = crate::output::copy_to_clipboard(hwnd, &self.image);
            }
            CMD_SAVE => {
                if let Ok(path) = crate::output::next_path() {
                    let _ = crate::output::save_png(&self.image, &path);
                }
            }
            CMD_ACTUAL_SIZE => self.set_zoom(hwnd, 1.0),
            CMD_CLOSE => {
                DestroyWindow(hwnd);
            }
            _ => {}
        }
    }

    unsafe fn menu(&mut self, hwnd: HWND) {
        let menu = CreatePopupMenu();
        let item = |id: usize, text: &str, flags: MENU_ITEM_FLAGS| {
            let text = wide(text);
            AppendMenuW(menu, MF_STRING | flags, id, text.as_ptr());
        };
        item(CMD_COPY, "Copy\tCtrl+C", 0);
        item(CMD_SAVE, "Save to library", 0);
        item(CMD_ACTUAL_SIZE, "Actual size\tCtrl+0", 0);
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        item(0, "Wheel: fade · Ctrl+wheel: zoom", MF_GRAYED);
        item(CMD_CLOSE, "Close\tEsc", 0);
        let mut p: POINT = zeroed();
        GetCursorPos(&mut p);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, p.x, p.y, 0, hwnd, null());
        DestroyMenu(menu);
        self.command(hwnd, cmd as usize);
    }
}

fn ctrl_down() -> bool {
    unsafe { GetKeyState(VK_CONTROL as i32) < 0 }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(lparam as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let pin = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Pin;
    if pin.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let pin = &mut *pin;
    match msg {
        // Drag anywhere to move it.
        WM_NCHITTEST => HTCAPTION as LRESULT,
        WM_NCLBUTTONDBLCLK => {
            DestroyWindow(hwnd);
            0
        }
        WM_CONTEXTMENU => {
            pin.menu(hwnd);
            0
        }
        WM_MOUSEWHEEL => {
            let notches = ((wparam >> 16) & 0xFFFF) as u16 as i16 as f32 / WHEEL_DELTA as f32;
            if ctrl_down() {
                pin.set_zoom(hwnd, zoom_after(pin.zoom, notches));
            } else {
                pin.alpha = alpha_after(pin.alpha, notches);
                SetLayeredWindowAttributes(hwnd, 0, pin.alpha, LWA_ALPHA);
            }
            0
        }
        WM_KEYDOWN => {
            match wparam as u16 {
                VK_ESCAPE => pin.command(hwnd, CMD_CLOSE),
                k if k == b'C' as u16 && ctrl_down() => pin.command(hwnd, CMD_COPY),
                k if k == b'0' as u16 && ctrl_down() => pin.command(hwnd, CMD_ACTUAL_SIZE),
                _ => {}
            }
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            pin.paint(hwnd);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Starts a pin window for `path` as its own process, so it outlives the library and tray.
pub fn spawn(path: &Path) {
    let Ok(exe) = std::env::current_exe() else { return };
    unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    let _ = std::process::Command::new(exe).arg("--pin").arg(path).spawn();
}

/// Saves `img` to a temporary file and pins it.
pub fn pin_image(img: &Image) -> Result<(), String> {
    std::fs::create_dir_all(temp_dir()).map_err(|e| e.to_string())?;
    let path = temp_path();
    crate::output::save_png(img, &path)?;
    spawn(&path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_and_fade_stay_in_range() {
        assert!((zoom_after(1.0, 1.0) - 1.1).abs() < 1e-5);
        assert_eq!(zoom_after(3.9, 5.0), 4.0);
        assert_eq!(zoom_after(0.3, -5.0), 0.25);
        assert_eq!(alpha_after(255, -1.0), 230);
        assert_eq!(alpha_after(60, -3.0), MIN_ALPHA, "never fades out entirely");
        assert_eq!(alpha_after(250, 1.0), 255);
    }

    #[test]
    fn zooming_keeps_the_point_under_the_cursor() {
        // Cursor 102 px into a window at x=100 (100 px into the image); doubling the zoom puts
        // that image point 200 px in, so the window moves left by 100.
        assert_eq!(anchored(100, 202, 400, 800), 0);
        assert_eq!(anchored(100, 202, 400, 400), 100);
    }
}
