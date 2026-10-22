//! Full-screen selection overlay shown over a frozen capture of the desktop.

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::atomic::Ordering::Relaxed;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::capture::{self, dib_from_pixels, Screenshot, Target};
use crate::hook::OVERLAY_HWND;
use crate::util::*;
use crate::WM_APP_OVERLAY_KEY;

const CLASS: &str = "RSnip.Overlay";
const ACCENT: COLORREF = 0x00FF9E3B; // RGB(59, 158, 255)
const PILL: COLORREF = 0x00202020;
const WHITE: COLORREF = 0x00FFFFFF;
const BLACK: COLORREF = 0x00000000;
const HINT: &str = "Drag to select an area   ·   Click a window   ·   Enter: this screen   ·   A: all screens   ·   Esc: cancel\n\
                    Arrows: nudge 1 px (Shift: 10)   ·   M: magnifier   ·   C: copy colour";
const VK_A: u16 = b'A' as u16;
const VK_C: u16 = b'C' as u16;
const VK_F: u16 = b'F' as u16;
const VK_M: u16 = b'M' as u16;
/// Source pixels across the magnifier (odd, so one sits in the middle).
const MAG_SPAN: i32 = 15;

/// A memory DC with a bitmap selected into it.
struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
}

impl Surface {
    unsafe fn new(reference: HDC, bitmap: HBITMAP) -> Surface {
        let dc = CreateCompatibleDC(reference);
        let old = SelectObject(dc, bitmap);
        Surface { dc, bitmap, old }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

struct State {
    width: i32,
    height: i32,
    scale: f32,
    bright: Surface,
    dim: Surface,
    /// Back buffer that frames are composed in before being copied to the window.
    back: Surface,
    targets: Vec<Target>,
    /// Index into `targets` of the window/monitor under the cursor.
    hover: Option<usize>,
    monitors: Vec<RECT>,
    /// Last known mouse position, used to pick the monitor for Enter.
    cursor: POINT,
    hint: Option<RECT>,
    /// Where the left button went down, while it is held.
    anchor: Option<POINT>,
    dragging: bool,
    /// The highlighted rectangle: the drag selection, or the window under the cursor.
    current: Option<RECT>,
    /// Virtual-screen position of the image's top-left, for showing coordinates.
    origin: POINT,
    magnifier: bool,
    /// Replaces the magnifier's coordinates after C copies a colour, until the mouse moves.
    note: Option<String>,
    done: bool,
    result: Option<Selection>,
}

/// What the user picked, in image coordinates.
pub enum Selection {
    Area(RECT),
    /// A clicked window. `occluded` means other windows covered part of it in the frozen capture.
    Window { hwnd: HWND, rect: RECT, occluded: bool },
}

pub unsafe fn register_class(hinstance: HINSTANCE) {
    let class = wide(CLASS);
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wndproc),
        hInstance: hinstance,
        hCursor: LoadCursorW(null_mut(), IDC_CROSS),
        lpszClassName: class.as_ptr(),
        ..zeroed()
    };
    RegisterClassExW(&wc);
}

fn dim(pixels: &[u8]) -> Vec<u8> {
    pixels.iter().map(|&c| ((c as u32 * 115) >> 8) as u8).collect()
}

