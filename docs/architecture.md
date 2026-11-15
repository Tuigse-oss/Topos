# Architecture

Topos is one crate with a platform-free library and a Windows-only shell.
Everything that decides what the map shows is in the library and is tested
on any OS. The shell only draws it and turns input into calls on the
library.

## Library (`src/lib.rs`)

The library is `#![forbid(unsafe_code)]` and has no Windows dependencies.

| Module | What it does |
|---|---|
| `scanner.rs` | Parallel scan (`std::fs::read_dir` on a rayon pool), progress counters, cancellation, and the drive list. |
| `tree.rs` | The scanned tree, stored compactly (`Tree`, `Dir`, `NodeRef`), index paths, removal, and its on-disk form. `Node` builds small trees for tests. |
| `cache.rs` | Each drive's last scan on disk (`%LOCALAPPDATA%\Topos`), written atomically and read back as untrusted input. |
| `layout.rs` | The pure squarified layout of one level. |
| `treemap.rs` | The nested map: folders, title strips, insets, grouping of small items, hit testing, and whole-pixel tiles. |
| `view.rs` | Tile colours, file types, the map painted into a BGRA pixel buffer, and caption placement. |
| `session.rs` | What the window shows and does: scan and Recycle Bin workers, zoom, history, selection, settings, titles, and tooltips. |
| `settings.rs` | Setup defaults and bounded file reading, with complete writes before the saved choices are replaced. Older settings files still load. |
| `format.rs` | Byte counts, digit grouping, and file times. |
| `theme.rs` | The light and dark palettes for the map and the window, and the Appearance choice. Tests pin WCAG AA contrast for every text colour on every background it is drawn on. |
| `icon.rs` | The program icon, drawn in code from the map's own colours on whole pixels (crisp at every size), and the PNG, ICO, and BMP encoders the exe, Setup, and the Store package use. |
| `emblem.rs` | The Tuigse LLC emblem for About, scaled from `assets/tuigse-emblem-256.bgra`. |

## Program

- `src/main.rs` parses the command line. On Windows it runs `src/win/`;
  elsewhere it says Topos is a Windows program.
- `src/win/` is the Win32 shell on
  [winsafe](https://crates.io/crates/winsafe), whose safe wrappers let the
  program keep `#![forbid(unsafe_code)]` too:
  - `mod.rs`: the window, drive field, map, breadcrumb, status row, menus,
    task dialogs, and theme switching.
  - `toolbar.rs`: the hand-drawn flat toolbar with Segoe MDL2 symbols.
  - `pages.rs`: Help's Using Topos and About windows.
  - `gdi.rs`: fonts, fills, text, and frame upload.
- `build.rs` embeds the application manifest (common controls 6, system
  DPI awareness) with `embed-manifest`, and writes the icon groups and the
  version details straight into a `.res` file the linker takes, so no
  resource compiler is needed. The window has its own icon group whose
  large frames are the small drawings doubled pixel for pixel, because
  Windows halves the large frame for the title bar.

## Design notes

- **No `unsafe` in Topos.** All Windows API access (the window and GDI
  through `winsafe`, drives through `sysinfo`, the Recycle Bin through
  `trash`, opening files through `opener`) goes through safe wrappers.
- **The one unchecked GDI buffer.** `SetDIBits` (wrapped as safe by
  winsafe) reads a caller's buffer without knowing its length. It is only
  reached through `gdi::frame_bitmap`, which first checks the buffer is
  exactly width × height × 4 bytes.
- **Nested message loops.** Dialogs and menus run their own message
  loops. The shell never holds a `RefCell` borrow across them and paints
  with `try_borrow`, so a nested paint cannot panic.
- **Deletes are tied to one map.** A delete confirmed in the dialog
  applies only to the map it was asked about. If a scan or an earlier
  delete finished while the question was open (so index paths may point
  elsewhere), nothing is deleted. The move to the Recycle Bin runs off the
  UI thread, one at a time.
- **Paint once per layout.** The map is laid out and painted once per
  scan, zoom, resize, or settings change and kept as a bitmap. Moving the
  pointer only composites the outline and tooltip over it.

## Other files

| Path | What it is |
|---|---|
| `examples/art.rs` | Writes `Topos.ico`, Setup's wizard images, and the Store package's logos from `icon.rs`, plus a preview sheet: `cargo run --release --example art -- installer-art`. |
| `examples/scan_timing.rs`, `examples/view_timing.rs` | Wall-clock scan and map-painting samples on the real target. See [performance.md](performance.md). |
| `benches/scan_layout.rs` | Gungraun benches (Callgrind instructions and DHAT heap) for the layout, the nested map, the painted view, and the scan cache. CI compares them against the base commit on every pull request. |
| `installer/` | The Inno Setup script (one per architecture) and the install and MSIX test scripts. |
| `packaging/` | The Store package and the winget, Chocolatey, and Scoop templates. See [packaging/README.md](../packaging/README.md). |
| `scripts/third-party-notices.py` | Regenerates `THIRD-PARTY-NOTICES.txt`: the license text of every crate linked into `Topos.exe` (from `cargo tree` for both Windows targets) and the Rust standard library's notices. CI fails when it is out of date. |
