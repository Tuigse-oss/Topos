//! The scanned tree, stored compactly, and the edits the program makes to it.
//!
//! A scan of a whole drive holds millions of entries, so a folder's direct
//! contents live together in one [`Dir`]: a slice of fixed-size entries,
//! their names back to back in one string (in entry order), and the child
//! folders' own `Dir`s. There is no allocation per file (24 bytes plus
//! the name's own bytes). Code reads
//! the tree through [`NodeRef`], a small copyable view of one entry.
//!
//! A folder's size is always the sum of its children's sizes, and its
//! children are ordered largest first (ties by name) by the scanner. A node
//! is addressed by its *index path*: the child index at each level below
//! the root, so `[]` is the root and `[2, 0]` is the first child of the
//! root's third child. Index paths stay valid across edits except where
//! [`adjust_after_removal`] says otherwise.
//!
//! [`Node`] is a plain owned tree for building small trees in tests and
//! benches; [`Tree::from`] packs it. [`Tree::write_to`] and
//! [`Tree::read_from`] store a tree on disk for the scan cache.

use std::io::{self, Read, Write};

/// A scanned tree: the root folder and everything below it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    /// The root's name: the path it was scanned from.
    name: Box<str>,
    size: u64,
    modified: i64,
    unreadable: bool,
    root: Dir,
}

/// One folder's direct contents.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dir {
    /// Files and subfolders, largest first.
    entries: Box<[Entry]>,
    /// The entries' names, back to back.
    names: Box<str>,
    /// Each subfolder's contents; entries point here by index.
    dirs: Box<[Dir]>,
}

/// One file or folder inside a [`Dir`]: 24 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    /// Bytes; for a folder, everything below it.
    size: u64,
    /// Last write time in whole seconds since the Unix epoch, or [`NO_TIME`].
    modified: i64,
    /// Where the name starts in the `Dir`'s `names`; it ends where the next
    /// entry's starts (or at the end), since names are stored in entry order.
    name_start: u32,
    /// [`NO_DIR`] for a file. For a folder, its index into the `Dir`'s
    /// `dirs`, with [`UNREADABLE`] set when it could not be listed.
    dir: u32,
}

/// The stored last-write time when there is none. The earliest second an
/// `i64` holds, far before any file time Windows can report (1601), so no
/// real time maps to it (an `Option<i64>` would take 16 bytes).
const NO_TIME: i64 = i64::MIN;
const NO_DIR: u32 = u32::MAX;
const UNREADABLE: u32 = 1 << 31;
/// The most subfolders one folder can index (the top bit is the flag, and
/// all ones means "a file").
const MAX_DIRS: u32 = UNREADABLE - 1;

impl Entry {
    fn is_dir(&self) -> bool {
        self.dir != NO_DIR
    }

    fn dir_index(&self) -> Option<usize> {
        self.is_dir().then_some((self.dir & !UNREADABLE) as usize)
    }

    fn unreadable(&self) -> bool {
        self.is_dir() && self.dir & UNREADABLE != 0
    }
}

fn time_of(modified: i64) -> Option<i64> {
    (modified != NO_TIME).then_some(modified)
}

/// A view of one file or folder in a [`Tree`]: two references and an
/// index, so walking the tree builds nothing. The name is sliced out only
/// when asked for.
#[derive(Debug, Clone, Copy)]
pub struct NodeRef<'a>(Place<'a>);

#[derive(Debug, Clone, Copy)]
enum Place<'a> {
    Root(&'a Tree),
    /// Entry `index` of a folder's contents.
    Child(&'a Dir, &'a Entry, usize),
}

