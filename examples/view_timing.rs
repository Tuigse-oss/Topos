//! Wall-clock samples of what the window does once per layout — the nested
//! map, every tile painted into a pixel buffer, and the captions placed —
//! for a real scan on the machine that runs it.
//!
//! ```text
//! cargo run --release --example view_timing -- C:\ 1280 720 5
//! ```
//!
//! Scans once, then prints one `Instant`-timed sample per round. GDI's share
//! (one bitmap copy plus the caption text) is not included. Samples for
//! before/after comparison on one machine, not statistical claims.

use std::path::{Path, PathBuf};
use std::time::Instant;

use topos::scanner;
use topos::view::{self, ViewKey};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next().map(PathBuf::from) else {
        eprintln!("usage: view_timing <PATH> [WIDTH] [HEIGHT] [ROUNDS]");
        std::process::exit(2);
    };
    let mut number = |default: u32| args.next().and_then(|v| v.parse().ok()).unwrap_or(default);
    let (width, height, rounds) = (number(1280), number(720), number(5));
    let scan = Instant::now();
    let result = match scanner::scan_root(&root) {
        Ok(result) => result,
        Err(err) => {
            eprintln!("scan of {} failed: {err}", root.display());
            std::process::exit(1);
        }
    };
    println!(
        "scanned {} files in {:.3} s",
        result.stats.files,
        scan.elapsed().as_secs_f64()
    );
    let drives = scanner::drives();
    let drive = drives
        .iter()
        .find(|drive| Path::new(&drive.mount) == root.as_path());
    for round in 0..rounds {
        let key = ViewKey {
            generation: 1,
            zoom: Vec::new(),
            width,
            height,
            scale: 1.0,
            line: 14,
            free_space: drive.map(|drive| drive.free_bytes),
            by_type: false,
            // The classic full detail, so numbers compare with earlier runs.
            density: topos::treemap::Density::Detailed,
            dark: false,
        };
        let start = Instant::now();
        let (view, frame) = view::build(result.root.root(), key, drive, &result.stats);
        println!(
            "round {round}: {:.2} ms  tiles={} captions={} bytes={}",
            start.elapsed().as_secs_f64() * 1000.0,
            view.tiles.len(),
            view.captions.len(),
            frame.bgra().len()
        );
    }
}
