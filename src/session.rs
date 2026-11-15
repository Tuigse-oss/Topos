//! The state behind the window, with no window system in it: the scan on
//! screen, where the map is zoomed, the selection, the settings, and the
//! background work (scans and Recycle Bin moves) on worker threads.
//!
//! The shell turns input into calls here, polls [`Session::poll`] while
//! [`Session::busy`], and draws what it reads back. Nothing here blocks the
//! caller: scans, deletes, and the [`cache`] run on their own threads, and
//! large trees are freed off the caller's thread too.
//!
//! While a drive is rescanned, its last scan (from the cache at start, or
//! the map already on screen on Reload) stays on screen marked as old;
//! deletes wait for the fresh scan, and the zoom and selection carry over
//! to it by name.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::cache::{self, Header};
use crate::format::{self, exact_bytes, group_digits, pretty_bytes};
use crate::scanner::{self, DriveInfo, ScanError, ScanProgress, ScanResult, ScanStats};
use crate::tree::{self, NodeRef, Tree};
use crate::view::{self, Frame, View, ViewKey};

pub use crate::settings::Settings;

/// The scan on screen.
#[derive(Debug)]
pub struct Scanned {
    /// Where the scan started; index paths are relative to it.
    pub root: PathBuf,
    /// Shared only while the cache writer saves it; edits wait for that.
    pub tree: Arc<Tree>,
    pub stats: ScanStats,
    /// When the scan finished, in seconds since the Unix epoch.
    pub scanned_at: i64,
}

/// What the cache reader sends: the header first (for progress), then the
/// tree.
enum Cached {
    Header(Header),
    Tree(Header, Tree, Duration),
}

/// Stack for the threads that walk a tree recursively (the cache's reader
/// and writer), as for the scan.
const TREE_STACK_BYTES: usize = 8 * 1024 * 1024;

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|now| i64::try_from(now.as_secs()).ok())
        .unwrap_or(0)
}

/// A running scan: its worker reports once, through `rx`.
struct ScanJob {
    root: PathBuf,
    progress: Arc<ScanProgress>,
    rx: Receiver<Result<ScanResult, ScanError>>,
    started: Instant,
}

/// A move to the Recycle Bin on a worker thread (large folders take a
/// while). Its result applies only to the scan it was asked on.
struct TrashJob {
    node_path: Vec<usize>,
    fs_path: PathBuf,
    generation: u64,
    rx: Receiver<Result<(), String>>,
    /// The worker's answer, kept while the tree is still being saved.
    outcome: Option<Result<(), String>>,
}

/// What [`Session::delete_request`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteRequest {
    /// Ask first; call [`Session::delete_confirmed`] with `node_path` and
    /// `generation` on a yes.
    Confirm {
        node_path: Vec<usize>,
        fs_path: PathBuf,
        size: u64,
        /// The tree the question was about; the answer applies only to it.
        generation: u64,
    },
    /// The move to the Recycle Bin has started.
    Started,
    /// Nothing to delete now; [`Session::status`] says why.
    Refused,
}

/// Background work, as the progress bars show it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Activity {
    /// A scan; `done` is the share finished (0 to 0.99) when the last scan
    /// of the same root gives an estimate, `None` when nothing does.
    Scanning { done: Option<f32> },
    /// Moving the selection to the Recycle Bin.
    Deleting,
}

pub struct Session {
    drives: Vec<DriveInfo>,
    /// What Reload scans: a drive root or any folder.
    target: Option<PathBuf>,
    scan: Option<ScanJob>,
    trash: Option<TrashJob>,
    scanned: Option<Scanned>,
    /// Bumped whenever `scanned` changes, so cached views go stale.
    generation: u64,
    /// Index path of the folder filling the map.
    zoom: Vec<usize>,
    /// Index path (from the scan root) of the selected file or folder.
    selected: Option<Vec<usize>>,
    pub settings: Settings,
    /// Whether Windows apps are set to dark, which the System appearance
    /// follows; the shell keeps it current.
    pub system_dark: bool,
    status: String,
    /// Where each drive's last scan is kept; `None` turns the cache off.
    cache_dir: Option<PathBuf>,
    /// The cache reader for the running scan's root.
    cached: Option<Receiver<Cached>>,
    /// Set while the map on screen is an earlier scan: when it was taken.
    stale_since: Option<i64>,
    /// How long the cached map took to read, for the status line.
    cache_read_in: Option<Duration>,
    /// Files the running scan should find (from the root's last scan).
    expected_files: Option<u64>,
    /// Folders zoomed away from, most recent last (Back), and the ones Back
    /// left (Forward).
    back: Vec<Vec<usize>>,
    forward: Vec<Vec<usize>>,
}

/// How many folders Back remembers.
const HISTORY_LIMIT: usize = 100;

impl Session {
    /// A session over `drives` that starts scanning `start`, or the first
    /// drive when there is none, keeping drives' last scans in `cache_dir`.
    #[must_use]
    pub fn new(start: Option<PathBuf>, drives: Vec<DriveInfo>, cache_dir: Option<PathBuf>) -> Self {
        let target = start.or_else(|| drives.first().map(|drive| PathBuf::from(&drive.mount)));
        let mut session = Self {
            drives,
            target,
            scan: None,
            trash: None,
            scanned: None,
            generation: 0,
            zoom: Vec::new(),
            selected: None,
            settings: Settings::default(),
            system_dark: false,
            status: String::from("Pick a drive to scan."),
            cache_dir,
            cached: None,
            stale_since: None,
            cache_read_in: None,
            expected_files: None,
            back: Vec::new(),
            forward: Vec::new(),
        };
        match session.target.clone() {
            Some(root) => session.start_scan(root),
            None => session.status = String::from("No drives found."),
        }
        session
    }

    #[must_use]
    pub fn drives(&self) -> &[DriveInfo] {
        &self.drives
    }

    #[must_use]
    pub fn target(&self) -> Option<&Path> {
        self.target.as_deref()
    }

    #[must_use]
    pub fn scanned(&self) -> Option<&Scanned> {
        self.scanned.as_ref()
    }

    #[must_use]
    pub fn zoom(&self) -> &[usize] {
        &self.zoom
    }