impl<'a> NodeRef<'a> {
    /// A folder's contents; `None` for a file.
    fn contents(&self) -> Option<&'a Dir> {
        match self.0 {
            Place::Root(tree) => Some(&tree.root),
            Place::Child(parent, entry, _) => {
                entry.dir_index().and_then(|index| parent.dirs.get(index))
            }
        }
    }

    #[must_use]
    pub fn name(&self) -> &'a str {
        match self.0 {
            Place::Root(tree) => &tree.name,
            Place::Child(parent, _, index) => parent.name_of(index),
        }
    }

    /// Size in bytes. For a folder, the sum over everything below it.
    #[must_use]
    pub fn size(&self) -> u64 {
        match self.0 {
            Place::Root(tree) => tree.size,
            Place::Child(_, entry, _) => entry.size,
        }
    }

    /// Last write time in whole seconds since the Unix epoch, if known.
    #[must_use]
    pub fn modified(&self) -> Option<i64> {
        time_of(match self.0 {
            Place::Root(tree) => tree.modified,
            Place::Child(_, entry, _) => entry.modified,
        })
    }

    #[must_use]
    pub fn is_dir(&self) -> bool {
        match self.0 {
            Place::Root(_) => true,
            Place::Child(_, entry, _) => entry.is_dir(),
        }
    }

    /// A folder whose listing could not be read: shown, contents unknown.
    #[must_use]
    pub fn unreadable(&self) -> bool {
        match self.0 {
            Place::Root(tree) => tree.unreadable,
            Place::Child(_, entry, _) => entry.unreadable(),
        }
    }

    /// Number of direct children (0 for a file).
    #[must_use]
    pub fn child_count(&self) -> usize {
        self.contents().map_or(0, |dir| dir.entries.len())
    }

    /// The child at `index`.
    #[must_use]
    pub fn child(&self, index: usize) -> Option<NodeRef<'a>> {
        self.contents()?.get(index)
    }

    /// The direct children, in order (largest first).
    #[must_use]
    pub fn children(&self) -> Children<'a> {
        let dir = self.contents();
        Children {
            dir,
            front: 0,
            back: dir.map_or(0, |dir| dir.entries.len()),
        }
    }

    /// The node at `path` below this one.
    #[must_use]
    pub fn node_at(self, path: &[usize]) -> Option<NodeRef<'a>> {
        path.iter().try_fold(self, |node, &index| node.child(index))
    }
}

/// The children of a [`NodeRef`], in order; see [`NodeRef::children`].
#[derive(Debug, Clone)]
pub struct Children<'a> {
    dir: Option<&'a Dir>,
    front: usize,
    back: usize,
}

impl<'a> Iterator for Children<'a> {
    type Item = NodeRef<'a>;

    fn next(&mut self) -> Option<NodeRef<'a>> {
        if self.front >= self.back {
            return None;
        }
        self.front += 1;
        self.dir?.get(self.front - 1)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.back - self.front;
        (left, Some(left))
    }
}

impl DoubleEndedIterator for Children<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.front >= self.back {
            return None;
        }
        self.back -= 1;
        self.dir?.get(self.back)
    }
}

impl ExactSizeIterator for Children<'_> {}

impl Dir {
    /// Total size of the folder's contents.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.entries
            .iter()
            .fold(0_u64, |sum, entry| sum.saturating_add(entry.size))
    }

    fn get(&self, index: usize) -> Option<NodeRef<'_>> {
        let entry = self.entries.get(index)?;
        Some(NodeRef(Place::Child(self, entry, index)))
    }

    /// Entry `index`'s name: from its start to the next entry's start.
    fn name_of(&self, index: usize) -> &str {
        let Some(entry) = self.entries.get(index) else {
            return "";
        };
        let start = entry.name_start as usize;
        let end = self
            .entries
            .get(index + 1)
            .map_or(self.names.len(), |next| next.name_start as usize);
        self.names.get(start..end).unwrap_or_default()
    }

    /// Files and folders below this folder, not counting itself.
    fn counts(&self) -> (u64, u64) {
        let (mut files, mut folders) = (0_u64, 0_u64);
        let mut stack: Vec<&Dir> = vec![self];
        while let Some(dir) = stack.pop() {
            for entry in &dir.entries {
                if entry.is_dir() {
                    folders += 1;
                } else {
                    files += 1;
                }
            }
            stack.extend(dir.dirs.iter());
        }
        (files, folders)
    }
}

/// What [`Tree::remove_at`] took out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removed {
    /// Bytes, everything below it included.
    pub size: u64,
    /// Files and folders removed, the node itself included.
    pub files: u64,
    pub folders: u64,
    /// A removed folder's contents (empty for a file), handed back so a
    /// large one can be freed off the caller's thread.
    pub contents: Dir,
}

