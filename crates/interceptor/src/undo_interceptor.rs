use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use codeagent_common::{CodeAgentError, Result, StepId};

use crate::manifest::StepManifest;
use crate::preimage::{capture_creation_marker, capture_preimage, path_hash};
use crate::rollback;
use crate::step_tracker::StepTracker;
use crate::write_interceptor::WriteInterceptor;

/// Information about a crash recovery that was performed on startup.
#[derive(Debug, Clone)]
pub struct RecoveryInfo {
    /// Number of paths that had their preimages restored.
    pub paths_restored: usize,
    /// Number of paths that were deleted (created during the incomplete step).
    pub paths_deleted: usize,
    /// Whether the manifest was present and parseable.
    pub manifest_valid: bool,
}

pub struct UndoInterceptor {
    working_root: PathBuf,
    undo_dir: PathBuf,
    step_tracker: StepTracker,
    inner: Mutex<UndoInterceptorInner>,
}

struct UndoInterceptorInner {
    /// Relative paths already captured in the current step (first-touch guard).
    touched_paths: HashSet<String>,
    /// Manifest for the current in-progress step.
    current_manifest: Option<StepManifest>,
}

impl UndoInterceptor {
    pub fn new(working_root: PathBuf, undo_dir: PathBuf) -> Self {
        // Initialize the on-disk layout
        let version_path = undo_dir.join("version");
        if !version_path.exists() {
            fs::create_dir_all(&undo_dir).unwrap_or(());
            let _ = fs::write(&version_path, "1");
            fs::create_dir_all(undo_dir.join("wal")).unwrap_or(());
            fs::create_dir_all(undo_dir.join("steps")).unwrap_or(());
        }

        let step_tracker = StepTracker::new();

        // Reconstruct completed steps from on-disk steps/ directory
        let steps_dir = undo_dir.join("steps");
        if steps_dir.exists() {
            let mut step_ids: Vec<StepId> = Vec::new();
            if let Ok(entries) = fs::read_dir(&steps_dir) {
                for entry in entries.flatten() {
                    if let Some(name) = entry.file_name().to_str() {
                        if let Ok(id) = name.parse::<StepId>() {
                            step_ids.push(id);
                        }
                    }
                }
            }
            step_ids.sort();
            for id in step_ids {
                step_tracker.add_completed_step(id);
            }
        }

        Self {
            working_root,
            undo_dir,
            step_tracker,
            inner: Mutex::new(UndoInterceptorInner {
                touched_paths: HashSet::new(),
                current_manifest: None,
            }),
        }
    }

    /// Open a new undo step.
    pub fn open_step(&self, id: StepId) -> Result<()> {
        self.step_tracker.open_step(id)?;

        let mut inner = self.inner.lock().unwrap();
        inner.touched_paths.clear();
        inner.current_manifest = Some(StepManifest::new(id));

        // Create WAL directory for this step
        let wal_dir = self.wal_in_progress_dir();
        if wal_dir.exists() {
            fs::remove_dir_all(&wal_dir)?;
        }
        fs::create_dir_all(wal_dir.join("preimages"))?;

        Ok(())
    }

    /// Close the current step, promoting WAL to steps/.
    pub fn close_step(&self, id: StepId) -> Result<()> {
        // Write the manifest before promotion
        {
            let inner = self.inner.lock().unwrap();
            if let Some(ref manifest) = inner.current_manifest {
                manifest.write_to(&self.wal_in_progress_dir())?;
            }
        }

        // Promote WAL to steps/
        let wal_dir = self.wal_in_progress_dir();
        let step_dir = self.step_dir(id);
        if wal_dir.exists() {
            if step_dir.exists() {
                fs::remove_dir_all(&step_dir)?;
            }
            fs::rename(&wal_dir, &step_dir)?;
        }

        self.step_tracker.close_step(id)?;

        let mut inner = self.inner.lock().unwrap();
        inner.touched_paths.clear();
        inner.current_manifest = None;

        Ok(())
    }

    /// Rollback the most recent N steps (pop semantics — removed from history).
    pub fn rollback(&self, count: usize) -> Result<()> {
        let completed = self.step_tracker.completed_steps();
        let steps_to_rollback: Vec<StepId> =
            completed.iter().rev().take(count).copied().collect();

        for step_id in &steps_to_rollback {
            let step_dir = self.step_dir(*step_id);
            if step_dir.exists() {
                rollback::rollback_step(&step_dir, &self.working_root)?;
                fs::remove_dir_all(&step_dir)?;
                self.step_tracker.remove_completed_step(*step_id);
            }
        }

        Ok(())
    }