    #[must_use]
    pub fn selected(&self) -> Option<&[usize]> {
        self.selected.as_deref()
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn set_status(&mut self, status: String) {
        self.status = status;
    }

    #[must_use]
    pub fn is_scanning(&self) -> bool {
        self.scan.is_some()
    }

    #[must_use]
    pub fn is_deleting(&self) -> bool {
        self.trash.is_some()
    }

    /// Whether the target is a whole drive (so it has free space to show).
    #[must_use]
    pub fn target_is_drive(&self) -> bool {
        self.target.as_deref().is_some_and(|target| {
            self.drives
                .iter()
                .any(|drive| Path::new(&drive.mount) == target)
        })
    }

    /// What background work is running, for the progress bars.
    #[must_use]
    pub fn activity(&self) -> Option<Activity> {
        if let Some(job) = &self.scan {
            let (files, ..) = job.progress.snapshot();
            return Some(Activity::Scanning {
                done: share_done(files, self.expected_files),
            });
        }
        self.trash.as_ref().map(|_| Activity::Deleting)
    }

    /// Whether background work is running, so the shell should keep
    /// calling [`Self::poll`].
    #[must_use]
    pub fn busy(&self) -> bool {
        self.scan.is_some() || self.trash.is_some() || self.cached.is_some()
    }

    /// When the map on screen is an earlier scan of the target (shown while
    /// it is rescanned): when that scan was taken.
    #[must_use]
    pub fn stale_since(&self) -> Option<i64> {
        self.stale_since
    }

    /// Whether Delete can act now: something is selected, the map is the
    /// latest scan, and no other delete is running.
    #[must_use]
    pub fn can_delete(&self) -> bool {
        self.selected.as_ref().is_some_and(|path| !path.is_empty())
            && self.stale_since.is_none()
            && self.trash.is_none()
    }

    fn cache_file(&self, root: &Path) -> Option<PathBuf> {
        let dir = self.cache_dir.as_ref()?;
        let is_drive = self
            .drives
            .iter()
            .any(|drive| Path::new(&drive.mount) == root);
        is_drive.then(|| cache::file_for(dir, root))
    }

    /// The drive holding `path`: the one whose mount point is its longest
    /// prefix.
    #[must_use]
    pub fn drive_of(&self, path: &Path) -> Option<&DriveInfo> {
        self.drives
            .iter()
            .filter(|drive| path.starts_with(&drive.mount))
            .max_by_key(|drive| drive.mount.len())
    }

    /// Start a worker-thread scan of `root`, cancelling any scan still
    /// running. A map of the same root stays on screen, marked as old, until
    /// the new scan replaces it; for another root the map is dropped and the
    /// root's cached scan (if any) is read to show meanwhile.
    pub fn start_scan(&mut self, root: PathBuf) {
        self.stop_scan();
        self.cached = None;
        self.expected_files = None;
        let same_root = self
            .scanned
            .as_ref()
            .is_some_and(|scanned| scanned.root == root);
        if same_root {
            if let Some(scanned) = &self.scanned {
                self.stale_since = Some(scanned.scanned_at);
                self.expected_files = Some(scanned.stats.files);
            }
        } else {
            if let Some(old) = self.scanned.take() {
                // Freeing millions of nodes takes a moment; keep it off the UI.
                thread::spawn(move || drop(old));
            }
            self.stale_since = None;
            self.zoom.clear();
            self.selected = None;
            self.back.clear();
            self.forward.clear();
            self.generation += 1;
            if let Some(file) = self.cache_file(&root) {
                self.cached = Some(read_cache(file, root.to_string_lossy().into_owned()));
            }
        }
        self.cache_read_in = None;
        self.target = Some(root.clone());
        let progress = Arc::new(ScanProgress::new());
        let (tx, rx) = mpsc::channel();
        let worker_root = root.clone();
        let worker_progress = Arc::clone(&progress);
        thread::spawn(move || {
            let outcome = scanner::scan(&worker_root, &worker_progress);
            // The session may have moved on (cancelled or closed); fine.
            let _ = tx.send(outcome);
        });
        self.status = format!("Scanning {} …", root.display());
        self.scan = Some(ScanJob {
            root,
            progress,
            rx,
            started: Instant::now(),
        });
    }

    pub fn reload(&mut self) {
        if let Some(root) = self.target.clone() {
            self.start_scan(root);
        }
    }

    /// Cancel the running scan, if any; its worker stops at the next folder.
    pub fn stop_scan(&mut self) {
        if let Some(job) = self.scan.take() {
            job.progress.cancel();
            self.status = format!("Stopped scanning {}.", job.root.display());
        }
    }

    /// Collect finished background work. Returns whether anything changed.
    pub fn poll(&mut self) -> bool {
        let cached = self.poll_cache();
        let scanned = self.poll_scan();
        let trashed = self.poll_trash();
        cached || scanned || trashed
    }

    /// Take what the cache reader has sent: an estimate for progress, then
    /// the old map to show until the scan finishes.
    fn poll_cache(&mut self) -> bool {
        let Some(rx) = &self.cached else {
            return false;
        };
        let mut changed = false;
        loop {
            match rx.try_recv() {
                Ok(Cached::Header(header)) => {
                    self.expected_files = Some(header.stats.files);
                }
                Ok(Cached::Tree(header, tree, took)) => {
                    self.cached = None;
                    let root = PathBuf::from(&header.root);
                    let showable = self.scanned.is_none()
                        && self.scan.as_ref().is_some_and(|job| job.root == root);
                    if showable {
                        self.scanned = Some(Scanned {
                            root,
                            tree: Arc::new(tree),
                            stats: header.stats,
                            scanned_at: header.scanned_at,
                        });
                        self.stale_since = Some(header.scanned_at);
                        self.cache_read_in = Some(took);
                        self.generation += 1;
                        changed = true;
                    } else {
                        thread::spawn(move || drop(tree));
                    }
                    return changed;
                }
                Err(TryRecvError::Empty) => return changed,
                Err(TryRecvError::Disconnected) => {
                    self.cached = None;
                    return changed;
                }
            }
        }
    }

    fn poll_scan(&mut self) -> bool {
        let Some(job) = &self.scan else {
            return false;
        };
        let outcome = match job.rx.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => Err(ScanError::Root(std::io::Error::other(
                "the scan worker stopped unexpectedly",
            ))),
        };
        let elapsed = job.started.elapsed();
        let root = job.root.clone();
        self.scan = None;
        match outcome {
            Ok(result) => {
                self.status = format!(
                    "Scanned {} in {:.1} s: {} in {} files.",
                    root.display(),
                    elapsed.as_secs_f64(),
                    pretty_bytes(result.stats.bytes),
                    group_digits(result.stats.files)
                );
                let fresh = Scanned {
                    root,
                    tree: Arc::new(result.root),
                    stats: result.stats,
                    scanned_at: now_seconds(),
                };
                if let Some(old) = self.scanned.take() {
                    // Keep the view where it was on the old map, by name.
                    self.zoom = carry_path(&old.tree, &fresh.tree, &self.zoom, true);
                    for path in self.back.iter_mut().chain(self.forward.iter_mut()) {
                        *path = carry_path(&old.tree, &fresh.tree, path, true);
                    }
                    self.selected = self.selected.as_ref().and_then(|selected| {
                        let carried = carry_path(&old.tree, &fresh.tree, selected, false);
                        (carried.len() == selected.len()).then_some(carried)
                    });
                    thread::spawn(move || drop(old));
                }
                self.scanned = Some(fresh);
                self.stale_since = None;
                self.cache_read_in = None;
                self.cached = None;
                self.expected_files = None;
                self.generation += 1;
                self.save_cache();
            }
            Err(ScanError::Cancelled) => {}
            Err(err) => self.status = format!("Scan of {} failed: {err}", root.display()),
        }
        true
    }

    /// Write the scan on screen to the cache, on its own thread.
    fn save_cache(&self) {
        let Some(scanned) = &self.scanned else {
            return;
        };
        let Some(file) = self.cache_file(&scanned.root) else {
            return;
        };
        let header = Header {
            root: scanned.root.to_string_lossy().into_owned(),
            scanned_at: scanned.scanned_at,
            stats: scanned.stats,
        };
        let tree = Arc::clone(&scanned.tree);
        // A failed save only means no instant map next time.
        let _ = thread::Builder::new()
            .name(String::from("topos-cache-save"))
            .stack_size(TREE_STACK_BYTES)
            .spawn(move || {
                let _ = cache::save(&file, &header, &tree);
            });
    }

