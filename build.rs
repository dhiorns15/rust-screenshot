//! Embeds the app icon and version info in the Windows exe, so Explorer and the taskbar show them.

use std::path::Path;

#[path = "src/icon_art.rs"]
mod icon_art;

const SIZES: [u32; 6] = [16, 24, 32, 48, 64, 256];

/// A Windows `.ico` holding a PNG image per size.
fn ico() -> Vec<u8> {
    let images: Vec<Vec<u8>> = SIZES.iter().map(|&n| png(n)).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(SIZES.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * SIZES.len() as u32;
    for (&n, image) in SIZES.iter().zip(&images) {
        // 256 is stored as 0.
        let side = if n >= 256 { 0 } else { n as u8 };
        out.extend_from_slice(&[side, side, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(image.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += image.len() as u32;
    }
    images.iter().for_each(|image| out.extend_from_slice(image));
    out
}

fn png(n: u32) -> Vec<u8> {
    let rgba: Vec<u8> = icon_art::render(n as i32).chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, n, n);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().unwrap().write_image_data(&rgba).unwrap();
    out
}

fn main() {
    println!("cargo:rerun-if-changed=src/icon_art.rs");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = std::env::var("OUT_DIR").unwrap();
    let icon = Path::new(&out).join("rsnip.ico");
    let ico = ico();
    std::fs::write(&icon, &ico).unwrap();
    // Also next to the exe (OUT_DIR is target/<profile>/build/rsnip-<hash>/out), for the installer.
    if let Some(profile_dir) = Path::new(&out).ancestors().nth(3) {
        let _ = std::fs::write(profile_dir.join("rsnip.ico"), &ico);
    }

    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let numbers: Vec<u16> = version.split(['.', '-']).take(3).map(|p| p.parse().unwrap_or(0)).collect();
    let [major, minor, patch] = [0, 1, 2].map(|i| numbers.get(i).copied().unwrap_or(0));
    let rc = format!(
        r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "RSnip contributors"
      VALUE "FileDescription", "RSnip screenshot tool"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "rsnip"
      VALUE "LegalCopyright", "MIT License"
      VALUE "OriginalFilename", "rsnip.exe"
      VALUE "ProductName", "RSnip"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = icon.display().to_string().replace('\\', "\\\\"),
    );
    let rc_path = Path::new(&out).join("rsnip.rc");
    std::fs::write(&rc_path, rc).unwrap();
    embed_resource::compile(&rc_path, embed_resource::NONE).manifest_optional().unwrap();
}
