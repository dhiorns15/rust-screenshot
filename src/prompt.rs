//! Small "press the new shortcut" window shown while recording a hotkey.

use std::cell::RefCell;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::util::wide;
use crate::WM_APP_HOTKEY_RECORDED;

const CLASS: &str = "RSnip.Prompt";

thread_local! {
    static TEXT: RefCell<Vec<u16>> = RefCell::new(Vec::new());
    static WINDOW: RefCell<HWND> = RefCell::new(null_mut());
}

pub unsafe fn register_class(hinstance: HINSTANCE) {
    let class = wide(CLASS);
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wndproc),
        hInstance: hinstance,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        hbrBackground: CreateSolidBrush(0x00202020),
        lpszClassName: class.as_ptr(),
        ..zeroed()
    };
    RegisterClassExW(&wc);
}

pub unsafe fn show(owner: HWND, current: &str) {
    close();
    let text = format!(
        "Press the new screenshot shortcut\n\nCurrent: {current}\nEsc or click here to cancel"
    );
    TEXT.with(|t| *t.borrow_mut() = text.encode_utf16().collect());

    let scale = GetDpiForSystem() as f32 / 96.0;
    let (w, h) = ((380.0 * scale) as i32, (150.0 * scale) as i32);
    let mut cursor: POINT = zeroed();
    GetCursorPos(&mut cursor);
    let mut info: MONITORINFO = zeroed();
    info.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST), &mut info);
    let work = info.rcWork;
    let x = work.left + (work.right - work.left - w) / 2;
    let y = work.top + (work.bottom - work.top - h) / 2;

    let class = wide(CLASS);
    let hwnd = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        class.as_ptr(),
        null(),
        WS_POPUP | WS_BORDER,
        x,
        y,
        w,
        h,
        owner,
        null_mut(),
        GetModuleHandleW(null()),
        null(),
    );
    WINDOW.with(|win| *win.borrow_mut() = hwnd);
    ShowWindow(hwnd, SW_SHOW);
    SetForegroundWindow(hwnd);
}

pub unsafe fn close() {
    let hwnd = WINDOW.with(|w| std::mem::replace(&mut *w.borrow_mut(), null_mut()));
    if !hwnd.is_null() {
        DestroyWindow(hwnd);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            let scale = GetDpiForSystem() as f32 / 96.0;
            let face = wide("Segoe UI");
            let font = CreateFontW(
                -(15.0 * scale) as i32,
                0,
                0,
                0,
                FW_NORMAL as i32,
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
            let old = SelectObject(hdc, font);
            SetBkMode(hdc, TRANSPARENT as i32);
            SetTextColor(hdc, 0x00FFFFFF);
            let mut r: RECT = zeroed();
            GetClientRect(hwnd, &mut r);
            TEXT.with(|t| {
                let t = t.borrow();
                // Vertically centre the block by measuring it first.
                let mut measure = r;
                DrawTextW(hdc, t.as_ptr(), t.len() as i32, &mut measure, DT_CENTER | DT_CALCRECT | DT_NOPREFIX);
                r.top += (r.bottom - r.top - (measure.bottom - measure.top)) / 2;
                DrawTextW(hdc, t.as_ptr(), t.len() as i32, &mut r, DT_CENTER | DT_NOPREFIX);
            });
            SelectObject(hdc, old);
            DeleteObject(font);
            EndPaint(hwnd, &ps);
            0
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
            let owner = GetWindow(hwnd, GW_OWNER);
            PostMessageW(owner, WM_APP_HOTKEY_RECORDED, 0, 0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