    /// Status text while a scan runs: counts, the share done and time left
    /// when the root's last scan says how many files to expect, and a note
    /// when the map on screen is that last scan.
    #[must_use]
    pub fn progress_line(&self) -> Option<String> {
        let job = self.scan.as_ref()?;
        let (files, folders, bytes) = job.progress.snapshot();
        let elapsed = job.started.elapsed().as_secs_f64();
        let mut line = format!(
            "Scanning {} … {} files, {} folders, {}",
            job.root.display(),
            group_digits(files),
            group_digits(folders),
            pretty_bytes(bytes),
        );
        match progress(files, self.expected_files, elapsed) {
            Some((percent, Some(left))) => {
                line.push_str(&format!(" · {percent}% · about {left} s left"));
            }
            Some((percent, None)) => line.push_str(&format!(" · {percent}%")),
            None => line.push_str(&format!(" ({elapsed:.0} s)")),
        }
        if let Some(when) = self.stale_since.and_then(format::local_time) {
            line.push_str(&format!(" · showing the scan from {when}"));
            if let Some(took) = self.cache_read_in {
                line.push_str(&format!(" (read in {:.2} s)", took.as_secs_f64()));
            }
        }
        Some(line)
    }

    /// Filesystem path of the node at an index path.
    #[must_use]
    pub fn fs_path(&self, path: &[usize]) -> Option<PathBuf> {
        let scanned = self.scanned.as_ref()?;
        let names = scanned.tree.names_along(path)?;
        let mut out = scanned.root.clone();
        out.extend(names);
        Some(out)
    }

    /// Free bytes to show as a tile: only for a whole drive, at the top.
    #[must_use]
    pub fn free_space_tile(&self) -> Option<u64> {
        if !self.settings.show_free_space || !self.zoom.is_empty() {
            return None;
        }
        let root = &self.scanned.as_ref()?.root;
        self.drives
            .iter()
            .find(|drive| Path::new(&drive.mount) == root)
            .map(|drive| drive.free_bytes)
    }

    pub fn select(&mut self, path: Option<Vec<usize>>) {
        self.selected = path.filter(|path| {
            self.scanned
                .as_ref()
                .is_some_and(|scanned| scanned.tree.node_at(path).is_some())
        });
    }

    fn is_folder(&self, path: &[usize]) -> bool {
        self.scanned
            .as_ref()
            .and_then(|scanned| scanned.tree.node_at(path))
            .is_some_and(|node| node.is_dir())
    }

    /// Show the folder at `path`, remembering the one left for Back; a path
    /// that is not a folder, or the folder already shown, changes nothing.
    fn navigate(&mut self, path: Vec<usize>) {
        if path == self.zoom || !self.is_folder(&path) {
            return;
        }
        let left = std::mem::replace(&mut self.zoom, path);
        self.back.push(left);
        if self.back.len() > HISTORY_LIMIT {
            self.back.remove(0);
        }
        self.forward.clear();
    }

    /// Zoom to the folder at `path`; anything else is ignored.
    pub fn zoom_to(&mut self, path: Vec<usize>) {
        self.navigate(path);
    }

    /// One level down toward the tile at `relative` (an index path below
    /// the zoomed folder): into the zoomed folder's child that holds it, if
    /// that is a folder. What a scroll up over the map does.
    pub fn zoom_toward(&mut self, relative: &[usize]) {
        if let Some(&first) = relative.first() {
            let mut path = self.zoom.clone();
            path.push(first);
            self.navigate(path);
        }
    }

    #[must_use]
    pub fn can_go_back(&self) -> bool {
        self.back_target().is_some()
    }

    #[must_use]
    pub fn can_go_forward(&self) -> bool {
        self.forward_target().is_some()
    }

    /// Where Back goes: the most recent remembered folder that still exists
    /// and is not the one shown.
    #[must_use]
    pub fn back_target(&self) -> Option<&[usize]> {
        self.history_target(&self.back)
    }

    /// Where Forward goes.
    #[must_use]
    pub fn forward_target(&self) -> Option<&[usize]> {
        self.history_target(&self.forward)
    }

