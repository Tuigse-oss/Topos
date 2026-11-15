//! Squarified treemap layout.
//!
//! [`squarify`] tiles a rectangle with the given sizes so that every tile's
//! area is proportional to its size and tiles stay as square as possible
//! (Bruls, Huizing, van Wijk). It is pure and deterministic: the same sizes
//! in the same order always produce the same tiles, which is what the unit
//! tests and the Gungraun bench pin.

/// A tile in the treemap, in the same coordinate space as the input area.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    /// Area of the tile; zero-sized tiles still occupy no space.
    #[must_use]
    pub fn area(self) -> f32 {
        self.w * self.h
    }
}

/// Lay `sizes` out inside `area`, preserving input order in the output.
/// Entries with a size of zero get a zero-area tile at the current cursor.
/// A zero total, or an empty input, yields no tiles.
#[must_use]
pub fn squarify(sizes: &[u64], area: Rect) -> Vec<Rect> {
    let total: u64 = sizes.iter().sum();
    if sizes.is_empty() || total == 0 || area.w <= 0.0 || area.h <= 0.0 {
        return Vec::new();
    }
    let scale = f64::from(area.w) * f64::from(area.h) / total as f64;

    // Stable descending order keeps big tiles together; the output is
    // re-sorted back to input order before returning. The comparator is a
    // total order (size descending, index ascending), so the unstable sort
    // yields the exact same permutation as the stable one without the merge
    // buffer. (A pairs-sort was tried here and measured no better.)
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_unstable_by(|&a, &b| sizes[b].cmp(&sizes[a]).then(a.cmp(&b)));
    let rows = form_rows(&order, sizes, scale, area);

    let mut tiles: Vec<Rect> = Vec::with_capacity(sizes.len());
    let mut rest = Rect { ..area };
    for mut row in rows {
        // `order` position and `tiles` position advance together, so tile k
        // belongs to order[k]; map them back to input order below.
        flush_row(&mut tiles, &mut rest, &mut row);
    }

    let mut by_input: Vec<Rect> = vec![area; sizes.len()];
    for (tile, &index) in tiles.iter().zip(order.iter()) {
        by_input[index] = *tile;
    }
    by_input
}

/// Split the descending `order` into greedy rows: an entry joins the current
/// row only while it does not worsen the row's worst aspect ratio. `rest`
/// is simulated exactly as [`squarify`] consumes it, so rows and tiles agree.
fn form_rows(order: &[usize], sizes: &[u64], scale: f64, area: Rect) -> Vec<Vec<(usize, f64)>> {
    let mut rows: Vec<Vec<(usize, f64)>> = Vec::new();
    let mut rest = area;
    let mut row: Vec<(usize, f64)> = Vec::new();
    // Running aggregates of the current row, updated in push order so the
    // sums match a fresh scan bit for bit while avoiding a rescan per item.
    let mut agg = RowAgg::empty();
    for &index in order {
        let value = sizes[index] as f64 * scale;
        let full = agg.add(value);
        if agg.len > 0 && row_worsened(&agg, &full, shorter_side(rest)) {
            shrink(&mut rest, &row);
            rows.push(std::mem::take(&mut row));
            row.push((index, value));
            agg = RowAgg::single(value);
        } else {
            row.push((index, value));
            agg = full;
        }
    }
    if !row.is_empty() {
        shrink(&mut rest, &row);
        rows.push(row);
    }
    rows
}

/// Running sum/max/min of the row being formed. `add` folds in push order,
/// so these aggregates equal a fresh scan of the row exactly.
#[derive(Debug, Clone, Copy)]
struct RowAgg {
    len: usize,
    sum: f64,
    max: f64,
    min: f64,
}

impl RowAgg {
    fn empty() -> Self {
        Self {
            len: 0,
            sum: 0.0,
            max: 0.0,
            min: f64::INFINITY,
        }
    }

    fn single(value: f64) -> Self {
        Self::empty().add(value)
    }

