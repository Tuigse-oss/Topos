//! Parallel filesystem scanner.
//!
//! [`scan`] walks a directory tree with `std::fs::read_dir` on a rayon
//! fork-join pool and builds the [`Node`] tree directly: each directory task
//! returns its own finished node, so there is no flat path list to regroup
//! afterwards. Sizes and times come from the directory listing itself
//! (`DirEntry::metadata`), which on Windows is the `FindNextFileW` record —
//! no file is opened per entry.
//!
//! Symbolic links and directory junctions are counted in `links` and never
//! followed, so scans always terminate. Folders that cannot be listed are
//! kept (marked `unreadable`) and counted in `skipped`, as are entries that
//! cannot be read; neither aborts the scan. Names starting with `.` are
//! scanned like any other.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rayon::prelude::*;

use crate::tree::{Dir, DirBuilder, Removed, Tree};

/// Stack for each scan thread. Rayon nests stolen work on a waiting thread's
/// stack, so give deep trees generous room.
const SCAN_STACK_BYTES: usize = 8 * 1024 * 1024;

/// Totals for a finished scan.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ScanStats {
    /// Regular files (and other non-directory, non-link entries).
    pub files: u64,
    /// Directories, the scan root included.
    pub folders: u64,
    /// Symbolic links and junctions: counted, never followed.
    pub links: u64,
    /// Folders that could not be listed plus entries that could not be read.
    pub skipped: u64,
    /// Sum of file sizes.
    pub bytes: u64,
}

impl ScanStats {
    /// Take a removed subtree out of the totals: its files, its folders, and
    /// its bytes. Links and unreadable entries are not tracked per node, so
    /// those two counts keep their scan-time values until the next scan.
    pub fn forget(&mut self, removed: &Removed) {
        self.files = self.files.saturating_sub(removed.files);
        self.folders = self.folders.saturating_sub(removed.folders);
        self.bytes = self.bytes.saturating_sub(removed.size);
    }

    fn absorb(&mut self, other: ScanStats) {
        self.files += other.files;
        self.folders += other.folders;
        self.links += other.links;
        self.skipped += other.skipped;
        self.bytes = self.bytes.saturating_add(other.bytes);
    }
}

/// A finished scan: the tree plus its totals.
#[derive(Debug)]
pub struct ScanResult {
    pub root: Tree,
    pub stats: ScanStats,
}

/// Live counters and the cancel flag, shared between a running scan and
/// whoever watches it.
#[derive(Debug, Default)]
pub struct ScanProgress {
    files: AtomicU64,
    folders: AtomicU64,
    bytes: AtomicU64,
    cancelled: AtomicBool,
}

impl ScanProgress {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask the scan to stop; it returns [`ScanError::Cancelled`] soon after.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// Files, folders, and bytes seen so far.
    #[must_use]
    pub fn snapshot(&self) -> (u64, u64, u64) {
        (
            self.files.load(Ordering::Relaxed),
            self.folders.load(Ordering::Relaxed),
            self.bytes.load(Ordering::Relaxed),
        )
    }

    fn record(&self, files: u64, bytes: u64) {
        self.files.fetch_add(files, Ordering::Relaxed);
        self.folders.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
    }
}

/// Why a scan produced no tree.
#[derive(Debug)]
pub enum ScanError {
    /// The root is missing, not a directory, or cannot be listed.
    Root(io::Error),
    /// [`ScanProgress::cancel`] was called.
    Cancelled,
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Root(err) => write!(f, "{err}"),
            Self::Cancelled => f.write_str("scan cancelled"),
        }
    }
}

impl std::error::Error for ScanError {}

/// Scan `root` without progress reporting or cancellation.
///
/// # Errors
/// When the root is missing, not a directory, or cannot be listed.
pub fn scan_root(root: &Path) -> io::Result<ScanResult> {
    scan(root, &ScanProgress::new()).map_err(|err| match err {
        ScanError::Root(err) => err,
        ScanError::Cancelled => io::Error::new(io::ErrorKind::Interrupted, "scan cancelled"),
    })
}

