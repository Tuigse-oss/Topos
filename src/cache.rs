//! The last scan of each drive, kept on disk so the next start can show a
//! map at once while a fresh scan runs.
//!
//! One file per drive root in the cache folder: a header (format version,
//! the root path, when it was scanned, the scan totals) and then the tree
//! ([`Tree::write_to`]). Files are written to a temporary name and renamed
//! into place, so a crash mid-write never leaves a half file under the real
//! name. Reading treats the file as untrusted: anything malformed is an
//! error, and the caller simply scans as if there were no cache.

use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use crate::scanner::ScanStats;
use crate::tree::Tree;

const MAGIC: &[u8; 8] = b"TOPOSCAN";
/// Bump when the format changes; older files are then ignored.
const VERSION: u32 = 1;
const BUFFER: usize = 1 << 20;

/// What a cache file says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// The scanned root, as it was given to the scan.
    pub root: String,
    /// When the scan finished, in seconds since the Unix epoch.
    pub scanned_at: i64,
    pub stats: ScanStats,
}

/// The cache file for `root` inside `dir`: the root's letters and digits
/// with everything else as `_` (`C:\` becomes `C__.toposcan`).
#[must_use]
pub fn file_for(dir: &Path, root: &Path) -> PathBuf {
    let name: String = root
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    dir.join(format!("{name}.toposcan"))
}

/// Write a scan to `path` (through a temporary file in the same folder).
///
/// # Errors
/// Any error creating, writing, or renaming the file.
pub fn save(path: &Path, header: &Header, tree: &Tree) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let temporary = path.with_extension("toposcan.part");
    let mut out = BufWriter::with_capacity(BUFFER, File::create(&temporary)?);
    out.write_all(MAGIC)?;
    out.write_all(&VERSION.to_le_bytes())?;
    let root = header.root.as_bytes();
    let root_len = u32::try_from(root.len()).map_err(|_| io::Error::other("root path too long"))?;
    out.write_all(&root_len.to_le_bytes())?;
    out.write_all(root)?;
    out.write_all(&header.scanned_at.to_le_bytes())?;
    let s = header.stats;
    for value in [s.files, s.folders, s.links, s.skipped, s.bytes] {
        out.write_all(&value.to_le_bytes())?;
    }
    tree.write_to(&mut out)?;
    out.into_inner().map_err(io::IntoInnerError::into_error)?;
    fs::rename(&temporary, path)
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("not a usable scan cache: {what}"),
    )
}

fn read_header(input: &mut impl Read) -> io::Result<Header> {
    let mut magic = [0; 8];
    input.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(invalid("not a cache file"));
    }
    let mut word = [0; 4];
    input.read_exact(&mut word)?;
    if u32::from_le_bytes(word) != VERSION {
        return Err(invalid("another version"));
    }
    input.read_exact(&mut word)?;
    let root_len = u32::from_le_bytes(word);
    // A drive root's path is a few bytes; refuse anything absurd before
    // reading it.
    if root_len > 32 * 1024 {
        return Err(invalid("root path"));
    }
    let mut root = vec![0; root_len as usize];
    input.read_exact(&mut root)?;
    let root = String::from_utf8(root).map_err(|_| invalid("root path"))?;
    let mut long = [0; 8];
    input.read_exact(&mut long)?;
    let scanned_at = i64::from_le_bytes(long);
    let mut values = [0_u64; 5];
    for value in &mut values {
        input.read_exact(&mut long)?;
        *value = u64::from_le_bytes(long);
    }
    let [files, folders, links, skipped, bytes] = values;
    Ok(Header {
        root,
        scanned_at,
        stats: ScanStats {
            files,
            folders,
            links,
            skipped,
            bytes,
        },
    })
}

/// Just the header of `path`: quick, for scan progress estimates.
///
/// # Errors
/// When the file is missing, unreadable, or not a cache of this version.
pub fn load_header(path: &Path) -> io::Result<Header> {
    read_header(&mut BufReader::new(File::open(path)?))
}

/// The whole cached scan in `path`, which must be of `root`.
///
/// # Errors
/// When the file is missing or unreadable, of another root or version,
/// damaged anywhere, or has bytes after the tree.
pub fn load(path: &Path, root: &str) -> io::Result<(Header, Tree)> {
    let mut input = BufReader::with_capacity(BUFFER, File::open(path)?);
    let header = read_header(&mut input)?;
    if header.root != root {
        return Err(invalid("another root"));
    }
    let tree = Tree::read_from(&mut input)?;
    let mut rest = [0; 1];
    if input.read(&mut rest)? != 0 {
        return Err(invalid("trailing bytes"));
    }
    Ok((header, tree))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::Node;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("topos-cache-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample() -> (Header, Tree) {
        let tree = Tree::from(Node::dir(
            "C:\\",
            vec![
                Node::dir("Users", vec![Node::file("a.bin", 300, Some(5))], None),
                Node::file("pagefile.sys", 100, None),
            ],
            None,
        ));
        let header = Header {
            root: String::from("C:\\"),
            scanned_at: 1_794_004_274,
            stats: ScanStats {
                files: 2,
                folders: 2,
                links: 1,
                skipped: 0,
                bytes: 400,
            },
        };
        (header, tree)
    }

    #[test]
    fn file_names_are_plain() {
        assert_eq!(
            file_for(Path::new("cache"), Path::new("C:\\")),
            Path::new("cache").join("C__.toposcan")
        );
        assert_eq!(
            file_for(Path::new("c"), Path::new("/mnt/data")),
            Path::new("c").join("_mnt_data.toposcan")
        );
    }

    #[test]
    fn scans_round_trip() {
        let dir = temp_dir("round");
        let (header, tree) = sample();
        let path = file_for(&dir, Path::new("C:\\"));
        save(&path, &header, &tree).unwrap();
        assert!(!path.with_extension("toposcan.part").exists());
        assert_eq!(load_header(&path).unwrap(), header);
        assert_eq!(load(&path, "C:\\").unwrap(), (header, tree));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saves_replace_the_old_file() {
        let dir = temp_dir("replace");
        let (mut header, tree) = sample();
        let path = file_for(&dir, Path::new("C:\\"));
        save(&path, &header, &tree).unwrap();
        header.scanned_at += 60;
        save(&path, &header, &tree).unwrap();
        assert_eq!(load_header(&path).unwrap().scanned_at, header.scanned_at);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unusable_files_are_errors() {
        let dir = temp_dir("bad");
        let (header, tree) = sample();
        let path = file_for(&dir, Path::new("C:\\"));
        assert!(load(&path, "C:\\").is_err(), "missing");
        save(&path, &header, &tree).unwrap();
        assert!(load(&path, "D:\\").is_err(), "another root");
        let bytes = fs::read(&path).unwrap();
        let mut longer = bytes.clone();
        longer.push(0);
        fs::write(&path, &longer).unwrap();
        assert!(load(&path, "C:\\").is_err(), "trailing bytes");
        let mut version = bytes.clone();
        version[8] = 99;
        fs::write(&path, &version).unwrap();
        assert!(load_header(&path).is_err(), "another version");
        for cut in [0, 7, 20, bytes.len() / 2, bytes.len() - 1] {
            fs::write(&path, &bytes[..cut]).unwrap();
            assert!(load(&path, "C:\\").is_err(), "cut at {cut}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }
}