/// Lets the user pick a region of `shot`. Returns it in image coordinates, or `None` if cancelled.
pub fn select(shot: &Screenshot, targets: Vec<Target>) -> Option<Selection> {
    unsafe {
        let (w, h) = (shot.image.width, shot.image.height);
        let screen = GetDC(null_mut());
        let bright = Surface::new(screen, dib_from_pixels(screen, w, h, &shot.image.data));
        let dim = Surface::new(screen, dib_from_pixels(screen, w, h, &dim(&shot.image.data)));
        let back = Surface::new(screen, CreateCompatibleBitmap(screen, w, h));
        ReleaseDC(null_mut(), screen);
        if bright.bitmap.is_null() || dim.bitmap.is_null() || back.bitmap.is_null() {
            return None;
        }

        let scale = GetDpiForSystem() as f32 / 96.0;
        let face = wide("Segoe UI");
        let font = CreateFontW(
            -(14.0 * scale).round() as i32,
            0,
            0,
            0,
            FW_SEMIBOLD as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            (DEFAULT_PITCH | FF_DONTCARE) as u32,
            face.as_ptr(),
        );
        SelectObject(back.dc, font);
        SetBkMode(back.dc, TRANSPARENT as i32);
        SetTextColor(back.dc, WHITE);

        let mut cursor: POINT = zeroed();
        GetCursorPos(&mut cursor);
        let mut monitor_info: MONITORINFO = zeroed();
        monitor_info.cbSize = size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST), &mut monitor_info);

        let cursor_in_image = POINT { x: cursor.x - shot.x, y: cursor.y - shot.y };
        let mut state = Box::new(State {
            width: w,
            height: h,
            scale,
            bright,
            dim,
            back,
            targets,
            hover: None,
            monitors: capture::monitor_rects(shot),
            cursor: cursor_in_image,
            hint: None,
            anchor: None,
            dragging: false,
            current: None,
            origin: POINT { x: shot.x, y: shot.y },
            magnifier: true,
            note: None,
            done: false,
            result: None,
        });
        state.hint = Some(state.hint_rect(&offset(&monitor_info.rcMonitor, -shot.x, -shot.y)));
        state.hover = state.target_at(cursor_in_image);
        state.current = state.hover.map(|i| state.targets[i].rect);
        let ptr = Box::into_raw(state);

        let class = wide(CLASS);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class.as_ptr(),
            null(),
            WS_POPUP,
            shot.x,
            shot.y,
            w,
            h,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            ptr as *const c_void,
        );
        if hwnd.is_null() {
            drop(Box::from_raw(ptr));
            DeleteObject(font);
            return None;
        }
        // Appear instantly instead of fading in.
        let disable: BOOL = 1;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_TRANSITIONS_FORCEDISABLED as u32,
            &disable as *const BOOL as *const c_void,
            size_of::<BOOL>() as u32,
        );
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        OVERLAY_HWND.store(hwnd as isize, Relaxed);

        let mut msg: MSG = zeroed();
        loop {
            // The window procedure sets `done` while dispatching.
            if (*ptr).done {
                break;
            }
            match GetMessageW(&mut msg, null_mut(), 0, 0) {
                0 => {
                    PostQuitMessage(msg.wParam as i32);
                    break;
                }
                -1 => break,
                _ => {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }

        OVERLAY_HWND.store(0, Relaxed);
        ShowWindow(hwnd, SW_HIDE);
        DestroyWindow(hwnd);
        let result = (*ptr).result.take();
        drop(Box::from_raw(ptr));
        DeleteObject(font);
        result
    }
}

impl State {
    fn px(&self, v: f32) -> i32 {
        (v * self.scale).round() as i32
    }

    fn border(&self) -> i32 {
        self.px(2.0).max(1)
    }

    fn target_at(&self, p: POINT) -> Option<usize> {
        self.targets.iter().position(|t| contains(&t.rect, p.x, p.y))
    }

    /// The selection made by releasing the mouse: the dragged area, or the clicked target.
    fn selection(&self) -> Option<Selection> {
        if self.dragging {
            return self.current.map(Selection::Area);
        }
        let i = self.hover?;
        let Target { rect, window } = self.targets[i];
        if window.is_null() {
            return Some(Selection::Area(rect));
        }
        // Targets are in z-order, so only windows listed earlier can be on top of this one.
        let occluded = self.targets[..i].iter().any(|t| !t.window.is_null() && !is_empty(&intersect(&t.rect, &rect)));
        Some(Selection::Window { hwnd: window, rect, occluded })
    }

    fn drag_rect(&self, a: POINT, b: POINT) -> RECT {
        rect(
            a.x.min(b.x).clamp(0, self.width - 1),
            a.y.min(b.y).clamp(0, self.height - 1),
            (a.x.max(b.x) + 1).clamp(1, self.width),
            (a.y.max(b.y) + 1).clamp(1, self.height),
        )
    }

    fn label_text(r: &RECT) -> Vec<u16> {
        format!("{} × {}", r.right - r.left, r.bottom - r.top).encode_utf16().collect()
    }