/// Scan `root`, publishing counters to `progress` and stopping early when it
/// is cancelled. The root node's name is the root path as given.
///
/// # Errors
/// [`ScanError::Root`] when the root is missing, not a directory, or cannot
/// be listed; [`ScanError::Cancelled`] after [`ScanProgress::cancel`].
pub fn scan(root: &Path, progress: &ScanProgress) -> Result<ScanResult, ScanError> {
    let metadata = fs::metadata(root).map_err(ScanError::Root)?;
    if !metadata.is_dir() {
        return Err(ScanError::Root(io::Error::new(
            io::ErrorKind::NotADirectory,
            format!("{} is not a folder", root.display()),
        )));
    }
    let listing = fs::read_dir(root).map_err(ScanError::Root)?;
    let pool = rayon::ThreadPoolBuilder::new()
        .stack_size(SCAN_STACK_BYTES)
        .thread_name(|index| format!("topos-scan-{index}"))
        .build()
        .map_err(|err| ScanError::Root(io::Error::other(err)))?;
    let modified = metadata.modified().ok().and_then(unix_seconds);
    let (contents, stats) = pool.install(|| walk_listing(listing, progress));
    if progress.is_cancelled() {
        return Err(ScanError::Cancelled);
    }
    let tree = Tree::new(&root.to_string_lossy(), modified, contents);
    Ok(ScanResult { root: tree, stats })
}

/// A subdirectory found in a listing, waiting for its own task.
struct Pending {
    path: PathBuf,
    name: String,
    modified: Option<i64>,
}

/// A subfolder's contents and totals, and whether it could not be listed.
fn walk_dir(pending: &Pending, progress: &ScanProgress) -> (Dir, ScanStats, bool) {
    if progress.is_cancelled() {
        return (Dir::default(), ScanStats::default(), false);
    }
    match fs::read_dir(&pending.path) {
        Ok(listing) => {
            let (contents, stats) = walk_listing(listing, progress);
            (contents, stats, false)
        }
        Err(_) => {
            progress.record(0, 0);
            let stats = ScanStats {
                folders: 1,
                skipped: 1,
                ..ScanStats::default()
            };
            (Dir::default(), stats, true)
        }
    }
}

/// One folder's contents (its subfolders scanned in parallel) and totals,
/// the folder itself counted.
fn walk_listing(listing: fs::ReadDir, progress: &ScanProgress) -> (Dir, ScanStats) {
    let mut stats = ScanStats {
        folders: 1,
        ..ScanStats::default()
    };
    let mut contents = DirBuilder::new();
    let mut subdirs: Vec<Pending> = Vec::new();
    for entry in listing {
        let Ok(entry) = entry else {
            stats.skipped += 1;
            continue;
        };
        let Ok(file_type) = entry.file_type() else {
            stats.skipped += 1;
            continue;
        };
        if file_type.is_symlink() {
            // Junctions report as links on Windows too: counted, not followed.
            stats.links += 1;
            continue;
        }
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        let metadata = entry.metadata();
        let modified = metadata
            .as_ref()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(unix_seconds);
        if file_type.is_dir() {
            subdirs.push(Pending {
                path: entry.path(),
                name: name.into_owned(),
                modified,
            });
            continue;
        }
        let Ok(metadata) = metadata else {
            stats.skipped += 1;
            continue;
        };
        stats.files += 1;
        stats.bytes = stats.bytes.saturating_add(metadata.len());
        contents.file(&name, metadata.len(), modified);
    }
    progress.record(stats.files, stats.bytes);

    let subtrees: Vec<(Dir, ScanStats, bool)> = subdirs
        .par_iter()
        .map(|pending| walk_dir(pending, progress))
        .collect();
    for (pending, (dir, sub_stats, unreadable)) in subdirs.iter().zip(subtrees) {
        stats.absorb(sub_stats);
        contents.dir(&pending.name, dir.size(), pending.modified, unreadable, dir);
    }
    (contents.finish(), stats)
}

/// Whole seconds since the Unix epoch, negative before it.
fn unix_seconds(time: SystemTime) -> Option<i64> {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs()).ok(),
        Err(before) => i64::try_from(before.duration().as_secs())
            .ok()
            .map(|secs| -secs),
    }
}

