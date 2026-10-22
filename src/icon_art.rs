//! The app icon drawn in code (blue tile with white selection brackets). Pure Rust, so the build
//! script can also use it to embed the exe icon.

const BLUE: [f32; 3] = [37.0, 99.0, 235.0];
const WHITE: [f32; 3] = [255.0; 3];

fn inside_rounded_square(u: f32, v: f32, inset: f32, radius: f32) -> bool {
    let half = 0.5 - inset - radius;
    let (qx, qy) = ((u - 0.5).abs() - half, (v - 0.5).abs() - half);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) <= radius
}

fn sample(u: f32, v: f32) -> Option<[f32; 3]> {
    if !inside_rounded_square(u, v, 0.03, 0.2) {
        return None;
    }
    let (a, b, t, len) = (0.22, 0.78, 0.09, 0.2);
    let within = |p: f32, lo: f32, hi: f32| p >= lo && p <= hi;
    let on_side = |p: f32| within(p, a, a + t) || within(p, b - t, b);
    let near_corner = |p: f32| within(p, a, a + len) || within(p, b - len, b);
    let bracket = (on_side(u) && near_corner(v)) || (on_side(v) && near_corner(u));
    let dot = (u - 0.5).powi(2) + (v - 0.5).powi(2) <= 0.08f32.powi(2);
    Some(if bracket || dot { WHITE } else { BLUE })
}

/// The icon as BGRA pixels with straight alpha.
pub fn render(size: i32) -> Vec<u8> {
    const SS: usize = 4; // supersampling per axis
    let n = size as usize;
    let mut data = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let mut sum = [0f32; 3];
            let mut hits = 0f32;
            for sy in 0..SS {
                for sx in 0..SS {
                    let u = (x as f32 + (sx as f32 + 0.5) / SS as f32) / n as f32;
                    let v = (y as f32 + (sy as f32 + 0.5) / SS as f32) / n as f32;
                    if let Some(c) = sample(u, v) {
                        sum.iter_mut().zip(c).for_each(|(s, c)| *s += c);
                        hits += 1.0;
                    }
                }
            }
            if hits > 0.0 {
                let i = (y * n + x) * 4;
                data[i] = (sum[2] / hits) as u8;
                data[i + 1] = (sum[1] / hits) as u8;
                data[i + 2] = (sum[0] / hits) as u8;
                data[i + 3] = (hits / (SS * SS) as f32 * 255.0).round() as u8;
            }
        }
    }
    data
}

#[cfg(test)]
mod tests {
    #[test]
    fn renders_square_bgra_with_transparent_corners() {
        let px = super::render(32);
        assert_eq!(px.len(), 32 * 32 * 4);
        assert_eq!(px[3], 0, "rounded corner is transparent");
        let centre = (16 * 32 + 16) * 4;
        assert_eq!(&px[centre..centre + 4], &[255, 255, 255, 255], "white dot in the middle");
    }
}
