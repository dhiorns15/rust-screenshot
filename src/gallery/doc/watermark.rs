//! A watermark in a corner of exported images: a line of text, or a picture (a logo, say).

use std::path::PathBuf;

use ab_glyph::FontVec;
use eframe::egui::Pos2;
use image::RgbaImage;

use super::{glyph_coverage, text_size};

/// What's put on the image, where, and how strongly.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Watermark {
    pub mark: Mark,
    pub corner: Corner,
    /// 0 (invisible) to 100 (solid).
    pub opacity: u32,
    /// A picture's width as a percentage of the image's width; text's size as a percentage of the
    /// image's height.
    pub scale: u32,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Mark {
    Text(String),
    /// A PNG, read each time it's put on an image so a changed file is used at once. One that's
    /// missing or can't be read puts nothing on it.
    Image(PathBuf),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
}

impl Corner {
    pub const ALL: [Corner; 4] = [Corner::TopLeft, Corner::TopRight, Corner::BottomLeft, Corner::BottomRight];

    pub fn name(self) -> &'static str {
        match self {
            Corner::TopLeft => "Top left",
            Corner::TopRight => "Top right",
            Corner::BottomLeft => "Bottom left",
            Corner::BottomRight => "Bottom right",
        }
    }
}

impl Mark {
    /// The size it starts at: a logo a fifth of the width, text a twentieth of the height.
    pub fn default_scale(&self) -> u32 {
        match self {
            Mark::Text(_) => 5,
            Mark::Image(_) => 20,
        }
    }
}

/// How dark the outline round text is, so white text reads on light backgrounds too.
const OUTLINE: f32 = 0.8;

/// The PNG at `path`, if it can be read.
pub fn read_picture(path: &std::path::Path) -> Result<RgbaImage, String> {
    let reader = image::ImageReader::open(path).map_err(|e| e.to_string())?;
    let reader = reader.with_guessed_format().map_err(|e| e.to_string())?;
    if reader.format() != Some(image::ImageFormat::Png) {
        return Err("It isn't a PNG".into());
    }
    reader.decode().map(|img| img.to_rgba8()).map_err(|e| e.to_string())
}

/// Puts `wm` on `img`, in its corner. Text needs `font`; without one, or with a picture that can't
/// be read, the image is left as it is.
pub fn apply(img: &mut RgbaImage, wm: &Watermark, font: Option<&FontVec>) {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 || wm.opacity == 0 {
        return;
    }
    let margin = ((w.min(h) as f32 * 0.02).round() as u32).max(4);
    let scale = wm.scale.clamp(1, 100) as f32 / 100.0;
    let layer = match &wm.mark {
        Mark::Text(text) => {
            let Some(font) = font.filter(|_| !text.trim().is_empty()) else { return };
            text_layer(font, text.trim(), h as f32 * scale, w.saturating_sub(2 * margin).max(1) as f32)
        }
        Mark::Image(path) => {
            let Ok(picture) = read_picture(path) else { return };
            let width = ((w as f32 * scale).round() as u32).max(1);
            let height = ((picture.height() as f32 * width as f32 / picture.width().max(1) as f32).round() as u32).max(1);
            image::imageops::resize(&picture, width, height, image::imageops::FilterType::CatmullRom)
        }
    };
    let (lw, lh) = (layer.width() as i64, layer.height() as i64);
    let (w, h, m) = (w as i64, h as i64, margin as i64);
    let (x, y) = match wm.corner {
        Corner::TopLeft => (m, m),
        Corner::TopRight => (w - lw - m, m),
        Corner::BottomLeft => (m, h - lh - m),
        Corner::BottomRight => (w - lw - m, h - lh - m),
    };
    blend(img, &layer, x, y, wm.opacity.min(100) as f32 / 100.0);
}

