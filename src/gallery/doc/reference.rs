//! Slow, obviously-correct versions of the image effects, kept as test oracles for the fast ones
//! in `doc.rs` (an idea from photocraft). They work on raw RGBA bytes, one output pixel at a time.

use super::MOSAIC;

/// A region of the image in whole pixels: (x0, y0, width, height).
pub type Region = (usize, usize, usize, usize);

/// Each pixel becomes the average of the part of its image-aligned mosaic block inside the region.
pub fn pixelate(px: &mut [u8], width: usize, (x0, y0, rw, rh): Region) {
    let src = px.to_vec();
    let (x1, y1) = (x0 + rw, y0 + rh);
    for y in y0..y1 {
        for x in x0..x1 {
            let (bx, by) = (x / MOSAIC * MOSAIC, y / MOSAIC * MOSAIC);
            let (xs, xe) = (bx.max(x0), (bx + MOSAIC).min(x1));
            let (ys, ye) = (by.max(y0), (by + MOSAIC).min(y1));
            let n = ((xe - xs) * (ye - ys)) as u32;
            for c in 0..4 {
                let sum: u32 = (ys..ye).flat_map(|v| (xs..xe).map(move |u| (u, v))).map(|(u, v)| src[(v * width + u) * 4 + c] as u32).sum();
                px[(y * width + x) * 4 + c] = (sum / n) as u8;
            }
        }
    }
}

/// Three rounds of horizontal then vertical box blur over the region, edges repeating outward.
pub fn blur(px: &mut [u8], width: usize, (x0, y0, rw, rh): Region) {
    let r = ((rw.min(rh) / 6).clamp(4, 14)) as isize;
    let mut region: Vec<[u8; 4]> = (0..rh * rw).map(|i| {
        let (x, y) = (x0 + i % rw, y0 + i / rw);
        px[(y * width + x) * 4..(y * width + x) * 4 + 4].try_into().unwrap()
    }).collect();
    let average = |get: &dyn Fn(isize) -> [u8; 4]| -> [u8; 4] {
        let mut out = [0u8; 4];
        for (c, value) in out.iter_mut().enumerate() {
            let sum: u32 = (-r..=r).map(|k| get(k)[c] as u32).sum();
            *value = (sum / (2 * r as u32 + 1)) as u8;
        }
        out
    };
    for _ in 0..3 {
        let prev = region.clone();
        for y in 0..rh {
            for x in 0..rw {
                region[y * rw + x] = average(&|k| prev[y * rw + (x as isize + k).clamp(0, rw as isize - 1) as usize]);
            }
        }
        let prev = region.clone();
        for y in 0..rh {
            for x in 0..rw {
                region[y * rw + x] = average(&|k| prev[(y as isize + k).clamp(0, rh as isize - 1) as usize * rw + x]);
            }
        }
    }
    for (i, p) in region.iter().enumerate() {
        let (x, y) = (x0 + i % rw, y0 + i / rw);
        px[(y * width + x) * 4..(y * width + x) * 4 + 4].copy_from_slice(p);
    }
}
