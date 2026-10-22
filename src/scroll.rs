//! Scrolling capture: scrolls a window with the mouse wheel and stitches the frames together, for
//! pages and documents longer than the screen. Esc stops it early.

use std::mem::{size_of, zeroed};
use std::time::Duration;

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::DwmFlush;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::capture::{capture_rect, pump_messages_for, Image};
use crate::stitch::{Step, Stitcher};

const MAX_FRAMES: usize = 60;
const MAX_HEIGHT: usize = 30_000;
const MAX_NOTCHES: i32 = 8;

/// Scrolls the window at `area` (screen coordinates) to the end, capturing as it goes.
pub fn capture(area: RECT, window: Option<HWND>) -> Option<Image> {
    unsafe {
        let centre = POINT { x: (area.left + area.right) / 2, y: (area.top + area.bottom) / 2 };
        let window = window.unwrap_or_else(|| GetAncestor(WindowFromPoint(centre), GA_ROOT));
        if !window.is_null() {
            SetForegroundWindow(window);
        }
        let mut saved: POINT = zeroed();
        GetCursorPos(&mut saved);
        // The wheel scrolls whatever is under the mouse.
        SetCursorPos(centre.x, centre.y);
        settle();
        // Clear any earlier Esc press, so only one during the capture stops it.
        GetAsyncKeyState(VK_ESCAPE as i32);

        let first = capture_rect(area)?;
        let height = first.height as usize;
        let mut stitcher = Stitcher::new(&first);
        let mut notches = 1;
        for _ in 0..MAX_FRAMES {
            if GetAsyncKeyState(VK_ESCAPE as i32) != 0 {
                break;
            }
            wheel(-notches);
            settle();
            let Some(frame) = capture_rect(area) else { break };
            match stitcher.push(&frame) {
                // Scroll further per step while each step reveals little, keeping plenty of overlap.
                Step::Added(d) => {
                    if d * 4 < height && notches < MAX_NOTCHES {
                        notches += 1;
                    }
                }
                Step::End | Step::Lost => break,
            }
            if stitcher.height() > MAX_HEIGHT {
                break;
            }
        }
        SetCursorPos(saved.x, saved.y);
        Some(stitcher.finish())
    }
}

/// Lets the window scroll and repaint before the next capture.
unsafe fn settle() {
    pump_messages_for(Duration::from_millis(160));
    DwmFlush();
}

unsafe fn wheel(notches: i32) {
    let mut input: INPUT = zeroed();
    input.r#type = INPUT_MOUSE;
    input.Anonymous.mi.dwFlags = MOUSEEVENTF_WHEEL;
    input.Anonymous.mi.mouseData = (notches * WHEEL_DELTA as i32) as _;
    SendInput(1, &input, size_of::<INPUT>() as i32);
}
