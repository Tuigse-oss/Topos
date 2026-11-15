//! Nested treemap layout: the whole visible tree at once.
//!
//! The visible folder's contents fill the area. Every folder tile with room
//! gets a title strip, and its own contents are laid out inside it, below
//! the strip, recursively — until tiles get too small to show. That is what
//! lets a disk map show where space goes several levels down. Each level is
//! a [`layout::squarify`] of the folder's children by size. Children too
//! small to draw one by one share a single group tile instead of turning
//! into a smear of slivers.
//!
//! Tiles land on whole pixels, so borders stay crisp and neighbours share
//! edges exactly. Tiles come out in pre-order — a folder before everything
//! inside it, siblings in child order — so painting in order draws contents
//! over their folder, and the last tile containing a point is the deepest
//! one under it ([`hit`]).

use crate::layout::{self, Rect};
use crate::tree::NodeRef;

/// `Tile::parent` of a top-level tile.
pub const NO_PARENT: u32 = u32::MAX;

/// How much detail the map shows: items whose tile would be smaller than
/// the density's side (squared) share one group tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Density {
    /// Every item down to 8 × 8 pixels, the classic look.
    Detailed,
    /// Only tiles with room for a name (about 30 × 30 pixels).
    #[default]
    Balanced,
    /// Fewer, larger tiles (about 48 × 48 pixels).
    Simple,
}

impl Density {
    pub const ALL: [Density; 3] = [Density::Detailed, Density::Balanced, Density::Simple];

    /// The word the settings file stores.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Density::Detailed => "detailed",
            Density::Balanced => "balanced",
            Density::Simple => "simple",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|density| density.code() == text)
    }

    /// The smallest tile side drawn on its own, in 96-dpi pixels.
    #[must_use]
    pub fn side(self) -> f32 {
        match self {
            Density::Detailed => 8.0,
            Density::Balanced => 30.0,
            Density::Simple => 48.0,
        }
    }
}

/// Pixel sizes that shape the map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    /// Height of a folder's title strip.
    pub header: f32,
    /// Gap between a folder's edge and its contents (sides and bottom).
    pub inset: f32,
    /// Tiles narrower or shorter than this are not emitted; their folder's
    /// colour shows through instead.
    pub min_side: f32,
    /// A folder opens (shows its contents) only when the area left for them
    /// is at least this wide and tall.
    pub min_content: f32,
    /// Children whose tile would cover fewer square pixels than this are
    /// drawn together as one group tile, when there are two or more.
    pub group_below: f32,
}

impl Style {
    /// The map's sizes at a display scale with `density`'s grouping.
    #[must_use]
    pub fn with_density(scale: f32, density: Density) -> Self {
        let mut style = Self::for_scale(scale);
        let scale = style.header / 14.0;
        style.group_below = (density.side() * scale).round().powi(2);
        style
    }
}

impl Style {
    /// The map's sizes at a display scale (physical pixels per logical
    /// point), in physical pixels.
    #[must_use]
    pub fn for_scale(scale: f32) -> Self {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        Self {
            header: (14.0 * scale).round(),
            inset: (2.0 * scale).round().max(1.0),
            min_side: (2.0 * scale).round().max(1.0),
            min_content: (6.0 * scale).round(),
            group_below: (Density::Detailed.side() * scale).round().powi(2),
        }
    }
}

impl Default for Style {
    fn default() -> Self {
        Self::for_scale(1.0)
    }
}

/// What a tile stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileKind {
    Dir,
    File,
    /// The drive's free space, shown beside the root's contents.
    FreeSpace,
    /// A folder's children that are too small to draw one by one, as one
    /// tile; `Tile::child` holds how many, `folders` how many of those are
    /// folders, and `largest` the child index of the biggest of them.
    Group {
        folders: u32,
        largest: u32,
    },
}

impl TileKind {
    /// Whether the tile shows one file or folder (it has an index path).
    #[must_use]
    pub fn is_node(self) -> bool {
        matches!(self, TileKind::Dir | TileKind::File)
    }
}

/// One drawn tile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tile {
    /// Whole-pixel rectangle.
    pub rect: Rect,
    pub kind: TileKind,
    /// 0 for the visible folder's own contents, +1 per level below.
    pub depth: u16,
    /// Index of the enclosing folder's tile, or [`NO_PARENT`].
    pub parent: u32,
    /// Index of this node in its parent node's `children`; for a group, how
    /// many children it stands for; unused for [`TileKind::FreeSpace`].
    pub child: u32,
    /// The bytes the tile stands for: the node's size, the free space, or a
    /// group's total.
    pub bytes: u64,
    /// The folder has a title strip `Style::header` tall at its top.
    pub header: bool,
    /// The folder's contents are laid out inside it.
    pub open: bool,
}