/// `text` in white with a dark outline, `size` pixels high but no wider than `max_width`, on
/// transparency.
fn text_layer(font: &FontVec, text: &str, size: f32, max_width: f32) -> RgbaImage {
    // The outline's width, and the extent of the text with it.
    let laid_out = |size: f32| {
        let extent = text_size(font, text, size);
        let r = ((size * 0.06).round() as usize).max(1);
        (r, extent.x.ceil() as usize + 2 * r + 2, extent.y.ceil() as usize + 2 * r + 2)
    };
    let mut size = size.max(4.0);
    for _ in 0..3 {
        let (_, lw, _) = laid_out(size);
        if lw as f32 <= max_width || size <= 4.0 {
            break;
        }
        size = (size * max_width / lw as f32).max(4.0);
    }
    let (r, lw, lh) = laid_out(size);
    let mut coverage = vec![0f32; lw * lh];
    glyph_coverage(font, Pos2::new(r as f32 + 1.0, r as f32 + 1.0), text, size, |x, y, c| {
        if x >= 0 && y >= 0 && (x as usize) < lw && (y as usize) < lh {
            let i = y as usize * lw + x as usize;
            coverage[i] = coverage[i].max(c);
        }
    });
    // The outline: the text grown by `r` all round (the most coverage within `r`, across then down).
    let mut across = vec![0f32; lw * lh];
    for y in 0..lh {
        for x in 0..lw {
            let row = &coverage[y * lw..(y + 1) * lw];
            across[y * lw + x] = row[x.saturating_sub(r)..(x + r + 1).min(lw)].iter().fold(0.0, |a, &b| a.max(b));
        }
    }
    RgbaImage::from_fn(lw as u32, lh as u32, |x, y| {
        let (x, y) = (x as usize, y as usize);
        let outline = (y.saturating_sub(r)..(y + r + 1).min(lh)).map(|y| across[y * lw + x]).fold(0.0, f32::max) * OUTLINE;
        let c = coverage[y * lw + x];
        // White text over its dark outline.
        let a = c + outline * (1.0 - c);
        let white = if a > 0.0 { 255.0 * c / a } else { 0.0 };
        image::Rgba([white.round() as u8, white.round() as u8, white.round() as u8, (a * 255.0).round() as u8])
    })
}

