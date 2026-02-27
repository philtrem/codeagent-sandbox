use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FileType {
    Regular,
    Directory,
    Symlink,
}

impl fmt::Display for FileType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileType::Regular => write!(f, "regular"),
            FileType::Directory => write!(f, "directory"),
            FileType::Symlink => write!(f, "symlink"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EntrySnapshot {
    pub file_type: FileType,
    /// blake3 hash of file contents; None for directories and symlinks.
    pub content_hash: Option<[u8; 32]>,
    pub size: u64,
    /// Unix mode bits (all 12 bits). On Windows, this is a synthetic value.
    pub mode: u32,
    /// mtime as nanoseconds since Unix epoch.
    pub mtime_ns: i128,
    /// For symlinks: the target path string. None for other types.
    pub symlink_target: Option<String>,
    /// Extended attributes (key -> value). Empty on platforms without xattr support.
    pub xattrs: BTreeMap<String, Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct TreeSnapshot {
    /// Relative paths from the root -> snapshot of each entry.
    pub entries: BTreeMap<PathBuf, EntrySnapshot>,
}

impl TreeSnapshot {
    /// Capture a complete snapshot of a directory tree.
    /// Paths stored are relative to `root`. The root directory itself is not included.
    pub fn capture(root: &Path) -> Self {
        let mut entries = BTreeMap::new();
        Self::walk_recursive(root, root, &mut entries);
        TreeSnapshot { entries }
    }

    fn walk_recursive(
        root: &Path,
        current: &Path,
        entries: &mut BTreeMap<PathBuf, EntrySnapshot>,
    ) {
        let read_dir = match fs::read_dir(current) {
            Ok(rd) => rd,
            Err(_) => return,
        };

        let mut children: Vec<_> = read_dir.filter_map(|e| e.ok()).collect();
        children.sort_by_key(|e| e.file_name());

        for entry in children {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .expect("path should be under root")
                .to_path_buf();

            let metadata = match fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };

            let file_type = if metadata.is_symlink() {
                FileType::Symlink
            } else if metadata.is_dir() {
                FileType::Directory
            } else {
                FileType::Regular
            };

            let content_hash = if file_type == FileType::Regular {
                match fs::read(&path) {
                    Ok(contents) => Some(*blake3::hash(&contents).as_bytes()),
                    Err(_) => None,
                }
            } else {
                None
            };

            let symlink_target = if file_type == FileType::Symlink {
                fs::read_link(&path)
                    .ok()
                    .map(|t| t.to_string_lossy().into_owned())
            } else {
                None
            };

            let mode = read_mode(&metadata);
            let mtime_ns = read_mtime_ns(&metadata);
            let xattrs = read_xattrs(&path);

            entries.insert(
                relative,
                EntrySnapshot {
                    file_type,
                    content_hash,
                    size: metadata.len(),
                    mode,
                    mtime_ns,
                    symlink_target,
                    xattrs,
                },
            );

            if file_type == FileType::Directory {
                Self::walk_recursive(root, &path, entries);
            }
        }
    }
}

#[cfg(unix)]
fn read_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    metadata.mode()
}

#[cfg(not(unix))]
fn read_mode(metadata: &fs::Metadata) -> u32 {
    if metadata.is_dir() {
        0o755
    } else {
        0o644
    }
}

fn read_mtime_ns(metadata: &fs::Metadata) -> i128 {
    match metadata.modified() {
        Ok(mtime) => match mtime.duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_nanos() as i128,
            Err(err) => -(err.duration().as_nanos() as i128),
        },
        Err(_) => 0,
    }
}

#[cfg(target_os = "linux")]
fn read_xattrs(_path: &Path) -> BTreeMap<String, Vec<u8>> {
    // xattr reading can be added via libc or the xattr crate when needed.
    // For now, return an empty map to avoid adding a dependency.
    BTreeMap::new()
}

#[cfg(not(target_os = "linux"))]
fn read_xattrs(_path: &Path) -> BTreeMap<String, Vec<u8>> {
    BTreeMap::new()
}