    fn history_target<'a>(&self, stack: &'a [Vec<usize>]) -> Option<&'a [usize]> {
        stack
            .iter()
            .rev()
            .find(|path| **path != self.zoom && self.is_folder(path))
            .map(Vec::as_slice)
    }

    /// A folder's name and size for hints, such as `Projects (62.7 Mb)`;
    /// the scan root goes by its full path.
    #[must_use]
    pub fn folder_label(&self, path: &[usize]) -> Option<String> {
        let scanned = self.scanned.as_ref()?;
        let node = scanned.tree.node_at(path)?;
        let name = if path.is_empty() {
            scanned.root.display().to_string()
        } else {
            node.name().to_owned()
        };
        Some(format!("{name} ({})", pretty_bytes(node.size())))
    }

    /// Back to the folder shown before, like a browser's Back.
    pub fn go_back(&mut self) {
        while let Some(path) = self.back.pop() {
            if path != self.zoom && self.is_folder(&path) {
                let left = std::mem::replace(&mut self.zoom, path);
                self.forward.push(left);
                return;
            }
        }
    }

    /// Forward again to where Back came from.
    pub fn go_forward(&mut self) {
        while let Some(path) = self.forward.pop() {
            if path != self.zoom && self.is_folder(&path) {
                let left = std::mem::replace(&mut self.zoom, path);
                self.back.push(left);
                return;
            }
        }
    }

    #[must_use]
    pub fn can_zoom_in(&self) -> bool {
        self.zoom_in_target().is_some()
    }

    fn zoom_in_target(&self) -> Option<Vec<usize>> {
        let scanned = self.scanned.as_ref()?;
        zoom_in_target(&scanned.tree, &self.zoom, self.selected.as_deref())
    }

    pub fn zoom_in(&mut self) {
        if let Some(path) = self.zoom_in_target() {
            self.navigate(path);
        }
    }

    /// Up one level, selecting the folder just left so it stays marked.
    pub fn zoom_out(&mut self) {
        if let Some((_, parent)) = self.zoom.split_last() {
            let parent = parent.to_vec();
            self.selected = Some(self.zoom.clone());
            self.navigate(parent);
        }
    }

    /// The status line for a pointer over a tile whose double-click opens
    /// `open` (an index path below the zoomed folder; empty for the zoomed
    /// folder itself): what a double-click and a scroll up would open.
    #[must_use]
    pub fn zoom_hint(&self, open: &[usize]) -> Option<String> {
        let first = *open.first()?;
        let scanned = self.scanned.as_ref()?;
        let describe = |relative: &[usize]| {
            let mut path = self.zoom.clone();
            path.extend_from_slice(relative);
            let node = scanned.tree.node_at(&path)?;
            Some(format!("{} ({})", node.name(), pretty_bytes(node.size())))
        };
        let deep = describe(open)?;
        if open.len() == 1 {
            return Some(format!("Double-click or scroll up to open {deep}."));
        }
        let near = describe(&[first])?;
        Some(format!(
            "Double-click to open {deep}; scroll up to open {near}."
        ))
    }

    /// Hover text for a group tile: `count` small items (`folders` of them
    /// folders, the biggest at child index `largest`) totalling `bytes`,
    /// inside `folder` (relative to the zoomed folder; empty for the zoomed
    /// folder itself).
    #[must_use]
    pub fn group_tooltip(
        &self,
        folder: &[usize],
        count: u32,
        bytes: u64,
        folders: u32,
        largest: u32,
    ) -> (String, String) {
        let files = count.saturating_sub(folders);
        let what = match (folders, files) {
            (0, _) => "files",
            (_, 0) => "folders",
            _ => "items",
        };
        let title = format!("{} small {what}", format::group_digits(u64::from(count)));
        let mut body = format!("{}\n{}", exact_bytes(bytes), pretty_bytes(bytes));
        if folders > 0 && files > 0 {
            body.push_str(&format!(
                "\n{} folders and {} files",
                format::group_digits(u64::from(folders)),
                format::group_digits(u64::from(files))
            ));
        }
        let mut path = self.zoom.clone();
        path.extend_from_slice(folder);
        let holder = self.scanned.as_ref().and_then(|s| s.tree.node_at(&path));
        if let Some(biggest) = holder.and_then(|holder| holder.child(largest as usize)) {
            body.push_str(&format!(
                "\nLargest: {} ({})",
                biggest.name(),
                pretty_bytes(biggest.size())
            ));
        }
        body.push_str("\nEach is too small to draw on its own");
        match holder.filter(|_| !folder.is_empty()) {
            Some(node) => body.push_str(&format!(
                " here.\nDouble-click to open {}, where they get more room.",
                node.name()
            )),
            None => body.push_str(" at this size; Setup > Detailed shows more of them."),
        }
        (title, body)
    }

    /// The status line after moving the selection with the keyboard.
    #[must_use]
    pub fn selection_line(&self) -> Option<String> {
        let path = self.selected.as_ref()?;
        let node = self.scanned.as_ref()?.tree.node_at(path)?;
        let kind = if node.is_dir() { "folder" } else { "file" };
        Some(format!(
            "Selected {kind} {}: {} ({}).",
            node.name(),
            pretty_bytes(node.size()),
            exact_bytes(node.size())
        ))
    }

    /// Decide what Delete does with the selection.
    pub fn delete_request(&mut self) -> DeleteRequest {
        let Some(node_path) = self.selected.clone().filter(|path| !path.is_empty()) else {
            self.status = String::from("Nothing selected.");
            return DeleteRequest::Refused;
        };
        if self.trash.is_some() {
            self.status = String::from("Still moving the last item to the Recycle Bin.");
            return DeleteRequest::Refused;
        }
        if self.stale_since.is_some() {
            self.status =
                String::from("This map is the last scan; delete once the fresh scan is in.");
            return DeleteRequest::Refused;
        }
        let size = self
            .scanned
            .as_ref()
            .and_then(|scanned| scanned.tree.node_at(&node_path))
            .map(|node| node.size());
        let (Some(fs_path), Some(size)) = (self.fs_path(&node_path), size) else {
            return DeleteRequest::Refused;
        };
        if self.settings.confirm_before_delete {
            return DeleteRequest::Confirm {
                node_path,
                fs_path,
                size,
                generation: self.generation,
            };
        }
        self.delete(&node_path);
        DeleteRequest::Started
    }

    /// Delete after a yes to [`DeleteRequest::Confirm`]. While the question
    /// was open, a scan or an earlier delete may have finished and changed
    /// what `node_path` points at; then nothing is deleted.
    pub fn delete_confirmed(&mut self, node_path: &[usize], generation: u64) -> bool {
        if generation != self.generation {
            self.status = String::from("The map changed while asking; select the item again.");
            return false;
        }
        self.delete(node_path);
        self.trash.is_some()
    }

    /// Move a node's file or folder to the Recycle Bin (never a permanent
    /// delete) on a worker thread; [`Self::poll`] takes it off the map.
    pub fn delete(&mut self, node_path: &[usize]) {
        if self.stale_since.is_some() {
            self.status =
                String::from("This map is the last scan; delete once the fresh scan is in.");
            return;
        }
        if self.trash.is_some() || node_path.is_empty() {
            return;
        }
        let Some(fs_path) = self.fs_path(node_path) else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let worker_path = fs_path.clone();
        thread::spawn(move || {
            let outcome = trash::delete(&worker_path).map_err(|err| err.to_string());
            let _ = tx.send(outcome);
        });
        self.status = format!("Moving {} to the Recycle Bin …", fs_path.display());
        self.trash = Some(TrashJob {
            node_path: node_path.to_vec(),
            fs_path,
            generation: self.generation,
            rx,
            outcome: None,
        });
    }

    fn poll_trash(&mut self) -> bool {
        let Some(job) = &mut self.trash else {
            return false;
        };
        if job.outcome.is_none() {
            job.outcome = match job.rx.try_recv() {
                Ok(outcome) => Some(outcome),
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => {
                    Some(Err(String::from("the delete worker stopped")))
                }
            };
        }
        // The cache writer may still hold the tree; try again next poll.
        let still_current = job.generation == self.generation;
        let tree_busy = self
            .scanned
            .as_mut()
            .is_some_and(|scanned| Arc::get_mut(&mut scanned.tree).is_none());
        if still_current && tree_busy && matches!(job.outcome, Some(Ok(()))) {
            return false;
        }
        if let Some(mut job) = self.trash.take()
            && let Some(outcome) = job.outcome.take()
        {
            self.finish_delete(&job, outcome);
            if self.scanned.is_some() {
                self.drives = scanner::drives();
            }
        }
        true
    }

    /// Finish a Recycle Bin move: take the node out of the tree, subtract it
    /// from every folder above it and from the totals, and keep the zoom —
    /// no rescan. A move that finishes after a rescan started changes
    /// nothing; the new scan already sees the disk as it is.
    fn finish_delete(&mut self, job: &TrashJob, outcome: Result<(), String>) {
        if let Err(err) = outcome {
            self.status = format!(
                "Could not move {} to the Recycle Bin: {err}",
                job.fs_path.display()
            );
            return;
        }
        let still_current = job.generation == self.generation;
        let removed = match self.scanned.as_mut() {
            Some(scanned) if still_current => {
                let removed =
                    Arc::get_mut(&mut scanned.tree).and_then(|tree| tree.remove_at(&job.node_path));
                if let Some(node) = &removed {
                    scanned.stats.forget(node);
                }
                removed
            }
            _ => None,
        };
        let Some(removed) = removed else {
            self.status = format!("Moved {} to the Recycle Bin.", job.fs_path.display());
            return;
        };
        let parent = &job.node_path[..job.node_path.len() - 1];
        let adjust = |path: &[usize]| {
            tree::adjust_after_removal(path, &job.node_path).unwrap_or_else(|| parent.to_vec())
        };
        self.zoom = adjust(&self.zoom);
        for path in self.back.iter_mut().chain(self.forward.iter_mut()) {
            *path = adjust(path);
        }
        self.selected = None;
        self.generation += 1;
        self.status = format!(
            "Moved {} ({}) to the Recycle Bin; the space is freed when the bin is emptied.",
            job.fs_path.display(),
            pretty_bytes(removed.size)
        );
        // Freeing a large subtree takes a moment; keep it off the caller.
        thread::spawn(move || drop(removed));
    }

    /// Window title: the target, with its drive's totals.
    #[must_use]
    pub fn title(&self) -> String {
        match self.target.as_ref() {
            Some(target) => match self.drive_of(target) {
                Some(drive) => format!(
                    "{} - {} Total - {} Free - Topos",
                    target.display(),
                    pretty_bytes(drive.total_bytes),
                    pretty_bytes(drive.free_bytes)
                ),
                None => format!("{} - Topos", target.display()),
            },
            None => String::from("Topos"),
        }
    }

    /// Counts for the status bar, once a scan is on screen.
    #[must_use]
    pub fn totals_line(&self) -> Option<String> {
        let stats = self.scanned.as_ref()?.stats;
        Some(format!(
            "Files: {}   Folders: {}   Links: {}   Unreadable: {}",
            group_digits(stats.files),
            group_digits(stats.folders),
            group_digits(stats.links),
            group_digits(stats.skipped)
        ))
    }

    /// The zoomed folder's ancestry, scan root first: each name with the
    /// index path that zooms back to it.
    #[must_use]
    pub fn breadcrumb(&self) -> Vec<(String, Vec<usize>)> {
        let Some(scanned) = &self.scanned else {
            return Vec::new();
        };
        let mut crumbs = vec![(scanned.root.display().to_string(), Vec::new())];
        let names = scanned.tree.names_along(&self.zoom).unwrap_or_default();
        for (depth, name) in names.into_iter().enumerate() {
            crumbs.push((name.to_owned(), self.zoom[..=depth].to_vec()));
        }
        crumbs
    }

    /// Hover text for the node at `relative` (an index path below the zoomed
    /// folder), or for the free-space tile when `relative` is `None`.
    #[must_use]
    pub fn tooltip(&self, relative: Option<&[usize]>) -> (String, String) {
        let Some(relative) = relative else {
            let free = self.free_space_tile().unwrap_or(0);
            return (
                String::from("Free space"),
                format!("{}\n{}", exact_bytes(free), pretty_bytes(free)),
            );
        };
        let mut path = self.zoom.clone();
        path.extend_from_slice(relative);
        let node = self
            .scanned
            .as_ref()
            .and_then(|scanned| scanned.tree.node_at(&path));
        tooltip_text(node, self.fs_path(&path).as_deref())
    }

    /// Whether the map and window draw dark: the Appearance choice, with
    /// System following Windows.
    #[must_use]
    pub fn dark(&self) -> bool {
        self.settings.appearance.is_dark(self.system_dark)
    }

    /// The key for a map `width` × `height` pixels at `scale`, with caption
    /// lines `line` pixels high; `None` before a scan is on screen.
    #[must_use]
    pub fn view_key(&self, width: u32, height: u32, scale: f32, line: u32) -> Option<ViewKey> {
        self.scanned.as_ref()?;
        Some(ViewKey {
            generation: self.generation,
            zoom: self.zoom.clone(),
            width,
            height,
            scale,
            line,
            free_space: self.free_space_tile(),
            by_type: self.settings.color_by_type,
            density: self.settings.density,
            dark: self.dark(),
        })
    }

    /// Lay out and paint the map for `key`.
    #[must_use]
    pub fn build_view(&self, key: ViewKey) -> Option<(View, Frame)> {
        let scanned = self.scanned.as_ref()?;
        let visible = scanned.tree.node_at(&key.zoom)?;
        let drive = self.drive_of(&scanned.root);
        Some(view::build(visible, key, drive, &scanned.stats))
    }
}

