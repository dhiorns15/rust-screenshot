//! Scrolling capture on Linux, on an X11 desktop (`rsnip --scroll`), as on Windows (see
//! src/scroll.rs): the mouse wheel scrolls the part that was picked, through the X server's XTEST
//! extension, and its frames are stitched into one tall image (see `stitch`). Esc stops it early.
//! On Wayland an app can't scroll another's windows, so it isn't offered there.

use std::time::Duration;

use image::RgbaImage;
use x11rb::connection::{Connection as _, RequestConnection as _};
use x11rb::protocol::xproto::{ConnectionExt as _, GrabMode, Keycode, ModMask, Window, BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT};
use x11rb::protocol::xtest::{self, ConnectionExt as _};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

use super::recorder::Area;
use super::screenshot::read_x_area;
use crate::stitch::{Frame, Step, Stitcher};

const MAX_FRAMES: usize = 60;
const MAX_HEIGHT: usize = 30_000;
const MAX_NOTCHES: u32 = 8;

/// How long the window has to scroll and repaint before the next frame is read.
const SETTLE: Duration = Duration::from_millis(160);
/// How long the picker's window has to go, and the desktop to repaint what was under it, before
/// the first frame.
const FIRST_SETTLE: Duration = Duration::from_millis(500);

/// The mouse wheel's "down" button.
const WHEEL_DOWN: u8 = 5;
/// Esc's keysym (`XK_Escape`).
const ESCAPE: u32 = 0xff1b;

/// What is scrolled and read: the screen, or a stand-in in tests.
trait Page {
    /// Turns the mouse wheel down this many notches, and waits for it to repaint.
    fn scroll(&mut self, notches: u32);
    /// What the area shows now.
    fn grab(&mut self) -> Result<Frame, String>;
    /// Esc was pressed.
    fn stopped(&mut self) -> bool;
}

/// Scrolls the X11 desktop's `area` (in screen pixels) to the end, capturing as it goes.
pub fn capture(area: Area) -> Result<RgbaImage, String> {
    let mut screen = Screen::open(area).map_err(|e| format!("Couldn't scroll the window: {e}"))?;
    let out = scroll_through(&mut screen).map_err(|e| format!("Couldn't read the screen: {e}"))?;
    drop(screen);
    RgbaImage::from_raw(out.width as u32, out.height as u32, out.data).ok_or_else(|| "Couldn't stitch the frames together".into())
}