impl Tile {
    /// Half-open containment, so a point on a shared edge hits one tile.
    #[must_use]
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.rect.x
            && y >= self.rect.y
            && x < self.rect.x + self.rect.w
            && y < self.rect.y + self.rect.h
    }
}

/// Lay out `root`'s contents in `area`, plus a free-space tile of
/// `free_space` bytes when given and non-zero.
#[must_use]
pub fn layout(root: NodeRef<'_>, area: Rect, free_space: Option<u64>, style: &Style) -> Vec<Tile> {
    let area = snap(area);
    let mut tiles = Vec::new();
    let free_space = free_space.filter(|&bytes| bytes > 0);
    let free_rect = place_children(
        root,
        area,
        free_space.unwrap_or(0),
        0,
        NO_PARENT,
        style,
        &mut tiles,
    );
    if let (Some(bytes), Some(rect)) = (free_space, free_rect) {
        let rect = snap(rect);
        if rect.w >= style.min_side && rect.h >= style.min_side {
            tiles.push(Tile {
                rect,
                kind: TileKind::FreeSpace,
                depth: 0,
                parent: NO_PARENT,
                child: u32::MAX,
                bytes,
                header: false,
                open: false,
            });
        }
    }
    tiles
}

/// Lay `folder`'s children out in `area` at `depth` (their tiles' parent is
/// `parent`), grouping the ones too small to draw. `extra` bytes (the
/// drive's free space beside the root) take their share of the area too;
/// their rectangle is returned for the caller to draw.
fn place_children(
    folder: NodeRef<'_>,
    area: Rect,
    extra: u64,
    depth: u16,
    parent: u32,
    style: &Style,
    tiles: &mut Vec<Tile>,
) -> Option<Rect> {
    let (sizes, small) = split(folder, area, extra, style);
    place_split(
        folder, area, extra, depth, parent, style, tiles, &sizes, small,
    )
}

/// A folder's children's sizes, and which of them are too small to draw one
/// by one in `area` (see [`small_children`]).
fn split(
    folder: NodeRef<'_>,
    area: Rect,
    extra: u64,
    style: &Style,
) -> (Vec<u64>, Option<Vec<bool>>) {
    let sizes: Vec<u64> = folder.children().map(|child| child.size()).collect();
    let total = sizes.iter().sum::<u64>().saturating_add(extra);
    let small = small_children(&sizes, total, area.w * area.h, style.group_below);
    (sizes, small)
}

/// [`place_children`] with the split already made.
#[expect(
    clippy::too_many_arguments,
    reason = "the recursion's whole state; a struct for one private call adds nothing"
)]
fn place_split(
    folder: NodeRef<'_>,
    area: Rect,
    extra: u64,
    depth: u16,
    parent: u32,
    style: &Style,
    tiles: &mut Vec<Tile>,
    sizes: &[u64],
    small: Option<Vec<bool>>,
) -> Option<Rect> {
    let mut layout_sizes = Vec::with_capacity(sizes.len() + 2);
    let mut singles = Vec::with_capacity(sizes.len());
    for (index, &size) in sizes.iter().enumerate() {
        if !small.as_ref().is_some_and(|small| small[index]) {
            singles.push(index);
            layout_sizes.push(size);
        }
    }
    let group = small.map(|small| {
        let members = || (0..sizes.len()).filter(|&index| small[index]);
        let bytes: u64 = members().map(|index| sizes[index]).sum();
        let count = u32::try_from(members().count()).unwrap_or(u32::MAX);
        let folders = members()
            .filter(|&index| folder.child(index).is_some_and(|c| c.is_dir()))
            .count();
        let largest = members().max_by_key(|&index| sizes[index]).unwrap_or(0);
        layout_sizes.push(bytes);
        let to_u32 = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
        (count, bytes, to_u32(folders), to_u32(largest))
    });
    if extra > 0 {
        layout_sizes.push(extra);
    }
    let rects = layout::squarify(&layout_sizes, area);
    for (&index, &rect) in singles.iter().zip(&rects) {
        if let Some(child) = folder.child(index) {
            place(child, index, rect, depth, parent, style, tiles);
        }
    }
    let mut rest = rects.iter().skip(singles.len());
    if let Some((count, bytes, folders, largest)) = group
        && let Some(&rect) = rest.next()
    {
        let rect = snap(rect);
        if rect.w >= style.min_side && rect.h >= style.min_side {
            tiles.push(Tile {
                rect,
                kind: TileKind::Group { folders, largest },
                depth,
                parent,
                child: count,
                bytes,
                header: false,
                open: false,
            });
        }
    }
    if extra > 0 {
        rest.next().copied()
    } else {
        None
    }
}