/// One mounted filesystem for the drive picker.
#[derive(Debug, Clone)]
pub struct DriveInfo {
    /// Display name, e.g. `C:\` on Windows or `/` elsewhere.
    pub mount: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
}

/// List mounted filesystems with capacity. Backed by `sysinfo` so this crate
/// needs no `unsafe` of its own on any platform.
#[must_use]
pub fn drives() -> Vec<DriveInfo> {
    use sysinfo::Disks;
    // Listing can fail (sysinfo 0.40 reports why). Topos then offers no
    // drives, as on a system without any; folders still scan by path.
    let Ok(disks) = Disks::new_with_refreshed_list() else {
        return Vec::new();
    };
    let mut out: Vec<DriveInfo> = disks
        .iter()
        .map(|disk| DriveInfo {
            mount: disk.mount_point().to_string_lossy().into_owned(),
            total_bytes: disk.total_space(),
            free_bytes: disk.available_space(),
        })
        .collect();
    out.sort_by(|a, b| a.mount.cmp(&b.mount));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh fixture directory, removed again when the guard drops.
    struct Fixture(PathBuf);

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// root/{a.txt 100}, root/sub/{b.txt 300}, root/sub/deep/{c.txt 600}
    fn fixture(name: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "topos-scanner-test-{}-{}",
            name,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub").join("deep")).unwrap();
        fs::write(dir.join("a.txt"), vec![1_u8; 100]).unwrap();
        fs::write(dir.join("sub").join("b.txt"), vec![2_u8; 300]).unwrap();
        fs::write(dir.join("sub").join("deep").join("c.txt"), vec![3_u8; 600]).unwrap();
        Fixture(dir)
    }

    #[test]
    fn rolls_sizes_up_and_counts_entries() {
        let dir = fixture("rollup");
        let result = scan_root(&dir.0).unwrap();
        assert_eq!(
            result.stats,
            ScanStats {
                files: 3,
                folders: 3,
                links: 0,
                skipped: 0,
                bytes: 1000,
            }
        );
        let root = result.root.root();
        assert_eq!(root.size(), 1000);
        let sub = root.child(0).unwrap();
        assert_eq!((sub.name(), sub.size(), sub.is_dir()), ("sub", 900, true));
        assert_eq!(sub.child(0).unwrap().name(), "deep");
        assert_eq!(sub.child(0).unwrap().size(), 600);
        assert_eq!(result.root.counts(), (3, 3));
    }

    #[test]
    fn forgetting_a_removed_folder_updates_the_totals() {
        let dir = fixture("forget");
        let mut result = scan_root(&dir.0).unwrap();
        let sub = result.root.remove_at(&[0]).unwrap();
        result.stats.forget(&sub);
        assert_eq!((result.stats.files, result.stats.folders), (1, 1));
        assert_eq!(result.stats.bytes, 100);
        assert_eq!(result.stats.bytes, result.root.root().size());
    }

    #[test]
    fn root_is_named_by_its_path_and_entries_carry_times() {
        let dir = fixture("names");
        let result = scan_root(&dir.0).unwrap();
        let root = result.root.root();
        assert_eq!(root.name(), &*dir.0.to_string_lossy());
        assert!(root.modified().is_some());
        assert!(root.children().all(|child| child.modified().is_some()));
    }

    #[test]
    fn children_are_largest_first_then_by_name() {
        let dir = fixture("order");
        fs::write(dir.0.join("tie-b"), [0_u8; 100]).unwrap();
        fs::write(dir.0.join("tie-a"), [0_u8; 100]).unwrap();
        let result = scan_root(&dir.0).unwrap();
        let names: Vec<&str> = result.root.root().children().map(|c| c.name()).collect();
        assert_eq!(names, vec!["sub", "a.txt", "tie-a", "tie-b"]);
    }

    #[test]
    fn dot_prefixed_entries_are_scanned() {
        let dir = fixture("hidden");
        fs::create_dir_all(dir.0.join(".cache")).unwrap();
        fs::write(dir.0.join(".cache").join("blob"), vec![4_u8; 2000]).unwrap();
        fs::write(dir.0.join(".profile"), vec![5_u8; 50]).unwrap();
        let result = scan_root(&dir.0).unwrap();
        assert_eq!(result.stats.files, 5);
        assert_eq!(result.stats.bytes, 3050);
        assert_eq!(result.root.root().size(), 3050);
        assert_eq!(result.root.node_at(&[0]).unwrap().name(), ".cache");
    }

    #[test]
    fn empty_folders_are_kept() {
        let dir = fixture("empty");
        fs::create_dir_all(dir.0.join("nothing")).unwrap();
        let result = scan_root(&dir.0).unwrap();
        assert_eq!(result.stats.folders, 4);
        let empty = result
            .root
            .root()
            .children()
            .find(|c| c.name() == "nothing");
        assert!(empty.is_some_and(|node| node.is_dir() && node.size() == 0));
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_folder_is_kept_and_counted_as_skipped() {
        use std::os::unix::fs::PermissionsExt;
        let dir = fixture("unreadable");
        let locked = dir.0.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::write(locked.join("secret"), [0_u8; 10]).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        // Root ignores permission bits; nothing to measure then.
        let enforced = fs::read_dir(&locked).is_err();
        let result = scan_root(&dir.0);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        if enforced {
            let result = result.unwrap();
            assert_eq!(result.stats.skipped, 1);
            assert_eq!(result.stats.files, 3);
            assert_eq!(result.stats.folders, 4);
            let node = result.root.root().children().find(|c| c.name() == "locked");
            assert!(node.is_some_and(|node| node.unreadable() && node.is_dir()));
        }
    }

    #[cfg(unix)]
    #[test]
    fn links_are_counted_and_never_followed() {
        let dir = fixture("links");
        // A link back to the root would loop forever if followed.
        std::os::unix::fs::symlink(&dir.0, dir.0.join("sub").join("loop")).unwrap();
        std::os::unix::fs::symlink(dir.0.join("a.txt"), dir.0.join("a-link")).unwrap();
        let result = scan_root(&dir.0).unwrap();
        assert_eq!(result.stats.links, 2);
        assert_eq!(result.stats.files, 3);
        assert_eq!(result.stats.bytes, 1000);
        assert_eq!(result.stats.skipped, 0);
    }

    #[test]
    fn progress_counts_match_the_totals() {
        let dir = fixture("progress");
        let progress = ScanProgress::new();
        let result = scan(&dir.0, &progress).unwrap();
        assert_eq!(progress.snapshot(), (3, 3, 1000));
        assert_eq!(result.stats.files, 3);
    }

    #[test]
    fn cancelled_scan_returns_cancelled() {
        let dir = fixture("cancel");
        let progress = ScanProgress::new();
        progress.cancel();
        assert!(matches!(scan(&dir.0, &progress), Err(ScanError::Cancelled)));
    }

    #[test]
    fn missing_root_is_an_error_not_a_panic() {
        let missing = std::env::temp_dir().join("topos-scanner-test-no-such-dir-xyz");
        let _ = fs::remove_dir_all(&missing);
        assert!(scan_root(&missing).is_err());
    }

    #[test]
    fn file_root_is_an_error() {
        let dir = fixture("file-root");
        let err = scan_root(&dir.0.join("a.txt")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotADirectory);
    }

    #[test]
    fn unix_seconds_handles_both_sides_of_the_epoch() {
        let later = UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let earlier = UNIX_EPOCH - std::time::Duration::from_secs(86_400);
        assert_eq!(unix_seconds(later), Some(1_700_000_000));
        assert_eq!(unix_seconds(earlier), Some(-86_400));
    }

    #[test]
    fn drives_list_is_sorted_and_sized() {
        let list = drives();
        assert!(!list.is_empty());
        let mut mounts: Vec<&str> = list.iter().map(|info| info.mount.as_str()).collect();
        mounts.sort_unstable();
        let ordered: Vec<&str> = list.iter().map(|info| info.mount.as_str()).collect();
        assert_eq!(mounts, ordered);
        assert!(list.iter().all(|info| info.total_bytes >= info.free_bytes));
    }
}