// --- Comparison ---

pub struct SnapshotCompareOptions {
    /// Maximum allowed difference in mtime nanoseconds. Default: 1_000_000 (1ms).
    pub mtime_tolerance_ns: i128,
    /// Whether to compare xattrs. Default: true on Linux, false otherwise.
    pub check_xattrs: bool,
    /// Glob patterns for paths to exclude from comparison.
    pub exclude_patterns: Vec<String>,
}

impl Default for SnapshotCompareOptions {
    fn default() -> Self {
        Self {
            mtime_tolerance_ns: 1_000_000,
            check_xattrs: cfg!(target_os = "linux"),
            exclude_patterns: Vec::new(),
        }
    }
}

#[derive(Debug)]
enum SnapshotDiff {
    Missing {
        path: PathBuf,
    },
    Extra {
        path: PathBuf,
    },
    TypeMismatch {
        path: PathBuf,
        expected: FileType,
        actual: FileType,
    },
    ContentMismatch {
        path: PathBuf,
    },
    SizeMismatch {
        path: PathBuf,
        expected: u64,
        actual: u64,
    },
    ModeMismatch {
        path: PathBuf,
        expected: u32,
        actual: u32,
    },
    MtimeMismatch {
        path: PathBuf,
        expected: i128,
        actual: i128,
        tolerance: i128,
    },
    SymlinkTargetMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    XattrMismatch {
        path: PathBuf,
        detail: String,
    },
}

impl fmt::Display for SnapshotDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SnapshotDiff::Missing { path } => write!(f, "  MISSING: {}", path.display()),
            SnapshotDiff::Extra { path } => write!(f, "  EXTRA:   {}", path.display()),
            SnapshotDiff::TypeMismatch {
                path,
                expected,
                actual,
            } => write!(
                f,
                "  TYPE:    {} (expected {expected}, got {actual})",
                path.display()
            ),
            SnapshotDiff::ContentMismatch { path } => {
                write!(f, "  CONTENT: {} (hash differs)", path.display())
            }
            SnapshotDiff::SizeMismatch {
                path,
                expected,
                actual,
            } => write!(
                f,
                "  SIZE:    {} (expected {expected}, got {actual})",
                path.display()
            ),
            SnapshotDiff::ModeMismatch {
                path,
                expected,
                actual,
            } => write!(
                f,
                "  MODE:    {} (expected {expected:04o}, got {actual:04o})",
                path.display()
            ),
            SnapshotDiff::MtimeMismatch {
                path,
                expected,
                actual,
                tolerance,
            } => write!(
                f,
                "  MTIME:   {} (expected {expected}, got {actual}, tolerance {tolerance})",
                path.display()
            ),
            SnapshotDiff::SymlinkTargetMismatch {
                path,
                expected,
                actual,
            } => write!(
                f,
                "  SYMLINK: {} (expected \"{expected}\", got \"{actual}\")",
                path.display()
            ),
            SnapshotDiff::XattrMismatch { path, detail } => {
                write!(f, "  XATTR:   {} ({detail})", path.display())
            }
        }
    }
}

fn path_matches_exclude(path: &Path, patterns: &[String]) -> bool {
    let path_str = path.to_string_lossy();
    patterns.iter().any(|pattern| {
        // Simple glob matching: support * as wildcard
        if pattern.contains('*') {
            let parts: Vec<&str> = pattern.split('*').collect();
            if parts.len() == 2 {
                let (prefix, suffix) = (parts[0], parts[1]);
                path_str.starts_with(prefix) && path_str.ends_with(suffix)
            } else {
                false
            }
        } else {
            *path_str == *pattern
        }
    })
}

