//! Topos: disk-space treemap visualizer.
//!
//! The library holds everything the window shows and does: the parallel
//! filesystem [`scanner`], the scanned [`tree`] and its edits, the pure
//! squarified [`layout`], the nested [`treemap`] built from it, the painted
//! map and its captions ([`view`]), the [`format`] of the map's text, and the
//! [`session`] that ties them together with background scans and deletes,
//! keeping each drive's last scan in the [`cache`] for an instant start.
//! The light and dark palettes live in [`theme`]; the program icon
//! ([`icon`]) is drawn in code from the map's colours, and About shows the
//! publisher's [`emblem`].
//! The Win32 shell in `src/win/` only draws that state and forwards input,
//! so the unit tests pin the behavior on any platform and the Gungraun
//! benches in `benches/scan_layout.rs` measure the real hot paths.

#![forbid(unsafe_code)]

pub mod cache;
pub mod emblem;
pub mod format;
pub mod icon;
pub mod layout;
pub mod scanner;
pub mod session;
pub mod settings;
pub mod theme;
pub mod tree;
pub mod treemap;
pub mod view;

#[cfg(test)]
mod notices_tests {
    /// Every crate the notices name is a version Cargo.lock still pins, and
    /// the standard library's notices are there. CI reruns
    /// `scripts/third-party-notices.py` and fails on any difference, which
    /// catches a newly linked crate.
    #[test]
    fn notices_name_locked_versions() {
        // A Windows checkout may turn LF into CRLF; the lookup below spans
        // two lines, so compare with LF line ends.
        let lock = include_str!("../Cargo.lock").replace("\r\n", "\n");
        let notices = include_str!("../THIRD-PARTY-NOTICES.txt");
        let mut named = 0;
        for line in notices.lines() {
            let Some((crate_version, _)) = line.split_once(" (license: ") else {
                continue;
            };
            let (name, version) = crate_version
                .split_once(' ')
                .expect("a notice heading is `name version (license: ...)`");
            let locked = format!("name = \"{name}\"\nversion = \"{version}\"\n");
            assert!(
                lock.contains(&locked),
                "{name} {version} is not in Cargo.lock; run scripts/third-party-notices.py"
            );
            named += 1;
        }
        assert!(named > 40, "only {named} crates named");
        assert!(notices.contains("The Rust Standard Library"));
    }
}