    /// Get the list of completed step IDs.
    pub fn completed_steps(&self) -> Vec<StepId> {
        self.step_tracker.completed_steps()
    }

    /// Recover from a crash by rolling back any incomplete step in the WAL.
    /// Returns `None` if no recovery was needed, or `Some(RecoveryInfo)` with details.
    pub fn recover(&self) -> Result<Option<RecoveryInfo>> {
        let wal_dir = self.wal_in_progress_dir();

        if !wal_dir.exists() {
            return Ok(None);
        }

        let preimage_dir = wal_dir.join("preimages");
        let manifest_path = wal_dir.join("manifest.json");

        let has_preimages = preimage_dir.exists()
            && fs::read_dir(&preimage_dir)
                .map(|mut entries| entries.next().is_some())
                .unwrap_or(false);
        let has_manifest = manifest_path.exists();

        // Empty WAL entry (step opened but no operations before crash)
        if !has_preimages && !has_manifest {
            fs::remove_dir_all(&wal_dir)?;
            return Ok(Some(RecoveryInfo {
                paths_restored: 0,
                paths_deleted: 0,
                manifest_valid: false,
            }));
        }

        // Try to load or reconstruct the manifest
        let (manifest, manifest_valid) = if has_manifest {
            match StepManifest::read_from(&wal_dir) {
                Ok(m) => (m, true),
                Err(_) => {
                    let m = self.rebuild_manifest_from_preimages(&preimage_dir)?;
                    (m, false)
                }
            }
        } else {
            let m = self.rebuild_manifest_from_preimages(&preimage_dir)?;
            (m, false)
        };

        let paths_restored = manifest.entries.values().filter(|e| e.existed_before).count();
        let paths_deleted = manifest.entries.values().filter(|e| !e.existed_before).count();

        if !manifest.entries.is_empty() {
            // Write the reconstructed manifest so rollback_step can read it
            if !manifest_valid {
                manifest.write_to(&wal_dir)?;
            }
            rollback::rollback_step(&wal_dir, &self.working_root)?;
        }

        fs::remove_dir_all(&wal_dir)?;

        Ok(Some(RecoveryInfo {
            paths_restored,
            paths_deleted,
            manifest_valid,
        }))
    }

    /// Reconstruct a StepManifest by scanning preimage metadata files.
    /// Used during recovery when manifest.json is missing or corrupt.
    fn rebuild_manifest_from_preimages(
        &self,
        preimage_dir: &Path,
    ) -> Result<StepManifest> {
        use crate::preimage::PreimageMetadata;

        let mut manifest = StepManifest::new(0);

        if !preimage_dir.exists() {
            return Ok(manifest);
        }

        for entry in fs::read_dir(preimage_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            if !name_str.ends_with(".meta.json") {
                continue;
            }

            let hash = name_str
                .strip_suffix(".meta.json")
                .unwrap()
                .to_string();

            let meta_json = fs::read_to_string(entry.path())?;
            match serde_json::from_str::<PreimageMetadata>(&meta_json) {
                Ok(meta) => {
                    manifest.add_entry(
                        &meta.relative_path,
                        &hash,
                        meta.existed_before,
                        meta.file_type.as_str(),
                    );
                }
                Err(_) => {
                    continue;
                }
            }
        }

        Ok(manifest)
    }

    fn wal_in_progress_dir(&self) -> PathBuf {
        self.undo_dir.join("wal").join("in_progress")
    }

    fn step_dir(&self, id: StepId) -> PathBuf {
        self.undo_dir.join("steps").join(id.to_string())
    }

    /// Ensure the preimage for an existing path is captured on first touch.
    /// Returns true if this was the first touch (preimage was captured).
    fn ensure_preimage(&self, file_path: &Path) -> Result<bool> {
        let relative = file_path.strip_prefix(&self.working_root).map_err(|_| {
            CodeAgentError::Preimage {
                path: file_path.to_path_buf(),
                message: "path outside working root".to_string(),
            }
        })?;
        let relative_str = normalized_relative_path(relative);

        let mut inner = self.inner.lock().unwrap();

        // First-touch check
        if inner.touched_paths.contains(&relative_str) {
            return Ok(false);
        }

        // Path must exist to capture a preimage
        if file_path.symlink_metadata().is_err() {
            return Ok(false);
        }

        let wal_preimage_dir = self.wal_in_progress_dir().join("preimages");
        let hash = path_hash(relative);

        let meta = capture_preimage(file_path, &self.working_root, &wal_preimage_dir)?;
        if let Some(ref mut manifest) = inner.current_manifest {
            manifest.add_entry(&relative_str, &hash, true, meta.file_type.as_str());
        }

        inner.touched_paths.insert(relative_str);
        Ok(true)
    }