/// Where Zoom In goes from `zoom`: into the selected folder (or the selected
/// file's folder) when it lies below the view; otherwise into the largest
/// folder in view. `None` when there is nowhere deeper to go.
#[must_use]
pub fn zoom_in_target(
    tree: &Tree,
    zoom: &[usize],
    selected: Option<&[usize]>,
) -> Option<Vec<usize>> {
    let from_selection = selected.and_then(|selected| {
        let node = tree.node_at(selected)?;
        let folder = if node.is_dir() {
            selected
        } else {
            &selected[..selected.len().checked_sub(1)?]
        };
        (folder.len() > zoom.len() && folder.starts_with(zoom)).then(|| folder.to_vec())
    });
    from_selection.or_else(|| {
        let visible = tree.node_at(zoom)?;
        let (index, _) = visible
            .children()
            .enumerate()
            .filter(|(_, child)| child.is_dir())
            .max_by_key(|(_, child)| child.size())?;
        let mut path = zoom.to_vec();
        path.push(index);
        Some(path)
    })
}

/// Read a drive's cached scan on its own thread: the header, then the tree.
fn read_cache(file: PathBuf, root: String) -> Receiver<Cached> {
    let (tx, rx) = mpsc::channel();
    let spawned = thread::Builder::new()
        .name(String::from("topos-cache-read"))
        .stack_size(TREE_STACK_BYTES)
        .spawn(move || {
            let started = Instant::now();
            let Ok(header) = cache::load_header(&file) else {
                return;
            };
            if header.root != root || tx.send(Cached::Header(header)).is_err() {
                return;
            }
            if let Ok((header, tree)) = cache::load(&file, &root) {
                let _ = tx.send(Cached::Tree(header, tree, started.elapsed()));
            }
        });
    // No thread, no cache: the scan goes on as if there were none.
    drop(spawned);
    rx
}

/// The share of a scan done, from the files found so far against the files
/// the last scan found: capped at 0.99 while running, since a drive can
/// gain files; `None` without an estimate.
#[must_use]
pub fn share_done(found: u64, expected: Option<u64>) -> Option<f32> {
    let expected = expected.filter(|&expected| expected > 0)?;
    // A share of at most 0.99: well inside f32.
    Some((found as f64 / expected as f64).min(0.99) as f32)
}

/// The share of a scan done and the seconds left, from the files found so
/// far against the files the last scan found: percent capped at 99 while
/// running, and a time once 5% is done. `None` without an estimate.
#[must_use]
pub fn progress(found: u64, expected: Option<u64>, elapsed: f64) -> Option<(u64, Option<u64>)> {
    let expected = expected.filter(|&expected| expected > 0)?;
    let fraction = (found as f64 / expected as f64).min(0.99);
    // In 0..=99 by the clamp above.
    let percent = (fraction * 100.0).floor() as u64;
    let left = (fraction >= 0.05).then(|| (elapsed * (1.0 - fraction) / fraction).ceil() as u64);
    Some((percent, left))
}

/// `path` in `old` carried over to `new` by name: the longest prefix whose
/// names all exist in `new` (folders only when `folders_only`).
fn carry_path(old: &Tree, new: &Tree, path: &[usize], folders_only: bool) -> Vec<usize> {
    let Some(names) = old.names_along(path) else {
        return Vec::new();
    };
    let mut carried = Vec::with_capacity(names.len());
    let mut node = new.root();
    for name in names {
        let Some((index, child)) = node
            .children()
            .enumerate()
            .find(|(_, child)| child.name() == name)
        else {
            break;
        };
        if folders_only && !child.is_dir() {
            break;
        }
        carried.push(index);
        node = child;
    }
    carried
}