    /// Size of `text` in the overlay font, which may span several lines.
    unsafe fn text_size(&self, text: &[u16]) -> SIZE {
        let mut r = rect(0, 0, 0, 0);
        DrawTextW(self.back.dc, text.as_ptr(), text.len() as i32, &mut r, DT_CALCRECT | DT_NOPREFIX);
        SIZE { cx: r.right - r.left, cy: r.bottom - r.top }
    }

    fn zoom(&self) -> i32 {
        self.px(8.0)
    }

    /// The magnifier box and its label for the cursor at `p`.
    unsafe fn magnifier_rects(&self, p: POINT) -> (RECT, RECT) {
        let label_height = self.text_size(&wide_text("0")).cy + 2 * self.px(4.0);
        magnifier_layout(p, MAG_SPAN * self.zoom(), label_height, self.px(20.0), self.width, self.height)
    }

    /// Everything the magnifier draws for the cursor at `p`.
    unsafe fn magnifier_area(&self, p: POINT) -> RECT {
        let (zoomed, label) = self.magnifier_rects(p);
        inflate(&union(&zoomed, &label), 2)
    }

    unsafe fn invalidate_magnifier(&self, hwnd: HWND, p: POINT) {
        InvalidateRect(hwnd, &self.magnifier_area(p), 0);
    }

    /// The captured colour at `p` as `#RRGGBB`.
    unsafe fn hex_at(&self, p: POINT) -> String {
        let c = GetPixel(self.bright.dc, p.x, p.y);
        format!("#{:02X}{:02X}{:02X}", c & 0xFF, (c >> 8) & 0xFF, (c >> 16) & 0xFF)
    }

    /// A zoomed view of the pixels around the cursor, for placing selection edges exactly.
    unsafe fn draw_magnifier(&self) {
        let (p, dc, z) = (self.cursor, self.back.dc, self.zoom());
        let (zoomed, label) = self.magnifier_rects(p);
        let half = MAG_SPAN / 2;
        SetStretchBltMode(dc, COLORONCOLOR);
        StretchBlt(dc, zoomed.left, zoomed.top, MAG_SPAN * z, MAG_SPAN * z, self.bright.dc, p.x - half, p.y - half, MAG_SPAN, MAG_SPAN, SRCCOPY);
        let centre = rect(zoomed.left + half * z, zoomed.top + half * z, zoomed.left + (half + 1) * z, zoomed.top + (half + 1) * z);
        frame(dc, &inflate(&centre, 1), BLACK);
        frame(dc, &centre, WHITE);
        frame(dc, &inflate(&zoomed, 1), ACCENT);
        let text = match &self.note {
            Some(note) => note.clone(),
            None => format!("{}, {}   {}", p.x + self.origin.x, p.y + self.origin.y, self.hex_at(p)),
        };
        self.draw_pill(&label, &wide_text(&text), self.px(4.0));
    }

    unsafe fn hint_rect(&self, monitor: &RECT) -> RECT {
        let text: Vec<u16> = HINT.encode_utf16().collect();
        let size = self.text_size(&text);
        let (w, h) = (size.cx + 2 * self.px(16.0), size.cy + 2 * self.px(10.0));
        let x = monitor.left + (monitor.right - monitor.left - w) / 2;
        let y = monitor.top + self.px(24.0);
        rect(x, y, x + w, y + h)
    }

    /// The size label sits below the selection, or above/inside it when there is no room.
    unsafe fn label_rect(&self, r: &RECT) -> RECT {
        let size = self.text_size(&Self::label_text(r));
        let (w, h) = (size.cx + 2 * self.px(8.0), size.cy + 2 * self.px(4.0));
        let gap = self.px(6.0);
        let mut y = r.bottom + gap;
        if y + h > self.height {
            y = r.top - gap - h;
        }
        if y < 0 {
            y = r.top + gap;
        }
        let x = r.left.clamp(0, (self.width - w).max(0));
        rect(x, y, x + w, y + h)
    }

    /// Everything drawn for a highlighted rectangle: frame plus size label.
    unsafe fn decoration_rect(&self, r: &RECT) -> RECT {
        union(&inflate(r, self.border() + 1), &self.label_rect(r))
    }

