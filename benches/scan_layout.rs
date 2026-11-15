//! Gungraun benches for the Topos hot paths: the squarified layout, the
//! nested treemap built from it, the painted map (layout, pixels, and
//! captions) the window copies to the screen, and the last scan written to
//! and read back from its cache file.
//!
//! Run with `cargo bench` (needs gungraun-runner and Valgrind). Each bench
//! runs under Callgrind (instructions) and DHAT (heap bytes and blocks).
//! Both count work instead of timing it, so a change can be compared with
//! the code before it: CI fails a pull request that makes any bench need
//! over 1% more instructions, or allocate or peak at over 2% more heap (see
//! CONTRIBUTING.md).
//!
//! There is deliberately no scan bench here. Gungraun collects only the
//! benchmark function's own thread by default, so a scan that runs on a
//! rayon pool reads as pool setup plus waiting; and Callgrind cannot see the
//! cost that dominates on Windows anyway (directory enumeration in the
//! kernel). Measure scans with `examples/scan_timing.rs` on the target.

use std::hint::black_box;

use gungraun::{Dhat, LibraryBenchmarkConfig, library_benchmark, library_benchmark_group, main};
use topos::layout::{self, Rect};
use topos::tree::{Node, Tree};
// Not imported by name: the bench group below is a module named `treemap`.
use topos::scanner::ScanStats;
use topos::treemap::Style;
use topos::view::ViewKey;

const AREA: Rect = Rect {
    x: 0.0,
    y: 0.0,
    w: 1024.0,
    h: 700.0,
};

/// Deterministic skewed sizes shaped like a real drive: a few huge entries,
/// many small ones.
fn synthetic_sizes(count: usize) -> Vec<u64> {
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    (0..count)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            1 + state % 40_000_000
        })
        .collect()
}

/// A drive-shaped tree: 12 top folders, 12 subfolders each, 40 files in
/// every subfolder (5,760 files), with the same skewed sizes.
fn synthetic_tree() -> Node {
    let mut sizes = synthetic_sizes(12 * 12 * 40).into_iter();
    let top: Vec<Node> = (0..12)
        .map(|t| {
            let subs: Vec<Node> = (0..12)
                .map(|s| {
                    let mut files: Vec<Node> = (0..40)
                        .map(|f| Node::file(format!("f{f}"), sizes.next().unwrap_or(1), None))
                        .collect();
                    files.sort_by_key(|file| std::cmp::Reverse(file.size));
                    Node::dir(format!("s{s}"), files, None)
                })
                .collect();
            Node::dir(format!("t{t}"), subs, None)
        })
        .collect();
    Node::dir("root", top, None)
}

/// A bigger tree for the cache: 20 top folders, 50 subfolders each, 100
/// files in every subfolder (100,000 files), so per-entry work dominates.
fn synthetic_cache_tree() -> Tree {
    let mut sizes = synthetic_sizes(20 * 50 * 100).into_iter();
    let top: Vec<Node> = (0..20)
        .map(|t| {
            let subs: Vec<Node> = (0..50)
                .map(|s| {
                    let files: Vec<Node> = (0..100)
                        .map(|f| {
                            let size = sizes.next().unwrap_or(1);
                            Node::file(format!("file-{f}.dat"), size, Some(1_700_000_000 + f))
                        })
                        .collect();
                    Node::dir(format!("folder-{s}"), files, Some(1_700_000_000))
                })
                .collect();
            Node::dir(format!("top-{t}"), subs, Some(1_700_000_000))
        })
        .collect();
    Tree::from(Node::dir("C:\\", top, None))
}

fn cache_bytes(tree: &Tree) -> Vec<u8> {
    let mut bytes = Vec::new();
    tree.write_to(&mut bytes)
        .expect("writing to a Vec cannot fail");
    bytes
}

#[library_benchmark]
#[bench::tiles_1k(synthetic_sizes(1_000))]
#[bench::tiles_5k(synthetic_sizes(5_000))]
fn bench_layout(sizes: Vec<u64>) -> Vec<Rect> {
    black_box(layout::squarify(black_box(&sizes), AREA))
}

// The whole nested map for a full-HD canvas, as the window lays it out.
#[library_benchmark]
#[bench::tree_5760_files(Tree::from(synthetic_tree()))]
fn bench_treemap(tree: Tree) -> usize {
    let area = Rect {
        x: 0.0,
        y: 0.0,
        w: 1920.0,
        h: 1080.0,
    };
    black_box(
        topos::treemap::layout(
            black_box(&tree).root(),
            area,
            Some(1 << 30),
            &Style::default(),
        )
        .len(),
    )
}

// What the window does once per layout: the nested map, every tile painted
// into a full-HD pixel buffer, and the captions placed.
#[library_benchmark]
#[bench::tree_5760_files(Tree::from(synthetic_tree()))]
fn bench_view(tree: Tree) -> usize {
    let key = ViewKey {
        generation: 1,
        zoom: Vec::new(),
        width: 1920,
        height: 1080,
        scale: 1.0,
        line: 14,
        free_space: Some(1 << 30),
        by_type: false,
        // The classic full detail, so numbers compare with earlier runs.
        density: topos::treemap::Density::Detailed,
        dark: false,
    };
    let (view, frame) =
        topos::view::build(black_box(&tree).root(), key, None, &ScanStats::default());
    black_box(view.tiles.len() + view.captions.len() + frame.bgra().len())
}

// Saving the last scan: the tree in its cache-file form.
#[library_benchmark]
#[bench::files_100k(synthetic_cache_tree())]
fn bench_cache_write(tree: Tree) -> usize {
    let mut bytes = Vec::new();
    black_box(&tree)
        .write_to(&mut bytes)
        .expect("writing to a Vec cannot fail");
    black_box(bytes.len())
}

// Starting from the last scan: the cache file's bytes read back as
// untrusted input, which is most of the time to the first map.
#[library_benchmark]
#[bench::files_100k(cache_bytes(&synthetic_cache_tree()))]
fn bench_cache_read(bytes: Vec<u8>) -> u64 {
    let tree = Tree::read_from(&mut black_box(bytes.as_slice())).expect("valid cache bytes");
    black_box(tree.root().size())
}

library_benchmark_group!(
    name = treemap;
    benchmarks = bench_layout, bench_treemap, bench_view
);

library_benchmark_group!(
    name = cache;
    benchmarks = bench_cache_write, bench_cache_read
);

main!(
    config = LibraryBenchmarkConfig::default().tool(Dhat::default());
    library_benchmark_groups = treemap, cache
);
