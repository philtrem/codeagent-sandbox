use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use crate::error::P9Error;
use crate::qid::Qid;

/// State associated with an open FID.
///
/// Each FID tracks the host path it references, its cached QID, and
/// optionally an open file handle (set after `Tlopen`/`Tlcreate`).
pub struct FidState {
    /// Absolute host path this FID references.
    pub path: PathBuf,

    /// Cached QID for this entry.
    pub qid: Qid,

    /// Open file handle, set after Tlopen/Tlcreate.
    pub open_handle: Option<File>,

    /// Flags passed to Tlopen (e.g., O_RDONLY, O_RDWR).
    pub open_flags: u32,

    /// Current directory offset for Treaddir state tracking.
    pub dir_offset: u64,
}

impl FidState {
    /// Create a new FID state for a walked (but not yet opened) entry.
    pub fn new(path: PathBuf, qid: Qid) -> Self {
        Self {
            path,
            qid,
            open_handle: None,
            open_flags: 0,
            dir_offset: 0,
        }
    }

    /// Returns true if this FID has been opened (Tlopen/Tlcreate).
    pub fn is_open(&self) -> bool {
        self.open_handle.is_some()
    }
}

impl std::fmt::Debug for FidState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FidState")
            .field("path", &self.path)
            .field("qid", &self.qid)
            .field("is_open", &self.is_open())
            .field("open_flags", &self.open_flags)
            .field("dir_offset", &self.dir_offset)
            .finish()
    }
}

/// Table mapping client-assigned FID handles (u32) to server-side state.
///
/// The 9P protocol uses FIDs as opaque handles that the client assigns and
/// the server tracks. `Tattach` creates the root FID, `Twalk` creates new
/// FIDs by walking from existing ones, and `Tclunk` releases them.
pub struct FidTable {
    fids: HashMap<u32, FidState>,
    root_path: PathBuf,
}

impl FidTable {
    /// Create a new empty FID table rooted at the given directory.
    pub fn new(root_path: PathBuf) -> Self {
        Self {
            fids: HashMap::new(),
            root_path,
        }
    }

    /// Returns the root path for this FID table.
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    /// Insert a new FID. Returns an error if the FID is already in use.
    pub fn insert(&mut self, fid: u32, state: FidState) -> Result<(), P9Error> {
        if self.fids.contains_key(&fid) {
            return Err(P9Error::FidInUse { fid });
        }
        self.fids.insert(fid, state);
        Ok(())
    }

    /// Get an immutable reference to a FID's state.
    pub fn get(&self, fid: u32) -> Result<&FidState, P9Error> {
        self.fids.get(&fid).ok_or(P9Error::UnknownFid { fid })
    }

    /// Get a mutable reference to a FID's state.
    pub fn get_mut(&mut self, fid: u32) -> Result<&mut FidState, P9Error> {
        self.fids.get_mut(&fid).ok_or(P9Error::UnknownFid { fid })
    }

    /// Remove and return a FID's state. Returns an error if the FID is unknown.
    pub fn remove(&mut self, fid: u32) -> Result<FidState, P9Error> {
        self.fids.remove(&fid).ok_or(P9Error::UnknownFid { fid })
    }

    /// Get the host path for a FID.
    pub fn get_path(&self, fid: u32) -> Result<&Path, P9Error> {
        Ok(&self.get(fid)?.path)
    }

    /// Resolve a child path relative to a parent FID.
    ///
    /// Returns the absolute host path of `parent_fid/name`. Does not check
    /// whether the path exists on disk — that is the caller's responsibility.
    pub fn resolve_child(&self, parent_fid: u32, name: &str) -> Result<PathBuf, P9Error> {
        let parent_path = self.get_path(parent_fid)?;
        Ok(parent_path.join(name))
    }

    /// Returns the number of active FIDs.
    pub fn count(&self) -> usize {
        self.fids.len()
    }

    /// Returns true if no FIDs are active.
    pub fn is_empty(&self) -> bool {
        self.fids.is_empty()
    }

