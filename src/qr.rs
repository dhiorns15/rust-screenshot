//! Reading QR codes in an image, for the Capture text action (a code's content is copied instead
//! of the text around it) and the editor's Copy text tool. It's pure Rust (rqrr), so it works on
//! every system RSnip runs on, whether or not there's an engine to read text, and it's quick enough
//! to try on a whole screen before reading the text.

use eframe::egui::{pos2, Rect};

/// A QR code found in an image.
#[derive(Clone, Debug, PartialEq)]
pub struct Code {
    pub text: String,
    /// Where it is, in image pixels.
    pub bounds: Rect,
}

/// The QR codes in an image `width` × `height` pixels whose pixel (x, y) has the brightness
/// `luma(x, y)` (0 black, 255 white), top to bottom and left to right.
pub fn find(width: usize, height: usize, luma: impl Fn(usize, usize) -> u8) -> Vec<Code> {
    let codes = decode(width, height, &luma);
    if !codes.is_empty() {
        return codes;
    }
    // Light on dark, as some dark pages and apps draw them.
    decode(width, height, |x, y| 255 - luma(x, y))
}

/// The QR codes in an RGBA image (see `find`).
#[cfg_attr(windows, allow(dead_code))]
pub fn find_in_rgba(img: &image::RgbaImage) -> Vec<Code> {
    let (w, raw) = (img.width() as usize, img.as_raw());
    find(w, img.height() as usize, |x, y| {
        let i = (y * w + x) * 4;
        luma(raw[i], raw[i + 1], raw[i + 2], raw[i + 3])
    })
}

/// A pixel's brightness, as it would look over white: a transparent background is white, as it
/// is in most of the places a picture with a code ends up.
pub fn luma(r: u8, g: u8, b: u8, a: u8) -> u8 {
    let y = (77 * r as u32 + 150 * g as u32 + 29 * b as u32) >> 8;
    ((y * a as u32 + 255 * (255 - a as u32)) / 255) as u8
}

fn decode(width: usize, height: usize, luma: impl Fn(usize, usize) -> u8) -> Vec<Code> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let mut img = rqrr::PreparedImage::prepare_from_greyscale(width, height, luma);
    let mut codes: Vec<Code> = img
        .detect_grids()
        .into_iter()
        .filter_map(|grid| {
            let (_, text) = grid.decode().ok()?;
            let corners = grid.bounds.map(|p| pos2(p.x as f32, p.y as f32));
            let bounds = corners.iter().fold(Rect::NOTHING, |r, &p| r.union(Rect::from_min_max(p, p)));
            (!text.is_empty()).then_some(Code { text, bounds })
        })
        .collect();
    codes.sort_by(|a, b| (a.bounds.top(), a.bounds.left()).partial_cmp(&(b.bounds.top(), b.bounds.left())).unwrap_or(std::cmp::Ordering::Equal));
    codes
}

/// `text` if it's a web link (http or https) to open in the browser, trimmed.
pub fn link(text: &str) -> Option<&str> {
    let text = text.trim();
    let scheme = ["http://", "https://"].into_iter().find(|s| text.get(..s.len()).is_some_and(|t| t.eq_ignore_ascii_case(s)))?;
    let rest = &text[scheme.len()..];
    (!rest.is_empty() && !text.chars().any(|c| c.is_whitespace() || c.is_control())).then_some(text)
}

/// The start of `text` for a notification or a toolbar: its first line, cut at `max` characters.
pub fn shorten(text: &str, max: usize) -> String {
    let first = text.trim().lines().next().unwrap_or_default();
    let mut out: String = first.chars().take(max).collect();
    if out.len() < text.trim().len() {
        out.push('…');
    }
    out
}

/// What the Capture text action found in an area.
#[derive(Debug, PartialEq)]
pub enum Found {
    /// The contents of the QR codes in it, top to bottom.
    Codes(Vec<String>),
    /// No code: the text read from it.
    Text(String),
}

/// What the Capture text action copies: the contents of the QR codes in `codes` if there are any,
/// else the text `read_text` reads (which is only asked for then: reading text is far slower).
pub fn code_or_text(codes: Vec<Code>, read_text: impl FnOnce() -> Result<String, String>) -> Result<Found, String> {
    if codes.is_empty() {
        read_text().map(Found::Text)
    } else {
        Ok(Found::Codes(codes.into_iter().map(|c| c.text).collect()))
    }
}

impl Found {
    /// What to copy, and the notification's title and text saying so; `None` if nothing was found.
    pub fn message(&self) -> Option<(String, String, String)> {
        match self {
            Found::Codes(codes) => {
                let all = codes.join("\n");
                let title = if codes.len() == 1 { "QR code copied".to_string() } else { format!("{} QR codes copied", codes.len()) };
                Some((all, title, shorten(&codes[0], 60)))
            }
            Found::Text(text) if text.trim().is_empty() => None,
            Found::Text(text) => {
                let lines = text.lines().count();
                let title = if lines == 1 { "Copied 1 line of text".to_string() } else { format!("Copied {lines} lines of text") };
                Some((text.clone(), title, text.lines().next().unwrap_or_default().chars().take(60).collect()))
            }
        }
    }

