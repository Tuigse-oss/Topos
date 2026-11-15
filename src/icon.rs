//! The Topos icon, drawn in code, and the image files it ships in.
//!
//! The icon is a folder with a title strip holding smaller tiles: one corner
//! of the map, in the map's own colours. A treemap is nothing but
//! rectangles, so every size is drawn on whole pixels with no
//! anti-aliasing — crisp from 16 px up, like the map's captions. Small sizes
//! drop the detail that would only turn to noise.
//!
//! `build.rs` includes this file to put the icon and the version details in
//! `Topos.exe`, so it uses nothing outside itself; `examples/art.rs` writes
//! the installer's and the Store package's images from it. The encoders
//! (PNG, ICO, BMP) are minimal but complete for what they write.

/// The sizes in the exe's icon and `Topos.ico`: Windows' shell sizes at
/// 100, 125, 150, and 200 percent scaling, and the 256-pixel tile.
pub const ICO_SIZES: [usize; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

/// The sizes in the window's own icon group. Windows loads a window's icon
/// once, at the large-icon size, and halves it for the title bar; so each
/// large size here is the small drawing doubled pixel for pixel, and the
/// halving gives back exactly the small drawing (see [`render_window`]).
pub const WINDOW_SIZES: [usize; 7] = [16, 20, 24, 32, 40, 48, 64];

/// Straight (not premultiplied) RGBA, rows top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// An opaque colour as red, green, blue.
type Color = [u8; 3];

// The map's depth colours and tile outline (`view::DEPTH_COLORS`,
// `view::BORDER`, `view::FILE_TINT`); a test keeps them equal.
const ROSE: Color = [0xF4, 0xA8, 0xA0];
const ORANGE: Color = [0xFF, 0xC8, 0x8C];
const YELLOW: Color = [0xFF, 0xF0, 0x82];
const GREEN: Color = [0xA4, 0xEC, 0x8E];
const INK: Color = [64, 64, 64];
const FILE_TINT: f32 = 0.3;

impl Image {
    /// A fully transparent image.
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            rgba: vec![0; width * height * 4],
        }
    }

    /// The RGBA bytes at a pixel.
    #[must_use]
    pub fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        let at = (y * self.width + x) * 4;
        [
            self.rgba[at],
            self.rgba[at + 1],
            self.rgba[at + 2],
            self.rgba[at + 3],
        ]
    }

    /// Fill `x0..x1` by `y0..y1`, clipped to the image, fully opaque.
    fn fill(&mut self, x0: usize, y0: usize, x1: usize, y1: usize, color: Color) {
        for y in y0.min(self.height)..y1.min(self.height) {
            for x in x0.min(self.width)..x1.min(self.width) {
                let at = (y * self.width + x) * 4;
                self.rgba[at..at + 4].copy_from_slice(&[color[0], color[1], color[2], 0xFF]);
            }
        }
    }

    /// A tile: `color` inside an `INK` outline `line` pixels wide.
    fn tile(&mut self, b: Area, line: usize, color: Color) {
        self.fill(b.x0, b.y0, b.x1, b.y1, INK);
        if b.x1 > b.x0 + 2 * line && b.y1 > b.y0 + 2 * line {
            self.fill(b.x0 + line, b.y0 + line, b.x1 - line, b.y1 - line, color);
        }
    }

    /// Copy `src` with its top-left corner at `x`, `y`, where it is opaque.
    pub fn place(&mut self, src: &Image, x: usize, y: usize) {
        for sy in 0..src.height {
            for sx in 0..src.width {
                let px = src.pixel(sx, sy);
                let (dx, dy) = (x + sx, y + sy);
                if px[3] != 0 && dx < self.width && dy < self.height {
                    let at = (dy * self.width + dx) * 4;
                    self.rgba[at..at + 4].copy_from_slice(&px);
                }
            }
        }
    }
}

