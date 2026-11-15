//! What the map looks like, apart from any window system: tile colours, the
//! map as a pixel buffer, and the captions to draw over it.
//!
//! [`build`] lays out the visible folder ([`treemap::layout`]), paints every
//! tile into a [`Frame`] — a plain top-down BGRA buffer the shell copies to
//! the screen in one call — and lists the [`Caption`]s that fit. Tiles come
//! in pre-order, so painting them in order draws contents over their folder.

use crate::format::{self, exact_bytes, group_digits, pretty_bytes};
use crate::layout::Rect;
use crate::scanner::{DriveInfo, ScanStats};
use crate::theme::Palette;
use crate::tree::NodeRef;
use crate::treemap::{self, Density, NO_PARENT, Style, Tile, TileKind};

/// An opaque colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    #[must_use]
    pub const fn gray(level: u8) -> Self {
        Self::new(level, level, level)
    }

    /// Move `amount` (0 to 1) of the way toward `other`.
    #[must_use]
    pub fn mix(self, other: Rgb, amount: f32) -> Self {
        let amount = amount.clamp(0.0, 1.0);
        // In 0..=255 by construction, since amount is in 0..=1.
        let mix = |from: u8, to: u8| {
            (f32::from(from) + (f32::from(to) - f32::from(from)) * amount).round() as u8
        };
        Self::new(
            mix(self.r, other.r),
            mix(self.g, other.g),
            mix(self.b, other.b),
        )
    }

    /// Move `amount` (0 to 1) of the way toward white.
    #[must_use]
    pub fn lighten(self, amount: f32) -> Self {
        let amount = amount.clamp(0.0, 1.0);
        // In 0..=255 by construction, since amount is in 0..=1.
        let mix = |channel: u8| {
            let channel = f32::from(channel);
            (channel + (255.0 - channel) * amount).round() as u8
        };
        Self::new(mix(self.r), mix(self.g), mix(self.b))
    }
}

/// Folder colours by depth below the visible folder, cycling (the light
/// palette; [`crate::theme`] holds both).
pub const DEPTH_COLORS: [Rgb; 7] = [
    Rgb::new(0xF4, 0xA8, 0xA0), // rose
    Rgb::new(0xFF, 0xC8, 0x8C), // orange
    Rgb::new(0xFF, 0xF0, 0x82), // yellow
    Rgb::new(0xA4, 0xEC, 0x8E), // green
    Rgb::new(0x90, 0xEC, 0xEC), // cyan
    Rgb::new(0xB8, 0xBC, 0xFF), // lavender
    Rgb::new(0xF2, 0xA2, 0xF2), // magenta
];
/// Tile outlines.
pub const BORDER: Rgb = Rgb::gray(64);
/// Behind the map, where no tile is.
pub const CANVAS: Rgb = Rgb::gray(150);
pub const FREE_FILL: Rgb = Rgb::gray(232);
pub const UNREADABLE_FILL: Rgb = Rgb::gray(176);
/// How far a group's hatching moves from its fill toward the outline colour.
pub const HATCH: f32 = 0.22;
/// How far a file's colour moves from its level's folder colour toward white.
pub const FILE_TINT: f32 = 0.3;

/// Fill colour: folders by depth below the scan root (`depth_offset` is
/// the visible folder's own depth, so a folder keeps its colour at every
/// zoom); files a lighter shade of their level's colour, or their type's
/// colour when `by_type`; groups of small items as files; unreadable
/// folders grey.
#[must_use]
pub fn tile_fill(
    tile: &Tile,
    node: Option<NodeRef<'_>>,
    by_type: bool,
    palette: &Palette,
    depth_offset: usize,
) -> Rgb {
    let level = (usize::from(tile.depth) + depth_offset) % palette.depth.len();
    let base = palette.depth[level];
    match tile.kind {
        TileKind::FreeSpace => palette.free_fill,
        TileKind::Dir if node.is_some_and(|node| node.unreadable()) => palette.unreadable_fill,
        TileKind::Dir => base,
        TileKind::File if by_type => {
            palette.types[FileType::of(node.map_or("", |node| node.name())) as usize]
        }
        TileKind::File | TileKind::Group { .. } => base.lighten(palette.file_tint),
    }
}

/// What kind of file a name is, by its extension, for colouring by type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Video,
    Audio,
    Images,
    /// Archives and disk images.
    Archives,
    /// Programs, libraries, drivers, and installers.
    Programs,
    Documents,
    /// Source code and data files.
    Code,
    Other,
}

impl FileType {
    pub const ALL: [FileType; 8] = [
        FileType::Video,
        FileType::Audio,
        FileType::Images,
        FileType::Archives,
        FileType::Programs,
        FileType::Documents,
        FileType::Code,
        FileType::Other,
    ];