/// Scrolls `page` down until it stops moving (or `MAX_FRAMES`, `MAX_HEIGHT` or Esc), stitching what
/// it shows, as src/scroll.rs does on Windows.
fn scroll_through(page: &mut impl Page) -> Result<Frame, String> {
    let first = page.grab()?;
    let height = first.height;
    let mut stitcher = Stitcher::new(&first);
    let mut notches = 1;
    for _ in 0..MAX_FRAMES {
        if page.stopped() {
            break;
        }
        page.scroll(notches);
        let Ok(frame) = page.grab() else { break };
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
    Ok(stitcher.finish())
}

/// The X server's screen, with the pointer over the area (the wheel scrolls whatever is under it)
/// and Esc taken, until dropped: then the pointer goes back and Esc is let go.
struct Screen {
    conn: RustConnection,
    root: Window,
    area: Area,
    /// Esc's key, while it's taken here; `None` if another app has it.
    escape: Option<Keycode>,
    /// Where the pointer was.
    pointer: (i16, i16),
}

impl Screen {
    fn open(area: Area) -> Result<Screen, String> {
        let (conn, number) = x11rb::connect(None).map_err(|e| e.to_string())?;
        let root = conn.setup().roots[number].root;
        if conn.extension_information(xtest::X11_EXTENSION_NAME).map_err(|e| e.to_string())?.is_none() {
            return Err("the X server has no XTEST extension".into());
        }
        let was = conn.query_pointer(root).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?;
        let escape = escape_key(&conn).filter(|&key| conn.grab_key(false, root, ModMask::ANY, key, GrabMode::ASYNC, GrabMode::ASYNC).is_ok_and(|c| c.check().is_ok()));
        let screen = Screen { conn, root, area, escape, pointer: (was.root_x, was.root_y) };
        let centre = |start: u32, length: u32| (start + length / 2) as i16;
        screen.conn.warp_pointer(x11rb::NONE, root, 0, 0, 0, 0, centre(area.x, area.width), centre(area.y, area.height)).map_err(|e| e.to_string())?;
        screen.conn.flush().map_err(|e| e.to_string())?;
        std::thread::sleep(FIRST_SETTLE);
        Ok(screen)
    }
}

impl Page for Screen {
    fn scroll(&mut self, notches: u32) {
        for _ in 0..notches {
            for kind in [BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT] {
                let _ = self.conn.xtest_fake_input(kind, WHEEL_DOWN, x11rb::CURRENT_TIME, self.root, 0, 0, 0);
            }
        }
        let _ = self.conn.flush();
        std::thread::sleep(SETTLE);
    }

    fn grab(&mut self) -> Result<Frame, String> {
        let Area { x, y, width, height } = self.area;
        let image = read_x_area(&self.conn, self.root, (x as i16, y as i16), (width as u16, height as u16))?;
        Ok(Frame { width: width as usize, height: height as usize, data: image.into_raw() })
    }

    fn stopped(&mut self) -> bool {
        let mut pressed = false;
        while let Ok(Some(event)) = self.conn.poll_for_event() {
            pressed |= matches!(event, Event::KeyPress(key) if Some(key.detail) == self.escape);
        }
        pressed
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        if let Some(key) = self.escape {
            let _ = self.conn.ungrab_key(key, self.root, ModMask::ANY);
        }
        let _ = self.conn.warp_pointer(x11rb::NONE, self.root, 0, 0, 0, 0, self.pointer.0, self.pointer.1);
        // Done by the time the capture is delivered, not just sent.
        let _ = self.conn.get_input_focus().map(|c| c.reply());
    }
}

/// The key that types Esc on this keyboard.
pub(super) fn escape_key(conn: &impl x11rb::connection::Connection) -> Option<Keycode> {
    let (min, max) = (conn.setup().min_keycode, conn.setup().max_keycode);
    let map = conn.get_keyboard_mapping(min, max - min + 1).ok()?.reply().ok()?;
    let index = map.keysyms.chunks(usize::from(map.keysyms_per_keycode.max(1))).position(|keysyms| keysyms.contains(&ESCAPE))?;
    Some(min + index as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tall page in a window `view` pixels high, which each notch of the wheel scrolls by `notch`
    /// pixels until its end, as a browser would.
    struct FakePage {
        width: usize,
        view: usize,
        /// Every row of the page, each one different.
        rows: Vec<Vec<u8>>,
        notch: usize,
        offset: usize,
        /// How many notches each scroll was.
        scrolls: Vec<u32>,
        /// Esc is pressed after this many scrolls.
        esc_after: Option<usize>,
        /// The frame after this many scrolls can't be read.
        unreadable_after: Option<usize>,
    }

    impl FakePage {
        fn new(height: usize, view: usize, notch: usize) -> FakePage {
            let width = 6;
            let rows = (0..height).map(|y| (0..width).flat_map(|x| [y as u8, (y >> 8) as u8, (y >> 16) as u8 ^ x as u8, 255]).collect()).collect();
            FakePage { width, view, rows, notch, offset: 0, scrolls: Vec::new(), esc_after: None, unreadable_after: None }
        }

        /// All of the page from the top, `height` rows of it.
        fn top(&self, height: usize) -> Vec<u8> {
            self.rows[..height].concat()
        }
    }

    impl Page for FakePage {
        fn scroll(&mut self, notches: u32) {
            self.scrolls.push(notches);
            self.offset = (self.offset + notches as usize * self.notch).min(self.rows.len() - self.view);
        }

        fn grab(&mut self) -> Result<Frame, String> {
            if self.unreadable_after == Some(self.scrolls.len()) {
                return Err("gone".into());
            }
            Ok(Frame { width: self.width, height: self.view, data: self.rows[self.offset..self.offset + self.view].concat() })
        }

        fn stopped(&mut self) -> bool {
            self.esc_after.is_some_and(|n| self.scrolls.len() >= n)
        }
    }

    #[test]
    fn scrolls_to_the_end_and_stitches_the_whole_page() {
        let mut page = FakePage::new(1000, 200, 40);
        let out = scroll_through(&mut page).unwrap();
        assert_eq!((out.width, out.height), (6, 1000));
        assert!(out.data == page.top(1000), "the page, each row once");
        // A notch shows a fifth of the window, so it scrolls two at a time from then on (three after
        // the short step at the bottom); the last scroll shows nothing new, which is the end.
        assert_eq!(page.scrolls, [1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3]);
    }

    #[test]
    fn scrolls_further_each_time_while_little_comes_into_view() {
        let mut page = FakePage::new(3000, 400, 3);
        scroll_through(&mut page).unwrap();
        assert_eq!(page.scrolls[..10], [1, 2, 3, 4, 5, 6, 7, 8, 8, 8], "up to 8 notches");
        assert!(page.scrolls.iter().all(|&n| n <= MAX_NOTCHES));
    }

    #[test]
    fn stops_after_so_many_frames_or_so_tall() {
        // A notch shows a quarter of the window: it stays one at a time.
        let mut page = FakePage::new(20_000, 400, 100);
        let out = scroll_through(&mut page).unwrap();
        assert_eq!(page.scrolls.len(), MAX_FRAMES);
        assert_eq!(out.height, 400 + MAX_FRAMES * 100);
        assert!(out.data == page.top(out.height));

        let mut page = FakePage::new(40_000, 1200, 280);
        let out = scroll_through(&mut page).unwrap();
        assert!(page.scrolls.len() < MAX_FRAMES, "{} scrolls", page.scrolls.len());
        assert!(out.height > MAX_HEIGHT && out.height < MAX_HEIGHT + 1200, "{} pixels high", out.height);
        assert!(out.data == page.top(out.height));
    }

    #[test]
    fn esc_stops_it_with_what_it_has() {
        let mut page = FakePage::new(1000, 200, 40);
        page.esc_after = Some(3);
        let out = scroll_through(&mut page).unwrap();
        assert_eq!(page.scrolls, [1, 2, 2]);
        assert_eq!(out.height, 200 + 5 * 40);
        assert!(out.data == page.top(out.height));
    }

    #[test]
    fn a_frame_that_cant_be_read_or_lined_up_ends_it() {
        let mut page = FakePage::new(1000, 200, 40);
        page.unreadable_after = Some(2);
        assert_eq!(scroll_through(&mut page).unwrap().height, 200 + 40, "what came before");

        // Scrolled further than the window is high: nothing to line it up by.
        let mut page = FakePage::new(1000, 200, 250);
        let out = scroll_through(&mut page).unwrap();
        assert_eq!(page.scrolls, [1]);
        assert!(out.data == page.top(200), "the first frame");

        let mut page = FakePage::new(1000, 200, 40);
        page.unreadable_after = Some(0);
        assert_eq!(scroll_through(&mut page).err().as_deref(), Some("gone"), "nothing to start from");
    }
}