/// A pixel rectangle, `x0..x1` by `y0..y1`.
#[derive(Debug, Clone, Copy)]
struct Area {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

fn lighten(color: Color, amount: f32) -> Color {
    color.map(|c| (f32::from(c) + (255.0 - f32::from(c)) * amount).round() as u8)
}

fn scaled(size: usize, fraction: f32) -> usize {
    (size as f32 * fraction).round() as usize
}

/// The icon at `size` × `size` pixels on a transparent background.
///
/// A rose folder whose title strip carries a caption mark holds an orange
/// folder (with a lighter file inside it from 32 px) beside a yellow and a
/// green tile. Neighbouring tiles share one outline, as on the map.
#[must_use]
pub fn render(size: usize) -> Image {
    let mut image = Image::new(size, size);
    if size < 8 {
        return image;
    }
    let pad = size / 16;
    let line = scaled(size, 1.0 / 40.0).max(1);
    // The parent's colour showing between its outline and its children.
    let gap = scaled(size, 1.0 / 28.0).max(1);
    let strip = scaled(size, 0.2).max(3 + line);
    let detail = size >= 32;

    let outer = Area {
        x0: pad,
        y0: pad,
        x1: size - pad,
        y1: size - pad,
    };
    image.tile(outer, line, ROSE);
    if detail {
        caption(&mut image, outer, strip, line, gap);
    }

    // The children fill the folder below its strip; their outlines sit
    // `gap` pixels inside the folder's own.
    let inner = Area {
        x0: outer.x0 + line + gap,
        y0: outer.y0 + strip,
        x1: outer.x1 - line - gap,
        y1: outer.y1 - line - gap,
    };
    let split = inner.x0 + scaled(inner.x1 - inner.x0, 0.58);
    let left = Area { x1: split, ..inner };
    image.tile(left, line, ORANGE);
    let shelf = inner.y0 + scaled(inner.y1 - inner.y0, 0.55);
    // Shared outlines: each neighbour starts on the previous one's edge.
    let top_right = Area {
        x0: split - line,
        y1: shelf,
        ..inner
    };
    let bottom_right = Area {
        x0: split - line,
        y0: shelf - line,
        ..inner
    };
    image.tile(top_right, line, YELLOW);
    image.tile(bottom_right, line, GREEN);

    if detail {
        let child_strip = scaled(strip, 0.8).max(3 + line);
        caption(&mut image, left, child_strip, line, gap);
        let file = Area {
            x0: left.x0 + line + gap,
            y0: left.y0 + child_strip,
            x1: left.x1 - line - gap,
            y1: left.y1 - line - gap,
        };
        if file.x1 > file.x0 + 2 * line + 2 && file.y1 > file.y0 + 2 * line + 2 {
            image.tile(file, line, lighten(ORANGE, FILE_TINT));
        }
    }
    image
}

/// A short dark bar in a folder's title strip, standing in for its name.
fn caption(image: &mut Image, folder: Area, strip: usize, line: usize, gap: usize) {
    let thick = scaled(strip, 0.18).max(1);
    let top = folder.y0 + line + (strip - line).saturating_sub(thick) / 2;
    let x0 = folder.x0 + line + gap + scaled(folder.x1 - folder.x0, 0.04);
    let x1 = folder.x0 + scaled(folder.x1 - folder.x0, 0.5);
    if x1 > x0 {
        image.fill(x0, top, x1, top + thick, INK);
    }
}

/// The window icon at `size`: the small drawings as they are, and from
/// 32 pixels up the drawing at half the size, doubled pixel for pixel, so
/// that Windows' halving for the title bar (32 to 16 at 100 % scaling, 48
/// to 24 at 150 %) lands on the crisp small drawing instead of blurring the
/// detailed large one.
#[must_use]
pub fn render_window(size: usize) -> Image {
    if size >= 32 && size.is_multiple_of(2) {
        magnify(&render_window(size / 2), 2)
    } else {
        render(size)
    }
}

/// Nearest-neighbour enlargement by a whole factor.
#[must_use]
pub fn magnify(image: &Image, zoom: usize) -> Image {
    let mut out = Image::new(image.width * zoom, image.height * zoom);
    for y in 0..out.height {
        for x in 0..out.width {
            let px = image.pixel(x / zoom, y / zoom);
            let at = (y * out.width + x) * 4;
            out.rgba[at..at + 4].copy_from_slice(&px);
        }
    }
    out
}

/// The icon at `art` pixels centred on a transparent `width` × `height`
/// canvas, for the Store's tile images.
#[must_use]
pub fn render_centered(width: usize, height: usize, art: usize) -> Image {
    let mut canvas = Image::new(width, height);
    let icon = render(art.min(width).min(height));
    canvas.place(&icon, (width - icon.width) / 2, (height - icon.height) / 2);
    canvas
}

// ---- file formats ----------------------------------------------------------

/// One icon frame as it is stored in an `.ico` file or an `RT_ICON`
/// resource: PNG at 256 pixels, a 32-bit DIB below (what every Windows
/// version reads at small sizes).
#[must_use]
pub fn icon_frame(image: &Image) -> Vec<u8> {
    if image.width >= 256 {
        return png(image);
    }
    let (width, height) = (image.width, image.height);
    let mask_row = width.div_ceil(32) * 4;
    let mut out = Vec::with_capacity(40 + width * height * 4 + mask_row * height);
    out.extend(40u32.to_le_bytes());
    out.extend((width as i32).to_le_bytes());
    // A DIB in an icon counts the colour and mask rows together.
    out.extend((height as i32 * 2).to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(32u16.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(((width * height * 4 + mask_row * height) as u32).to_le_bytes());
    out.extend([0u8; 16]);
    // Bottom-up BGRA, then an all-zero mask: the alpha channel decides.
    for y in (0..height).rev() {
        for x in 0..width {
            let [r, g, b, a] = image.pixel(x, y);
            out.extend([b, g, r, a]);
        }
    }
    out.resize(out.len() + mask_row * height, 0);
    out
}

/// The directory fields shared by an `.ico` file and an `RT_GROUP_ICON`
/// resource entry: width, height (0 for 256), colours, reserved, planes,
/// bits per pixel, and the frame's byte length.
#[must_use]
pub fn icon_dir_fields(size: usize, bytes: usize) -> Vec<u8> {
    let edge = if size >= 256 { 0 } else { size as u8 };
    let mut out = vec![edge, edge, 0, 0];
    out.extend(1u16.to_le_bytes());
    out.extend(32u16.to_le_bytes());
    out.extend((bytes as u32).to_le_bytes());
    out
}

/// An `.ico` file holding the icon at each of `sizes`.
#[must_use]
pub fn ico_file(sizes: &[usize]) -> Vec<u8> {
    let frames: Vec<Vec<u8>> = sizes.iter().map(|&s| icon_frame(&render(s))).collect();
    let mut out = Vec::new();
    out.extend(0u16.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend((sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len();
    for (&size, frame) in sizes.iter().zip(&frames) {
        out.extend(icon_dir_fields(size, frame.len()));
        out.extend((offset as u32).to_le_bytes());
        offset += frame.len();
    }
    for frame in &frames {
        out.extend(frame);
    }
    out
}

/// A 32-bit bottom-up BMP with premultiplied alpha, the form Inno Setup
/// blends when `WizardImageAlphaFormat=premultiplied`.
#[must_use]
pub fn bmp_premultiplied(image: &Image) -> Vec<u8> {
    let (width, height) = (image.width, image.height);
    let pixels = width * height * 4;
    let mut out = Vec::with_capacity(54 + pixels);
    out.extend(b"BM");
    out.extend(((54 + pixels) as u32).to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(54u32.to_le_bytes());
    out.extend(40u32.to_le_bytes());
    out.extend((width as i32).to_le_bytes());
    out.extend((height as i32).to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(32u16.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend((pixels as u32).to_le_bytes());
    // 96 dpi.
    out.extend(3780u32.to_le_bytes());
    out.extend(3780u32.to_le_bytes());
    out.extend([0u8; 8]);
    for y in (0..height).rev() {
        for x in 0..width {
            let [r, g, b, a] = image.pixel(x, y);
            let pre = |c: u8| ((u16::from(c) * u16::from(a) + 127) / 255) as u8;
            out.extend([pre(b), pre(g), pre(r), a]);
        }
    }
    out
}

/// A PNG of `image` (8-bit RGBA). Each row takes whichever of the None, Sub,
/// and Up filters leaves the smallest residue, and the stream is deflated
/// with fixed Huffman codes and run-length matches: flat tiles become runs
/// of zeros, so the 256-pixel icon stays a few kilobytes.
#[must_use]
pub fn png(image: &Image) -> Vec<u8> {
    let (width, height) = (image.width, image.height);
    let stride = width * 4;
    let mut raw = Vec::with_capacity(height * (stride + 1));
    let zero_row = vec![0u8; stride];
    for y in 0..height {
        let row = &image.rgba[y * stride..(y + 1) * stride];
        let above = if y == 0 {
            &zero_row[..]
        } else {
            &image.rgba[(y - 1) * stride..y * stride]
        };
        let sub: Vec<u8> = (0..stride)
            .map(|i| row[i].wrapping_sub(if i >= 4 { row[i - 4] } else { 0 }))
            .collect();
        let up: Vec<u8> = (0..stride).map(|i| row[i].wrapping_sub(above[i])).collect();
        let cost = |bytes: &[u8]| -> u64 {
            bytes
                .iter()
                .map(|&b| u64::from((b as i8).unsigned_abs()))
                .sum()
        };
        let choices: [(u8, &[u8]); 3] = [(0, row), (1, &sub), (2, &up)];
        let (kind, bytes) = choices
            .into_iter()
            .min_by_key(|(_, bytes)| cost(bytes))
            .unwrap_or((0, row));
        raw.push(kind);
        raw.extend_from_slice(bytes);
    }

    let mut out = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    let mut header = Vec::with_capacity(13);
    header.extend((width as u32).to_be_bytes());
    header.extend((height as u32).to_be_bytes());
    header.extend([8, 6, 0, 0, 0]);
    png_chunk(&mut out, b"IHDR", &header);
    png_chunk(&mut out, b"IDAT", &zlib(&raw));
    png_chunk(&mut out, b"IEND", &[]);
    out
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend((data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend(kind);
    out.extend(data);
    let crc = crc32(&out[start..]);
    out.extend(crc.to_be_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                0xEDB8_8320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Bits written least significant first, as deflate packs them.
struct Bits {
    out: Vec<u8>,
    acc: u32,
    used: u32,
}

impl Bits {
    fn put(&mut self, value: u32, count: u32) {
        self.acc |= value << self.used;
        self.used += count;
        while self.used >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.used -= 8;
        }
    }

    /// A Huffman code, which deflate stores most significant bit first.
    fn code(&mut self, code: u32, count: u32) {
        let reversed = code.reverse_bits() >> (32 - count);
        self.put(reversed, count);
    }

    /// A literal or length symbol under the fixed Huffman code.
    fn symbol(&mut self, symbol: u32) {
        match symbol {
            0..=143 => self.code(0x30 + symbol, 8),
            144..=255 => self.code(0x190 + symbol - 144, 9),
            256..=279 => self.code(symbol - 256, 7),
            _ => self.code(0xC0 + symbol - 280, 8),
        }
    }

    /// A match of `length` (3 to 258) at distance 1: repeat the last byte.
    fn repeat(&mut self, length: u32) {
        const BASE: [u32; 29] = [
            3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99,
            115, 131, 163, 195, 227, 258,
        ];
        const EXTRA: [u32; 29] = [
            0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
        ];
        let index = BASE.iter().rposition(|&base| base <= length).unwrap_or(0);
        self.symbol(257 + index as u32);
        self.put(length - BASE[index], EXTRA[index]);
        // Distance code 0 (distance 1), five bits, no extra bits.
        self.code(0, 5);
    }
}

/// A zlib stream: one final deflate block with fixed codes, where every run
/// of a repeated byte becomes a distance-1 match.
fn zlib(data: &[u8]) -> Vec<u8> {
    let mut bits = Bits {
        out: vec![0x78, 0x01],
        acc: 0,
        used: 0,
    };
    // BFINAL, then BTYPE 01 (fixed Huffman codes).
    bits.put(1, 1);
    bits.put(1, 2);
    let mut i = 0;
    while i < data.len() {
        let byte = data[i];
        bits.symbol(u32::from(byte));
        i += 1;
        let mut run = 0;
        while i + run < data.len() && data[i + run] == byte && run < 258 {
            run += 1;
        }
        if run >= 3 {
            bits.repeat(run as u32);
            i += run;
        }
    }
    bits.symbol(256);
    if bits.used > 0 {
        bits.out.push(bits.acc as u8);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    bits.out.extend(((b << 16) | a).to_be_bytes());
    bits.out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view;

    fn rgb(color: view::Rgb) -> Color {
        [color.r, color.g, color.b]
    }

    #[test]
    fn the_icon_uses_the_maps_own_colours() {
        assert_eq!(ROSE, rgb(view::DEPTH_COLORS[0]));
        assert_eq!(ORANGE, rgb(view::DEPTH_COLORS[1]));
        assert_eq!(YELLOW, rgb(view::DEPTH_COLORS[2]));
        assert_eq!(GREEN, rgb(view::DEPTH_COLORS[3]));
        assert_eq!(INK, rgb(view::BORDER));
        assert!((FILE_TINT - view::FILE_TINT).abs() < f32::EPSILON);
    }

    #[test]
    fn every_size_is_crisp_and_framed() {
        for size in [
            16, 20, 24, 30, 32, 36, 40, 44, 48, 55, 64, 96, 150, 256, 600,
        ] {
            let image = render(size);
            let palette = [ROSE, ORANGE, YELLOW, GREEN, INK, lighten(ORANGE, FILE_TINT)];
            for px in image.rgba.chunks(4) {
                // No anti-aliasing: every pixel is fully transparent or a
                // palette colour at full opacity.
                match px[3] {
                    0 => assert_eq!(px, [0, 0, 0, 0], "size {size}"),
                    0xFF => assert!(palette.contains(&[px[0], px[1], px[2]]), "size {size}"),
                    alpha => panic!("size {size}: partial alpha {alpha}"),
                }
            }
            // The corners stay clear (the icon sits on any background) and
            // the outline starts one sixteenth in.
            let pad = size / 16;
            if pad > 0 {
                assert_eq!(image.pixel(0, 0)[3], 0, "size {size}");
            }
            assert_eq!(image.pixel(pad, pad), [64, 64, 64, 0xFF], "size {size}");
            assert_eq!(
                image.pixel(size - 1 - pad, size - 1 - pad),
                [64, 64, 64, 0xFF],
                "size {size}"
            );
            // Each tile colour shows.
            for color in [ROSE, ORANGE, YELLOW, GREEN] {
                assert!(
                    image
                        .rgba
                        .chunks(4)
                        .any(|px| px[..3] == color && px[3] == 0xFF),
                    "size {size} lacks {color:?}"
                );
            }
        }
    }

    #[test]
    fn halving_a_window_icon_gives_the_small_drawing() {
        for size in [32, 40, 48, 64] {
            let big = render_window(size);
            let small = render_window(size / 2);
            if size / 2 < 32 {
                assert_eq!(small, render(size / 2));
            }
            // Every 2 × 2 block of the large frame is one small pixel.
            for y in 0..size {
                for x in 0..size {
                    assert_eq!(
                        big.pixel(x, y),
                        small.pixel(x / 2, y / 2),
                        "{size} at {x},{y}"
                    );
                }
            }
        }
        assert_eq!(render_window(16), render(16));
        assert_eq!(render_window(24), render(24));
    }

    #[test]
    fn small_sizes_drop_the_detail() {
        let file = lighten(ORANGE, FILE_TINT);
        let has = |image: &Image| image.rgba.chunks(4).any(|px| px[..3] == file);
        assert!(!has(&render(24)));
        assert!(has(&render(32)));
        assert!(has(&render(256)));
    }

    #[test]
    fn pngs_decode_to_the_same_pixels() {
        for size in [1, 16, 33, 256] {
            let image = if size == 1 {
                Image {
                    width: 1,
                    height: 1,
                    rgba: vec![1, 2, 3, 4],
                }
            } else {
                render(size)
            };
            let bytes = png(&image);
            let decoder = png::Decoder::new(std::io::Cursor::new(&bytes));
            let mut reader = decoder.read_info().expect("valid PNG header");
            let mut pixels = vec![0; reader.output_buffer_size().expect("size")];
            let info = reader.next_frame(&mut pixels).expect("valid PNG data");
            assert_eq!((info.width as usize, info.height as usize), (size, size));
            assert_eq!(info.color_type, png::ColorType::Rgba);
            assert_eq!(&pixels[..info.buffer_size()], &image.rgba[..]);
        }
        // Flat tiles compress: the 256-pixel icon is a few kilobytes.
        assert!(png(&render(256)).len() < 8 * 1024);
    }

    #[test]
    fn long_runs_and_every_length_code_round_trip() {
        // A row of noise then runs of every length from 1 to 600.
        let mut data: Vec<u8> = (0..=255).collect();
        for length in 1..=600 {
            data.extend(std::iter::repeat_n((length % 7) as u8, length));
        }
        let packed = zlib(&data);
        let unpacked = miniz_oxide::inflate::decompress_to_vec_zlib(&packed).expect("inflates");
        assert_eq!(unpacked, data);
    }

    #[test]
    fn an_ico_file_lists_every_frame_where_it_is() {
        let bytes = ico_file(&ICO_SIZES);
        let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        assert_eq!((u16_at(0), u16_at(2), u16_at(4)), (0, 1, 8));
        let mut end = 6 + 16 * ICO_SIZES.len();
        for (index, &size) in ICO_SIZES.iter().enumerate() {
            let entry = 6 + 16 * index;
            let edge = if size == 256 { 0 } else { size as u8 };
            assert_eq!((bytes[entry], bytes[entry + 1]), (edge, edge));
            assert_eq!(u16_at(entry + 6), 32);
            let (len, offset) = (u32_at(entry + 8) as usize, u32_at(entry + 12) as usize);
            assert_eq!(offset, end);
            let frame = &bytes[offset..offset + len];
            if size == 256 {
                assert_eq!(&frame[..8], b"\x89PNG\r\n\x1a\n");
            } else {
                assert_eq!(u32::from_le_bytes(frame[..4].try_into().unwrap()), 40);
                assert_eq!(len, 40 + size * size * 4 + size.div_ceil(32) * 4 * size);
            }
            end += len;
        }
        assert_eq!(end, bytes.len());
    }

    #[test]
    fn bmps_are_premultiplied_and_bottom_up() {
        let image = Image {
            width: 2,
            height: 2,
            rgba: vec![
                200, 100, 50, 128, /* top left */ 0, 0, 0, 0, //
                10, 20, 30, 255, /* bottom left */ 0, 0, 0, 0,
            ],
        };
        let bytes = bmp_premultiplied(&image);
        assert_eq!(&bytes[..2], b"BM");
        assert_eq!(bytes.len(), 54 + 16);
        // The first stored row is the bottom one, in BGRA.
        assert_eq!(&bytes[54..58], &[30, 20, 10, 255]);
        // Top left, premultiplied by 128/255.
        assert_eq!(&bytes[62..66], &[25, 50, 100, 128]);
    }

    #[test]
    fn centred_art_sits_in_the_middle_of_a_wide_tile() {
        let tile = render_centered(310, 150, 100);
        assert_eq!((tile.width, tile.height), (310, 150));
        assert_eq!(tile.pixel(0, 0)[3], 0);
        // The 100-px icon's outline starts at its own 1/16 padding.
        assert_eq!(tile.pixel(105 + 6, 25 + 6), [64, 64, 64, 0xFF]);
    }
}