    /// The legend's word for the type.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            FileType::Video => "Video",
            FileType::Audio => "Audio",
            FileType::Images => "Images",
            FileType::Archives => "Archives",
            FileType::Programs => "Programs",
            FileType::Documents => "Documents",
            FileType::Code => "Code & data",
            FileType::Other => "Other",
        }
    }

    /// The type of a file name, by its extension (any letter case).
    #[must_use]
    pub fn of(name: &str) -> FileType {
        let Some((_, extension)) = name.rsplit_once('.') else {
            return FileType::Other;
        };
        let extension = extension.to_ascii_lowercase();
        let is = |list: &[&str]| list.contains(&extension.as_str());
        if is(&[
            "mp4", "mkv", "mov", "avi", "wmv", "webm", "m4v", "mpg", "mpeg", "m2ts", "mts", "vob",
            "flv", "3gp", "ts",
        ]) {
            FileType::Video
        } else if is(&[
            "mp3", "flac", "wav", "aac", "m4a", "ogg", "opus", "wma", "aif", "aiff", "mid", "midi",
        ]) {
            FileType::Audio
        } else if is(&[
            "jpg", "jpeg", "png", "gif", "bmp", "tif", "tiff", "webp", "heic", "heif", "avif",
            "raw", "cr2", "cr3", "nef", "arw", "dng", "psd", "svg", "ico",
        ]) {
            FileType::Images
        } else if is(&[
            "zip", "7z", "rar", "tar", "gz", "tgz", "bz2", "xz", "zst", "cab", "iso", "img", "vhd",
            "vhdx", "vmdk", "vdi", "wim", "esd", "dmg",
        ]) {
            FileType::Archives
        } else if is(&[
            "exe", "dll", "sys", "msi", "msix", "msp", "appx", "ocx", "drv", "scr", "com", "efi",
            "lib", "so", "pdb", "jar",
        ]) {
            FileType::Programs
        } else if is(&[
            "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "rtf", "txt",
            "md", "epub", "one", "pst", "ost",
        ]) {
            FileType::Documents
        } else if is(&[
            "rs", "c", "h", "cpp", "hpp", "cs", "java", "py", "js", "ts", "go", "rb", "php",
            "html", "css", "json", "xml", "yaml", "yml", "toml", "csv", "sql", "db", "sqlite",
            "log", "ini", "cfg", "ps1", "sh", "bat",
        ]) {
            FileType::Code
        } else {
            FileType::Other
        }
    }
}

/// How long the sweeping segment of a progress bar with no estimate takes
/// to cross the bar, in seconds.
pub const SWEEP_SECONDS: f32 = 1.5;

/// Where a progress bar `width` pixels long is filled, as `(start,
/// length)`: from the left by the share `done`; with no estimate, a
/// segment a quarter of the bar long sweeping left to right, entering and
/// leaving at the ends, `seconds` after the work began.
#[must_use]
pub fn progress_span(width: i32, done: Option<f32>, seconds: f32) -> (i32, i32) {
    if width <= 0 {
        return (0, 0);
    }
    if let Some(done) = done {
        // In 0..=width by the clamp.
        let length = (width as f32 * done.clamp(0.0, 1.0)).round() as i32;
        return (0, length);
    }
    let segment = (width / 4).max(1);
    let travel = width + segment;
    let phase = (seconds.max(0.0) % SWEEP_SECONDS) / SWEEP_SECONDS;
    // In -segment..width.
    let start = (phase * travel as f32) as i32 - segment;
    let (from, to) = (start.max(0), (start + segment).min(width));
    (from, (to - from).max(0))
}

/// A whole-pixel rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PixelRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl PixelRect {
    /// The pixel rectangle of a snapped layout rectangle.
    #[must_use]
    pub fn from_rect(rect: Rect) -> Self {
        // Layout rectangles lie inside the canvas, far inside i32.
        Self {
            x: rect.x.round() as i32,
            y: rect.y.round() as i32,
            w: rect.w.round() as i32,
            h: rect.h.round() as i32,
        }
    }

    #[must_use]
    pub fn right(self) -> i32 {
        self.x.saturating_add(self.w)
    }

    #[must_use]
    pub fn bottom(self) -> i32 {
        self.y.saturating_add(self.h)
    }

    #[must_use]
    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// The rectangle `by` pixels smaller on every side.
    #[must_use]
    pub fn shrink(self, by: i32) -> Self {
        Self {
            x: self.x + by,
            y: self.y + by,
            w: (self.w - 2 * by).max(0),
            h: (self.h - 2 * by).max(0),
        }
    }
}

/// An opaque top-down 32-bit image: four bytes per pixel in blue, green,
/// red, unused order — the layout of a 32-bit Windows DIB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    width: usize,
    height: usize,
    /// Always exactly `width * height * 4` bytes.
    bgra: Vec<u8>,
}

impl Frame {
    /// A `width` × `height` frame filled with `background`.
    #[must_use]
    pub fn new(width: usize, height: usize, background: Rgb) -> Self {
        let pixel = [background.b, background.g, background.r, 0xFF];
        // `repeat` fills by doubling copies, not one pixel at a time.
        let bgra = pixel.repeat(width.saturating_mul(height));
        Self {
            width,
            height,
            bgra,
        }
    }

    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    #[must_use]
    pub fn height(&self) -> usize {
        self.height
    }

    /// The pixels, `width * height * 4` bytes, rows top to bottom.
    #[must_use]
    pub fn bgra(&self) -> &[u8] {
        &self.bgra
    }