/// Hover title and body for a node.
#[must_use]
pub fn tooltip_text(node: Option<NodeRef<'_>>, path: Option<&Path>) -> (String, String) {
    let Some(node) = node else {
        return (String::new(), String::new());
    };
    let mut body = format!(
        "{}\n{}",
        exact_bytes(node.size()),
        pretty_bytes(node.size())
    );
    if let Some(time) = node.modified().and_then(format::local_time) {
        body.push('\n');
        body.push_str(&time);
    }
    if node.unreadable() {
        body.push_str("\nContents could not be read.");
    }
    if let Some(path) = path {
        body.push('\n');
        body.push_str(&path.display().to_string());
    }
    (node.name().to_owned(), body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::Node;
    use std::fs;
    use std::time::Duration;

    /// A fresh, empty directory for one test.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("topos-session-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Poll until the running scan finishes.
    fn finish_scan(session: &mut Session) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while session.is_scanning() {
            session.poll();
            assert!(Instant::now() < deadline, "scan did not finish");
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// root/{big/{a.bin 300, b.bin 100}, small/{c.bin 50}, top.bin 10}
    fn scanned_session(name: &str) -> (Session, PathBuf) {
        let dir = temp_dir(name);
        fs::create_dir_all(dir.join("big")).unwrap();
        fs::create_dir_all(dir.join("small")).unwrap();
        fs::write(dir.join("big").join("a.bin"), vec![0u8; 300]).unwrap();
        fs::write(dir.join("big").join("b.bin"), vec![0u8; 100]).unwrap();
        fs::write(dir.join("small").join("c.bin"), vec![0u8; 50]).unwrap();
        fs::write(dir.join("top.bin"), vec![0u8; 10]).unwrap();
        let mut session = Session::new(Some(dir.clone()), Vec::new(), None);
        finish_scan(&mut session);
        (session, dir)
    }

    fn nav_tree() -> Tree {
        Tree::from(Node::dir(
            "root",
            vec![
                Node::dir(
                    "big",
                    vec![
                        Node::dir("inner", vec![Node::file("x", 5, None)], None),
                        Node::file("y", 1, None),
                    ],
                    None,
                ),
                Node::dir("small", vec![Node::file("z", 2, None)], None),
                Node::file("loose", 1, None),
            ],
            None,
        ))
    }

    #[test]
    fn zoom_in_prefers_the_selection_then_the_largest_folder() {
        let tree = nav_tree();
        assert_eq!(zoom_in_target(&tree, &[], None), Some(vec![0]));
        assert_eq!(zoom_in_target(&tree, &[], Some(&[1])), Some(vec![1]));
        // A selected file zooms to its folder.
        assert_eq!(zoom_in_target(&tree, &[], Some(&[1, 0])), Some(vec![1]));
        // A selection outside the view falls back to the largest folder.
        assert_eq!(zoom_in_target(&tree, &[0], Some(&[1])), Some(vec![0, 0]));
        // A file at the top has no folder below the view.
        assert_eq!(zoom_in_target(&tree, &[], Some(&[2])), Some(vec![0]));
        assert_eq!(zoom_in_target(&tree, &[0, 0], None), None);
    }

    #[test]
    fn a_scan_fills_the_session() {
        let (session, dir) = scanned_session("scan");
        let scanned = session.scanned().unwrap();
        assert_eq!(scanned.root, dir);
        assert_eq!(scanned.stats.files, 4);
        assert_eq!(scanned.tree.root().size(), 460);
        assert!(session.status().starts_with("Scanned "));
        assert!(!session.busy());
        assert_eq!(session.generation(), 2, "one bump to start, one to show");
        let crumbs = session.breadcrumb();
        assert_eq!(crumbs, vec![(dir.display().to_string(), Vec::new())]);
        assert_eq!(
            session.totals_line().unwrap(),
            "Files: 4   Folders: 3   Links: 0   Unreadable: 0",
            "the scan root counts as a folder"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn zooming_follows_folders_and_marks_the_way_back() {
        let (mut session, dir) = scanned_session("zoom");
        // Children sort by size: big (400), small (50), top.bin (10).
        session.zoom_to(vec![2]);
        assert!(session.zoom().is_empty(), "a file is not a zoom target");
        session.zoom_in();
        assert_eq!(session.zoom(), &[0]);
        let crumbs = session.breadcrumb();
        assert_eq!(crumbs[1], (String::from("big"), vec![0]));
        session.zoom_out();
        assert!(session.zoom().is_empty());
        assert_eq!(session.selected(), Some(&[0][..]));
        session.select(Some(vec![9, 9]));
        assert_eq!(session.selected(), None, "no such node");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn deletes_ask_first_by_default() {
        let (mut session, dir) = scanned_session("ask");
        assert_eq!(session.delete_request(), DeleteRequest::Refused);
        assert_eq!(session.status(), "Nothing selected.");
        session.select(Some(vec![1]));
        let generation = session.generation();
        assert_eq!(
            session.delete_request(),
            DeleteRequest::Confirm {
                node_path: vec![1],
                fs_path: dir.join("small"),
                size: 50,
                generation,
            }
        );
        assert!(!session.is_deleting(), "nothing moves before a yes");
        assert!(dir.join("small").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_yes_to_an_old_question_deletes_nothing() {
        let (mut session, dir) = scanned_session("stale");
        session.select(Some(vec![1]));
        let DeleteRequest::Confirm {
            node_path,
            generation,
            ..
        } = session.delete_request()
        else {
            panic!("expected a question");
        };
        // A rescan finishes while the question is open.
        session.reload();
        finish_scan(&mut session);
        assert!(!session.delete_confirmed(&node_path, generation));
        assert!(!session.is_deleting());
        assert!(session.status().contains("changed"));
        assert!(dir.join("small").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_finished_delete_takes_the_node_off_the_map() {
        let (mut session, dir) = scanned_session("finish");
        session.zoom_to(vec![0]);
        session.select(Some(vec![0, 0]));
        let generation = session.generation();
        let (_tx, rx) = mpsc::channel();
        let job = TrashJob {
            node_path: vec![0, 0],
            fs_path: dir.join("big").join("a.bin"),
            generation,
            rx,
            outcome: None,
        };
        session.finish_delete(&job, Ok(()));
        let scanned = session.scanned().unwrap();
        assert_eq!(scanned.tree.root().size(), 160);
        assert_eq!(scanned.tree.node_at(&[0]).unwrap().size(), 100);
        assert_eq!(scanned.stats.files, 3);
        assert_eq!(session.zoom(), &[0]);
        assert_eq!(session.selected(), None);
        assert!(session.generation() > generation);
        assert!(session.status().starts_with("Moved "));

        // A move that finishes after a rescan started changes nothing.
        let stale = TrashJob {
            node_path: vec![0, 0],
            fs_path: dir.join("big").join("b.bin"),
            generation,
            rx: mpsc::channel().1,
            outcome: None,
        };
        session.finish_delete(&stale, Ok(()));
        assert_eq!(session.scanned().unwrap().tree.root().size(), 160);

        // A failed move keeps the node and says why.
        let failed = TrashJob {
            node_path: vec![0, 0],
            fs_path: dir.join("big").join("b.bin"),
            generation: session.generation(),
            rx: mpsc::channel().1,
            outcome: None,
        };
        session.finish_delete(&failed, Err(String::from("in use")));
        assert_eq!(session.scanned().unwrap().tree.root().size(), 160);
        assert!(session.status().ends_with("in use"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn removing_the_zoomed_folder_zooms_to_its_parent() {
        let (mut session, dir) = scanned_session("parent");
        session.zoom_to(vec![1]);
        let job = TrashJob {
            node_path: vec![1],
            fs_path: dir.join("small"),
            generation: session.generation(),
            rx: mpsc::channel().1,
            outcome: None,
        };
        session.finish_delete(&job, Ok(()));
        assert!(session.zoom().is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_view_follows_the_zoom_and_settings() {
        let (mut session, dir) = scanned_session("view");
        let key = session.view_key(400, 300, 1.0, 14).unwrap();
        assert_eq!(key.free_space, None, "a folder scan has no free space");
        let (view, frame) = session.build_view(key.clone()).unwrap();
        assert_eq!((frame.width(), frame.height()), (400, 300));
        assert!(!view.tiles.is_empty());
        session.settings.color_by_type = true;
        assert_ne!(session.view_key(400, 300, 1.0, 14).unwrap(), key);
        // Dark follows Windows under System, and a fixed choice overrides it.
        assert!(!session.view_key(400, 300, 1.0, 14).unwrap().dark);
        session.system_dark = true;
        assert!(session.view_key(400, 300, 1.0, 14).unwrap().dark);
        session.settings.appearance = crate::theme::Appearance::Light;
        assert!(!session.dark());
        session.settings.appearance = crate::theme::Appearance::Dark;
        session.system_dark = false;
        assert!(session.dark());
        session.settings.appearance = crate::theme::Appearance::System;
        session.zoom_to(vec![0]);
        let zoomed = session.view_key(400, 300, 1.0, 14).unwrap();
        assert_eq!(zoomed.zoom, vec![0]);
        let (view, _) = session.build_view(zoomed).unwrap();
        assert_eq!(view.tiles.len(), 2, "a.bin and b.bin");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn back_and_forward_walk_where_the_map_has_been() {
        // root = [big = [a.bin, b.bin], small = [c.bin], top.bin]
        let (mut session, dir) = scanned_session("history");
        assert!(!session.can_go_back() && !session.can_go_forward());
        session.zoom_to(vec![0]);
        session.zoom_to(vec![1]);
        assert_eq!(session.zoom(), [1]);
        session.go_back();
        assert_eq!(session.zoom(), [0]);
        session.go_back();
        assert!(session.zoom().is_empty());
        assert!(!session.can_go_back());
        assert_eq!(session.forward_target(), Some(&[0][..]));
        session.go_forward();
        assert_eq!(session.zoom(), [0]);
        assert_eq!(session.back_target(), Some(&[][..]));
        assert_eq!(session.forward_target(), Some(&[1][..]));
        // A new step drops what Forward had, and Back undoes a zoom out.
        session.zoom_out();
        assert!(session.zoom().is_empty());
        assert!(!session.can_go_forward());
        session.go_back();
        assert_eq!(session.zoom(), [0]);
        // Files and the folder already shown add nothing to Back.
        let before = session.back.len();
        session.zoom_to(vec![0]);
        session.zoom_to(vec![2]);
        session.zoom_to(vec![0, 0]);
        assert_eq!(session.back.len(), before);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn history_follows_a_rescan_and_ends_with_the_target() {
        let (mut session, dir) = scanned_session("history-rescan");
        session.zoom_to(vec![0]);
        session.zoom_to(vec![1]);
        session.reload();
        finish_scan(&mut session);
        assert_eq!(session.zoom(), [1]);
        session.go_back();
        assert_eq!(session.zoom(), [0], "carried over by name");
        let other = temp_dir("history-other");
        session.start_scan(other.clone());
        finish_scan(&mut session);
        assert!(!session.can_go_back() && !session.can_go_forward());
        fs::remove_dir_all(&dir).unwrap();
        fs::remove_dir_all(&other).unwrap();
    }

    #[test]
    fn scrolling_up_goes_one_level_toward_the_pointer() {
        let (mut session, dir) = scanned_session("toward");
        // Over b.bin inside big: into big, not further.
        session.zoom_toward(&[0, 1]);
        assert_eq!(session.zoom(), [0]);
        // Over a file directly in view: nowhere to go.
        session.zoom_toward(&[0]);
        session.zoom_toward(&[]);
        assert_eq!(session.zoom(), [0]);
        assert!(session.can_go_back());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hints_name_what_a_double_click_and_a_scroll_open() {
        let dir = temp_dir("hint");
        fs::create_dir_all(dir.join("outer").join("inner")).unwrap();
        fs::write(dir.join("outer").join("inner").join("f.bin"), vec![0u8; 64]).unwrap();
        let mut session = Session::new(Some(dir.clone()), Vec::new(), None);
        finish_scan(&mut session);
        assert_eq!(session.zoom_hint(&[]), None, "the folder on the map");
        let near = session.zoom_hint(&[0]).unwrap();
        assert!(
            near.starts_with("Double-click or scroll up to open outer ("),
            "{near}"
        );
        let deep = session.zoom_hint(&[0, 0]).unwrap();
        assert!(deep.starts_with("Double-click to open inner ("), "{deep}");
        assert!(deep.contains("; scroll up to open outer ("), "{deep}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn groups_and_keyboard_selections_describe_themselves() {
        let (mut session, dir) = scanned_session("describe");
        // root = [big = [a.bin, b.bin], small = [c.bin], top.bin]
        let (title, body) = session.group_tooltip(&[0], 2, 1234, 0, 1);
        assert_eq!(title, "2 small files");
        assert!(body.starts_with("1,234 bytes\n"), "{body}");
        assert!(body.contains("\nLargest: b.bin ("), "{body}");
        assert!(body.contains("Double-click to open big"), "{body}");
        let (title, body) = session.group_tooltip(&[], 1500, 9, 500, 0);
        assert_eq!(title, "1,500 small items");
        assert!(body.contains("500 folders and 1,000 files"), "{body}");
        assert!(body.contains("Largest: big ("), "{body}");
        assert!(
            body.ends_with("Setup > Detailed shows more of them."),
            "{body}"
        );
        assert_eq!(session.selection_line(), None);
        let root_label = session.folder_label(&[]).unwrap();
        assert!(
            root_label.starts_with(&dir.display().to_string()),
            "{root_label}"
        );
        assert!(session.folder_label(&[0]).unwrap().starts_with("big ("));
        assert_eq!(session.folder_label(&[9]), None);
        session.select(Some(vec![0, 0]));
        let line = session.selection_line().unwrap();
        assert!(line.starts_with("Selected file a.bin: "), "{line}");
        session.select(Some(vec![1]));
        assert!(
            session
                .selection_line()
                .unwrap()
                .starts_with("Selected folder small: ")
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn titles_and_tooltips_describe_the_target() {
        let (session, dir) = scanned_session("title");
        assert_eq!(session.title(), format!("{} - Topos", dir.display()));
        let (title, body) = session.tooltip(Some(&[0]));
        assert_eq!(title, "big");
        assert!(body.starts_with("400 bytes\n"));
        assert!(body.ends_with(&dir.join("big").display().to_string()));
        let (free, _) = session.tooltip(None);
        assert_eq!(free, "Free space");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_stopped_scan_leaves_nothing_on_screen() {
        let dir = temp_dir("stop");
        let mut session = Session::new(Some(dir.clone()), Vec::new(), None);
        session.stop_scan();
        assert!(!session.busy());
        assert!(session.scanned().is_none());
        assert!(session.status().starts_with("Stopped scanning"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unreadable_folders_say_so_in_tooltips() {
        let mut node = Node::dir("System Volume Information", Vec::new(), None);
        node.unreadable = true;
        let tree = Tree::from(Node::dir("root", vec![node], None));
        let (_, body) = tooltip_text(tree.node_at(&[0]), None);
        assert!(body.contains("could not be read"));
    }

    #[test]
    fn drives_are_matched_by_longest_mount() {
        let drives = vec![
            DriveInfo {
                mount: String::from("/"),
                total_bytes: 10,
                free_bytes: 5,
            },
            DriveInfo {
                mount: String::from("/mnt/data"),
                total_bytes: 20,
                free_bytes: 7,
            },
        ];
        let session = Session {
            drives,
            target: None,
            scan: None,
            trash: None,
            scanned: None,
            generation: 0,
            zoom: Vec::new(),
            selected: None,
            settings: Settings::default(),
            system_dark: false,
            status: String::new(),
            cache_dir: None,
            cached: None,
            stale_since: None,
            cache_read_in: None,
            expected_files: None,
            back: Vec::new(),
            forward: Vec::new(),
        };
        assert_eq!(
            session.drive_of(Path::new("/mnt/data/x")).unwrap().mount,
            "/mnt/data"
        );
        assert_eq!(session.drive_of(Path::new("/home")).unwrap().mount, "/");
        assert!(!session.target_is_drive(), "no target");
        let session = Session {
            target: Some(PathBuf::from("/mnt/data")),
            ..session
        };
        assert!(session.target_is_drive());
        let session = Session {
            target: Some(PathBuf::from("/mnt/data/photos")),
            ..session
        };
        assert!(
            !session.target_is_drive(),
            "a folder has no free space tile"
        );
    }

    #[test]
    fn the_share_done_is_capped_and_needs_an_estimate() {
        assert_eq!(share_done(50, Some(200)), Some(0.25));
        assert_eq!(share_done(500, Some(200)), Some(0.99));
        assert_eq!(share_done(5, None), None);
        assert_eq!(share_done(5, Some(0)), None);
    }

    #[test]
    fn activity_follows_the_scan() {
        let dir = temp_dir("activity");
        fs::write(dir.join("f.bin"), vec![0u8; 10]).unwrap();
        let mut session = Session::new(Some(dir.clone()), Vec::new(), None);
        assert!(matches!(
            session.activity(),
            Some(Activity::Scanning { done: None })
        ));
        finish_scan(&mut session);
        assert_eq!(session.activity(), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn progress_is_estimated_from_the_last_scan() {
        assert_eq!(progress(10, None, 1.0), None);
        assert_eq!(progress(10, Some(0), 1.0), None);
        assert_eq!(progress(0, Some(100), 1.0), Some((0, None)));
        assert_eq!(progress(50, Some(100), 2.0), Some((50, Some(2))));
        assert_eq!(progress(25, Some(100), 1.0), Some((25, Some(3))));
        // More files than last time: still running, so never 100%.
        assert_eq!(progress(150, Some(100), 3.0).map(|p| p.0), Some(99));
    }

    #[test]
    fn paths_carry_over_by_name() {
        let old = Tree::from(Node::dir(
            "r",
            vec![
                Node::dir("a", vec![Node::file("x", 2, None)], None),
                Node::dir("b", vec![Node::file("y", 1, None)], None),
            ],
            None,
        ));
        // `b` grew past `a`, so the order flipped; `x` is gone.
        let new = Tree::from(Node::dir(
            "r",
            vec![
                Node::dir("b", vec![Node::file("y", 9, None)], None),
                Node::dir("a", Vec::new(), None),
            ],
            None,
        ));
        assert_eq!(carry_path(&old, &new, &[1, 0], false), vec![0, 0]);
        assert_eq!(carry_path(&old, &new, &[0, 0], false), vec![1]);
        assert_eq!(
            carry_path(&old, &new, &[1, 0], true),
            vec![0],
            "y is a file"
        );
        assert!(carry_path(&old, &new, &[7], false).is_empty());
    }

    /// A session whose only "drive" is `dir`, caching into `cache`.
    fn drive_session(dir: &Path, cache: &Path) -> Session {
        let drives = vec![DriveInfo {
            mount: dir.to_string_lossy().into_owned(),
            total_bytes: 1 << 30,
            free_bytes: 1 << 20,
        }];
        Session::new(Some(dir.to_path_buf()), drives, Some(cache.to_path_buf()))
    }

    #[test]
    fn the_last_scan_shows_until_the_fresh_one_lands() {
        let (_, dir) = scanned_session("cached");
        let cache_dir = temp_dir("cached-store");
        let file = cache::file_for(&cache_dir, &dir);

        // First run: scan, then the cache is written in the background.
        let mut first = drive_session(&dir, &cache_dir);
        finish_scan(&mut first);
        let deadline = Instant::now() + Duration::from_secs(20);
        while cache::load(&file, &dir.to_string_lossy()).is_err() {
            assert!(Instant::now() < deadline, "cache was not written");
            thread::sleep(Duration::from_millis(5));
        }
        drop(first);

        // Second run: show the cached map before taking the scan result.
        fs::write(dir.join("big").join("new.bin"), vec![0u8; 40]).unwrap();
        let mut second = drive_session(&dir, &cache_dir);
        let deadline = Instant::now() + Duration::from_secs(20);
        while !second.poll_cache() {
            assert!(Instant::now() < deadline, "cache was not read");
            thread::sleep(Duration::from_millis(5));
        }
        assert!(second.is_scanning());
        assert!(second.stale_since().is_some());
        assert_eq!(second.scanned().unwrap().tree.root().size(), 460);
        assert!(
            second
                .progress_line()
                .unwrap()
                .contains("showing the scan from")
        );
        second.zoom_to(vec![0]);
        second.select(Some(vec![0, 0]));
        assert!(!second.can_delete(), "no deletes from an old map");
        assert_eq!(second.delete_request(), DeleteRequest::Refused);

        // The fresh scan replaces it; the zoom and selection follow by name.
        finish_scan(&mut second);
        assert!(second.stale_since().is_none());
        assert_eq!(second.scanned().unwrap().tree.root().size(), 500);
        assert_eq!(second.zoom(), &[0]);
        assert_eq!(second.selected(), Some(&[0, 0][..]));
        assert!(second.can_delete());
        fs::remove_dir_all(&dir).unwrap();
        fs::remove_dir_all(&cache_dir).unwrap();
    }

    #[test]
    fn reload_keeps_the_map_until_the_new_scan() {
        let (mut session, dir) = scanned_session("reload");
        session.zoom_to(vec![0]);
        session.reload();
        assert!(
            session.scanned().is_some(),
            "the map stays during the rescan"
        );
        assert!(session.stale_since().is_some());
        assert!(session.progress_line().unwrap().contains('%'));
        finish_scan(&mut session);
        assert!(session.stale_since().is_none());
        assert_eq!(session.zoom(), &[0]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_delete_waits_for_the_cache_writer_to_let_go() {
        let (mut session, dir) = scanned_session("wait");
        session.select(Some(vec![2]));
        let held = Arc::clone(&session.scanned().unwrap().tree);
        session.trash = Some(TrashJob {
            node_path: vec![2],
            fs_path: dir.join("top.bin"),
            generation: session.generation(),
            rx: mpsc::channel().1,
            outcome: Some(Ok(())),
        });
        assert!(!session.poll_trash(), "tree still shared");
        assert!(session.is_deleting());
        drop(held);
        assert!(session.poll_trash());
        assert!(!session.is_deleting());
        assert_eq!(session.scanned().unwrap().tree.root().size(), 450);
        fs::remove_dir_all(&dir).unwrap();
    }
}