impl Tree {
    /// The root folder.
    #[must_use]
    pub fn root(&self) -> NodeRef<'_> {
        NodeRef(Place::Root(self))
    }

    /// The node at `path`, or `None` when the path leaves the tree.
    #[must_use]
    pub fn node_at(&self, path: &[usize]) -> Option<NodeRef<'_>> {
        self.root().node_at(path)
    }

    /// Names along `path`, root excluded; `None` when the path leaves the
    /// tree.
    #[must_use]
    pub fn names_along(&self, path: &[usize]) -> Option<Vec<&str>> {
        let mut names = Vec::with_capacity(path.len());
        let mut node = self.root();
        for &index in path {
            node = node.child(index)?;
            names.push(node.name());
        }
        Some(names)
    }

    /// Files and folders in the tree, the root folder included.
    #[must_use]
    pub fn counts(&self) -> (u64, u64) {
        let (files, folders) = self.root.counts();
        (files, folders + 1)
    }

    /// Remove the node at `path` (never the root) and subtract its size from
    /// every ancestor. Sibling order is left alone, so other index paths
    /// change only as [`adjust_after_removal`] describes. The removed name's
    /// bytes stay in its folder's name string until the next scan.
    pub fn remove_at(&mut self, path: &[usize]) -> Option<Removed> {
        let (&last, parents) = path.split_last()?;
        let removed = remove_below(&mut self.root, parents, last)?;
        self.size = self.size.saturating_sub(removed.size);
        Some(removed)
    }
}

fn remove_below(dir: &mut Dir, parents: &[usize], last: usize) -> Option<Removed> {
    let Some((&first, rest)) = parents.split_first() else {
        return remove_entry(dir, last);
    };
    let index = dir.entries.get(first)?.dir_index()?;
    let removed = remove_below(dir.dirs.get_mut(index)?, rest, last)?;
    if let Some(entry) = dir.entries.get_mut(first) {
        entry.size = entry.size.saturating_sub(removed.size);
    }
    Some(removed)
}

/// Take entry `index` out of `dir`: its entry, its name's bytes (later names
/// move down), and for a folder its contents (later folder indices move
/// down).
fn remove_entry(dir: &mut Dir, index: usize) -> Option<Removed> {
    let entry = *dir.entries.get(index)?;
    let start = entry.name_start as usize;
    let end = dir
        .entries
        .get(index + 1)
        .map_or(dir.names.len(), |next| next.name_start as usize);
    let (Some(before), Some(after)) = (dir.names.get(..start), dir.names.get(end..)) else {
        return None;
    };
    let mut names = String::with_capacity(before.len() + after.len());
    names.push_str(before);
    names.push_str(after);
    // `end - start` came from offsets that fit in u32.
    let shift = u32::try_from(end - start).unwrap_or(0);

    let mut entries = std::mem::take(&mut dir.entries).into_vec();
    entries.remove(index);
    for later in &mut entries[index..] {
        later.name_start = later.name_start.saturating_sub(shift);
    }
    let removed = match entry.dir_index() {
        None => Removed {
            size: entry.size,
            files: 1,
            folders: 0,
            contents: Dir::default(),
        },
        Some(gone_index) => {
            let mut dirs = std::mem::take(&mut dir.dirs).into_vec();
            let gone = dirs.remove(gone_index);
            for other in &mut entries {
                if let Some(other_index) = other.dir_index()
                    && other_index > gone_index
                {
                    other.dir -= 1;
                }
            }
            dir.dirs = dirs.into_boxed_slice();
            let (files, folders) = gone.counts();
            Removed {
                size: entry.size,
                files,
                folders: folders + 1,
                contents: gone,
            }
        }
    };
    dir.entries = entries.into_boxed_slice();
    dir.names = names.into_boxed_str();
    Some(removed)
}

/// Builds one folder's [`Dir`]: add its files and subfolders in any order,
/// then [`finish`](Self::finish) sorts them largest first.
#[derive(Debug, Default)]
pub struct DirBuilder {
    /// Entries with their name's length (names here are in the order added).
    entries: Vec<(Entry, u32)>,
    names: String,
    dirs: Vec<Dir>,
}