    /// Record that a path was newly created (did not exist before the step).
    fn record_creation(&self, file_path: &Path) -> Result<()> {
        let relative = file_path.strip_prefix(&self.working_root).map_err(|_| {
            CodeAgentError::Preimage {
                path: file_path.to_path_buf(),
                message: "path outside working root".to_string(),
            }
        })?;
        let relative_str = normalized_relative_path(relative);

        let mut inner = self.inner.lock().unwrap();

        if inner.touched_paths.contains(&relative_str) {
            return Ok(());
        }

        let wal_preimage_dir = self.wal_in_progress_dir().join("preimages");
        let hash = path_hash(relative);

        let meta = capture_creation_marker(file_path, &self.working_root, &wal_preimage_dir)?;

        if let Some(ref mut manifest) = inner.current_manifest {
            manifest.add_entry(&relative_str, &hash, false, meta.file_type.as_str());
        }

        inner.touched_paths.insert(relative_str);
        Ok(())
    }

    /// Recursively capture preimages for all entries under a directory.
    fn capture_tree_preimages(&self, dir_path: &Path) -> Result<()> {
        if !dir_path.is_dir() {
            return Ok(());
        }
        for entry in fs::read_dir(dir_path)? {
            let entry = entry?;
            let path = entry.path();
            self.ensure_preimage(&path)?;
            if path.is_dir() {
                self.capture_tree_preimages(&path)?;
            }
        }
        Ok(())
    }
}

/// Normalize path separators to forward slashes for consistent comparison.
fn normalized_relative_path(relative: &Path) -> String {
    relative.to_string_lossy().replace('\\', "/")
}

impl WriteInterceptor for UndoInterceptor {
    fn pre_write(&self, path: &Path) -> Result<()> {
        if self.step_tracker.current_step().is_some() {
            self.ensure_preimage(path)?;
        }
        Ok(())
    }

    fn pre_unlink(&self, path: &Path, is_dir: bool) -> Result<()> {
        if self.step_tracker.current_step().is_some() {
            self.ensure_preimage(path)?;
            if is_dir {
                self.capture_tree_preimages(path)?;
            }
        }
        Ok(())
    }

    fn pre_rename(&self, from: &Path, to: &Path) -> Result<()> {
        if self.step_tracker.current_step().is_some() {
            self.ensure_preimage(from)?;
            // If destination exists, capture its preimage too
            if to.symlink_metadata().is_ok() {
                self.ensure_preimage(to)?;
            }
            // For directory renames, capture all children of the source
            if from.is_dir() {
                self.capture_tree_preimages(from)?;
            }
        }
        Ok(())
    }

    fn post_create(&self, path: &Path) -> Result<()> {
        if self.step_tracker.current_step().is_some() {
            self.record_creation(path)?;
        }
        Ok(())
    }

    fn post_mkdir(&self, path: &Path) -> Result<()> {
        if self.step_tracker.current_step().is_some() {
            self.record_creation(path)?;
        }
        Ok(())
    }

    fn pre_setattr(&self, path: &Path) -> Result<()> {
        if self.step_tracker.current_step().is_some() {
            self.ensure_preimage(path)?;
        }
        Ok(())
    }

    fn pre_link(&self, target: &Path, _link_path: &Path) -> Result<()> {
        if self.step_tracker.current_step().is_some() {
            self.ensure_preimage(target)?;
        }
        Ok(())
    }

    fn post_symlink(&self, _target: &Path, link_path: &Path) -> Result<()> {
        if self.step_tracker.current_step().is_some() {
            self.record_creation(link_path)?;
        }
        Ok(())
    }

    fn pre_xattr(&self, path: &Path) -> Result<()> {
        if self.step_tracker.current_step().is_some() {
            self.ensure_preimage(path)?;
        }
        Ok(())
    }

    fn pre_open_trunc(&self, path: &Path) -> Result<()> {
        if self.step_tracker.current_step().is_some() {
            self.ensure_preimage(path)?;
        }
        Ok(())
    }

    fn pre_fallocate(&self, path: &Path) -> Result<()> {
        if self.step_tracker.current_step().is_some() {
            self.ensure_preimage(path)?;
        }
        Ok(())
    }

    fn pre_copy_file_range(&self, dst_path: &Path) -> Result<()> {
        if self.step_tracker.current_step().is_some() {
            self.ensure_preimage(dst_path)?;
        }
        Ok(())
    }

    fn current_step(&self) -> Option<StepId> {
        self.step_tracker.current_step()
    }
}