    fn add(&self, value: f64) -> Self {
        Self {
            len: self.len + 1,
            sum: self.sum + value,
            max: self.max.max(value),
            min: self.min.min(value),
        }
    }

    fn worst(&self, side: f64) -> f64 {
        worst_value(self.sum, self.max, self.min, side)
    }
}

/// Extending the row worsens its worst aspect ratio: the break condition.
fn row_worsened(prev: &RowAgg, full: &RowAgg, side: f64) -> bool {
    prev.worst(side) < full.worst(side)
}

/// Shrink `rest` by the row's thickness, mirroring [`flush_row`]; factored
/// out so row formation and tile emission consume space identically. A row
/// runs along the shorter side, so it eats into the longer one.
fn shrink(rest: &mut Rect, row: &[(usize, f64)]) {
    let row_sum: f64 = row.iter().map(|(_, value)| *value).sum();
    if row_sum <= 0.0 {
        return;
    }
    if rest.w >= rest.h {
        let thickness = (row_sum / f64::from(rest.h)) as f32;
        rest.x += thickness;
        rest.w = (rest.w - thickness).max(0.0);
    } else {
        let thickness = (row_sum / f64::from(rest.w)) as f32;
        rest.y += thickness;
        rest.h = (rest.h - thickness).max(0.0);
    }
}

/// Shortest side of the remaining space; rows form along it, and the aspect
/// test in [`worst_value`] measures them against it.
fn shorter_side(rest: Rect) -> f64 {
    f64::from(rest.w.min(rest.h))
}

/// Worst aspect ratio of a candidate row laid along `side`. Test-only: the
/// tests rescan each row through this path to independently verify the
/// running aggregates in [`RowAgg`].
#[cfg(test)]
fn worst(values: impl Iterator<Item = f64>, side: f64) -> f64 {
    let mut sum = 0.0;
    let mut max = 0.0_f64;
    let mut min = f64::INFINITY;
    for value in values {
        sum += value;
        max = max.max(value);
        min = min.min(value);
    }
    worst_value(sum, max, min, side)
}

/// Worst aspect ratio from precomputed aggregates. One formula, two call
/// sites, so the running aggregates and a fresh scan always agree.
fn worst_value(sum: f64, max: f64, min: f64, side: f64) -> f64 {
    if sum <= 0.0 || min <= 0.0 || side <= 0.0 {
        return f64::INFINITY;
    }
    let side_sq = side * side;
    (side_sq * max / (sum * sum)).max(sum * sum / (side_sq * min))
}

