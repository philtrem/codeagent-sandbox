use std::collections::HashMap;
use std::ffi::CStr;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// FUSE root inode ID (kernel convention).
pub const FUSE_ROOT_ID: u64 = 1;

/// Maps FUSE inode numbers to host filesystem paths.
///
/// virtiofsd's `FileSystem` trait uses inode numbers for all operations.
/// `WriteInterceptor` uses host paths. This map bridges the gap.
///
/// Populated by `lookup`, `create`, `mkdir`, `mknod`, `symlink`, `link`.
/// Updated by `rename`. Removed by `unlink`, `rmdir`, `forget`.
///
/// Thread-safe via `RwLock` — virtiofsd's thread pool handles FUSE requests
/// concurrently, and all of them may update the map.
pub struct InodePathMap {
    map: RwLock<HashMap<u64, PathBuf>>,
    root: PathBuf,
}

impl InodePathMap {
    /// Create a new map with the root inode pre-populated.
    pub fn new(root: PathBuf) -> Self {
        let mut map = HashMap::new();
        map.insert(FUSE_ROOT_ID, root.clone());
        Self {
            map: RwLock::new(map),
            root,
        }
    }

    /// The shared directory root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Get the host path for an inode. Returns `ENOENT` if not tracked.
    pub fn get(&self, inode: u64) -> io::Result<PathBuf> {
        let map = self.map.read().unwrap();
        map.get(&inode).cloned().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("inode {inode} not tracked"),
            )
        })
    }

    /// Resolve parent inode + child name to a host path.
    ///
    /// Looks up the parent inode's path and appends the child name.
    /// Returns `ENOENT` if the parent inode is not tracked.
    pub fn resolve(&self, parent: u64, name: &CStr) -> io::Result<PathBuf> {
        let parent_path = self.get(parent)?;
        let name_str = name.to_str().map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "non-UTF-8 filename")
        })?;
        Ok(parent_path.join(name_str))
    }

    /// Track a new inode → path mapping.
    ///
    /// Called from `lookup`, `create`, `mkdir`, `mknod`, `symlink`, `link`.
    /// If the inode already exists (e.g., re-lookup), the path is updated.
    pub fn insert(&self, inode: u64, path: PathBuf) {
        let mut map = self.map.write().unwrap();
        map.insert(inode, path);
    }

    /// Remove a mapping.
    ///
    /// Called from `unlink`, `rmdir`, `forget`.
    pub fn remove(&self, inode: u64) {
        let mut map = self.map.write().unwrap();
        // Never remove the root inode.
        if inode != FUSE_ROOT_ID {
            map.remove(&inode);
        }
    }

    /// Update mapping after a rename operation.
    ///
    /// Resolves old and new paths from parent inodes + names, then:
    /// 1. Finds the inode at the old path and updates it to the new path.
    /// 2. Updates all child paths under the old prefix (subtree rename).
    pub fn rename(
        &self,
        old_parent: u64,
        old_name: &CStr,
        new_parent: u64,
        new_name: &CStr,
    ) -> io::Result<()> {
        let old_path = self.resolve(old_parent, old_name)?;
        let new_path = self.resolve(new_parent, new_name)?;
        self.rename_subtree(&old_path, &new_path);
        Ok(())
    }

    /// Update all paths under a renamed directory (prefix replacement).
    ///
    /// Any inode whose path starts with `old_prefix` gets its path updated
    /// to replace the old prefix with `new_prefix`.
    pub fn rename_subtree(&self, old_prefix: &Path, new_prefix: &Path) {
        let mut map = self.map.write().unwrap();
        let updates: Vec<(u64, PathBuf)> = map
            .iter()
            .filter_map(|(&inode, path)| {
                if path == old_prefix {
                    Some((inode, new_prefix.to_path_buf()))
                } else if let Ok(suffix) = path.strip_prefix(old_prefix) {
                    Some((inode, new_prefix.join(suffix)))
                } else {
                    None
                }
            })
            .collect();

        for (inode, new_path) in updates {
            map.insert(inode, new_path);
        }
    }

    /// Number of tracked inodes (including the root).
    pub fn len(&self) -> usize {
        self.map.read().unwrap().len()
    }

    /// Whether the map contains only the root inode.
    pub fn is_empty(&self) -> bool {
        self.len() <= 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::sync::Arc;

    fn cstr(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    #[test]
    fn root_inode_is_populated_at_construction() {
        let map = InodePathMap::new(PathBuf::from("/shared"));
        let path = map.get(FUSE_ROOT_ID).unwrap();
        assert_eq!(path, PathBuf::from("/shared"));
    }

    #[test]
    fn get_unknown_inode_returns_not_found() {
        let map = InodePathMap::new(PathBuf::from("/shared"));
        let err = map.get(999).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn insert_and_get() {
        let map = InodePathMap::new(PathBuf::from("/shared"));
        map.insert(2, PathBuf::from("/shared/file.txt"));
        assert_eq!(map.get(2).unwrap(), PathBuf::from("/shared/file.txt"));
    }

    #[test]
    fn resolve_parent_and_name() {
        let map = InodePathMap::new(PathBuf::from("/shared"));
        map.insert(10, PathBuf::from("/shared/subdir"));

        let name = cstr("hello.txt");
        let resolved = map.resolve(10, &name).unwrap();
        assert_eq!(resolved, PathBuf::from("/shared/subdir/hello.txt"));
    }

    #[test]
    fn resolve_root_and_name() {
        let map = InodePathMap::new(PathBuf::from("/shared"));

        let name = cstr("top-level.txt");
        let resolved = map.resolve(FUSE_ROOT_ID, &name).unwrap();
        assert_eq!(resolved, PathBuf::from("/shared/top-level.txt"));
    }

    #[test]
    fn resolve_unknown_parent_returns_not_found() {
        let map = InodePathMap::new(PathBuf::from("/shared"));
        let name = cstr("file.txt");
        let err = map.resolve(999, &name).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn remove_inode() {
        let map = InodePathMap::new(PathBuf::from("/shared"));
        map.insert(2, PathBuf::from("/shared/file.txt"));
        assert!(map.get(2).is_ok());

        map.remove(2);
        assert!(map.get(2).is_err());
    }

    #[test]
    fn remove_root_inode_is_noop() {
        let map = InodePathMap::new(PathBuf::from("/shared"));
        map.remove(FUSE_ROOT_ID);
        assert!(map.get(FUSE_ROOT_ID).is_ok());
    }

    #[test]
    fn rename_updates_inode_path() {
        let map = InodePathMap::new(PathBuf::from("/shared"));
        map.insert(10, PathBuf::from("/shared/dir_a"));
        map.insert(20, PathBuf::from("/shared/dir_b"));
        map.insert(30, PathBuf::from("/shared/dir_a/file.txt"));

        let old_name = cstr("dir_a");
        let new_name = cstr("dir_c");
        map.rename(FUSE_ROOT_ID, &old_name, FUSE_ROOT_ID, &new_name)
            .unwrap();

        // The directory inode should now point to the new path.
        assert_eq!(map.get(10).unwrap(), PathBuf::from("/shared/dir_c"));
        // Children should also be updated.
        assert_eq!(
            map.get(30).unwrap(),
            PathBuf::from("/shared/dir_c/file.txt")
        );
        // Unrelated inode should be unaffected.
        assert_eq!(map.get(20).unwrap(), PathBuf::from("/shared/dir_b"));
    }

    #[test]
    fn rename_across_directories() {
        let map = InodePathMap::new(PathBuf::from("/shared"));
        map.insert(10, PathBuf::from("/shared/src"));
        map.insert(20, PathBuf::from("/shared/dst"));
        map.insert(30, PathBuf::from("/shared/src/file.txt"));

        let old_name = cstr("file.txt");
        let new_name = cstr("moved.txt");
        map.rename(10, &old_name, 20, &new_name).unwrap();

        assert_eq!(
            map.get(30).unwrap(),
            PathBuf::from("/shared/dst/moved.txt")
        );
    }

    #[test]
    fn rename_subtree_updates_nested_paths() {
        let map = InodePathMap::new(PathBuf::from("/shared"));
        map.insert(10, PathBuf::from("/shared/a"));
        map.insert(20, PathBuf::from("/shared/a/b"));
        map.insert(30, PathBuf::from("/shared/a/b/c"));
        map.insert(40, PathBuf::from("/shared/a/b/c/file.txt"));

        map.rename_subtree(
            &PathBuf::from("/shared/a"),
            &PathBuf::from("/shared/x"),
        );

        assert_eq!(map.get(10).unwrap(), PathBuf::from("/shared/x"));
        assert_eq!(map.get(20).unwrap(), PathBuf::from("/shared/x/b"));
        assert_eq!(map.get(30).unwrap(), PathBuf::from("/shared/x/b/c"));
        assert_eq!(
            map.get(40).unwrap(),
            PathBuf::from("/shared/x/b/c/file.txt")
        );
    }

    #[test]
    fn insert_overwrites_existing_mapping() {
        let map = InodePathMap::new(PathBuf::from("/shared"));
        map.insert(2, PathBuf::from("/shared/old.txt"));
        map.insert(2, PathBuf::from("/shared/new.txt"));
        assert_eq!(map.get(2).unwrap(), PathBuf::from("/shared/new.txt"));
    }

    #[test]
    fn len_counts_all_inodes() {
        let map = InodePathMap::new(PathBuf::from("/shared"));
        assert_eq!(map.len(), 1); // root only
        map.insert(2, PathBuf::from("/shared/a"));
        assert_eq!(map.len(), 2);
        map.insert(3, PathBuf::from("/shared/b"));
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn concurrent_access() {
        let map = Arc::new(InodePathMap::new(PathBuf::from("/shared")));
        let mut handles = vec![];

        for i in 2..=100 {
            let map_clone = map.clone();
            handles.push(std::thread::spawn(move || {
                map_clone.insert(i, PathBuf::from(format!("/shared/file_{i}")));
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(map.len(), 100); // root + 99 files

        // Verify all entries are accessible
        for i in 2..=100 {
            assert_eq!(
                map.get(i).unwrap(),
                PathBuf::from(format!("/shared/file_{i}"))
            );
        }
    }

    #[test]
    fn concurrent_read_and_write() {
        let map = Arc::new(InodePathMap::new(PathBuf::from("/shared")));

        // Pre-populate some entries
        for i in 2..=50 {
            map.insert(i, PathBuf::from(format!("/shared/file_{i}")));
        }

        let mut handles = vec![];

        // Readers
        for _ in 0..10 {
            let map_clone = map.clone();
            handles.push(std::thread::spawn(move || {
                for i in 2..=50 {
                    let _ = map_clone.get(i);
                }
            }));
        }

        // Writers
        for i in 51..=100 {
            let map_clone = map.clone();
            handles.push(std::thread::spawn(move || {
                map_clone.insert(i, PathBuf::from(format!("/shared/file_{i}")));
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(map.len(), 100);
    }

    #[test]
    fn forget_removes_mapping() {
        let map = InodePathMap::new(PathBuf::from("/shared"));
        map.insert(2, PathBuf::from("/shared/a"));
        map.insert(3, PathBuf::from("/shared/b"));

        // forget is just remove
        map.remove(2);
        assert!(map.get(2).is_err());
        assert!(map.get(3).is_ok());
    }
}
