//! Wall-clock scan samples on the machine that runs it — the cost Callgrind
//! cannot see (directory enumeration in the kernel and filesystem).
//!
//! ```text
//! cargo run --release --example scan_timing -- "C:\Program Files" 3
//! ```
//!
//! Prints one `Instant`-timed sample per round. The first round may pay for a
//! cold cache; later rounds are warm. These are samples for before/after
//! comparison on one machine, not statistical claims.

use std::path::PathBuf;
use std::time::Instant;

use topos::scanner;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next().map(PathBuf::from) else {
        eprintln!("usage: scan_timing <PATH> [ROUNDS]");
        std::process::exit(2);
    };
    let rounds: usize = args.next().and_then(|r| r.parse().ok()).unwrap_or(3);
    for round in 0..rounds {
        let start = Instant::now();
        match scanner::scan_root(&root) {
            Ok(result) => {
                let stats = result.stats;
                println!(
                    "round {round}: {:.3} s  files={} folders={} links={} skipped={} bytes={}",
                    start.elapsed().as_secs_f64(),
                    stats.files,
                    stats.folders,
                    stats.links,
                    stats.skipped,
                    stats.bytes
                );
            }
            Err(err) => {
                eprintln!("scan of {} failed: {err}", root.display());
                std::process::exit(1);
            }
        }
    }
}