    /// The colour at a pixel, if it is inside the frame.
    #[must_use]
    pub fn pixel(&self, x: usize, y: usize) -> Option<Rgb> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let at = (y * self.width + x) * 4;
        Some(Rgb::new(
            self.bgra[at + 2],
            self.bgra[at + 1],
            self.bgra[at],
        ))
    }

    /// Draw a straight-alpha RGBA image with its top-left corner at `x`,
    /// `y`, blended over what is there and clipped to the frame.
    pub fn blend(&mut self, image: &crate::icon::Image, x: i32, y: i32) {
        // Where a source offset lands in the frame, if inside it.
        let land = |origin: i32, offset: usize, limit: usize| {
            let at = i64::from(origin) + i64::try_from(offset).ok()?;
            usize::try_from(at).ok().filter(|&at| at < limit)
        };
        for sy in 0..image.height {
            let Some(dy) = land(y, sy, self.height) else {
                continue;
            };
            for sx in 0..image.width {
                let Some(dx) = land(x, sx, self.width) else {
                    continue;
                };
                let [r, g, b, a] = image.pixel(sx, sy);
                if a == 0 {
                    continue;
                }
                let at = (dy * self.width + dx) * 4;
                let mix = |src: u8, dst: u8| {
                    let (src, dst, a) = (u32::from(src), u32::from(dst), u32::from(a));
                    // In 0..=255: a weighted mean of two bytes.
                    ((src * a + dst * (255 - a) + 127) / 255) as u8
                };
                self.bgra[at] = mix(b, self.bgra[at]);
                self.bgra[at + 1] = mix(g, self.bgra[at + 1]);
                self.bgra[at + 2] = mix(r, self.bgra[at + 2]);
            }
        }
    }

    /// Diagonal lines `spacing` pixels apart across a rectangle, clipped to
    /// the frame.
    pub fn hatch(&mut self, rect: PixelRect, color: Rgb, spacing: usize) {
        let spacing = spacing.max(2);
        let clamp_x = |v: i32| usize::try_from(v.max(0)).unwrap_or(0).min(self.width);
        let clamp_y = |v: i32| usize::try_from(v.max(0)).unwrap_or(0).min(self.height);
        let (x0, x1) = (clamp_x(rect.x), clamp_x(rect.right()));
        let (y0, y1) = (clamp_y(rect.y), clamp_y(rect.bottom()));
        let pixel = [color.b, color.g, color.r, 0xFF];
        for y in y0..y1 {
            // The first column on this row's line, then every `spacing`.
            let mut x = x0 + (spacing - (x0 + y) % spacing) % spacing;
            while x < x1 {
                let at = (y * self.width + x) * 4;
                self.bgra[at..at + 4].copy_from_slice(&pixel);
                x += spacing;
            }
        }
    }

    /// Fill a rectangle, clipped to the frame.
    pub fn fill(&mut self, rect: PixelRect, color: Rgb) {
        let clamp_x = |v: i32| usize::try_from(v.max(0)).unwrap_or(0).min(self.width);
        let clamp_y = |v: i32| usize::try_from(v.max(0)).unwrap_or(0).min(self.height);
        let (x0, x1) = (clamp_x(rect.x), clamp_x(rect.right()));
        let (y0, y1) = (clamp_y(rect.y), clamp_y(rect.bottom()));
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let stride = self.width * 4;
        let first = y0 * stride + x0 * 4;
        let len = (x1 - x0) * 4;
        let pixel = [color.b, color.g, color.r, 0xFF];
        for chunk in self.bgra[first..first + len].as_chunks_mut::<4>().0 {
            *chunk = pixel;
        }
        for y in y0 + 1..y1 {
            self.bgra
                .copy_within(first..first + len, y * stride + x0 * 4);
        }
    }
}

/// Text placed on the map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caption {
    /// Where the text may draw; it is clipped to this.
    pub clip: PixelRect,
    pub text: String,
    /// Centre in the tile when it fits (files, groups, and free space);
    /// folder names start at the left of their title strip.
    pub centered: bool,
    /// Shown after `text` on the same line when both fit (a folder's size
    /// on its title strip); dropped first when the strip is too narrow.
    pub detail: Option<String>,
}

/// Everything a laid-out map depends on. A [`View`] is reused while its key
/// stays the same.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewKey {
    /// Bumped whenever the scanned tree changes.
    pub generation: u64,
    /// Index path of the folder filling the map.
    pub zoom: Vec<usize>,
    pub width: u32,
    pub height: u32,
    /// Physical pixels per 96-dpi pixel.
    pub scale: f32,
    /// Height of one line of caption text, in pixels.
    pub line: u32,
    /// Bytes for the free-space tile, when it is shown.
    pub free_space: Option<u64>,
    /// Colour files by type rather than by depth.
    pub by_type: bool,
    /// How small a tile may be before it joins its folder's group.
    pub density: Density,
    /// Paint with the dark palette.
    pub dark: bool,
}

/// A laid-out map: its tiles (for hit testing and outlines) and captions.
/// The painted pixels come back from [`build`] separately, since the shell
/// only needs them once.
#[derive(Debug)]
pub struct View {
    pub key: ViewKey,
    pub tiles: Vec<Tile>,
    pub captions: Vec<Caption>,
}

