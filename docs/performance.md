# Performance

The numbers below are samples from one development machine (Windows 11,
20 logical CPUs, warm file-system cache): one `Instant` sample per run, or
a process polled until its CPU time stopped growing. They are there for
before-and-after comparison on that machine, not as statistical claims.

## Scanning

Scanning is measured as wall-clock time on Windows, where it matters:

```powershell
cargo run --release --example scan_timing -- "C:\Program Files" 3
```

| Folder | Files | Scan time |
|---|---|---|
| `C:\Program Files` | 146,885 | 0.136–0.141 s |
| A user profile | 1,531,839 | 2.008 s |
| `C:\` (whole drive) | 2,224,180 | 3.92–4.04 s |

Each folder is read with `std::fs::read_dir` on a rayon pool, and its node
is built in the same task, so there is no flat path list to regroup
afterwards. On Windows, std's `DirEntry::metadata` is the `FindNextFileW`
record itself, so no file is opened per entry. An earlier scanner on
`jwalk` took 69.4 s for the same profile, because its
`DirEntry::metadata` re-reads each file's metadata by path.

### Where the time goes

Under a wrapper that reads the process's user and kernel times, one thread
takes 26–28 s for `C:\`, of which 24 s is kernel time in the directory
calls and 2–3 s is Topos. More threads cut the wall time but contend in
the kernel: 20 threads use 52–57 s of kernel time for a 3.7–4.8 s scan,
and 12 threads save about 10% CPU but take about 20% longer. Topos keeps
one thread per logical CPU.

Measured and rejected, since neither is worth `unsafe` code:

- `FIND_FIRST_EX_LARGE_FETCH` (std already uses `FindExInfoBasic`): no
  change.
- `NtQueryDirectoryFile` with folders opened relative to their parent's
  handle: 12% less kernel time on one thread, within noise at 20.

## Against SpaceMonger 1.4

Whole `C:\` (2.23 million files, 402,000 folders), warm cache. Each program
was started and polled every 100 ms until its CPU time stopped growing
(`Get-Process`). SpaceMonger is timed from its drive dialog's OK, Topos
from process start.

| Program | First map | Up-to-date map | CPU time | Peak working set |
|---|---|---|---|---|
| SpaceMonger 1.4.0.1 | 34.5 s | 34.5 s | 34.5 s (one thread) | 303.5 MB |
| Topos, first run (no cache) | 4.8 s | 4.8 s | 66 s (20 threads) | 205 MB |
| Topos, from the last scan | ~0.8 s | 4.0–5.0 s | 56–64 s | 356–361 MB |

With a cache, the first map takes about 0.2 s of start-up plus reading the
125 MB cache file. That read takes 190 ms on its own and 0.59 s while the
scan threads compete for the CPU (the status line reports it). Holding the
old map and the new scan at once is what raises that run's peak, and the
first run's figure includes writing the cache.

## Memory

A folder's direct contents are one `Dir`: 24-byte entries (size, a time
with a sentinel for "unknown", a name offset, and a subfolder index whose
top bit flags "could not be read"), the names back to back in entry order,
and the subfolders' own `Dir`s. There is no allocation per file, and
freeing the whole `C:\` tree takes 30 ms. The scanner alone peaks at
194–198 MB on `C:\`, down from 313 MB with a 72-byte node and a heap block
per name.

The tree is read through `NodeRef`, a view that slices a name out only
when asked. Building names on every walk cost the nested map 10%.

## Layout, painting, and the scan cache

These run under Valgrind on Linux through
[Gungraun](https://crates.io/crates/gungraun): Callgrind counts the
instructions and DHAT counts the heap. Install the matching runner once and
point the build at it (the path is baked in at compile time through
`GUNGRAUN_RUNNER`):

```bash
cargo install gungraun-runner --version 0.20.0
export GUNGRAUN_RUNNER=~/.cargo/bin/gungraun-runner
cargo bench --bench scan_layout
```

| Benchmark | Instructions | Heap allocated |
|---|---|---|
| layout, 1k tiles | 478,318 | 87,712 B |
| layout, 5k tiles | 2,402,599 | 435,040 B |
| nested map, 5,760 files, 1920x1080 | 4,501,853 | 1,387,492 B |
| painted view (map + pixels + captions), same | 23,736,416 | 9,937,560 B |
| cache write, 100,000 files | 5,329,960 | 8,388,600 B |
| cache read, 100,000 files | 9,917,287 | 3,573,373 B |

The counts are the same on every run of the same build, which is what lets
CI gate on them.

### In CI

The `performance` job in `.github/workflows/ci.yml` runs every bench on the
base commit and then on the change, on the same runner, and fails a pull
request when any bench needs over 1% more instructions, or allocates or
peaks at over 2% more heap. The job summary lists every count next to the
base commit's. A deliberate trade-off is accepted by labelling the pull
request `performance-accepted` and re-running the job. Pushes to `main`
report the comparison and never fail.

The same comparison by hand:

```bash
git switch main
cargo bench --bench scan_layout -- --save-baseline=main
git switch my-change
cargo bench --bench scan_layout -- --baseline=main \
  --callgrind-limits=ir=1% --dhat-limits=totalbytes=2%,attgmaxbytes=2%
```

### Notes

Filling the frame's background with `<[u8]>::repeat` instead of one pixel
at a time halved the painting cost. Drawing tile borders as four edges
instead of one fill measured 12.7% slower (there are many small tiles), so
each tile is one border fill plus one inside fill.

There is no Callgrind scan bench. Gungraun collects only the benchmark
function's own thread by default, so the scan's rayon threads would count
as zero. Use `examples/scan_timing.rs` on Windows instead.

The layouts are pinned by `exact_tiles_for_fixed_inputs` (the layout,
derived by hand and cross-checked against an independent reference) and
`exact_tiles_for_a_fixed_tree` (the nested map, derived by hand).

On Windows, `examples/view_timing.rs` on the whole `C:\` (2,229,779 files)
lays out and paints the map in 5.8–7.4 ms at 1280x720 (5,758 tiles, 329
captions) and 17.7–21.8 ms at 2560x1400 (21,332 tiles). The window does
this once per layout (scan, zoom, resize, or settings change) and keeps
the result as a bitmap. Moving the pointer copies it through a reusable
drawing buffer, adds the outline and tooltip, and copies that to the
screen. The buffer is replaced on resize and released while the window is
minimised. While a map is on screen, scan progress updates the status
without repainting the map.