/// `layer` drawn over `img` with its top left at (`x`, `y`), `opacity` times as strongly.
fn blend(img: &mut RgbaImage, layer: &RgbaImage, x: i64, y: i64, opacity: f32) {
    let (w, h) = (img.width() as i64, img.height() as i64);
    for (lx, ly, src) in layer.enumerate_pixels() {
        let (px, py) = (x + lx as i64, y + ly as i64);
        if px < 0 || py < 0 || px >= w || py >= h {
            continue;
        }
        let a = src[3] as f32 / 255.0 * opacity;
        if a <= 0.0 {
            continue;
        }
        let dst = img.get_pixel_mut(px as u32, py as u32);
        let da = dst[3] as f32 / 255.0;
        let out_a = a + da * (1.0 - a);
        for c in 0..3 {
            dst[c] = ((src[c] as f32 * a + dst[c] as f32 * da * (1.0 - a)) / out_a).round() as u8;
        }
        dst[3] = (out_a * 255.0).round() as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::super::{apply_effects, bundled_font_bytes, Effects, BORDER_WIDTH, SHADOW_PAD};
    use super::*;

    fn font() -> FontVec {
        FontVec::try_from_vec(bundled_font_bytes().expect("eframe bundles a font")).unwrap()
    }

    fn flat(w: u32, h: u32, rgb: [u8; 3]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba([rgb[0], rgb[1], rgb[2], 255]))
    }

    fn text(text: &str, corner: Corner) -> Watermark {
        Watermark { mark: Mark::Text(text.into()), corner, opacity: 100, scale: 10 }
    }

    fn with(wm: &Watermark, img: RgbaImage, font: Option<&FontVec>) -> RgbaImage {
        apply_effects(img, &Effects { watermark: Some(wm.clone()), ..Effects::default() }, font)
    }

    /// The smallest box (x0, y0, x1, y1, exclusive) holding every pixel that differs.
    fn changed(before: &RgbaImage, after: &RgbaImage) -> Option<(u32, u32, u32, u32)> {
        let mut bounds: Option<(u32, u32, u32, u32)> = None;
        for (x, y, p) in after.enumerate_pixels() {
            if p != before.get_pixel(x, y) {
                let b = bounds.get_or_insert((x, y, x + 1, y + 1));
                *b = (b.0.min(x), b.1.min(y), b.2.max(x + 1), b.3.max(y + 1));
            }
        }
        bounds
    }

    /// A PNG picture in a folder of its own.
    fn picture(name: &str, img: &RgbaImage) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rsnip-watermark-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("logo.png");
        img.save(&path).unwrap();
        path
    }

    #[test]
    fn text_goes_in_its_corner_and_nowhere_else() {
        let font = font();
        let (w, h) = (400, 300);
        let base = flat(w, h, [128, 128, 128]);
        for corner in Corner::ALL {
            let out = with(&text("RSnip", corner), base.clone(), Some(&font));
            assert_eq!(out.dimensions(), (w, h), "the size stays");
            let (x0, y0, x1, y1) = changed(&base, &out).expect("something is drawn");
            if matches!(corner, Corner::TopLeft | Corner::BottomLeft) {
                assert!(x0 >= 4 && x1 < w / 2, "{corner:?}: {x0}..{x1} on the left");
            } else {
                assert!(x0 > w / 2 && x1 <= w - 4, "{corner:?}: {x0}..{x1} on the right");
            }
            if matches!(corner, Corner::TopLeft | Corner::TopRight) {
                assert!(y0 >= 4 && y1 < h / 2, "{corner:?}: {y0}..{y1} at the top");
            } else {
                assert!(y0 > h / 2 && y1 <= h - 4, "{corner:?}: {y0}..{y1} at the bottom");
            }
            // 10 % of the height, give or take the outline.
            assert!((24..=50).contains(&(y1 - y0)), "{corner:?}: {} high", y1 - y0);
            let lightest = out.pixels().map(|p| p[0]).max().unwrap();
            let darkest = out.pixels().map(|p| p[0]).min().unwrap();
            assert!(lightest > 240 && darkest < 60, "white with a dark outline: {lightest}, {darkest}");
        }
    }

    #[test]
    fn text_shrinks_to_fit_the_width() {
        let font = font();
        let base = flat(120, 400, [128, 128, 128]);
        let long = Watermark { scale: 20, ..text("A rather long watermark", Corner::BottomRight) };
        let (x0, _, x1, _) = changed(&base, &with(&long, base.clone(), Some(&font))).unwrap();
        assert!(x0 >= 4 && x1 <= 116, "{x0}..{x1}");
    }

    #[test]
    fn a_picture_blends_at_its_opacity_and_scale() {
        let red = picture("blend", &flat(20, 10, [255, 0, 0]));
        let base = flat(200, 100, [0, 0, 255]);
        let half = Watermark { mark: Mark::Image(red.clone()), corner: Corner::BottomRight, opacity: 50, scale: 25 };
        let out = with(&half, base.clone(), None);
        assert_eq!(out.dimensions(), (200, 100));
        // A quarter of 200 wide, so 50 × 25, 4 pixels in from the corner.
        assert_eq!(changed(&base, &out), Some((146, 71, 196, 96)));
        let [r, g, b, a] = out.get_pixel(170, 85).0;
        assert!(r.abs_diff(128) <= 1 && g == 0 && b.abs_diff(128) <= 1 && a == 255, "half red, half blue: {:?}", [r, g, b, a]);

        let solid = Watermark { opacity: 100, corner: Corner::TopLeft, scale: 10, ..half.clone() };
        let out = with(&solid, base.clone(), None);
        assert_eq!(changed(&base, &out), Some((4, 4, 24, 14)), "a tenth of the width");
        assert_eq!(out.get_pixel(10, 8).0, [255, 0, 0, 255]);
        assert_eq!(with(&Watermark { opacity: 0, ..solid }, base.clone(), None), base, "invisible");
        std::fs::remove_dir_all(red.parent().unwrap()).unwrap();
    }

    #[test]
    fn nothing_is_put_on_without_a_picture_or_a_font() {
        let base = flat(100, 80, [30, 160, 30]);
        let gone = Watermark { mark: Mark::Image(std::env::temp_dir().join("rsnip-no-such-watermark.png")), corner: Corner::TopLeft, opacity: 100, scale: 20 };
        assert_eq!(with(&gone, base.clone(), None), base, "a missing picture");
        let png = picture("not-png", &flat(10, 10, [255, 0, 0]));
        let jpeg = png.with_extension("jpg");
        image::DynamicImage::ImageRgba8(flat(10, 10, [255, 0, 0])).to_rgb8().save(&jpeg).unwrap();
        assert_eq!(with(&Watermark { mark: Mark::Image(jpeg.clone()), ..gone.clone() }, base.clone(), None), base, "not a PNG");
        assert!(read_picture(&jpeg).is_err());
        std::fs::write(&png, b"not really").unwrap();
        assert_eq!(with(&Watermark { mark: Mark::Image(png.clone()), ..gone }, base.clone(), None), base, "a damaged one");
        assert_eq!(with(&text("RSnip", Corner::TopLeft), base.clone(), None), base, "text without a font");
        assert_eq!(with(&text("  ", Corner::TopLeft), base.clone(), Some(&font())), base, "blank text");
        std::fs::remove_dir_all(png.parent().unwrap()).unwrap();
    }

    #[test]
    fn the_watermark_goes_on_before_the_frame_and_keeps_the_size() {
        let font = font();
        let base = flat(300, 200, [128, 128, 128]);
        let framed = Effects { watermark: Some(text("RSnip", Corner::TopLeft)), border: Some(eframe::egui::Color32::RED), shadow: true, ..Effects::default() };
        let plain = Effects { watermark: None, ..framed.clone() };
        let out = apply_effects(base.clone(), &framed, Some(&font));
        let without = apply_effects(base, &plain, Some(&font));
        assert_eq!(out.dimensions(), without.dimensions());
        let inset = SHADOW_PAD + BORDER_WIDTH;
        assert_eq!(out.dimensions(), (300 + 2 * inset, 200 + 2 * inset));
        let (x0, y0, x1, y1) = changed(&without, &out).expect("drawn");
        assert!(x0 >= inset + 4 && y0 >= inset + 4 && x1 < inset + 150 && y1 < inset + 100, "on the picture, inside the border: {x0},{y0}..{x1},{y1}");
    }
}