/// Lay out `visible` for `key`, paint it, and place its captions. `drive`
/// and `stats` fill in the free-space caption.
#[must_use]
pub fn build(
    visible: NodeRef<'_>,
    key: ViewKey,
    drive: Option<&DriveInfo>,
    stats: &ScanStats,
) -> (View, Frame) {
    let width = key.width as usize;
    let height = key.height as usize;
    let area = Rect {
        x: 0.0,
        y: 0.0,
        w: key.width as f32,
        h: key.height as f32,
    };
    let style = Style::with_density(key.scale, key.density);
    let tiles = treemap::layout(visible, area, key.free_space, &style);

    // Each tile's node, in one pass: tiles are pre-order, so a parent's node
    // is always resolved before its children's.
    let mut nodes: Vec<Option<NodeRef<'_>>> = Vec::with_capacity(tiles.len());
    for tile in &tiles {
        let node = if !tile.kind.is_node() {
            None
        } else if tile.parent == NO_PARENT {
            visible.child(tile.child as usize)
        } else {
            nodes[tile.parent as usize].and_then(|parent| parent.child(tile.child as usize))
        };
        nodes.push(node);
    }

    let palette = Palette::of(key.dark);
    let mut frame = Frame::new(width, height, palette.canvas);
    let mut captions = Vec::new();
    let line = key.line as f32;
    let depth_offset = key.zoom.len();
    for (tile, node) in tiles.iter().zip(&nodes) {
        let outer = PixelRect::from_rect(tile.rect);
        frame.fill(outer, palette.border);
        if outer.w > 2 && outer.h > 2 {
            let fill = tile_fill(tile, *node, key.by_type, palette, depth_offset);
            frame.fill(outer.shrink(1), fill);
            // A group of small items is hatched, so it never reads as one
            // big file.
            if matches!(tile.kind, TileKind::Group { .. }) {
                let spacing = (6.0 * key.scale).round().max(3.0) as usize;
                frame.hatch(outer.shrink(1), fill.mix(palette.border, HATCH), spacing);
            }
        }
        let caption = match tile.kind {
            TileKind::FreeSpace => drive.map(|drive| free_space_text(drive, stats)),
            TileKind::Group { folders, largest } => {
                let holder = if tile.parent == NO_PARENT {
                    Some(visible)
                } else {
                    nodes[tile.parent as usize]
                };
                let biggest = holder.and_then(|holder| holder.child(largest as usize));
                Some(group_text(tile, folders, biggest, line))
            }
            _ => node.map(|node| node_text(tile, node, line)),
        };
        if let Some((text, centered)) = caption
            && let Some(clip) = caption_clip(tile, &style, line, key.scale)
        {
            let detail = (tile.kind == TileKind::Dir).then(|| pretty_bytes(tile.bytes));
            captions.push(Caption {
                clip,
                text,
                centered,
                detail,
            });
        }
    }
    (
        View {
            key,
            tiles,
            captions,
        },
        frame,
    )
}

/// Where a tile's caption goes: a folder's title strip, or the inside of a
/// file or free-space tile. `None` when there is no room for text `line`
/// pixels high.
#[must_use]
pub fn caption_clip(tile: &Tile, style: &Style, line: f32, scale: f32) -> Option<PixelRect> {
    let r = tile.rect;
    let pad = (3.0 * scale).round();
    match tile.kind {
        TileKind::Dir if tile.header && r.w >= 2.0 * line => Some(PixelRect::from_rect(Rect {
            x: r.x + pad,
            y: r.y + 1.0,
            w: r.w - pad - 2.0,
            h: style.header - 1.0,
        })),
        TileKind::File | TileKind::FreeSpace | TileKind::Group { .. }
            if r.w >= 2.0 * line && r.h >= line =>
        {
            Some(PixelRect::from_rect(Rect {
                x: r.x + 2.0,
                y: r.y + 1.0,
                w: r.w - 4.0,
                h: r.h - 2.0,
            }))
        }
        _ => None,
    }
}

/// Caption for a file or folder tile, and whether to centre it. Big file
/// tiles add the exact size and the last-write time.
#[must_use]
pub fn node_text(tile: &Tile, node: NodeRef<'_>, line: f32) -> (String, bool) {
    let mut text = node.name().to_owned();
    if node.is_dir() {
        if node.unreadable() {
            text.push_str("  (no access)");
        }
        return (text, false);
    }
    if tile.rect.h >= 3.0 * line + 4.0 && tile.rect.w >= 6.0 * line {
        text.push('\n');
        text.push_str(&exact_bytes(node.size()));
        if let Some(time) = node.modified().and_then(format::local_time) {
            text.push('\n');
            text.push_str(&time);
        }
    }
    (text, true)
}