/// Which children are too small to draw one by one: those whose share of
/// `area` (by size, out of `total` bytes) is under `below` square pixels.
/// `None` unless at least two are, since one alone gains nothing from a
/// group. Decided by size, not position, so it holds even where a delete
/// has left siblings out of size order.
fn small_children(sizes: &[u64], total: u64, area: f32, below: f32) -> Option<Vec<bool>> {
    let per_byte = if total > 0 {
        f64::from(area) / total as f64
    } else {
        0.0
    };
    let small: Vec<bool> = sizes
        .iter()
        .map(|&size| (size as f64 * per_byte) < f64::from(below))
        .collect();
    (small.iter().filter(|&&small| small).count() >= 2).then_some(small)
}

fn place(
    node: NodeRef<'_>,
    child: usize,
    rect: Rect,
    depth: u16,
    parent: u32,
    style: &Style,
    tiles: &mut Vec<Tile>,
) {
    let rect = snap(rect);
    if rect.w < style.min_side || rect.h < style.min_side {
        return;
    }
    let (Ok(index), Ok(child)) = (u32::try_from(tiles.len()), u32::try_from(child)) else {
        return;
    };
    let mut tile = Tile {
        rect,
        kind: if node.is_dir() {
            TileKind::Dir
        } else {
            TileKind::File
        },
        depth,
        parent,
        child,
        bytes: node.size(),
        header: false,
        open: false,
    };
    if !node.is_dir() {
        tiles.push(tile);
        return;
    }
    let content = content_rect(rect, style);
    let roomy =
        node.child_count() > 0 && content.w >= style.min_content && content.h >= style.min_content;
    // A folder whose contents would all be too small to draw stays closed:
    // its own name and size say more than one tile of "N small items".
    let (sizes, small) = if roomy {
        split(node, content, 0, style)
    } else {
        (Vec::new(), None)
    };
    let all_small = small.as_ref().is_some_and(|small| small.iter().all(|&s| s));
    tile.open = roomy && !all_small;
    tile.header = tile.open || (rect.h >= style.header && rect.w >= style.header);
    tiles.push(tile);
    if !tile.open {
        return;
    }
    place_split(
        node,
        content,
        0,
        depth.saturating_add(1),
        index,
        style,
        tiles,
        &sizes,
        small,
    );
}

/// The area inside a folder tile where its contents go: below the title
/// strip, inset on the other three sides.
#[must_use]
pub fn content_rect(rect: Rect, style: &Style) -> Rect {
    Rect {
        x: rect.x + style.inset,
        y: rect.y + style.header,
        w: (rect.w - 2.0 * style.inset).max(0.0),
        h: (rect.h - style.header - style.inset).max(0.0),
    }
}

/// Round a rectangle's edges to whole pixels.
fn snap(rect: Rect) -> Rect {
    let x0 = rect.x.round();
    let y0 = rect.y.round();
    let x1 = (rect.x + rect.w).round();
    let y1 = (rect.y + rect.h).round();
    Rect {
        x: x0,
        y: y0,
        w: (x1 - x0).max(0.0),
        h: (y1 - y0).max(0.0),
    }
}

/// The deepest tile under a point.
#[must_use]
pub fn hit(tiles: &[Tile], x: f32, y: f32) -> Option<usize> {
    tiles.iter().rposition(|tile| tile.contains(x, y))
}

/// Index path, relative to the laid-out root, of the node a tile shows;
/// `None` for the free-space tile and groups.
#[must_use]
pub fn path_of(tiles: &[Tile], index: usize) -> Option<Vec<usize>> {
    let mut path = Vec::new();
    let mut tile = tiles.get(index)?;
    if !tile.kind.is_node() {
        return None;
    }
    loop {
        path.push(tile.child as usize);
        if tile.parent == NO_PARENT {
            break;
        }
        tile = tiles.get(tile.parent as usize)?;
    }
    path.reverse();
    Some(path)
}

/// The folder a double-click on tile `index` opens, as an index path
/// relative to the laid-out root (empty: the laid-out root itself): a
/// folder opens itself; a file or a group opens the folder holding it.
/// `None` for the free-space tile.
#[must_use]
pub fn folder_of(tiles: &[Tile], index: usize) -> Option<Vec<usize>> {
    let tile = tiles.get(index)?;
    match tile.kind {
        TileKind::Dir => path_of(tiles, index),
        TileKind::File | TileKind::Group { .. } if tile.parent == NO_PARENT => Some(Vec::new()),
        TileKind::File | TileKind::Group { .. } => path_of(tiles, tile.parent as usize),
        TileKind::FreeSpace => None,
    }
}