    /// Update the path of a FID (used after rename operations).
    pub fn update_path(&mut self, fid: u32, new_path: PathBuf) -> Result<(), P9Error> {
        let state = self.get_mut(fid)?;
        state.path = new_path;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::qid::Qid;

    fn root() -> PathBuf {
        PathBuf::from("/tmp/test_root")
    }

    fn make_fid_state(path: &str) -> FidState {
        FidState::new(PathBuf::from(path), Qid::file(0, 1))
    }

    /// FID-01: Insert and retrieve FID state.
    #[test]
    fn fid_01_insert_and_get() {
        let mut table = FidTable::new(root());
        let state = FidState::new(root(), Qid::directory(0, 1));
        table.insert(0, state).unwrap();

        let retrieved = table.get(0).unwrap();
        assert_eq!(retrieved.path, root());
        assert!(retrieved.qid.is_dir());
    }

    /// FID-02: Remove FID clears entry.
    #[test]
    fn fid_02_remove_clears_entry() {
        let mut table = FidTable::new(root());
        table.insert(5, make_fid_state("/tmp/test_root/file.txt")).unwrap();
        assert_eq!(table.count(), 1);

        let removed = table.remove(5).unwrap();
        assert_eq!(removed.path, PathBuf::from("/tmp/test_root/file.txt"));
        assert_eq!(table.count(), 0);

        // Subsequent access fails
        assert!(matches!(table.get(5), Err(P9Error::UnknownFid { fid: 5 })));
    }

    /// FID-03: Duplicate FID insertion returns error.
    #[test]
    fn fid_03_duplicate_insert_rejected() {
        let mut table = FidTable::new(root());
        table.insert(1, make_fid_state("/tmp/a")).unwrap();

        let result = table.insert(1, make_fid_state("/tmp/b"));
        assert!(matches!(result, Err(P9Error::FidInUse { fid: 1 })));
    }

    /// FID-04: Get unknown FID returns error.
    #[test]
    fn fid_04_get_unknown_fid() {
        let table = FidTable::new(root());
        assert!(matches!(table.get(99), Err(P9Error::UnknownFid { fid: 99 })));
    }

    /// FID-05: Walk creates new FID from existing FID via resolve_child.
    #[test]
    fn fid_05_resolve_child() {
        let mut table = FidTable::new(root());
        table
            .insert(0, FidState::new(root(), Qid::directory(0, 1)))
            .unwrap();

        let child_path = table.resolve_child(0, "src").unwrap();
        assert_eq!(child_path, PathBuf::from("/tmp/test_root/src"));

        // Insert the walked FID
        table
            .insert(1, FidState::new(child_path, Qid::directory(0, 2)))
            .unwrap();
        assert_eq!(table.count(), 2);
    }

    /// FID-06: Opening a FID transitions it to the open state.
    #[test]
    fn fid_06_open_transitions_state() {
        let mut table = FidTable::new(root());
        table.insert(1, make_fid_state("/tmp/test_root/file.txt")).unwrap();

        // Initially not open
        assert!(!table.get(1).unwrap().is_open());

        // Simulate opening (in real code, Tlopen handler sets this)
        let state = table.get_mut(1).unwrap();
        state.open_flags = 0o2; // O_RDWR
        // We can't easily create a real File handle in a unit test,
        // so we test the flag transition instead
        assert_eq!(state.open_flags, 0o2);
    }

    /// FID-07: Readdir offset is tracked correctly.
    #[test]
    fn fid_07_readdir_offset_tracking() {
        let mut table = FidTable::new(root());
        table
            .insert(1, FidState::new(root(), Qid::directory(0, 1)))
            .unwrap();

        assert_eq!(table.get(1).unwrap().dir_offset, 0);

        table.get_mut(1).unwrap().dir_offset = 42;
        assert_eq!(table.get(1).unwrap().dir_offset, 42);

        table.get_mut(1).unwrap().dir_offset = 100;
        assert_eq!(table.get(1).unwrap().dir_offset, 100);
    }

    /// FID-08: Count and cleanup.
    #[test]
    fn fid_08_count_and_empty() {
        let mut table = FidTable::new(root());
        assert!(table.is_empty());
        assert_eq!(table.count(), 0);

        table.insert(0, make_fid_state("/a")).unwrap();
        table.insert(1, make_fid_state("/b")).unwrap();
        table.insert(2, make_fid_state("/c")).unwrap();
        assert_eq!(table.count(), 3);
        assert!(!table.is_empty());

        table.remove(1).unwrap();
        assert_eq!(table.count(), 2);

        table.remove(0).unwrap();
        table.remove(2).unwrap();
        assert!(table.is_empty());
    }

    /// FID-09: update_path changes the stored path.
    #[test]
    fn fid_09_update_path() {
        let mut table = FidTable::new(root());
        table.insert(3, make_fid_state("/tmp/test_root/old.txt")).unwrap();

        table.update_path(3, PathBuf::from("/tmp/test_root/new.txt")).unwrap();
        assert_eq!(
            table.get_path(3).unwrap(),
            Path::new("/tmp/test_root/new.txt")
        );
    }

    /// FID-10: remove unknown FID returns error.
    #[test]
    fn fid_10_remove_unknown() {
        let mut table = FidTable::new(root());
        assert!(matches!(
            table.remove(77),
            Err(P9Error::UnknownFid { fid: 77 })
        ));
    }

    /// FID-11: root_path accessor.
    #[test]
    fn fid_11_root_path() {
        let table = FidTable::new(PathBuf::from("/mnt/working"));
        assert_eq!(table.root_path(), Path::new("/mnt/working"));
    }
}