    unsafe fn set_current(&mut self, hwnd: HWND, new: Option<RECT>) {
        let unchanged = match (&self.current, &new) {
            (Some(a), Some(b)) => same(a, b),
            (None, None) => true,
            _ => false,
        };
        if unchanged {
            return;
        }
        for r in self.current.iter().chain(new.iter()) {
            InvalidateRect(hwnd, &self.decoration_rect(r), 0);
        }
        self.current = new;
    }

    unsafe fn on_mouse_move(&mut self, hwnd: HWND, p: POINT) {
        if self.magnifier {
            self.invalidate_magnifier(hwnd, self.cursor);
            self.invalidate_magnifier(hwnd, p);
        }
        self.note = None;
        self.cursor = p;
        let new = match self.anchor {
            Some(a) => {
                let threshold = self.px(3.0);
                if (p.x - a.x).abs() >= threshold || (p.y - a.y).abs() >= threshold {
                    self.dragging = true;
                }
                if self.dragging {
                    Some(self.drag_rect(a, p))
                } else {
                    self.current
                }
            }
            None => {
                self.hover = self.target_at(p);
                self.hover.map(|i| self.targets[i].rect)
            }
        };
        self.set_current(hwnd, new);
    }

    unsafe fn hide_hint(&mut self, hwnd: HWND) {
        if let Some(r) = self.hint.take() {
            InvalidateRect(hwnd, &r, 0);
        }
    }

    fn finish(&mut self, result: Option<Selection>) {
        self.result = result;
        self.done = true;
    }

    unsafe fn on_key(&mut self, hwnd: HWND, vk: u16) {
        match vk {
            // Moving the mouse nudges whatever follows it: the hover point or the edge being dragged.
            VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN => {
                let step = if GetKeyState(VK_SHIFT as i32) < 0 { 10 } else { 1 };
                let (dx, dy) = match vk {
                    VK_LEFT => (-step, 0),
                    VK_RIGHT => (step, 0),
                    VK_UP => (0, -step),
                    _ => (0, step),
                };
                let mut p: POINT = zeroed();
                GetCursorPos(&mut p);
                SetCursorPos(p.x + dx, p.y + dy);
            }
            VK_M => {
                self.magnifier = !self.magnifier;
                self.invalidate_magnifier(hwnd, self.cursor);
            }
            VK_C => {
                let hex = self.hex_at(self.cursor);
                self.note = Some(match crate::output::copy_text_to_clipboard(hwnd, &hex) {
                    Ok(()) => format!("Copied {hex}"),
                    Err(e) => e,
                });
                self.invalidate_magnifier(hwnd, self.cursor);
            }
            VK_ESCAPE => self.finish(None),
            VK_RETURN | VK_F => {
                let all = rect(0, 0, self.width, self.height);
                let (x, y) = (self.cursor.x, self.cursor.y);
                let screen = self.monitors.iter().copied().find(|m| contains(m, x, y)).unwrap_or(all);
                self.finish(Some(Selection::Area(screen)));
            }
            VK_A => self.finish(Some(Selection::Area(rect(0, 0, self.width, self.height)))),
            _ => {}
        }
    }

    unsafe fn paint(&self, hwnd: HWND) {
        let mut ps: PAINTSTRUCT = zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let area = ps.rcPaint;
        blit(self.back.dc, &area, self.dim.dc);
        if let Some(r) = self.current {
            let visible = intersect(&r, &area);
            if !is_empty(&visible) {
                blit(self.back.dc, &visible, self.bright.dc);
            }
            self.draw_frame(&r);
            self.draw_pill(&self.label_rect(&r), &Self::label_text(&r), self.px(4.0));
        }
        if let Some(h) = self.hint {
            self.draw_pill(&h, &wide_text(HINT), self.px(8.0));
        }
        if self.magnifier {
            self.draw_magnifier();
        }
        blit(hdc, &area, self.back.dc);
        EndPaint(hwnd, &ps);
    }

    /// Draws the border just outside `r` so it never covers captured pixels.
    unsafe fn draw_frame(&self, r: &RECT) {
        let b = self.border();
        let brush = CreateSolidBrush(ACCENT);
        let strips = [
            rect(r.left - b, r.top - b, r.right + b, r.top),
            rect(r.left - b, r.bottom, r.right + b, r.bottom + b),
            rect(r.left - b, r.top, r.left, r.bottom),
            rect(r.right, r.top, r.right + b, r.bottom),
        ];
        for s in &strips {
            FillRect(self.back.dc, s, brush);
        }
        DeleteObject(brush);
    }