/// An arrow key's direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// The tile an arrow key moves the selection to from tile `index`: the
/// nearest file or folder in the same folder that lies entirely beyond
/// it in `direction`, preferring ones level with it (overlapping it across
/// the direction of travel). `None` at the edge of the folder.
#[must_use]
pub fn neighbour(tiles: &[Tile], index: usize, direction: Direction) -> Option<usize> {
    let from = tiles.get(index)?.rect;
    let (from_right, from_bottom) = (from.x + from.w, from.y + from.h);
    let centre = |r: Rect| (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let (cx, cy) = centre(from);
    let parent = tiles[index].parent;
    tiles
        .iter()
        .enumerate()
        .filter(|&(other, tile)| other != index && tile.parent == parent && tile.kind.is_node())
        .filter_map(|(other, tile)| {
            let r = tile.rect;
            let (tx, ty) = centre(r);
            let level_x = r.x < from_right && r.x + r.w > from.x;
            let level_y = r.y < from_bottom && r.y + r.h > from.y;
            // Gap along the direction, offset across it, and whether level.
            let (gap, across, level) = match direction {
                Direction::Right if r.x >= from_right - 0.5 => (r.x - from_right, ty - cy, level_y),
                Direction::Left if r.x + r.w <= from.x + 0.5 => {
                    (from.x - (r.x + r.w), ty - cy, level_y)
                }
                Direction::Down if r.y >= from_bottom - 0.5 => {
                    (r.y - from_bottom, tx - cx, level_x)
                }
                Direction::Up if r.y + r.h <= from.y + 0.5 => {
                    (from.y - (r.y + r.h), tx - cx, level_x)
                }
                _ => return None,
            };
            Some((other, !level, gap.max(0.0) + across.abs()))
        })
        .min_by(|a, b| a.1.cmp(&b.1).then(a.2.total_cmp(&b.2)))
        .map(|(other, ..)| other)
}

/// The tile showing the node at `path` (relative to the laid-out root), if
/// that node is drawn.
#[must_use]
pub fn find(tiles: &[Tile], path: &[usize]) -> Option<usize> {
    let mut parent = NO_PARENT;
    let mut start = 0;
    let mut found = None;
    for &child in path {
        let child = u32::try_from(child).ok()?;
        let index = (start..tiles.len()).find(|&index| {
            let tile = &tiles[index];
            tile.parent == parent && tile.child == child && tile.kind.is_node()
        })?;
        found = Some(index);
        parent = u32::try_from(index).ok()?;
        start = index + 1;
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{Node, Tree};

    const AREA: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 800.0,
        h: 500.0,
    };

    /// Lay out a test tree.
    fn lay(root: &Node, area: Rect, free_space: Option<u64>, style: &Style) -> Vec<Tile> {
        layout(Tree::from(root.clone()).root(), area, free_space, style)
    }

    fn file(name: &str, size: u64) -> Node {
        Node::file(name, size, None)
    }

    fn dir(name: &str, children: Vec<Node>) -> Node {
        Node::dir(name, children, None)
    }

    /// A drive-shaped tree: a few big folders several levels deep, many
    /// small files, an empty folder, and a zero-byte file.
    fn sample() -> Node {
        let many: Vec<Node> = (0..40).map(|i| file(&format!("f{i}"), 1_000 + i)).collect();
        dir(
            "C:\\",
            vec![
                dir(
                    "Users",
                    vec![
                        dir(
                            "User",
                            vec![
                                dir(
                                    "AppData",
                                    vec![dir("Local", vec![file("docker.vhdx", 900_000)])],
                                ),
                                file("ntuser.dat", 50_000),
                            ],
                        ),
                        dir("Public", many),
                    ],
                ),
                dir(
                    "Windows",
                    vec![file("explorer.exe", 300_000), dir("Temp", Vec::new())],
                ),
                file("pagefile.sys", 400_000),
                file("empty.txt", 0),
            ],
        )
    }

    fn inside(inner: Rect, outer: Rect) -> bool {
        inner.x >= outer.x
            && inner.y >= outer.y
            && inner.x + inner.w <= outer.x + outer.w
            && inner.y + inner.h <= outer.y + outer.h
    }

    fn overlap(a: Rect, b: Rect) -> bool {
        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
    }

    #[test]
    fn top_level_tiles_tile_the_area() {
        let tiles = lay(&sample(), AREA, Some(250_000), &Style::default());
        let top: Vec<&Tile> = tiles.iter().filter(|t| t.parent == NO_PARENT).collect();
        let covered: f32 = top.iter().map(|t| t.rect.area()).sum();
        assert!(
            (covered - AREA.area()).abs() <= AREA.w + AREA.h,
            "{covered}"
        );
        for (i, a) in top.iter().enumerate() {
            assert!(inside(a.rect, AREA));
            for b in &top[i + 1..] {
                assert!(!overlap(a.rect, b.rect), "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn tiles_are_whole_pixels_and_never_too_small() {
        let style = Style::default();
        let tiles = lay(&sample(), AREA, None, &style);
        for tile in &tiles {
            let r = tile.rect;
            for v in [r.x, r.y, r.w, r.h] {
                assert_eq!(v, v.round(), "{tile:?}");
            }
            assert!(r.w >= style.min_side && r.h >= style.min_side);
        }
    }

    #[test]
    fn contents_stay_inside_their_folder_below_the_strip() {
        let style = Style::default();
        let tiles = lay(&sample(), AREA, None, &style);
        assert!(tiles.iter().any(|t| t.parent != NO_PARENT));
        for (index, tile) in tiles.iter().enumerate() {
            if tile.parent == NO_PARENT {
                assert_eq!(tile.depth, 0);
                continue;
            }
            let parent = &tiles[tile.parent as usize];
            assert!((tile.parent as usize) < index, "pre-order");
            assert!(parent.open && parent.header && parent.kind == TileKind::Dir);
            assert_eq!(tile.depth, parent.depth + 1);
            let content = content_rect(parent.rect, &style);
            // Snapping may move an edge by at most half a pixel.
            let slack = Rect {
                x: content.x - 0.5,
                y: content.y - 0.5,
                w: content.w + 1.0,
                h: content.h + 1.0,
            };
            assert!(inside(tile.rect, slack), "{tile:?} outside {content:?}");
        }
    }

    #[test]
    fn siblings_never_overlap() {
        let tiles = lay(&sample(), AREA, Some(1), &Style::default());
        for (i, a) in tiles.iter().enumerate() {
            for b in &tiles[i + 1..] {
                if a.parent == b.parent {
                    assert!(!overlap(a.rect, b.rect), "{a:?} overlaps {b:?}");
                }
            }
        }
    }

    #[test]
    fn deep_folders_open_several_levels_down() {
        let root = sample();
        let tiles = lay(&root, AREA, None, &Style::default());
        let deepest = tiles.iter().map(|t| t.depth).max().unwrap();
        assert!(deepest >= 4, "only {deepest} levels shown");
        let vhdx = find(&tiles, &[0, 0, 0, 0, 0]).expect("docker.vhdx is drawn");
        assert_eq!(tiles[vhdx].kind, TileKind::File);
        assert_eq!(tiles[vhdx].depth, 4);
    }

    #[test]
    fn paths_and_find_round_trip_to_the_tree() {
        let root = sample();
        let tiles = lay(&root, AREA, Some(10_000), &Style::default());
        for (index, tile) in tiles.iter().enumerate() {
            match path_of(&tiles, index) {
                None => assert_eq!(tile.kind, TileKind::FreeSpace),
                Some(path) => {
                    assert_eq!(find(&tiles, &path), Some(index));
                    let node = Tree::from(root.clone()).node_at(&path).unwrap().is_dir();
                    assert_eq!(node, tile.kind == TileKind::Dir);
                }
            }
        }
        assert_eq!(find(&tiles, &[]), None);
        assert_eq!(find(&tiles, &[99]), None);
    }

    #[test]
    fn hit_returns_the_deepest_tile() {
        let root = sample();
        let tiles = lay(&root, AREA, None, &Style::default());
        let vhdx = find(&tiles, &[0, 0, 0, 0, 0]).unwrap();
        let r = tiles[vhdx].rect;
        assert_eq!(hit(&tiles, r.x + r.w / 2.0, r.y + r.h / 2.0), Some(vhdx));
        // The title strip of `Users` belongs to `Users` itself.
        let users = find(&tiles, &[0]).unwrap();
        let u = tiles[users].rect;
        assert_eq!(hit(&tiles, u.x + 3.0, u.y + 3.0), Some(users));
        assert_eq!(hit(&tiles, -1.0, -1.0), None);
        assert_eq!(hit(&tiles, AREA.w, 0.0), None);
    }

    #[test]
    fn free_space_tile_only_when_asked() {
        let root = sample();
        let style = Style::default();
        let none = lay(&root, AREA, None, &style);
        let zero = lay(&root, AREA, Some(0), &style);
        assert!(none.iter().all(|t| t.kind != TileKind::FreeSpace));
        assert_eq!(none, zero);
        // Free space equal to everything scanned takes about half the area.
        let tiles = lay(&root, AREA, Some(root.size), &style);
        let free: Vec<&Tile> = tiles
            .iter()
            .filter(|t| t.kind == TileKind::FreeSpace)
            .collect();
        assert_eq!(free.len(), 1);
        let share = free[0].rect.area() / AREA.area();
        assert!((share - 0.5).abs() < 0.02, "{share}");
        assert_eq!(path_of(&tiles, tiles.len() - 1), None);
    }

    #[test]
    fn small_folders_stay_closed_and_tiny_tiles_are_dropped() {
        let root = sample();
        let style = Style::default();
        let small = Rect {
            x: 0.0,
            y: 0.0,
            w: 40.0,
            h: 24.0,
        };
        let tiles = lay(&root, small, None, &style);
        assert!(
            tiles
                .iter()
                .all(|t| !t.open || t.rect.h >= style.header + 6.0)
        );
        // 40 tiny files in `Public` cannot all get 2x2 pixels in this area.
        let public_files = tiles.iter().filter(|t| t.depth == 1).count();
        assert!(public_files < 40);
        // Empty folders and zero-byte files never open or show.
        let wide = lay(&root, AREA, None, &style);
        let temp = find(&wide, &[1, 1]);
        assert!(temp.is_none() || !wide[temp.unwrap()].open);
        assert_eq!(find(&wide, &[3]), None);
    }

    #[test]
    fn degenerate_inputs_yield_no_tiles() {
        let style = Style::default();
        assert!(lay(&dir("x", Vec::new()), AREA, None, &style).is_empty());
        assert!(lay(&dir("x", vec![file("z", 0)]), AREA, None, &style).is_empty());
        let flat = Rect {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 100.0,
        };
        assert!(lay(&sample(), flat, Some(5), &style).is_empty());
    }

    #[test]
    fn density_sets_how_small_a_tile_may_be() {
        assert_eq!(
            Style::with_density(1.0, Density::Detailed).group_below,
            64.0
        );
        assert_eq!(
            Style::with_density(1.0, Density::Balanced).group_below,
            900.0
        );
        assert_eq!(
            Style::with_density(2.0, Density::Simple).group_below,
            9216.0
        );
        assert_eq!(
            Style::for_scale(1.0),
            Style::with_density(1.0, Density::Detailed)
        );
        for density in Density::ALL {
            assert_eq!(Density::parse(density.code()), Some(density));
        }
        assert_eq!(Density::default(), Density::Balanced);
        // Lower density means more grouping: fewer tiles, never more.
        let mut children: Vec<Node> = (0..400)
            .map(|i| file(&format!("f{i}"), 1_000 + i))
            .collect();
        children.push(file("big", 2_000_000));
        let root = dir("r", children);
        let count = |density| lay(&root, AREA, None, &Style::with_density(1.0, density)).len();
        assert!(count(Density::Detailed) > count(Density::Balanced));
        assert!(count(Density::Balanced) >= count(Density::Simple));
    }

    #[test]
    fn a_folder_of_only_small_items_stays_closed() {
        // `crowd` holds 500 tiny files and nothing else: at Balanced density
        // its tile is big enough to open, but every child would be grouped,
        // so it shows its name and size instead of one "500 small files".
        let crowd = dir(
            "crowd",
            (0..500).map(|i| file(&format!("c{i}"), 10)).collect(),
        );
        let mixed = dir(
            "mixed",
            std::iter::once(file("large", 4_000))
                .chain((0..500).map(|i| file(&format!("m{i}"), 10)))
                .collect(),
        );
        let root = dir("r", vec![crowd, mixed]);
        let style = Style::with_density(1.0, Density::Balanced);
        let tiles = lay(&root, AREA, None, &style);
        let by_name = |name: &str| {
            let tree = Tree::from(root.clone());
            let index = tree
                .root()
                .children()
                .position(|c| c.name() == name)
                .unwrap();
            tiles[find(&tiles, &[index]).unwrap()]
        };
        let crowd = by_name("crowd");
        assert!(!crowd.open && crowd.header, "closed, but named");
        let mixed = by_name("mixed");
        assert!(
            mixed.open,
            "a folder with one item worth drawing still opens"
        );
        assert!(
            tiles
                .iter()
                .all(|t| t.parent == NO_PARENT || tiles[t.parent as usize].open)
        );
    }

    #[test]
    fn groups_know_their_folders_and_largest_member() {
        let mut children = vec![file("big", 10_000_000)];
        children.extend((0..300).map(|i| file(&format!("f{i}"), 1_000)));
        children.extend((0..200).map(|i| dir(&format!("d{i}"), vec![file("x", 1_500)])));
        let tree = Tree::from(dir("r", children));
        let style = Style::with_density(1.0, Density::Balanced);
        let tiles = layout(tree.root(), AREA, None, &style);
        let group = tiles
            .iter()
            .find(|t| matches!(t.kind, TileKind::Group { .. }))
            .unwrap();
        let TileKind::Group { folders, largest } = group.kind else {
            unreachable!()
        };
        assert_eq!(group.child, 500);
        assert_eq!(folders, 200);
        // A 1,500-byte folder is the biggest of the small items.
        let biggest = tree.root().child(largest as usize).unwrap();
        assert!(biggest.is_dir() && biggest.size() == 1_500);
    }

    #[test]
    fn style_scales_with_the_display() {
        let one = Style::for_scale(1.0);
        let two = Style::for_scale(2.0);
        assert_eq!(one.header, 14.0);
        assert_eq!(two.header, 28.0);
        assert_eq!(Style::for_scale(f32::NAN), one);
        assert_eq!(Style::for_scale(0.0), one);
    }

    #[test]
    fn tiny_children_share_one_group_tile() {
        // One big file and 3,000 100-byte files: each small file would get
        // about 30 square pixels, under 8x8, so together they make one group
        // with a fifth of the map.
        let mut children = vec![file("big", 1_000_000)];
        children.extend((0..3000).map(|i| file(&format!("t{i}"), 100)));
        children.push(dir("tiny-dir", vec![file("x", 100)]));
        let root = dir("r", children);
        let tiles = lay(&root, AREA, None, &Style::default());
        let groups: Vec<&Tile> = tiles
            .iter()
            .filter(|t| matches!(t.kind, TileKind::Group { .. }))
            .collect();
        assert_eq!(groups.len(), 1);
        let group = groups[0];
        assert_eq!(group.child, 3001, "3,000 files and the tiny folder");
        assert_eq!(group.bytes, 300_100);
        assert!(group.rect.area() > 0.2 * AREA.area());
        // The tiny folder is the one folder among the small items.
        assert!(matches!(group.kind, TileKind::Group { folders: 1, .. }));
        assert_eq!(group.parent, NO_PARENT);
        // No tiny tile is drawn on its own, and the group has no path.
        assert_eq!(tiles.iter().filter(|t| t.kind.is_node()).count(), 1);
        let index = tiles.iter().position(|t| std::ptr::eq(t, group)).unwrap();
        assert_eq!(path_of(&tiles, index), None);
        assert_eq!(find(&tiles, &[3001]), None);
        // A double-click on the group opens the folder holding it.
        assert_eq!(folder_of(&tiles, index), Some(Vec::new()));
    }

    #[test]
    fn one_tiny_child_is_not_a_group() {
        let root = dir("r", vec![file("big", 1_000_000), file("tiny", 1)]);
        let tiles = lay(&root, AREA, None, &Style::default());
        assert!(tiles.iter().all(|t| t.kind.is_node()));
    }

    #[test]
    fn groups_form_inside_folders() {
        let mut inner: Vec<Node> = (0..2000).map(|i| file(&format!("t{i}"), 100)).collect();
        inner.push(file("big", 500_000));
        let root = dir("r", vec![dir("docs", inner), file("other", 500_000)]);
        let tiles = lay(&root, AREA, None, &Style::default());
        let docs = find(&tiles, &[0]).unwrap();
        let group = tiles
            .iter()
            .position(|t| matches!(t.kind, TileKind::Group { .. }))
            .unwrap();
        assert_eq!(tiles[group].parent as usize, docs);
        assert_eq!(tiles[group].child, 2000);
        assert!(matches!(
            tiles[group].kind,
            TileKind::Group { folders: 0, .. }
        ));
        assert_eq!(folder_of(&tiles, group), Some(vec![0]));
        // The big file keeps its own tile.
        let files_in_docs = tiles
            .iter()
            .filter(|t| t.parent as usize == docs && t.kind == TileKind::File)
            .count();
        assert_eq!(files_in_docs, 1);
    }

    #[test]
    fn small_children_are_found_by_size_not_position() {
        // As after a delete shrank a folder: sizes out of order.
        assert_eq!(
            small_children(&[1, 1000, 1, 1000], 2002, 2002.0, 64.0),
            Some(vec![true, false, true, false])
        );
        assert_eq!(small_children(&[1, 1000], 1001, 1001.0, 64.0), None);
        // Nothing to share when everything is empty.
        assert_eq!(
            small_children(&[0, 0, 0], 0, 100.0, 64.0),
            Some(vec![true; 3])
        );
    }

    #[test]
    fn folder_of_opens_folders_and_the_folder_of_files() {
        let root = sample();
        let tiles = lay(&root, AREA, None, &Style::default());
        let folder = find(&tiles, &[0]).unwrap();
        assert_eq!(folder_of(&tiles, folder), Some(vec![0]));
        let inner = find(&tiles, &[0, 0]).unwrap();
        let expected = if tiles[inner].kind == TileKind::Dir {
            vec![0, 0]
        } else {
            vec![0]
        };
        assert_eq!(folder_of(&tiles, inner), Some(expected));
    }

    #[test]
    fn arrows_move_between_neighbouring_siblings() {
        // Four equal files in a 200x200 area: a 2x2 grid.
        let root = dir(
            "r",
            vec![
                file("a", 100),
                file("b", 100),
                file("c", 100),
                file("d", 100),
            ],
        );
        let area = Rect {
            x: 0.0,
            y: 0.0,
            w: 200.0,
            h: 200.0,
        };
        let tiles = lay(&root, area, None, &Style::default());
        assert_eq!(tiles.len(), 4);
        let at = |x: f32, y: f32| hit(&tiles, x, y).unwrap();
        let (top_left, top_right) = (at(50.0, 50.0), at(150.0, 50.0));
        let (bottom_left, bottom_right) = (at(50.0, 150.0), at(150.0, 150.0));
        assert_eq!(
            neighbour(&tiles, top_left, Direction::Right),
            Some(top_right)
        );
        assert_eq!(
            neighbour(&tiles, top_left, Direction::Down),
            Some(bottom_left)
        );
        assert_eq!(
            neighbour(&tiles, bottom_right, Direction::Left),
            Some(bottom_left)
        );
        assert_eq!(
            neighbour(&tiles, bottom_right, Direction::Up),
            Some(top_right)
        );
        // At the edge there is nowhere to go.
        assert_eq!(neighbour(&tiles, top_left, Direction::Left), None);
        assert_eq!(neighbour(&tiles, top_left, Direction::Up), None);
        // Only siblings count: a folder's contents are not its neighbours.
        let nested = lay(&sample(), AREA, None, &Style::default());
        for index in 0..nested.len() {
            for direction in [
                Direction::Left,
                Direction::Right,
                Direction::Up,
                Direction::Down,
            ] {
                if let Some(next) = neighbour(&nested, index, direction) {
                    assert_eq!(nested[next].parent, nested[index].parent);
                    assert!(nested[next].kind.is_node());
                }
            }
        }
    }

    /// Exact tiles for a fixed tree: any change to nesting, insets, header
    /// rules, or snapping moves these numbers. Derived by hand: `a` and `b`
    /// split 200x100 into two 100x100 columns; `a`'s contents get the 96x84
    /// area below its 14-pixel strip (2-pixel inset), where `a1` (values
    /// 6048 vs 2016) takes a 72-wide column and `a2` the 24-wide rest.
    #[test]
    fn exact_tiles_for_a_fixed_tree() {
        let root = dir(
            "r",
            vec![
                dir("a", vec![file("a1", 300), file("a2", 100)]),
                file("b", 400),
            ],
        );
        let area = Rect {
            x: 0.0,
            y: 0.0,
            w: 200.0,
            h: 100.0,
        };
        let tiles = lay(&root, area, None, &Style::default());
        let rects: Vec<(TileKind, u32, [f32; 4])> = tiles
            .iter()
            .map(|t| (t.kind, t.parent, [t.rect.x, t.rect.y, t.rect.w, t.rect.h]))
            .collect();
        assert_eq!(
            rects,
            vec![
                (TileKind::Dir, NO_PARENT, [0.0, 0.0, 100.0, 100.0]),
                (TileKind::File, 0, [2.0, 14.0, 72.0, 84.0]),
                (TileKind::File, 0, [74.0, 14.0, 24.0, 84.0]),
                (TileKind::File, NO_PARENT, [100.0, 0.0, 100.0, 100.0]),
            ]
        );
        assert!(tiles[0].header && tiles[0].open);
    }
}
