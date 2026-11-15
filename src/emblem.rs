//! The Tuigse LLC emblem, the publisher's mark shown in About.
//!
//! The emblem is an image, not code: 256 × 256 pixels of straight BGRA in
//! `assets/tuigse-emblem-256.bgra`. It is Tuigse LLC's mark and is not
//! licensed under MIT or Apache-2.0 like the rest of Topos; forks should
//! remove or replace it.

use crate::icon::Image;

const EMBLEM: &[u8] = include_bytes!("../assets/tuigse-emblem-256.bgra");
const EMBLEM_SIZE: usize = 256;

/// The emblem at `size` × `size` pixels (straight RGBA), scaled by area
/// averaging in premultiplied colour so edges stay smooth at every display
/// scale.
#[must_use]
pub fn render(size: usize) -> Image {
    let mut out = Image::new(size, size);
    if size == 0 {
        return out;
    }
    let scale = EMBLEM_SIZE as f32 / size as f32;
    for y in 0..size {
        for x in 0..size {
            let (x0, x1) = (x as f32 * scale, (x + 1) as f32 * scale);
            let (y0, y1) = (y as f32 * scale, (y + 1) as f32 * scale);
            let mut acc = [0.0f32; 4];
            let mut weight = 0.0f32;
            for sy in y0.floor() as usize..(y1.ceil() as usize).min(EMBLEM_SIZE) {
                let wy = (y1.min(sy as f32 + 1.0) - y0.max(sy as f32)).max(0.0);
                for sx in x0.floor() as usize..(x1.ceil() as usize).min(EMBLEM_SIZE) {
                    let wx = (x1.min(sx as f32 + 1.0) - x0.max(sx as f32)).max(0.0);
                    let w = wx * wy;
                    let at = (sy * EMBLEM_SIZE + sx) * 4;
                    let alpha = f32::from(EMBLEM[at + 3]) / 255.0;
                    // Stored as BGRA.
                    acc[0] += f32::from(EMBLEM[at + 2]) * alpha * w;
                    acc[1] += f32::from(EMBLEM[at + 1]) * alpha * w;
                    acc[2] += f32::from(EMBLEM[at]) * alpha * w;
                    acc[3] += alpha * w;
                    weight += w;
                }
            }
            if weight <= 0.0 || acc[3] <= 0.0 {
                continue;
            }
            let alpha = acc[3] / weight;
            let at = (y * size + x) * 4;
            for channel in 0..3 {
                out.rgba[at + channel] = (acc[channel] / acc[3]).round().clamp(0.0, 255.0) as u8;
            }
            out.rgba[at + 3] = (alpha * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_asset_is_a_whole_square_image() {
        assert_eq!(EMBLEM.len(), EMBLEM_SIZE * EMBLEM_SIZE * 4);
    }

    #[test]
    fn full_size_is_the_asset_itself() {
        let image = render(EMBLEM_SIZE);
        for (index, px) in image.rgba.chunks(4).enumerate() {
            let src = &EMBLEM[index * 4..index * 4 + 4];
            assert_eq!(px[3], src[3]);
            if src[3] > 0 {
                assert_eq!([px[0], px[1], px[2]], [src[2], src[1], src[0]]);
            }
        }
    }

    #[test]
    fn small_sizes_keep_the_mark_visible() {
        for size in [32, 48, 64, 96, 128] {
            let image = render(size);
            let covered = image.rgba.chunks(4).filter(|px| px[3] > 128).count();
            assert!(covered > size * size / 4, "size {size}: {covered}");
        }
        assert!(render(0).rgba.is_empty());
    }
}
