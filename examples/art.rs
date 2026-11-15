//! Write every image Topos ships outside the exe, all drawn by
//! `topos::icon`, plus a preview sheet for review:
//!
//! ```text
//! cargo run --release --example art -- installer-art
//! ```
//!
//! - `Topos.ico`: Setup's own icon (the exe embeds the same frames).
//! - `wizard-small-<px>.bmp`: Setup's page-header image at 100 to 250 %
//!   (one per image-area size Inno Setup uses).
//! - `msix/Assets/`: the Store package's logos at every scale and target
//!   size, unplated variants included (the taskbar draws those on no
//!   backing plate).
//! - `preview.png`: every icon size on a light and a dark background, the
//!   small ones also magnified.

use std::fs;
use std::path::Path;

use topos::icon::{self, ICO_SIZES, Image};

/// Store scale qualifiers, in percent.
const SCALES: [usize; 5] = [100, 125, 150, 200, 400];
/// Store target sizes for the 44 × 44 logo (taskbar, Start, Alt-Tab).
const TARGET_SIZES: [usize; 14] = [16, 20, 24, 30, 32, 36, 40, 48, 60, 64, 72, 80, 96, 256];
/// Setup's small wizard image area at 100, 125, 150, 175, 200, 225, and
/// 250 % (Inno Setup 6.6 and later).
const WIZARD_SIZES: [usize; 7] = [58, 77, 97, 116, 124, 143, 159];

fn at_scale(base: usize, scale: usize) -> usize {
    (base * scale).div_ceil(100)
}

fn write(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap_or_else(|err| panic!("write {}: {err}", path.display()));
}

fn main() {
    let out = std::env::args_os()
        .nth(1)
        .unwrap_or_else(|| "installer-art".into());
    let out = Path::new(&out);
    let assets = out.join("msix").join("Assets");
    fs::create_dir_all(&assets).expect("create the output folders");

    write(&out.join("Topos.ico"), &icon::ico_file(&ICO_SIZES));
    for size in WIZARD_SIZES {
        let bmp = icon::bmp_premultiplied(&icon::render(size));
        write(&out.join(format!("wizard-small-{size}.bmp")), &bmp);
    }

    let mut count = 0;
    let mut png = |name: String, image: &Image| {
        write(&assets.join(name), &icon::png(image));
        count += 1;
    };
    for scale in SCALES {
        png(
            format!("Square44x44Logo.scale-{scale}.png"),
            &icon::render(at_scale(44, scale)),
        );
        png(
            format!("StoreLogo.scale-{scale}.png"),
            &icon::render(at_scale(50, scale)),
        );
        // Tiles show the icon at half their height, centred.
        let square = at_scale(150, scale);
        png(
            format!("Square150x150Logo.scale-{scale}.png"),
            &icon::render_centered(square, square, square / 2),
        );
        let (wide, high) = (at_scale(310, scale), at_scale(150, scale));
        png(
            format!("Wide310x150Logo.scale-{scale}.png"),
            &icon::render_centered(wide, high, high / 2),
        );
    }
    for size in TARGET_SIZES {
        let image = icon::render(size);
        // The art carries its own outline, so one drawing serves on a plate,
        // on a dark taskbar, and on a light one.
        for variant in ["", "_altform-unplated", "_altform-lightunplated"] {
            png(
                format!("Square44x44Logo.targetsize-{size}{variant}.png"),
                &image,
            );
        }
    }

    write(&out.join("preview.png"), &icon::png(&preview()));
    println!(
        "wrote Topos.ico, {} wizard images, {count} Store logos, and preview.png to {}",
        WIZARD_SIZES.len(),
        out.display()
    );
}

/// Every ICO size at 1:1 on light and dark, then 16 and 32 px magnified.
fn preview() -> Image {
    const GAP: usize = 16;
    const LIGHT: [u8; 4] = [243, 243, 243, 255];
    const DARK: [u8; 4] = [32, 32, 32, 255];
    let zooms = [(16, 8), (32, 4), (48, 3)];
    let row_height = 256 + 2 * GAP;
    let width = ICO_SIZES.iter().map(|s| s + GAP).sum::<usize>()
        + zooms.iter().map(|(s, z)| s * z + GAP).sum::<usize>()
        + GAP;
    let mut sheet = Image::new(width, 2 * row_height);
    for (row, background) in [LIGHT, DARK].into_iter().enumerate() {
        for y in row * row_height..(row + 1) * row_height {
            for x in 0..width {
                let at = (y * width + x) * 4;
                sheet.rgba[at..at + 4].copy_from_slice(&background);
            }
        }
        let top = row * row_height + GAP;
        let mut x = GAP;
        for size in ICO_SIZES {
            sheet.place(&icon::render(size), x, top + (256 - size) / 2);
            x += size + GAP;
        }
        for (size, zoom) in zooms {
            sheet.place(
                &icon::magnify(&icon::render(size), zoom),
                x,
                top + (256 - size * zoom) / 2,
            );
            x += size * zoom + GAP;
        }
    }
    sheet
}