impl DirBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn push_name(&mut self, name: &str) -> (u32, u32) {
        // A folder's names together past 4 GiB cannot come from a
        // filesystem; keep what fits rather than fail the scan.
        let room = (u32::MAX as usize).saturating_sub(self.names.len());
        let mut len = name.len().min(room);
        while !name.is_char_boundary(len) {
            len -= 1;
        }
        let start = u32::try_from(self.names.len()).unwrap_or(u32::MAX);
        self.names.push_str(&name[..len]);
        (start, u32::try_from(len).unwrap_or(0))
    }

    /// Add a file.
    pub fn file(&mut self, name: &str, size: u64, modified: Option<i64>) {
        let (name_start, len) = self.push_name(name);
        let entry = Entry {
            size,
            modified: modified.unwrap_or(NO_TIME),
            name_start,
            dir: NO_DIR,
        };
        self.entries.push((entry, len));
    }

    /// Add a subfolder with its contents (`size` is everything below it).
    pub fn dir(
        &mut self,
        name: &str,
        size: u64,
        modified: Option<i64>,
        unreadable: bool,
        contents: Dir,
    ) {
        let (name_start, len) = self.push_name(name);
        // Two billion subfolders in one folder cannot happen; saturating
        // keeps the index in range rather than panic.
        let index = u32::try_from(self.dirs.len()).map_or(MAX_DIRS, |index| index.min(MAX_DIRS));
        self.dirs.push(contents);
        let entry = Entry {
            size,
            modified: modified.unwrap_or(NO_TIME),
            name_start,
            dir: index | if unreadable { UNREADABLE } else { 0 },
        };
        self.entries.push((entry, len));
    }

    /// Total size of everything added.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.entries
            .iter()
            .fold(0_u64, |sum, (entry, _)| sum.saturating_add(entry.size))
    }

    /// The folder, largest first; ties in name order (names are unique in a
    /// folder, so the unstable sort is deterministic).
    #[must_use]
    pub fn finish(mut self) -> Dir {
        let names = &self.names;
        let name = |(entry, len): &(Entry, u32)| {
            let start = entry.name_start as usize;
            names.get(start..start + *len as usize).unwrap_or_default()
        };
        self.entries
            .sort_unstable_by(|a, b| b.0.size.cmp(&a.0.size).then_with(|| name(a).cmp(name(b))));
        self.pack()
    }

    /// The folder in the order things were added.
    #[must_use]
    pub fn finish_unsorted(self) -> Dir {
        self.pack()
    }

    /// Lay the names out in entry order, as [`Dir`] reads them.
    fn pack(self) -> Dir {
        let mut names = String::with_capacity(self.names.len());
        let entries: Vec<Entry> = self
            .entries
            .into_iter()
            .map(|(mut entry, len)| {
                let start = entry.name_start as usize;
                let name = self
                    .names
                    .get(start..start + len as usize)
                    .unwrap_or_default();
                entry.name_start = u32::try_from(names.len()).unwrap_or(u32::MAX);
                names.push_str(name);
                entry
            })
            .collect();
        Dir {
            entries: entries.into_boxed_slice(),
            names: names.into_boxed_str(),
            dirs: self.dirs.into_boxed_slice(),
        }
    }
}

impl Tree {
    /// A tree whose root folder is `name` (the scanned path) holding `root`.
    #[must_use]
    pub fn new(name: &str, modified: Option<i64>, root: Dir) -> Self {
        let size = root
            .entries
            .iter()
            .fold(0_u64, |sum, entry| sum.saturating_add(entry.size));
        Self {
            name: name.into(),
            size,
            modified: modified.unwrap_or(NO_TIME),
            unreadable: false,
            root,
        }
    }

    /// The root folder could not be listed.
    pub fn mark_unreadable(&mut self) {
        self.unreadable = true;
    }
}

// ---- on disk -----------------------------------------------------------
//
// A tree is stored as the root's name, size, time, and flag, then its folder
// contents recursively: entry count, names length, the entries (24 bytes
// each, little-endian), the names, subfolder count, and each subfolder.
// Reading treats the bytes as untrusted: every count and offset is checked,
// nesting is capped, and buffers grow only as data actually arrives, so a
// damaged file is an error, never a panic or a huge allocation.

/// Folders nested deeper than this are refused when read back. Real trees
/// are far shallower, and a cap keeps every recursive walk (reading, and
/// later freeing) well inside a thread's stack. A drive nested deeper still
/// scans; its cache is just not used.
pub const MAX_DEPTH: usize = 1024;

fn invalid(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("damaged tree data: {what}"),
    )
}

fn read_bytes<const N: usize>(input: &mut impl Read) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    input.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn read_u32(input: &mut impl Read) -> io::Result<u32> {
    read_bytes::<4>(input).map(u32::from_le_bytes)
}

fn read_u64(input: &mut impl Read) -> io::Result<u64> {
    read_bytes::<8>(input).map(u64::from_le_bytes)
}

/// Exactly `len` bytes, read as they arrive (a false length on a short
/// file ends in an error before any large buffer exists).
fn read_vec(input: &mut impl Read, len: u32) -> io::Result<Vec<u8>> {
    // Avoid repeated growth for ordinary names, without trusting a claimed
    // length enough to allocate its whole buffer before reading anything.
    let mut bytes = Vec::with_capacity(len.min(16 * 1024) as usize);
    input.take(u64::from(len)).read_to_end(&mut bytes)?;
    if bytes.len() != len as usize {
        return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
    }
    Ok(bytes)
}