/// Emit one row of tiles along the short side of `rest` (Bruls, Huizing, van
/// Wijk): in a wide space the row is a column at the left edge, in a tall
/// space a strip along the top. Shrinks `rest` to the leftover space. Always
/// drains `row`: every entry is emitted exactly once, so a flushed row can
/// never be re-emitted by a later row.
fn flush_row(tiles: &mut Vec<Rect>, rest: &mut Rect, row: &mut Vec<(usize, f64)>) {
    let row_sum: f64 = row.iter().map(|(_, value)| *value).sum();
    if row_sum <= 0.0 {
        for _ in row.iter() {
            tiles.push(Rect {
                x: rest.x,
                y: rest.y,
                w: 0.0,
                h: 0.0,
            });
        }
        row.clear();
        return;
    }
    let column = rest.w >= rest.h;
    let thickness = if column {
        (row_sum / f64::from(rest.h)) as f32
    } else {
        (row_sum / f64::from(rest.w)) as f32
    };
    let mut cursor = 0.0_f32;
    // `row` arrives in `order` (descending) sequence because `squarify`
    // pushes tiles row by row; remember positions by pushing in arrival order
    // and let the caller map them back.
    for (_, value) in row.iter() {
        let length = if column {
            (*value / row_sum * f64::from(rest.h)) as f32
        } else {
            (*value / row_sum * f64::from(rest.w)) as f32
        };
        tiles.push(if column {
            Rect {
                x: rest.x,
                y: rest.y + cursor,
                w: thickness,
                h: length,
            }
        } else {
            Rect {
                x: rest.x + cursor,
                y: rest.y,
                w: length,
                h: thickness,
            }
        });
        cursor += length;
    }
    if column {
        rest.x += thickness;
        rest.w = (rest.w - thickness).max(0.0);
    } else {
        rest.y += thickness;
        rest.h = (rest.h - thickness).max(0.0);
    }
    row.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 600.0,
        h: 400.0,
    };

    #[test]
    fn tiles_cover_the_container_proportionally() {
        let sizes = vec![600_u64, 300, 200, 100];
        let tiles = squarify(&sizes, AREA);
        assert_eq!(tiles.len(), sizes.len());
        let total: u64 = sizes.iter().sum();
        let area_total = AREA.area();
        for (tile, size) in tiles.iter().zip(sizes.iter()) {
            let expected = *size as f32 / total as f32 * area_total;
            assert!(
                (tile.area() - expected).abs() < expected * 0.02 + 1.0,
                "tile {tile:?} should cover ~{expected}"
            );
            assert!(tile.x >= -0.01 && tile.y >= -0.01);
            assert!(tile.x + tile.w <= AREA.w + 1.0);
            assert!(tile.y + tile.h <= AREA.h + 1.0);
        }
    }

    #[test]
    fn output_preserves_input_order() {
        // Ascending input must still map tile i to size i.
        let sizes = vec![50_u64, 400, 150, 900, 25];
        let tiles = squarify(&sizes, AREA);
        let total: u64 = sizes.iter().sum();
        for (tile, size) in tiles.iter().zip(sizes.iter()) {
            let expected = *size as f32 / total as f32 * AREA.area();
            assert!((tile.area() - expected).abs() < expected * 0.02 + 1.0);
        }
    }

    #[test]
    fn degenerate_inputs_yield_no_tiles() {
        assert!(squarify(&[], AREA).is_empty());
        assert!(squarify(&[0, 0, 0], AREA).is_empty());
        assert!(
            squarify(
                &[10, 20],
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 0.0,
                    h: 5.0
                }
            )
            .is_empty()
        );
    }

    #[test]
    fn single_entry_fills_the_container() {
        let tiles = squarify(&[42], AREA);
        assert_eq!(tiles, vec![AREA]);
    }

    #[test]
    fn tiles_stay_inside_the_container() {
        let sizes: Vec<u64> = (1..=50).map(|i| i * 37).collect();
        let tiles = squarify(&sizes, AREA);
        // Full coverage: no gaps, no overlaps, no overflow past the container.
        let covered: f32 = tiles.iter().map(|tile| tile.area()).sum();
        assert!((covered - AREA.area()).abs() < AREA.area() * 0.01);
        assert!(
            tiles
                .iter()
                .all(|tile| tile.x + tile.w <= AREA.w + 1.0 && tile.y + tile.h <= AREA.h + 1.0)
        );
    }

    #[test]
    fn rows_are_locally_optimal() {
        // The greedy invariant: a row never continues past the point where
        // one more entry would worsen its worst aspect ratio. Check it over
        // several data shapes, simulating the shrinking space exactly as
        // the layout consumes it.
        let shapes: Vec<Vec<u64>> = vec![
            vec![600, 300, 200, 100],
            (1..=50).map(|i| i * 37).collect(),
            vec![1_000_000, 999_999, 999_998, 5, 4, 3, 2, 1],
            vec![7; 25],
        ];
        for sizes in &shapes {
            let total: u64 = sizes.iter().sum();
            let scale = f64::from(AREA.w) * f64::from(AREA.h) / total as f64;
            let mut order: Vec<usize> = (0..sizes.len()).collect();
            order.sort_by(|&a, &b| sizes[b].cmp(&sizes[a]).then(a.cmp(&b)));
            let rows = form_rows(&order, sizes, scale, AREA);
            // Every entry appears exactly once, in descending order.
            let flat: Vec<usize> = rows
                .iter()
                .flat_map(|row| row.iter().map(|(i, _)| *i))
                .collect();
            assert_eq!(flat, order);
            // Extending any non-final row with the next row's first entry
            // would worsen its worst aspect ratio: hence the break.
            let mut rest = AREA;
            for pair in rows.windows(2) {
                let (head, next) = (&pair[0], &pair[1]);
                let side = shorter_side(rest);
                let base = worst(head.iter().map(|(_, v)| *v), side);
                let mut extended: Vec<f64> = head.iter().map(|(_, v)| *v).collect();
                extended.push(next[0].1);
                let worse = worst(extended.into_iter(), side);
                assert!(
                    base < worse,
                    "row {head:?} should not have continued: {base} vs {worse}"
                );
                shrink(&mut rest, head);
            }
        }
    }

    /// Worst aspect ratio (long side over short side) of a tile.
    fn aspect(tile: &Rect) -> f32 {
        tile.w.max(tile.h) / tile.w.min(tile.h)
    }

    #[test]
    fn rows_run_along_the_shorter_side() {
        // In a wide space the first row is a full-height column at the left;
        // in a tall one, a full-width strip at the top.
        let wide = squarify(&[600, 300, 200, 100], AREA);
        assert_eq!((wide[0].x, wide[0].y, wide[0].h), (0.0, 0.0, AREA.h));
        let tall_area = Rect {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 600.0,
        };
        let tall = squarify(&[600, 300, 200, 100], tall_area);
        assert_eq!((tall[0].x, tall[0].y, tall[0].w), (0.0, 0.0, tall_area.w));
    }

    #[test]
    fn tiles_stay_close_to_square() {
        // Squarified layouts keep skewed, drive-like inputs near square; the
        // old strip layout reached 18:1 on the four-tile case alone.
        let four = squarify(&[600, 300, 200, 100], AREA);
        assert!(four.iter().all(|tile| aspect(tile) <= 2.0), "{four:?}");
        let sizes: Vec<u64> = (1..=50).map(|i| i * 37).collect();
        let tiles = squarify(&sizes, AREA);
        let mean = tiles.iter().map(aspect).sum::<f32>() / tiles.len() as f32;
        assert!(mean < 2.0, "mean aspect {mean}");
    }

    #[test]
    fn deterministic_across_runs() {
        let sizes = vec![900_u64, 400, 150, 50, 25, 700, 320];
        assert_eq!(squarify(&sizes, AREA), squarify(&sizes, AREA));
    }

    /// Exact tiles for fixed inputs. Any change to row formation, aspect
    /// math, or float operation order moves these bits, so an optimization
    /// that claims identical behavior must keep this test green untouched.
    ///
    /// The first case is derived by hand from the algorithm (values scale by
    /// 200 to 120000/60000/40000/20000): a 300-wide column for the largest,
    /// then a 200-tall strip, a 200x200 square, and a 100x200 column — every
    /// tile within 2:1. Until 2026-11-06 this pinned four full-width strips
    /// (up to 18:1), because rows were scored against the shorter side but
    /// laid along the longer one.
    #[test]
    fn exact_tiles_for_fixed_inputs() {
        assert_eq!(
            squarify(&[600, 300, 200, 100], AREA),
            vec![
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 300.0,
                    h: 400.0
                },
                Rect {
                    x: 300.0,
                    y: 0.0,
                    w: 300.0,
                    h: 200.0
                },
                Rect {
                    x: 300.0,
                    y: 200.0,
                    w: 200.0,
                    h: 200.0
                },
                Rect {
                    x: 500.0,
                    y: 200.0,
                    w: 100.0,
                    h: 200.0
                },
            ]
        );
        // Cross-checked against an independent f64 reference implementation
        // (0, 210.526, 207.273, 189.474) on 2026-11-06.
        assert_eq!(
            squarify(&(1..=10).map(|i| i * 37).collect::<Vec<_>>(), AREA)[8],
            Rect {
                x: 0.0,
                y: 210.52632,
                w: 207.27272,
                h: 189.47368
            }
        );
    }
}