/// Caption for a group of small items: how many, and their total when
/// there is room for a second line.
#[must_use]
pub fn group_text(
    tile: &Tile,
    folders: u32,
    biggest: Option<NodeRef<'_>>,
    line: f32,
) -> (String, bool) {
    let count = tile.child;
    let files = count.saturating_sub(folders);
    let what = match (folders, files) {
        (0, _) => "files",
        (_, 0) => "folders",
        _ => "items",
    };
    let mut lines = vec![
        format!("{} small {what}", group_digits(u64::from(count))),
        pretty_bytes(tile.bytes),
    ];
    if folders > 0 && files > 0 {
        lines.push(format!(
            "{} folders, {} files",
            group_digits(u64::from(folders)),
            group_digits(u64::from(files))
        ));
    }
    if let Some(node) = biggest {
        lines.push(format!(
            "largest: {} ({})",
            node.name(),
            pretty_bytes(node.size())
        ));
    }
    // As many lines as the tile has room for, most important first.
    let room = ((tile.rect.h - 4.0) / line).floor().max(1.0) as usize;
    lines.truncate(room);
    (lines.join("\n"), true)
}

/// Caption for the free-space tile.
#[must_use]
pub fn free_space_text(drive: &DriveInfo, stats: &ScanStats) -> (String, bool) {
    (
        format!(
            "<Free Space: {}>\n{} Free\nFiles Total: {}\nFolders Total: {}",
            format::percent(drive.free_bytes, drive.total_bytes),
            pretty_bytes(drive.free_bytes),
            group_digits(stats.files),
            group_digits(stats.folders)
        ),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{Node, Tree};

    fn tile(kind: TileKind, depth: u16, w: f32, h: f32) -> Tile {
        Tile {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                w,
                h,
            },
            kind,
            depth,
            parent: NO_PARENT,
            child: 0,
            bytes: 0,
            header: false,
            open: false,
        }
    }

    fn key(width: u32, height: u32) -> ViewKey {
        ViewKey {
            generation: 1,
            zoom: Vec::new(),
            width,
            height,
            scale: 1.0,
            line: 14,
            free_space: None,
            by_type: false,
            density: Density::Detailed,
            dark: false,
        }
    }

    #[test]
    fn lighten_moves_toward_white() {
        let color = Rgb::new(100, 0, 255);
        assert_eq!(color.lighten(0.0), color);
        assert_eq!(color.lighten(1.0), Rgb::gray(255));
        assert_eq!(color.lighten(0.5), Rgb::new(178, 128, 255));
    }

    #[test]
    fn file_types_come_from_the_extension() {
        assert_eq!(FileType::of("Holiday.MP4"), FileType::Video);
        assert_eq!(FileType::of("track01.flac"), FileType::Audio);
        assert_eq!(FileType::of("beach.jpg"), FileType::Images);
        assert_eq!(FileType::of("disk.vhdx"), FileType::Archives);
        assert_eq!(FileType::of("pagefile.sys"), FileType::Programs);
        assert_eq!(FileType::of("report.pdf"), FileType::Documents);
        assert_eq!(FileType::of("main.rs"), FileType::Code);
        assert_eq!(FileType::of("README"), FileType::Other);
        assert_eq!(FileType::of("archive.unknown"), FileType::Other);
        assert_eq!(FileType::of(".gitignore"), FileType::Other);
        // Each type has its own colour and label, in order.
        for (index, kind) in FileType::ALL.into_iter().enumerate() {
            assert_eq!(kind as usize, index);
            assert!(!kind.label().is_empty());
        }
        let file = alone(Node::file("clip.mkv", 1, None));
        assert_eq!(
            tile_fill(
                &tile(TileKind::File, 0, 10.0, 10.0),
                file.node_at(&[0]),
                true,
                &crate::theme::LIGHT,
                0
            ),
            crate::theme::LIGHT.types[FileType::Video as usize]
        );
    }

    /// A tree whose only child is `node`, to read it back as a `NodeRef`.
    fn alone(node: Node) -> Tree {
        Tree::from(Node::dir("root", vec![node], None))
    }

    #[test]
    fn folder_colours_cycle_by_depth() {
        let folder = alone(Node::dir("d", Vec::new(), None));
        for depth in 0..10 {
            assert_eq!(
                tile_fill(
                    &tile(TileKind::Dir, depth, 10.0, 10.0),
                    folder.node_at(&[0]),
                    false,
                    &crate::theme::LIGHT,
                    0
                ),
                DEPTH_COLORS[usize::from(depth) % DEPTH_COLORS.len()]
            );
        }
        let file = alone(Node::file("f.bin", 1, None));
        assert_eq!(
            tile_fill(
                &tile(TileKind::File, 2, 10.0, 10.0),
                file.node_at(&[0]),
                false,
                &crate::theme::LIGHT,
                0
            ),
            DEPTH_COLORS[2].lighten(FILE_TINT)
        );
        let mut locked = Node::dir("locked", Vec::new(), None);
        locked.unreadable = true;
        let locked = alone(locked);
        assert_eq!(
            tile_fill(
                &tile(TileKind::Dir, 0, 10.0, 10.0),
                locked.node_at(&[0]),
                false,
                &crate::theme::LIGHT,
                0
            ),
            UNREADABLE_FILL
        );
    }

    #[test]
    fn progress_bars_fill_by_share_or_sweep() {
        assert_eq!(progress_span(200, Some(0.25), 9.0), (0, 50));
        assert_eq!(progress_span(200, Some(2.0), 0.0), (0, 200));
        assert_eq!(progress_span(0, None, 0.3), (0, 0));
        // The sweep enters from the left, crosses, and leaves at the right,
        // never outside the bar.
        assert_eq!(progress_span(200, None, 0.0), (0, 0));
        let mut seen_middle = false;
        for step in 0..150 {
            let (start, length) = progress_span(200, None, step as f32 * 0.01);
            assert!(start >= 0 && length >= 0 && start + length <= 200);
            assert!(length <= 50);
            seen_middle |= start > 70 && start < 130 && length == 50;
        }
        assert!(seen_middle);
        // It repeats.
        assert_eq!(
            progress_span(200, None, 0.4),
            progress_span(200, None, 0.4 + SWEEP_SECONDS)
        );
    }

    #[test]
    fn blending_mixes_by_alpha_and_clips() {
        let mut frame = Frame::new(3, 2, Rgb::gray(100));
        let image = crate::icon::Image {
            width: 2,
            height: 2,
            rgba: vec![
                200, 0, 0, 255, /* */ 0, 200, 0, 0, //
                0, 0, 200, 128, /* */ 255, 255, 255, 255,
            ],
        };
        frame.blend(&image, 2, 0);
        assert_eq!(frame.pixel(2, 0), Some(Rgb::new(200, 0, 0)));
        assert_eq!(frame.pixel(2, 1), Some(Rgb::new(50, 50, 150)));
        assert_eq!(frame.pixel(1, 0), Some(Rgb::gray(100)), "left alone");
        frame.blend(&image, -1, -1);
        assert_eq!(
            frame.pixel(0, 0),
            Some(Rgb::gray(255)),
            "clipped, not shifted"
        );
    }

    #[test]
    fn frame_fill_clips_to_the_frame() {
        let mut frame = Frame::new(4, 3, Rgb::gray(0));
        assert_eq!(frame.bgra().len(), 4 * 3 * 4);
        frame.fill(
            PixelRect {
                x: -2,
                y: 1,
                w: 4,
                h: 10,
            },
            Rgb::new(1, 2, 3),
        );
        assert_eq!(frame.pixel(0, 0), Some(Rgb::gray(0)));
        assert_eq!(frame.pixel(1, 1), Some(Rgb::new(1, 2, 3)));
        assert_eq!(frame.pixel(1, 2), Some(Rgb::new(1, 2, 3)));
        assert_eq!(frame.pixel(2, 1), Some(Rgb::gray(0)));
        assert_eq!(frame.pixel(4, 0), None);
        // Blue, green, red, unused.
        assert_eq!(
            &frame.bgra()[(4 + 1) * 4..(4 + 1) * 4 + 4],
            &[3, 2, 1, 0xFF]
        );
        frame.fill(
            PixelRect {
                x: 10,
                y: 10,
                w: 5,
                h: 5,
            },
            Rgb::gray(9),
        );
        frame.fill(
            PixelRect {
                x: 0,
                y: 0,
                w: 0,
                h: 3,
            },
            Rgb::gray(9),
        );
        assert!(frame.bgra().chunks(4).all(|px| px[0] != 9));
    }

    #[test]
    fn an_empty_frame_has_no_pixels() {
        let mut frame = Frame::new(0, 5, Rgb::gray(1));
        frame.fill(
            PixelRect {
                x: 0,
                y: 0,
                w: 3,
                h: 3,
            },
            Rgb::gray(2),
        );
        assert!(frame.bgra().is_empty());
    }

    #[test]
    fn built_frames_outline_every_tile_and_fill_inside() {
        let tree = Tree::from(Node::dir(
            "root",
            vec![
                Node::file("big.bin", 300, None),
                Node::file("small.bin", 100, None),
            ],
            None,
        ));
        let (view, frame) = build(tree.root(), key(200, 100), None, &ScanStats::default());
        assert_eq!((frame.width(), frame.height()), (200, 100));
        assert_eq!(view.tiles.len(), 2);
        for tile in &view.tiles {
            let r = PixelRect::from_rect(tile.rect);
            let at = |x: i32, y: i32| frame.pixel(x as usize, y as usize);
            assert_eq!(at(r.x, r.y), Some(BORDER));
            assert_eq!(at(r.right() - 1, r.bottom() - 1), Some(BORDER));
            assert_eq!(
                at(r.x + r.w / 2, r.y + r.h / 2),
                Some(DEPTH_COLORS[0].lighten(FILE_TINT))
            );
        }
        // Both files are big enough to be captioned, centred.
        assert_eq!(view.captions.len(), 2);
        assert!(view.captions.iter().all(|caption| caption.centered));
        assert_eq!(view.captions[0].text, "big.bin\n300 bytes");
    }

    #[test]
    fn colours_follow_depth_from_the_scan_root_at_every_zoom() {
        let tree = Tree::from(Node::dir(
            "root",
            vec![Node::dir(
                "outer",
                vec![Node::dir("inner", vec![Node::file("f", 50, None)], None)],
                None,
            )],
            None,
        ));
        // At the root, `inner` is two levels down.
        let (view, frame) = build(tree.root(), key(300, 200), None, &ScanStats::default());
        let inner = PixelRect::from_rect(view.tiles[1].rect);
        let colour_of_inner = frame.pixel(inner.x as usize + 5, inner.y as usize + 5);
        assert_eq!(colour_of_inner, Some(DEPTH_COLORS[1]));
        // Zoomed into `outer`, `inner` fills the top level yet keeps it.
        let mut zoomed = key(300, 200);
        zoomed.zoom = vec![0];
        let outer = tree.node_at(&[0]).unwrap();
        let (view, frame) = build(outer, zoomed, None, &ScanStats::default());
        let inner = PixelRect::from_rect(view.tiles[0].rect);
        assert_eq!(
            frame.pixel(inner.x as usize + 5, inner.y as usize + 5),
            colour_of_inner
        );
    }

    #[test]
    fn folder_strips_carry_their_size_and_groups_their_count() {
        let mut children = vec![Node::dir(
            "docs",
            vec![Node::file("a", 40_000, None), Node::file("b", 20_000, None)],
            None,
        )];
        children.extend((0..3000).map(|i| Node::file(format!("t{i}"), 10, None)));
        let tree = Tree::from(Node::dir("root", children, None));
        let (view, _) = build(tree.root(), key(400, 300), None, &ScanStats::default());
        let docs = view.captions.iter().find(|c| c.text == "docs").unwrap();
        assert_eq!(docs.detail.as_deref(), Some(pretty_bytes(60_000).as_str()));
        assert!(!docs.centered);
        let group = view
            .captions
            .iter()
            .find(|c| c.text.starts_with("3,000 small files"))
            .expect("a caption for the group");
        assert!(group.centered && group.detail.is_none());
        let lines: Vec<&str> = group.text.lines().collect();
        assert_eq!(lines[1], pretty_bytes(30_000));
        assert!(lines[2].starts_with("largest: t"), "{lines:?}");
        // Files carry no detail: their size is in the text when it fits.
        assert!(
            view.captions
                .iter()
                .filter(|c| c.centered)
                .all(|c| c.detail.is_none())
        );
    }

    #[test]
    fn group_captions_say_what_is_inside_as_room_allows() {
        let mut group = tile(
            TileKind::Group {
                folders: 3,
                largest: 0,
            },
            0,
            300.0,
            100.0,
        );
        group.child = 10;
        group.bytes = 5_000;
        let tree = alone(Node::dir("biggest", vec![Node::file("x", 900, None)], None));
        let biggest = tree.node_at(&[0]);
        let (text, centered) = group_text(&group, 3, biggest, 14.0);
        assert!(centered);
        assert_eq!(
            text.lines().collect::<Vec<_>>(),
            [
                "10 small items",
                pretty_bytes(5_000).as_str(),
                "3 folders, 7 files",
                format!("largest: biggest ({})", pretty_bytes(900)).as_str(),
            ]
        );
        // A short tile keeps the most important lines.
        group.rect.h = 34.0;
        let (text, _) = group_text(&group, 3, biggest, 14.0);
        assert_eq!(text, format!("10 small items\n{}", pretty_bytes(5_000)));
        // Only folders, or only files, say so.
        assert!(
            group_text(&group, 10, None, 14.0)
                .0
                .starts_with("10 small folders")
        );
        assert!(
            group_text(&group, 0, None, 14.0)
                .0
                .starts_with("10 small files")
        );
    }

    #[test]
    fn groups_are_hatched() {
        let mut children = vec![Node::file("big", 1_000_000, None)];
        children.extend((0..3000).map(|i| Node::file(format!("t{i}"), 100, None)));
        let tree = Tree::from(Node::dir("root", children, None));
        let (view, frame) = build(tree.root(), key(400, 300), None, &ScanStats::default());
        let group = view
            .tiles
            .iter()
            .find(|t| matches!(t.kind, TileKind::Group { .. }))
            .unwrap();
        let fill = DEPTH_COLORS[0].lighten(FILE_TINT);
        let hatch = fill.mix(BORDER, HATCH);
        let inside = PixelRect::from_rect(group.rect).shrink(1);
        let (mut plain, mut lined) = (0, 0);
        for y in inside.y..inside.bottom() {
            for x in inside.x..inside.right() {
                match frame.pixel(x as usize, y as usize) {
                    Some(c) if c == fill => plain += 1,
                    Some(c) if c == hatch => lined += 1,
                    other => panic!("unexpected {other:?} at {x},{y}"),
                }
            }
        }
        // One pixel in six on each row is a hatch line.
        assert!(lined * 4 < plain && lined * 8 > plain, "{lined} of {plain}");
        // Plain files are not hatched.
        let big = view
            .tiles
            .iter()
            .find(|t| t.kind == TileKind::File)
            .unwrap();
        let centre = PixelRect::from_rect(big.rect);
        for dx in 0..12 {
            let at = frame.pixel((centre.x + 20 + dx) as usize, (centre.y + 20) as usize);
            assert_eq!(at, Some(fill));
        }
    }

    #[test]
    fn a_dark_key_paints_with_the_dark_palette() {
        let tree = Tree::from(Node::dir(
            "root",
            vec![Node::dir("docs", vec![Node::file("a.txt", 50, None)], None)],
            None,
        ));
        let mut dark = key(300, 200);
        dark.dark = true;
        let (view, frame) = build(tree.root(), dark, None, &ScanStats::default());
        let palette = &crate::theme::DARK;
        let folder = PixelRect::from_rect(view.tiles[0].rect);
        let file = PixelRect::from_rect(view.tiles[1].rect);
        assert_eq!(
            frame.pixel(folder.x as usize, folder.y as usize),
            Some(palette.border)
        );
        assert_eq!(
            frame.pixel(folder.x as usize + 5, folder.y as usize + 5),
            Some(palette.depth[0])
        );
        assert_eq!(
            frame.pixel(
                (file.x + file.w / 2) as usize,
                (file.y + file.h / 2) as usize
            ),
            Some(palette.depth[1].lighten(palette.file_tint))
        );
    }

    #[test]
    fn folders_paint_under_their_contents() {
        let tree = Tree::from(Node::dir(
            "root",
            vec![Node::dir(
                "docs",
                vec![Node::file("a.txt", 50, None), Node::file("b.txt", 50, None)],
                None,
            )],
            None,
        ));
        let (view, frame) = build(tree.root(), key(300, 200), None, &ScanStats::default());
        assert_eq!(view.tiles.len(), 3);
        let folder = PixelRect::from_rect(view.tiles[0].rect);
        let file = PixelRect::from_rect(view.tiles[1].rect);
        // The strip keeps the folder colour; the file inside is lighter.
        assert_eq!(
            frame.pixel(folder.x as usize + 5, folder.y as usize + 5),
            Some(DEPTH_COLORS[0])
        );
        assert_eq!(
            frame.pixel(
                (file.x + file.w / 2) as usize,
                (file.y + file.h / 2) as usize
            ),
            Some(DEPTH_COLORS[1].lighten(FILE_TINT))
        );
        assert_eq!(view.captions[0].text, "docs");
        assert!(!view.captions[0].centered);
    }

    #[test]
    fn free_space_gets_a_tile_and_caption() {
        let tree = Tree::from(Node::dir("C:\\", vec![Node::file("a", 500, None)], None));
        let drive = DriveInfo {
            mount: String::from("C:\\"),
            total_bytes: 1000,
            free_bytes: 500,
        };
        let stats = ScanStats {
            files: 1,
            ..ScanStats::default()
        };
        let mut key = key(400, 200);
        key.free_space = Some(500);
        let (view, frame) = build(tree.root(), key, Some(&drive), &stats);
        let free = view
            .tiles
            .iter()
            .find(|tile| tile.kind == TileKind::FreeSpace)
            .unwrap();
        let r = PixelRect::from_rect(free.rect);
        assert_eq!(
            frame.pixel((r.x + r.w / 2) as usize, (r.y + r.h / 2) as usize),
            Some(FREE_FILL)
        );
        let caption = view
            .captions
            .iter()
            .find(|caption| caption.text.starts_with("<Free Space"))
            .unwrap();
        assert!(caption.text.contains("Files Total: 1"));
    }

    #[test]
    fn big_file_tiles_show_size_and_date() {
        let tree = alone(Node::file(
            "docker_data.vhdx",
            142_070_513_664,
            Some(1_794_004_274),
        ));
        let file = tree.node_at(&[0]).unwrap();
        let (text, centered) = node_text(&tile(TileKind::File, 0, 400.0, 200.0), file, 14.0);
        assert!(centered);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "docker_data.vhdx");
        assert_eq!(lines[1], "142,070,513,664 bytes");
        assert_eq!(lines.len(), 3);
        let small = node_text(&tile(TileKind::File, 0, 200.0, 20.0), file, 14.0);
        assert_eq!(small.0, "docker_data.vhdx");
    }

    #[test]
    fn captions_need_room() {
        let style = Style::default();
        let mut folder = tile(TileKind::Dir, 0, 100.0, 50.0);
        assert!(
            caption_clip(&folder, &style, 14.0, 1.0).is_none(),
            "no strip, no caption"
        );
        folder.header = true;
        let strip = caption_clip(&folder, &style, 14.0, 1.0).unwrap();
        assert_eq!(strip.h, style.header as i32 - 1);
        assert_eq!(strip.x, 3);
        assert!(caption_clip(&tile(TileKind::File, 0, 10.0, 50.0), &style, 14.0, 1.0).is_none());
        assert!(caption_clip(&tile(TileKind::File, 0, 60.0, 20.0), &style, 14.0, 1.0).is_some());
        assert!(caption_clip(&tile(TileKind::File, 0, 60.0, 10.0), &style, 14.0, 1.0).is_none());
    }

    #[test]
    fn unreadable_folders_say_so() {
        let mut node = Node::dir("System Volume Information", Vec::new(), None);
        node.unreadable = true;
        let tree = alone(node);
        let node = tree.node_at(&[0]).unwrap();
        let (text, centered) = node_text(&tile(TileKind::Dir, 0, 300.0, 40.0), node, 14.0);
        assert!(!centered);
        assert!(text.ends_with("(no access)"));
    }
}