fn write_len(out: &mut impl Write, len: usize) -> io::Result<()> {
    let len = u32::try_from(len).map_err(|_| invalid("length over 4 GiB"))?;
    out.write_all(&len.to_le_bytes())
}

impl Tree {
    /// Write the tree in the format [`Tree::read_from`] reads.
    ///
    /// # Errors
    /// Any write error from `out`.
    pub fn write_to(&self, out: &mut impl Write) -> io::Result<()> {
        write_len(out, self.name.len())?;
        out.write_all(self.name.as_bytes())?;
        out.write_all(&self.size.to_le_bytes())?;
        out.write_all(&self.modified.to_le_bytes())?;
        out.write_all(&[u8::from(self.unreadable)])?;
        write_dir(out, &self.root)
    }

    /// Read a tree written by [`Tree::write_to`].
    ///
    /// # Errors
    /// Read errors from `input`, and `InvalidData` when the bytes do not
    /// describe a well-formed tree.
    pub fn read_from(input: &mut impl Read) -> io::Result<Tree> {
        let name_len = read_u32(input)?;
        let name =
            String::from_utf8(read_vec(input, name_len)?).map_err(|_| invalid("root name"))?;
        let size = read_u64(input)?;
        let modified = read_u64(input)?.cast_signed();
        let unreadable = match read_bytes::<1>(input)? {
            [0] => false,
            [1] => true,
            _ => return Err(invalid("root flag")),
        };
        let root = read_dir(input, 0)?;
        Ok(Tree {
            name: name.into_boxed_str(),
            size,
            modified,
            unreadable,
            root,
        })
    }
}

fn write_dir(out: &mut impl Write, dir: &Dir) -> io::Result<()> {
    write_len(out, dir.entries.len())?;
    write_len(out, dir.names.len())?;
    for entry in &dir.entries {
        out.write_all(&entry.size.to_le_bytes())?;
        out.write_all(&entry.modified.to_le_bytes())?;
        out.write_all(&entry.name_start.to_le_bytes())?;
        out.write_all(&entry.dir.to_le_bytes())?;
    }
    out.write_all(dir.names.as_bytes())?;
    write_len(out, dir.dirs.len())?;
    for sub in &dir.dirs {
        write_dir(out, sub)?;
    }
    Ok(())
}

fn read_dir(input: &mut impl Read, depth: usize) -> io::Result<Dir> {
    if depth > MAX_DEPTH {
        return Err(invalid("nested too deep"));
    }
    let count = read_u32(input)?;
    let names_len = read_u32(input)?;
    // Most folders fit in these initial buffers. Cap both reservations:
    // counts in a damaged cache are untrusted until their contents arrive.
    let mut entries = Vec::with_capacity(count.min(256) as usize);
    let mut previous_start = 0;
    let mut folders = 0_u32;
    for _ in 0..count {
        let bytes = read_bytes::<24>(input)?;
        let field = |at: usize| -> [u8; 8] {
            let mut word = [0; 8];
            word.copy_from_slice(&bytes[at..at + 8]);
            word
        };
        let half = |at: usize| -> [u8; 4] {
            let mut word = [0; 4];
            word.copy_from_slice(&bytes[at..at + 4]);
            word
        };
        let entry = Entry {
            size: u64::from_le_bytes(field(0)),
            modified: i64::from_le_bytes(field(8)),
            name_start: u32::from_le_bytes(half(16)),
            dir: u32::from_le_bytes(half(20)),
        };
        if entry.name_start < previous_start || entry.name_start > names_len {
            return Err(invalid("name offsets"));
        }
        previous_start = entry.name_start;
        if entry.is_dir() {
            folders += 1;
        }
        entries.push(entry);
    }
    let names = String::from_utf8(read_vec(input, names_len)?).map_err(|_| invalid("names"))?;
    if entries
        .iter()
        .any(|entry| !names.is_char_boundary(entry.name_start as usize))
    {
        return Err(invalid("name offsets"));
    }
    // Every folder entry points at its own contents, once.
    let dir_count = read_u32(input)?;
    if dir_count != folders {
        return Err(invalid("folder count"));
    }
    let mut seen = vec![false; dir_count as usize];
    for entry in &entries {
        if let Some(index) = entry.dir_index() {
            match seen.get_mut(index) {
                Some(slot @ false) => *slot = true,
                _ => return Err(invalid("folder index")),
            }
        }
    }
    let mut dirs = Vec::with_capacity(dir_count.min(256) as usize);
    for _ in 0..dir_count {
        dirs.push(read_dir(input, depth + 1)?);
    }
    Ok(Dir {
        entries: entries.into_boxed_slice(),
        names: names.into_boxed_str(),
        dirs: dirs.into_boxed_slice(),
    })
}