    unsafe fn draw_pill(&self, r: &RECT, text: &[u16], radius: i32) {
        let dc = self.back.dc;
        let brush = CreateSolidBrush(PILL);
        let old_brush = SelectObject(dc, brush);
        let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
        RoundRect(dc, r.left, r.top, r.right + 1, r.bottom + 1, radius * 2, radius * 2);
        SelectObject(dc, old_pen);
        SelectObject(dc, old_brush);
        DeleteObject(brush);
        // DT_VCENTER only works for single lines, so centre by hand.
        let height = self.text_size(text).cy;
        let top = r.top + (r.bottom - r.top - height) / 2;
        let mut tr = rect(r.left, top, r.right, top + height);
        DrawTextW(dc, text.as_ptr(), text.len() as i32, &mut tr, DT_CENTER | DT_NOPREFIX);
    }
}

/// UTF-16 without a terminator, for the GDI text calls that take a length.
fn wide_text(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

unsafe fn frame(dc: HDC, r: &RECT, color: COLORREF) {
    let brush = CreateSolidBrush(color);
    FrameRect(dc, r, brush);
    DeleteObject(brush);
}

/// Places a `size`-pixel magnifier and its label `gap` pixels below-right of `p`, flipping to the
/// other side of the cursor where it would leave the `width` × `height` image.
fn magnifier_layout(p: POINT, size: i32, label_height: i32, gap: i32, width: i32, height: i32) -> (RECT, RECT) {
    let total = size + gap / 4 + label_height;
    let x = if p.x + gap + size <= width { p.x + gap } else { p.x - gap - size };
    let y = if p.y + gap + total <= height { p.y + gap } else { p.y - gap - total };
    let (x, y) = (x.max(0), y.max(0));
    let label_top = y + size + gap / 4;
    (rect(x, y, x + size, y + size), rect(x, label_top, x + size, label_top + label_height))
}

unsafe fn blit(dst: HDC, r: &RECT, src: HDC) {
    BitBlt(dst, r.left, r.top, r.right - r.left, r.bottom - r.top, src, r.left, r.top, SRCCOPY);
}

fn point(lparam: LPARAM) -> POINT {
    POINT { x: (lparam & 0xFFFF) as i16 as i32, y: ((lparam >> 16) & 0xFFFF) as i16 as i32 }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(lparam as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let st = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if st.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let st = &mut *st;
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            st.paint(hwnd);
            0
        }
        WM_MOUSEMOVE => {
            st.on_mouse_move(hwnd, point(lparam));
            0
        }
        WM_LBUTTONDOWN => {
            st.anchor = Some(point(lparam));
            st.dragging = false;
            st.hide_hint(hwnd);
            SetCapture(hwnd);
            0
        }
        WM_LBUTTONUP => {
            if st.anchor.take().is_some() {
                let selected = st.selection();
                st.finish(selected);
            }
            ReleaseCapture();
            0
        }
        WM_RBUTTONDOWN => {
            st.finish(None);
            0
        }
        WM_KEYDOWN | WM_APP_OVERLAY_KEY => {
            st.on_key(hwnd, wparam as u16);
            0
        }
        WM_CLOSE => {
            st.finish(None);
            0
        }
        // The overlay spans monitors; never let Windows rescale it.
        WM_DPICHANGED => 0,
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magnifier_flips_away_from_edges() {
        let (zoomed, label) = magnifier_layout(POINT { x: 100, y: 100 }, 120, 24, 20, 1920, 1080);
        assert_eq!((zoomed.left, zoomed.top), (120, 120));
        assert!(label.top > zoomed.bottom);
        let (zoomed, label) = magnifier_layout(POINT { x: 1900, y: 1070 }, 120, 24, 20, 1920, 1080);
        assert_eq!(zoomed.right, 1880, "left of the cursor");
        assert_eq!(label.bottom, 1050, "label ends above the cursor");
    }
}
