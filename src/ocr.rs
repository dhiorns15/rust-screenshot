//! Reading text out of an image with the OCR engine built into Windows (no download needed).
//!
//! `recognize` and `recognize_words` block while the engine runs, so call it off the UI thread.

use eframe::egui;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::{OcrEngine, OcrResult};
use windows::Storage::Streams::DataWriter;

use crate::capture::Image;
use crate::gallery::words::Word;

/// The recognised lines of text in `img`, joined with newlines.
pub fn recognize(img: &Image) -> Result<String, String> {
    let engine = engine()?;
    let max = OcrEngine::MaxImageDimension().unwrap_or(2600) as i32;
    let img = fit_for_ocr(img, max);
    run(&engine, &img).map_err(|e| format!("Text recognition failed: {}", e.message()))
}

/// The recognised words in `img`, with where each one is (in `img`'s pixels).
pub fn recognize_words(img: &Image) -> Result<Vec<Word>, String> {
    let engine = engine()?;
    let max = OcrEngine::MaxImageDimension().unwrap_or(2600) as i32;
    let fitted = fit_for_ocr(img, max);
    let scale = img.width as f32 / fitted.width as f32;
    let words = || -> windows::core::Result<Vec<Word>> {
        let mut words = Vec::new();
        for (line, l) in read(&engine, &fitted)?.Lines()?.into_iter().enumerate() {
            for word in l.Words()? {
                let r = word.BoundingRect()?;
                let rect = egui::Rect::from_min_size(egui::pos2(r.X * scale, r.Y * scale), egui::vec2(r.Width * scale, r.Height * scale));
                words.push(Word { text: word.Text()?.to_string_lossy(), rect, line });
            }
        }
        Ok(words)
    };
    words().map_err(|e| format!("Text recognition failed: {}", e.message()))
}

fn engine() -> Result<OcrEngine, String> {
    OcrEngine::TryCreateFromUserProfileLanguages().map_err(|_| {
        "No text-recognition language is installed. Add one under Settings → Time & language → Language & region.".to_string()
    })
}

fn read(engine: &OcrEngine, img: &Image) -> windows::core::Result<OcrResult> {
    let writer = DataWriter::new()?;
    writer.WriteBytes(&img.data)?;
    let buffer = writer.DetachBuffer()?;
    let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(&buffer, BitmapPixelFormat::Bgra8, img.width, img.height, BitmapAlphaMode::Ignore)?;
    engine.RecognizeAsync(&bitmap)?.get()
}

fn run(engine: &OcrEngine, img: &Image) -> windows::core::Result<String> {
    let lines: Vec<String> = read(engine, img)?.Lines()?.into_iter().map(|line| line.Text().map(|t| t.to_string_lossy())).collect::<Result<_, _>>()?;
    Ok(lines.join("\n"))
}

/// Scales `img` to suit the engine: small captures are enlarged (it reads small UI text far better
/// at 2×), and anything over its maximum size is shrunk to fit.
fn fit_for_ocr(img: &Image, max: i32) -> Image {
    let longest = img.width.max(img.height).max(1);
    let scale = if longest * 2 <= max.min(2000) { 2.0 } else { (max as f32 / longest as f32).min(1.0) };
    if scale == 1.0 {
        return Image { width: img.width, height: img.height, data: img.data.clone() };
    }
    let (w, h) = (((img.width as f32 * scale) as u32).max(1), ((img.height as f32 * scale) as u32).max(1));
    // Channel order doesn't matter for resampling, so BGRA goes through as if it were RGBA.
    let src = image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.data.clone()).expect("image size");
    let out = image::imageops::resize(&src, w, h, image::imageops::FilterType::CatmullRom);
    Image { width: w as i32, height: h as i32, data: out.into_raw() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_small_images_up_and_big_ones_down() {
        let img = |w: i32, h: i32| Image { width: w, height: h, data: vec![255; (w * h * 4) as usize] };
        let small = fit_for_ocr(&img(300, 40), 2600);
        assert_eq!((small.width, small.height), (600, 80));
        let big = fit_for_ocr(&img(5200, 1000), 2600);
        assert_eq!((big.width, big.height), (2600, 500));
        let mid = fit_for_ocr(&img(1500, 900), 2600);
        assert_eq!((mid.width, mid.height), (1500, 900));
    }
}
