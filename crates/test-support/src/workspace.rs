use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::snapshot::TreeSnapshot;

/// Creates an isolated working directory + undo directory for a single test.
/// Both directories are cleaned up when the workspace is dropped.
pub struct TempWorkspace {
    pub working_dir: PathBuf,
    pub undo_dir: PathBuf,
    _temp: TempDir,
}

impl Default for TempWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

impl TempWorkspace {
    /// Create an empty workspace with working_dir and undo_dir.
    pub fn new() -> Self {
        let temp = TempDir::new().expect("failed to create temp directory");
        let working_dir = temp.path().join("working");
        let undo_dir = temp.path().join("undo");
        std::fs::create_dir_all(&working_dir).expect("failed to create working_dir");
        std::fs::create_dir_all(&undo_dir).expect("failed to create undo_dir");
        Self {
            working_dir,
            undo_dir,
            _temp: temp,
        }
    }

    /// Create a workspace and populate working_dir using a fixture builder.
    pub fn with_fixture(f: impl FnOnce(&Path)) -> Self {
        let ws = Self::new();
        f(&ws.working_dir);
        ws
    }

    /// Capture a TreeSnapshot of the current working_dir state.
    pub fn snapshot(&self) -> TreeSnapshot {
        TreeSnapshot::capture(&self.working_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use crate::snapshot::{SnapshotCompareOptions, assert_tree_eq};

    #[test]
    fn workspace_new_creates_dirs() {
        let ws = TempWorkspace::new();
        assert!(ws.working_dir.exists());
        assert!(ws.undo_dir.exists());
        assert!(ws.working_dir.is_dir());
        assert!(ws.undo_dir.is_dir());
        // Both should be empty
        assert_eq!(std::fs::read_dir(&ws.working_dir).unwrap().count(), 0);
        assert_eq!(std::fs::read_dir(&ws.undo_dir).unwrap().count(), 0);
    }

    #[test]
    fn workspace_with_fixture() {
        let ws = TempWorkspace::with_fixture(fixtures::small_tree);
        assert!(ws.working_dir.join("small.txt").exists());
        assert!(ws.working_dir.join("src/main.rs").exists());
    }

    #[test]
    fn workspace_snapshot_round_trip() {
        let ws = TempWorkspace::with_fixture(fixtures::small_tree);
        let snap1 = ws.snapshot();
        let snap2 = ws.snapshot();
        assert_tree_eq(&snap1, &snap2, &SnapshotCompareOptions::default());
    }

    #[test]
    fn workspace_cleanup_on_drop() {
        let path;
        {
            let ws = TempWorkspace::new();
            path = ws.working_dir.clone();
            assert!(path.exists());
        }
        assert!(!path.exists());
    }
}