/// A plain owned tree for tests and benches; [`Tree::from`] packs it,
/// keeping the children's order as given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    pub children: Vec<Node>,
    /// For a directory: the sum of its children's sizes.
    pub size: u64,
    pub modified: Option<i64>,
    pub is_dir: bool,
    pub unreadable: bool,
}

impl Node {
    /// A file entry.
    #[must_use]
    pub fn file(name: impl Into<String>, size: u64, modified: Option<i64>) -> Self {
        Self {
            name: name.into(),
            children: Vec::new(),
            size,
            modified,
            is_dir: false,
            unreadable: false,
        }
    }

    /// A directory entry; its size is the sum of `children`.
    #[must_use]
    pub fn dir(name: impl Into<String>, children: Vec<Node>, modified: Option<i64>) -> Self {
        let size = children
            .iter()
            .fold(0_u64, |sum, child| sum.saturating_add(child.size));
        Self {
            name: name.into(),
            children,
            size,
            modified,
            is_dir: true,
            unreadable: false,
        }
    }

    fn pack_children(children: Vec<Node>) -> Dir {
        let mut builder = DirBuilder::new();
        for child in children {
            if child.is_dir {
                let contents = Self::pack_children(child.children);
                builder.dir(
                    &child.name,
                    child.size,
                    child.modified,
                    child.unreadable,
                    contents,
                );
            } else {
                builder.file(&child.name, child.size, child.modified);
            }
        }
        builder.finish_unsorted()
    }
}

impl From<Node> for Tree {
    fn from(node: Node) -> Self {
        let mut tree = Tree::new(
            &node.name,
            node.modified,
            Node::pack_children(node.children),
        );
        tree.size = node.size;
        tree.unreadable = node.unreadable;
        tree
    }
}