/// Compares two TreeSnapshots. Panics with a human-readable diff on mismatch.
pub fn assert_tree_eq(
    expected: &TreeSnapshot,
    actual: &TreeSnapshot,
    opts: &SnapshotCompareOptions,
) {
    let mut diffs = Vec::new();

    for (path, expected_entry) in &expected.entries {
        if path_matches_exclude(path, &opts.exclude_patterns) {
            continue;
        }
        match actual.entries.get(path) {
            None => diffs.push(SnapshotDiff::Missing {
                path: path.clone(),
            }),
            Some(actual_entry) => {
                compare_entries(path, expected_entry, actual_entry, opts, &mut diffs);
            }
        }
    }

    for path in actual.entries.keys() {
        if path_matches_exclude(path, &opts.exclude_patterns) {
            continue;
        }
        if !expected.entries.contains_key(path) {
            diffs.push(SnapshotDiff::Extra {
                path: path.clone(),
            });
        }
    }

    if !diffs.is_empty() {
        let mut msg = format!("Tree snapshots differ ({} differences):\n", diffs.len());
        for diff in &diffs {
            msg.push_str(&format!("{diff}\n"));
        }
        panic!("{msg}");
    }
}

fn compare_entries(
    path: &Path,
    expected: &EntrySnapshot,
    actual: &EntrySnapshot,
    opts: &SnapshotCompareOptions,
    diffs: &mut Vec<SnapshotDiff>,
) {
    if expected.file_type != actual.file_type {
        diffs.push(SnapshotDiff::TypeMismatch {
            path: path.to_path_buf(),
            expected: expected.file_type,
            actual: actual.file_type,
        });
        return; // No point comparing other fields if type differs
    }

    if expected.content_hash != actual.content_hash {
        diffs.push(SnapshotDiff::ContentMismatch {
            path: path.to_path_buf(),
        });
    }

    if expected.size != actual.size {
        diffs.push(SnapshotDiff::SizeMismatch {
            path: path.to_path_buf(),
            expected: expected.size,
            actual: actual.size,
        });
    }

    if expected.mode != actual.mode {
        diffs.push(SnapshotDiff::ModeMismatch {
            path: path.to_path_buf(),
            expected: expected.mode,
            actual: actual.mode,
        });
    }

    let mtime_diff = (expected.mtime_ns - actual.mtime_ns).abs();
    if mtime_diff > opts.mtime_tolerance_ns {
        diffs.push(SnapshotDiff::MtimeMismatch {
            path: path.to_path_buf(),
            expected: expected.mtime_ns,
            actual: actual.mtime_ns,
            tolerance: opts.mtime_tolerance_ns,
        });
    }

    if expected.symlink_target != actual.symlink_target {
        diffs.push(SnapshotDiff::SymlinkTargetMismatch {
            path: path.to_path_buf(),
            expected: expected
                .symlink_target
                .clone()
                .unwrap_or_else(|| "(none)".to_string()),
            actual: actual
                .symlink_target
                .clone()
                .unwrap_or_else(|| "(none)".to_string()),
        });
    }

    if opts.check_xattrs && expected.xattrs != actual.xattrs {
        diffs.push(SnapshotDiff::XattrMismatch {
            path: path.to_path_buf(),
            detail: "xattr sets differ".to_string(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn snapshot_empty_dir() {
        let dir = TempDir::new().unwrap();
        let snap = TreeSnapshot::capture(dir.path());
        assert!(snap.entries.is_empty());
    }

    #[test]
    fn snapshot_single_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("hello.txt"), "hello world").unwrap();

        let snap = TreeSnapshot::capture(dir.path());
        assert_eq!(snap.entries.len(), 1);

        let entry = &snap.entries[&PathBuf::from("hello.txt")];
        assert_eq!(entry.file_type, FileType::Regular);
        assert!(entry.content_hash.is_some());
        assert_eq!(entry.size, 11);
        assert!(entry.symlink_target.is_none());
    }

    #[test]
    fn snapshot_nested_dirs() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("a/b/c")).unwrap();
        fs::write(dir.path().join("a/b/c/file.txt"), "deep").unwrap();

        let snap = TreeSnapshot::capture(dir.path());
        assert!(snap.entries.contains_key(&PathBuf::from("a")));
        assert!(snap.entries.contains_key(&PathBuf::from("a/b")));
        assert!(snap.entries.contains_key(&PathBuf::from("a/b/c")));
        assert!(snap.entries.contains_key(&PathBuf::from("a/b/c/file.txt")));
        assert_eq!(snap.entries.len(), 4);

        assert_eq!(
            snap.entries[&PathBuf::from("a")].file_type,
            FileType::Directory
        );
        assert_eq!(
            snap.entries[&PathBuf::from("a/b/c/file.txt")].file_type,
            FileType::Regular
        );
    }

    #[test]
    fn snapshot_content_hash_deterministic() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("test.txt"), "same content").unwrap();
        let snap1 = TreeSnapshot::capture(dir.path());

        let dir2 = TempDir::new().unwrap();
        fs::write(dir2.path().join("test.txt"), "same content").unwrap();
        let snap2 = TreeSnapshot::capture(dir2.path());

        assert_eq!(
            snap1.entries[&PathBuf::from("test.txt")].content_hash,
            snap2.entries[&PathBuf::from("test.txt")].content_hash,
        );
    }

    #[test]
    fn snapshot_paths_are_relative() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("file.txt"), "data").unwrap();

        let snap = TreeSnapshot::capture(dir.path());
        for path in snap.entries.keys() {
            assert!(path.is_relative(), "path should be relative: {}", path.display());
            assert!(
                !path.to_string_lossy().starts_with('/'),
                "path should not start with /: {}",
                path.display()
            );
        }
    }

    #[test]
    fn assert_tree_eq_identical() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.txt"), "content a").unwrap();
        fs::write(dir.path().join("b.txt"), "content b").unwrap();

        let snap1 = TreeSnapshot::capture(dir.path());
        let snap2 = TreeSnapshot::capture(dir.path());

        assert_tree_eq(&snap1, &snap2, &SnapshotCompareOptions::default());
    }

    #[test]
    #[should_panic(expected = "MISSING")]
    fn assert_tree_eq_missing_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.txt"), "content").unwrap();
        let snap1 = TreeSnapshot::capture(dir.path());

        fs::remove_file(dir.path().join("a.txt")).unwrap();
        let snap2 = TreeSnapshot::capture(dir.path());

        assert_tree_eq(&snap1, &snap2, &SnapshotCompareOptions::default());
    }

    #[test]
    #[should_panic(expected = "EXTRA")]
    fn assert_tree_eq_extra_file() {
        let dir = TempDir::new().unwrap();
        let snap1 = TreeSnapshot::capture(dir.path());

        fs::write(dir.path().join("new.txt"), "new").unwrap();
        let snap2 = TreeSnapshot::capture(dir.path());

        assert_tree_eq(&snap1, &snap2, &SnapshotCompareOptions::default());
    }

    #[test]
    #[should_panic(expected = "CONTENT")]
    fn assert_tree_eq_content_change() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("file.txt"), "original").unwrap();
        let snap1 = TreeSnapshot::capture(dir.path());

        fs::write(dir.path().join("file.txt"), "modified").unwrap();
        let snap2 = TreeSnapshot::capture(dir.path());

        assert_tree_eq(&snap1, &snap2, &SnapshotCompareOptions::default());
    }

    #[test]
    fn assert_tree_eq_mtime_within_tolerance() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("file.txt"), "data").unwrap();

        let snap1 = TreeSnapshot::capture(dir.path());
        // Re-capture immediately — mtime should be the same or within tolerance
        let snap2 = TreeSnapshot::capture(dir.path());

        assert_tree_eq(&snap1, &snap2, &SnapshotCompareOptions::default());
    }

    #[test]
    fn assert_tree_eq_exclude_pattern() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("keep.txt"), "keep").unwrap();
        let snap1 = TreeSnapshot::capture(dir.path());

        // Add an extra file that would cause a diff, but exclude it via pattern
        fs::write(dir.path().join("ignore.log"), "log data").unwrap();
        let snap2 = TreeSnapshot::capture(dir.path());

        let opts = SnapshotCompareOptions {
            exclude_patterns: vec!["*.log".to_string()],
            ..Default::default()
        };
        assert_tree_eq(&snap1, &snap2, &opts);
    }
}