    /// The first web link among the codes, which clicking the notification opens on Windows (on
    /// Linux the notification only shows it).
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn link(&self) -> Option<&str> {
        match self {
            Found::Codes(codes) => codes.iter().find_map(|c| link(c)),
            Found::Text(_) => None,
        }
    }
}

/// A QR code of `text` drawn black on white, `scale` pixels to a module, with a quiet zone of 4
/// modules, at (`x`, `y`) on a white image `width` × `height`.
#[cfg(test)]
pub fn example(text: &str, scale: u32, at: (u32, u32), size: (u32, u32)) -> image::RgbaImage {
    let code = qrcode::QrCode::new(text.as_bytes()).unwrap();
    let modules = code.width() as u32;
    let colors = code.to_colors();
    let mut img = image::RgbaImage::from_pixel(size.0, size.1, image::Rgba([255, 255, 255, 255]));
    for my in 0..modules {
        for mx in 0..modules {
            if colors[(my * modules + mx) as usize] == qrcode::Color::Dark {
                for dy in 0..scale {
                    for dx in 0..scale {
                        let (x, y) = (at.0 + (4 + mx) * scale + dx, at.1 + (4 + my) * scale + dy);
                        img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
                    }
                }
            }
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    const URL: &str = "https://example.com/rsnip?q=1";

    #[test]
    fn finds_a_code_and_where_it_is() {
        let img = example(URL, 4, (100, 60), (400, 300));
        let codes = find_in_rgba(&img);
        assert_eq!(codes.len(), 1, "{codes:?}");
        assert_eq!(codes[0].text, URL);
        // The code itself, inside its quiet zone.
        let b = codes[0].bounds;
        assert!(b.left() >= 110.0 && b.top() >= 70.0 && b.right() <= 300.0 && b.bottom() <= 260.0, "{b:?}");
        assert!(b.width() > 50.0 && b.height() > 50.0, "{b:?}");
    }

    #[test]
    fn finds_small_and_light_on_dark_codes() {
        let small = example("small", 2, (10, 10), (120, 120));
        assert_eq!(find_in_rgba(&small).first().map(|c| c.text.as_str()), Some("small"));
        let mut dark = example("dark mode", 3, (20, 20), (200, 200));
        for p in dark.pixels_mut() {
            p.0 = [255 - p.0[0], 255 - p.0[1], 255 - p.0[2], 255];
        }
        assert_eq!(find_in_rgba(&dark).first().map(|c| c.text.as_str()), Some("dark mode"));
    }

    #[test]
    fn finds_nothing_in_a_picture_without_a_code() {
        let img = image::RgbaImage::from_fn(300, 200, |x, y| image::Rgba([(x % 7 * 30) as u8, (y % 11 * 20) as u8, 90, 255]));
        assert_eq!(find_in_rgba(&img), vec![]);
        assert_eq!(find(0, 0, |_, _| 0), vec![]);
    }

    #[test]
    fn a_transparent_background_counts_as_white() {
        let mut img = example("see through", 3, (0, 0), (150, 150));
        for p in img.pixels_mut() {
            if p.0[0] == 255 {
                p.0 = [0, 0, 0, 0];
            }
        }
        assert_eq!(find_in_rgba(&img).first().map(|c| c.text.as_str()), Some("see through"));
    }

    #[test]
    fn reads_the_text_only_when_there_is_no_code() {
        let read = Cell::new(0);
        let text = || {
            read.set(read.get() + 1);
            Ok("Hello".to_string())
        };
        let found = code_or_text(find_in_rgba(&example(URL, 3, (0, 0), (200, 200))), text).unwrap();
        assert_eq!(found, Found::Codes(vec![URL.into()]));
        assert_eq!(read.get(), 0, "no need to read the text");
        assert_eq!(found.link(), Some(URL));
        assert_eq!(found.message(), Some((URL.into(), "QR code copied".into(), URL.into())));

        let plain = image::RgbaImage::from_pixel(200, 100, image::Rgba([250, 250, 250, 255]));
        let found = code_or_text(find_in_rgba(&plain), text).unwrap();
        assert_eq!(found, Found::Text("Hello".into()));
        assert_eq!(read.get(), 1);
        assert_eq!(found.link(), None);
        assert_eq!(found.message(), Some(("Hello".into(), "Copied 1 line of text".into(), "Hello".into())));
        assert_eq!(code_or_text(vec![], || Ok("  ".into())).unwrap().message(), None, "no text found");
        assert_eq!(code_or_text(vec![], || Err("no engine".into())), Err("no engine".into()));
    }

    #[test]
    fn opens_only_web_links() {
        assert_eq!(link(" https://example.com/a?b=c "), Some("https://example.com/a?b=c"));
        assert_eq!(link("HTTP://EXAMPLE.COM"), Some("HTTP://EXAMPLE.COM"));
        for not in ["https://", "example.com", "ftp://example.com", "javascript:alert(1)", "file:///C:/Windows", "https://a b", "WIFI:S:home;T:WPA;P:secret;;", "mailto:a@b.c", "httpx://a"] {
            assert_eq!(link(not), None, "{not}");
        }
    }

    #[test]
    fn shortens_long_contents() {
        assert_eq!(shorten("short", 60), "short");
        assert_eq!(shorten("abcdef", 3), "abc…");
        assert_eq!(shorten("first\nsecond", 60), "first…");
    }
}