/// Where a stored index path points after the node at `removed` is taken
/// out: `None` when `path` was the removed node or inside it, otherwise the
/// same node's new path (a later sibling of the removed node, or anything
/// below one, moves one place left).
#[must_use]
pub fn adjust_after_removal(path: &[usize], removed: &[usize]) -> Option<Vec<usize>> {
    let (&gone, parent) = removed.split_last()?;
    if path.starts_with(removed) {
        return None;
    }
    let mut adjusted = path.to_vec();
    if path.len() > parent.len() && path.starts_with(parent) && path[parent.len()] > gone {
        adjusted[parent.len()] -= 1;
    }
    Some(adjusted)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// root(1110) = [ big(1000) = [a(600), b(400)], c(100), empty(0) = [], d(10) ]
    fn sample() -> Tree {
        Tree::from(Node::dir(
            "root",
            vec![
                Node::dir(
                    "big",
                    vec![Node::file("a", 600, Some(1)), Node::file("b", 400, None)],
                    None,
                ),
                Node::file("c", 100, None),
                Node::dir("empty", Vec::new(), None),
                Node::file("d", 10, None),
            ],
            Some(7),
        ))
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn entries_stay_small() {
        assert_eq!(std::mem::size_of::<Entry>(), 24);
        assert_eq!(std::mem::size_of::<Dir>(), 48);
    }

    #[test]
    fn dir_size_is_the_sum_of_children() {
        let tree = sample();
        let root = tree.root();
        assert_eq!(root.size(), 1110);
        assert_eq!(root.child(0).unwrap().size(), 1000);
        assert_eq!(root.child(2).unwrap().size(), 0);
        assert!(root.is_dir() && !root.child(1).unwrap().is_dir());
        assert!(
            root.child(2).unwrap().is_dir(),
            "an empty folder is a folder"
        );
        assert_eq!(root.modified(), Some(7));
        assert_eq!(tree.node_at(&[0, 0]).unwrap().modified(), Some(1));
        assert_eq!(tree.node_at(&[0, 1]).unwrap().modified(), None);
    }

    #[test]
    fn node_at_and_names_follow_index_paths() {
        let tree = sample();
        assert_eq!(tree.node_at(&[]).unwrap().name(), "root");
        assert_eq!(tree.node_at(&[0, 1]).unwrap().name(), "b");
        assert!(tree.node_at(&[0, 2]).is_none());
        assert!(tree.node_at(&[1, 0]).is_none());
        assert_eq!(tree.names_along(&[0, 1]).unwrap(), vec!["big", "b"]);
        assert!(tree.names_along(&[9]).is_none());
        let names: Vec<&str> = tree.root().children().map(|c| c.name()).collect();
        assert_eq!(names, ["big", "c", "empty", "d"]);
        assert_eq!(tree.root().children().next_back().unwrap().name(), "d");
    }

    #[test]
    fn counts_include_the_root() {
        assert_eq!(sample().counts(), (4, 3));
    }

    #[test]
    fn builders_sort_largest_first_then_by_name() {
        let mut builder = DirBuilder::new();
        builder.file("small", 1, None);
        builder.file("b-tie", 5, None);
        builder.dir("folder", 9, None, true, Dir::default());
        builder.file("a-tie", 5, None);
        assert_eq!(builder.size(), 20);
        let tree = Tree::new("r", None, builder.finish());
        let names: Vec<&str> = tree.root().children().map(|c| c.name()).collect();
        assert_eq!(names, ["folder", "a-tie", "b-tie", "small"]);
        assert!(tree.node_at(&[0]).unwrap().unreadable());
        assert_eq!(tree.root().size(), 20);
    }

    #[test]
    fn unicode_names_read_back() {
        let tree = Tree::from(Node::dir(
            "C:\\",
            vec![
                Node::file("日本語のファイル名.txt", 3, None),
                Node::file("ünïcödé", 2, None),
                Node::file("", 1, None),
            ],
            None,
        ));
        let names: Vec<&str> = tree.root().children().map(|c| c.name()).collect();
        assert_eq!(names, ["日本語のファイル名.txt", "ünïcödé", ""]);
    }

    #[test]
    fn remove_subtracts_size_from_every_ancestor() {
        let mut tree = sample();
        let removed = tree.remove_at(&[0, 0]).unwrap();
        assert_eq!((removed.size, removed.files, removed.folders), (600, 1, 0));
        assert_eq!(tree.node_at(&[0]).unwrap().size(), 400);
        assert_eq!(tree.root().size(), 510);
        assert_eq!(tree.node_at(&[0, 0]).unwrap().name(), "b");
        // Sibling order is untouched even though `big` is now smaller.
        assert_eq!(tree.node_at(&[0]).unwrap().name(), "big");
    }

    #[test]
    fn removing_a_folder_counts_what_was_inside() {
        let mut tree = sample();
        let removed = tree.remove_at(&[0]).unwrap();
        assert_eq!((removed.size, removed.files, removed.folders), (1000, 2, 1));
        assert_eq!(removed.contents.size(), 1000);
        assert_eq!(tree.root().size(), 110);
        // The folder after it still finds its own contents.
        let names: Vec<&str> = tree.root().children().map(|c| c.name()).collect();
        assert_eq!(names, ["c", "empty", "d"]);
        assert!(tree.node_at(&[1]).unwrap().is_dir());
        assert_eq!(tree.node_at(&[1]).unwrap().child_count(), 0);
        assert_eq!(tree.counts(), (2, 2));
    }

    #[test]
    fn removal_keeps_every_other_name_and_folder_intact() {
        let mut tree = Tree::from(Node::dir(
            "root",
            vec![
                Node::dir("first", vec![Node::file("f1", 5, None)], None),
                Node::file("middle-name", 4, None),
                Node::dir("second", vec![Node::file("s1", 3, None)], None),
                Node::file("last", 1, None),
            ],
            None,
        ));
        let mut locked = tree.clone();
        tree.remove_at(&[1]).unwrap();
        let names: Vec<&str> = tree.root().children().map(|c| c.name()).collect();
        assert_eq!(names, ["first", "second", "last"]);
        assert_eq!(tree.node_at(&[1, 0]).unwrap().name(), "s1");
        tree.remove_at(&[0]).unwrap();
        let names: Vec<&str> = tree.root().children().map(|c| c.name()).collect();
        assert_eq!(names, ["second", "last"]);
        assert_eq!(tree.node_at(&[0, 0]).unwrap().name(), "s1");
        assert_eq!(tree.root().size(), 4);
        // Removing the last entry leaves the one before it whole.
        locked.remove_at(&[3]).unwrap();
        assert_eq!(locked.node_at(&[2]).unwrap().name(), "second");
    }

    #[test]
    fn unreadable_folders_keep_their_flag_and_contents_index() {
        let mut builder = DirBuilder::new();
        builder.dir("open", 2, None, false, Dir::default());
        builder.dir("locked", 1, None, true, Dir::default());
        let tree = Tree::new("r", None, builder.finish());
        assert!(!tree.node_at(&[0]).unwrap().unreadable());
        assert!(tree.node_at(&[1]).unwrap().unreadable());
        assert!(tree.node_at(&[1]).unwrap().is_dir());
        assert_eq!(tree.node_at(&[1]).unwrap().child_count(), 0);
    }

    fn round_trip(tree: &Tree) -> io::Result<Tree> {
        let mut bytes = Vec::new();
        tree.write_to(&mut bytes)?;
        Tree::read_from(&mut bytes.as_slice())
    }

    #[test]
    fn trees_read_back_exactly() {
        let mut tree = sample();
        assert_eq!(round_trip(&tree).unwrap(), tree);
        tree.mark_unreadable();
        tree.remove_at(&[2]).unwrap();
        assert_eq!(round_trip(&tree).unwrap(), tree);
        let empty = Tree::new("C:\\", None, Dir::default());
        assert_eq!(round_trip(&empty).unwrap(), empty);
    }

    #[test]
    fn every_truncation_is_an_error() {
        let mut bytes = Vec::new();
        sample().write_to(&mut bytes).unwrap();
        for len in 0..bytes.len() {
            assert!(
                Tree::read_from(&mut &bytes[..len]).is_err(),
                "a tree cut at {len} of {} bytes was accepted",
                bytes.len()
            );
        }
    }

    #[test]
    fn damaged_bytes_are_errors_or_trees_never_panics() {
        let mut bytes = Vec::new();
        sample().write_to(&mut bytes).unwrap();
        for at in 0..bytes.len() {
            for flip in [0x01_u8, 0x80, 0xFF] {
                let mut damaged = bytes.clone();
                damaged[at] ^= flip;
                // Either outcome is fine; what matters is no panic and no
                // runaway allocation.
                if let Ok(tree) = Tree::read_from(&mut damaged.as_slice()) {
                    let _ = tree.counts();
                    let _ = tree
                        .root()
                        .children()
                        .map(|c| c.name().len())
                        .sum::<usize>();
                }
            }
        }
    }

    #[test]
    fn lying_counts_fail_fast() {
        // A name claiming 4 GiB, then nothing.
        let mut bytes = u32::MAX.to_le_bytes().to_vec();
        assert!(Tree::read_from(&mut bytes.as_slice()).is_err());
        // A root folder claiming 4 billion entries, then nothing.
        bytes = Vec::new();
        Tree::new("r", None, Dir::default())
            .write_to(&mut bytes)
            .unwrap();
        let at = bytes.len() - 12;
        bytes[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Tree::read_from(&mut bytes.as_slice()).is_err());
    }

    #[test]
    fn nesting_is_capped() {
        let mut node = Node::dir("leaf", Vec::new(), None);
        for _ in 0..MAX_DEPTH + 2 {
            node = Node::dir("d", vec![node], None);
        }
        // Built and written on a big stack, like the scan and the cache.
        let outcome = std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(move || round_trip(&Tree::from(node)).map(|_| ()))
            .unwrap()
            .join()
            .unwrap();
        assert!(outcome.is_err());
    }

    #[test]
    fn remove_refuses_the_root_and_missing_paths() {
        let mut tree = sample();
        assert!(tree.remove_at(&[]).is_none());
        assert!(tree.remove_at(&[4]).is_none());
        assert!(tree.remove_at(&[1, 0]).is_none());
        assert_eq!(tree, sample());
    }

    #[test]
    fn paths_shift_only_after_the_removed_sibling() {
        let removed = [0, 1];
        assert_eq!(adjust_after_removal(&[0, 1], &removed), None);
        assert_eq!(adjust_after_removal(&[0, 1, 4], &removed), None);
        assert_eq!(adjust_after_removal(&[0, 2], &removed), Some(vec![0, 1]));
        assert_eq!(
            adjust_after_removal(&[0, 3, 5], &removed),
            Some(vec![0, 2, 5])
        );
        assert_eq!(
            adjust_after_removal(&[0, 0, 9], &removed),
            Some(vec![0, 0, 9])
        );
        assert_eq!(adjust_after_removal(&[0], &removed), Some(vec![0]));
        assert_eq!(adjust_after_removal(&[1, 2], &removed), Some(vec![1, 2]));
        assert_eq!(adjust_after_removal(&[], &removed), Some(vec![]));
        assert_eq!(adjust_after_removal(&[3], &[1]), Some(vec![2]));
        assert_eq!(adjust_after_removal(&[3], &[]), None);
    }
}
