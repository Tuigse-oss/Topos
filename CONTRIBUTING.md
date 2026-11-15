# Contributing to Topos

Thanks for helping. Topos is free and open source, and it gets better
through people running it on real drives and telling us what they see.

## Ways to help

- **Report a bug.** Say what you scanned (a drive, a folder, a network
  share), what you expected, and what Topos did. A map that looks wrong, a
  size that disagrees with Explorer, or a scan that never finishes are all
  worth a report.
- **Suggest a feature.** Open an issue first, so we can agree it fits
  before you write code.
- **Improve the docs.** Typos, unclear wording, missing steps: small pull
  requests are welcome.
- **Send code.** See below.

Security problems go through private reporting instead; see
[SECURITY.md](SECURITY.md).

## Ground rules

These are what Topos is, so pull requests that change them will not be
merged:

- No network access, telemetry, analytics, ads, accounts, or update checks.
- No paid tiers, license keys, or nags.
- Delete always goes to the Recycle Bin, after a confirmation unless the
  user turned it off. Nothing is ever deleted permanently.
- Scans only read. They never follow symbolic links or junctions, and a
  folder that cannot be read is shown, not fatal.
- No `unsafe` code in Topos itself: the crate roots carry
  `#![forbid(unsafe_code)]`, and Windows is reached through safe wrappers.
- Settings stay per user, in `%LOCALAPPDATA%\Topos`. The installer writes
  no registry settings or machine-wide defaults.

## Working on the code

You need Rust (the version is pinned in `rust-toolchain.toml`, and `rustup`
picks it up). The program builds and runs on Windows with the MSVC build
tools, for 64-bit and 32-bit; the library and its tests also build and run
on Linux, which is also where the Callgrind benches run (see
[docs/performance.md](docs/performance.md)).

```bash
cargo fmt --all -- --check
rustfmt --check src/*.rs src/win/*.rs build.rs benches/*.rs examples/*.rs
cargo clippy --all-targets --all-features -- -D warnings
cargo clippy --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings
cargo clippy --all-targets --all-features --target i686-pc-windows-msvc -- -D warnings
cargo test
cargo build --release --target x86_64-pc-windows-msvc
cargo build --release --target i686-pc-windows-msvc
```

The bare `rustfmt` also reaches `build.rs`, which `cargo fmt` skips. The
Windows-target Clippy runs are what check `src/win/`, which only compiles
for Windows; they work from Linux too (add the targets with
`rustup target add`), since Clippy does not link. Plain
`cargo test` leaves out the Gungraun bench target, which needs Valgrind
and `gungraun-runner`; Clippy still compiles it.

[docs/architecture.md](docs/architecture.md) explains how the code is laid
out.

CI runs the same checks on every pull request, on Linux and on Windows for
both architectures, and also builds, installs, upgrades, and uninstalls the
installer and the Microsoft Store package.

- Keep pull requests focused on one change, and explain *why* in the
  description.
- Add a test for a fixed bug or new behavior. Most of Topos (scanner, tree,
  layout, map, view) is platform-free and tested on any OS.
- Performance is checked on every pull request. The `performance` CI job
  runs the benches in `benches/scan_layout.rs` (the layout, the nested map,
  the painted map, and the scan cache) under Callgrind and DHAT, first on
  the base commit and then on yours. It fails when any bench needs over 1%
  more instructions, or allocates or peaks at over 2% more heap, and the
  job summary shows every number side by side. If a slowdown is a
  deliberate trade-off, say why in the pull request; a maintainer can
  accept it with the `performance-accepted` label. New hot paths should
  get a bench. To run the same comparison locally (Linux, with Valgrind),
  see [docs/performance.md](docs/performance.md).
- Scanning speed on Windows is mostly time in the kernel, which Callgrind
  cannot see, so a change to the scanner also needs a before-and-after
  `examples/scan_timing.rs` sample on Windows in the pull request.
- If you change `Cargo.lock`, regenerate the notices with
  `python3 scripts/third-party-notices.py` (it rewrites
  `THIRD-PARTY-NOTICES.txt` itself).
- New dependencies need a good reason: each one is code that runs with
  access to every file on the machine.

## License

By contributing, you agree that your contribution is licensed under MIT or
Apache-2.0, at the user's option, like the rest of the project.
