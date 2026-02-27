use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use codeagent_common::{
    BarrierInfo, CodeAgentError, ExternalModificationPolicy, ResourceLimitsConfig, Result,
    RollbackResult, SafeguardConfig, SafeguardDecision, SafeguardEvent, StepId,
};

use crate::barrier::BarrierTracker;
use crate::gitignore::GitignoreFilter;
use crate::manifest::StepManifest;
use crate::preimage::{capture_creation_marker, capture_preimage, path_hash};
use crate::resource_limits;
use crate::rollback;
use crate::safeguard::{SafeguardHandler, SafeguardTracker};
use crate::step_tracker::StepTracker;
use crate::write_interceptor::WriteInterceptor;

/// The current on-disk format version. Compared against the `version` file
/// inside the undo directory on startup.
const CURRENT_VERSION: &str = "1";

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
    barrier_tracker: Mutex<BarrierTracker>,
    policy: ExternalModificationPolicy,
    resource_limits: Mutex<ResourceLimitsConfig>,
    safeguard_handler: Option<Box<dyn SafeguardHandler>>,
    gitignore_filter: Option<GitignoreFilter>,
    /// When true, undo operations are disabled due to a version mismatch.
    undo_disabled: Mutex<bool>,
    /// (expected, found) version strings when a mismatch is detected.
    version_mismatch_info: Mutex<Option<(String, String)>>,
    inner: Mutex<UndoInterceptorInner>,
}

struct UndoInterceptorInner {
    /// Relative paths already captured in the current step (first-touch guard).
    touched_paths: HashSet<String>,
    /// Manifest for the current in-progress step.
    current_manifest: Option<StepManifest>,
    /// Per-step safeguard counter and threshold tracker.
    safeguard_tracker: SafeguardTracker,
    /// Cumulative compressed preimage data size for the current step.
    current_step_data_size: u64,
    /// Set when the current step exceeds `max_single_step_size_bytes`.
    step_unprotected: bool,
}

impl UndoInterceptor {
    pub fn new(working_root: PathBuf, undo_dir: PathBuf) -> Self {
        Self::build(
            working_root,
            undo_dir,
            ExternalModificationPolicy::default(),
            SafeguardConfig::default(),
            None,
            ResourceLimitsConfig::default(),
            false,
        )
    }

    pub fn with_policy(
        working_root: PathBuf,
        undo_dir: PathBuf,
        policy: ExternalModificationPolicy,
    ) -> Self {
        Self::build(
            working_root,
            undo_dir,
            policy,
            SafeguardConfig::default(),
            None,
            ResourceLimitsConfig::default(),
            false,
        )
    }

    pub fn with_safeguard(
        working_root: PathBuf,
        undo_dir: PathBuf,
        policy: ExternalModificationPolicy,
        safeguard_config: SafeguardConfig,
        safeguard_handler: Box<dyn SafeguardHandler>,
    ) -> Self {
        Self::build(
            working_root,
            undo_dir,
            policy,
            safeguard_config,
            Some(safeguard_handler),
            ResourceLimitsConfig::default(),
            false,
        )
    }

    pub fn with_resource_limits(
        working_root: PathBuf,
        undo_dir: PathBuf,
        resource_limits: ResourceLimitsConfig,
    ) -> Self {
        Self::build(
            working_root,
            undo_dir,
            ExternalModificationPolicy::default(),
            SafeguardConfig::default(),
            None,
            resource_limits,
            false,
        )
    }

    pub fn with_gitignore(working_root: PathBuf, undo_dir: PathBuf) -> Self {
        Self::build(
            working_root,
            undo_dir,
            ExternalModificationPolicy::default(),
            SafeguardConfig::default(),
            None,
            ResourceLimitsConfig::default(),
            true,
        )
    }

    fn build(
        working_root: PathBuf,
        undo_dir: PathBuf,
        policy: ExternalModificationPolicy,
        safeguard_config: SafeguardConfig,
        safeguard_handler: Option<Box<dyn SafeguardHandler>>,
        resource_limits: ResourceLimitsConfig,
        respect_gitignore: bool,
    ) -> Self {
        let mut undo_disabled = false;
        let mut version_mismatch_info = None;

        // Initialize the on-disk layout or check the existing version
        let version_path = undo_dir.join("version");
        if version_path.exists() {
            let found_version = fs::read_to_string(&version_path)
                .unwrap_or_default()
                .trim()
                .to_string();
            if found_version != CURRENT_VERSION {
                undo_disabled = true;
                version_mismatch_info =
                    Some((CURRENT_VERSION.to_string(), found_version));
            }
        } else {
            fs::create_dir_all(&undo_dir).unwrap_or(());
            let _ = fs::write(&version_path, CURRENT_VERSION);
            fs::create_dir_all(undo_dir.join("wal")).unwrap_or(());
            fs::create_dir_all(undo_dir.join("steps")).unwrap_or(());
        }

        let step_tracker = StepTracker::new();

        // Reconstruct completed steps from on-disk steps/ directory
        if !undo_disabled {
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
        }

        // Load barriers from disk
        let barrier_tracker = BarrierTracker::load(&undo_dir);

        let gitignore_filter = if respect_gitignore {
            GitignoreFilter::build(&working_root)
        } else {
            None
        };

        Self {
            working_root,
            undo_dir,
            step_tracker,
            barrier_tracker: Mutex::new(barrier_tracker),
            policy,
            resource_limits: Mutex::new(resource_limits),
            safeguard_handler,
            gitignore_filter,
            undo_disabled: Mutex::new(undo_disabled),
            version_mismatch_info: Mutex::new(version_mismatch_info),
            inner: Mutex::new(UndoInterceptorInner {
                touched_paths: HashSet::new(),
                current_manifest: None,
                safeguard_tracker: SafeguardTracker::new(safeguard_config),
                current_step_data_size: 0,
                step_unprotected: false,
            }),
        }
    }

    /// Check if undo is disabled due to a version mismatch.
    fn check_undo_enabled(&self) -> Result<()> {
        if *self.undo_disabled.lock().unwrap() {
            let info = self.version_mismatch_info.lock().unwrap();
            if let Some((ref expected, ref found)) = *info {
                return Err(CodeAgentError::UndoDisabled {
                    expected_version: expected.clone(),
                    found_version: found.clone(),
                });
            }
        }
        Ok(())
    }

    /// Open a new undo step.
    pub fn open_step(&self, id: StepId) -> Result<()> {
        self.check_undo_enabled()?;
        self.step_tracker.open_step(id)?;

        let mut inner = self.inner.lock().unwrap();
        inner.touched_paths.clear();
        inner.current_manifest = Some(StepManifest::new(id));
        inner.safeguard_tracker.reset();
        inner.current_step_data_size = 0;
        inner.step_unprotected = false;

        // Create WAL directory for this step
        let wal_dir = self.wal_in_progress_dir();
        if wal_dir.exists() {
            fs::remove_dir_all(&wal_dir)?;
        }
        fs::create_dir_all(wal_dir.join("preimages"))?;

        Ok(())
    }

    /// Close the current step, promoting WAL to steps/.
    /// Returns the list of step IDs that were evicted due to resource limits.
    pub fn close_step(&self, id: StepId) -> Result<Vec<StepId>> {
        // Write the manifest before promotion, including unprotected flag
        {
            let inner = self.inner.lock().unwrap();
            if let Some(ref manifest) = inner.current_manifest {
                let mut manifest_to_write = manifest.clone();
                if inner.step_unprotected {
                    manifest_to_write.unprotected = true;
                }
                manifest_to_write.write_to(&self.wal_in_progress_dir())?;
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

        {
            let mut inner = self.inner.lock().unwrap();
            inner.touched_paths.clear();
            inner.current_manifest = None;
            inner.current_step_data_size = 0;
            inner.step_unprotected = false;
        }

        // Run eviction after step promotion
        let limits = self.resource_limits.lock().unwrap().clone();
        let steps_dir = self.undo_dir.join("steps");
        let mut barrier_tracker = self.barrier_tracker.lock().unwrap();
        let evicted = resource_limits::evict_if_needed(
            &steps_dir,
            &self.step_tracker,
            &mut barrier_tracker,
            &limits,
            &self.undo_dir,
        )?;

        Ok(evicted)
    }

    /// Rollback the most recent N steps (pop semantics — removed from history).
    ///
    /// If `force` is false and any undo barriers exist between the current state
    /// and the target, the rollback is rejected with `RollbackBlocked`.
    /// If `force` is true, barriers are crossed and removed.
    /// If any step in the rollback range is unprotected, returns `StepUnprotected`.
    pub fn rollback(&self, count: usize, force: bool) -> Result<RollbackResult> {
        let completed = self.step_tracker.completed_steps();
        let steps_to_rollback: Vec<StepId> =
            completed.iter().rev().take(count).copied().collect();

        // Check for unprotected steps
        for step_id in &steps_to_rollback {
            let step_dir = self.step_dir(*step_id);
            if step_dir.exists() {
                if let Ok(manifest) = StepManifest::read_from(&step_dir) {
                    if manifest.unprotected {
                        return Err(CodeAgentError::StepUnprotected { step_id: *step_id });
                    }
                }
            }
        }

        // Check for blocking barriers
        let mut barrier_tracker = self.barrier_tracker.lock().unwrap();
        let blocking: Vec<BarrierInfo> = barrier_tracker
            .barriers_blocking_rollback(&steps_to_rollback)
            .into_iter()
            .cloned()
            .collect();

        if !blocking.is_empty() && !force {
            return Err(CodeAgentError::RollbackBlocked {
                count: blocking.len(),
                barriers: blocking,
            });
        }

        // Perform the rollback
        for step_id in &steps_to_rollback {
            let step_dir = self.step_dir(*step_id);
            if step_dir.exists() {
                rollback::rollback_step(&step_dir, &self.working_root)?;
                fs::remove_dir_all(&step_dir)?;
                self.step_tracker.remove_completed_step(*step_id);
            }
        }

        // Remove crossed barriers (pop semantics)
        if !blocking.is_empty() {
            barrier_tracker.remove_barriers_for_steps(&steps_to_rollback);
            barrier_tracker.save(&self.undo_dir)?;
        }

        Ok(RollbackResult {
            steps_rolled_back: steps_to_rollback.len(),
            barriers_crossed: blocking,
        })
    }

    /// Get the list of completed step IDs.
    pub fn completed_steps(&self) -> Vec<StepId> {
        self.step_tracker.completed_steps()
    }

    /// Record an external modification, optionally creating an undo barrier.
    ///
    /// Under `Barrier` policy, creates a barrier and returns it.
    /// Under `Warn` policy, returns `None` (no barrier created).
    pub fn notify_external_modification(
        &self,
        affected_paths: Vec<PathBuf>,
    ) -> Result<Option<BarrierInfo>> {
        match self.policy {
            ExternalModificationPolicy::Barrier => {
                let completed = self.step_tracker.completed_steps();
                let after_step_id = completed.last().copied().unwrap_or(0);

                let mut barrier_tracker = self.barrier_tracker.lock().unwrap();
                let barrier = barrier_tracker.create_barrier(after_step_id, affected_paths);
                barrier_tracker.save(&self.undo_dir)?;
                Ok(Some(barrier))
            }
            ExternalModificationPolicy::Warn => Ok(None),
        }
    }

    /// Return all current undo barriers.
    pub fn barriers(&self) -> Vec<BarrierInfo> {
        self.barrier_tracker.lock().unwrap().barriers()
    }

    /// Whether undo is disabled due to a version mismatch.
    pub fn is_undo_disabled(&self) -> bool {
        *self.undo_disabled.lock().unwrap()
    }

    /// Returns the version mismatch info if undo is disabled, as (expected, found).
    pub fn version_mismatch(&self) -> Option<(String, String)> {
        self.version_mismatch_info.lock().unwrap().clone()
    }

    /// Discard the entire undo log and reinitialize with the current version.
    /// Used after a version mismatch when the user confirms discarding old history.
    pub fn discard(&self) -> Result<()> {
        // Remove the entire undo directory contents
        if self.undo_dir.exists() {
            fs::remove_dir_all(&self.undo_dir)?;
        }

        // Reinitialize the on-disk layout
        fs::create_dir_all(&self.undo_dir)?;
        fs::write(self.undo_dir.join("version"), CURRENT_VERSION)?;
        fs::create_dir_all(self.undo_dir.join("wal"))?;
        fs::create_dir_all(self.undo_dir.join("steps"))?;

        // Clear in-memory state
        {
            let mut inner = self.inner.lock().unwrap();
            inner.touched_paths.clear();
            inner.current_manifest = None;
            inner.current_step_data_size = 0;
            inner.step_unprotected = false;
        }

        // Clear step tracker (remove all completed steps)
        for step_id in self.step_tracker.completed_steps() {
            self.step_tracker.remove_completed_step(step_id);
        }

        // Clear barriers
        {
            let mut barrier_tracker = self.barrier_tracker.lock().unwrap();
            *barrier_tracker = BarrierTracker::load(&self.undo_dir);
        }

        // Re-enable undo
        *self.undo_disabled.lock().unwrap() = false;
        *self.version_mismatch_info.lock().unwrap() = None;

        Ok(())
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

    /// Roll back the current in-progress step and cancel it.
    /// Used when a safeguard denies the current operation — undoes all
    /// operations already applied in this step.
    fn rollback_current_step(&self) -> Result<()> {
        let wal_dir = self.wal_in_progress_dir();

        // Write manifest and clear inner state
        {
            let mut inner = self.inner.lock().unwrap();
            if let Some(ref manifest) = inner.current_manifest {
                manifest.write_to(&wal_dir)?;
            }
            inner.touched_paths.clear();
            inner.current_manifest = None;
            inner.safeguard_tracker.reset();
            inner.current_step_data_size = 0;
            inner.step_unprotected = false;
        }

        // Roll back using the WAL data
        if wal_dir.exists() {
            rollback::rollback_step(&wal_dir, &self.working_root)?;
            fs::remove_dir_all(&wal_dir)?;
        }

        // Cancel the step (not completed, just discarded)
        self.step_tracker.cancel_step()?;

        Ok(())
    }

    /// Process a safeguard event: call the handler and act on the decision.
    /// Returns `Ok(())` if there is no event or if the handler allows.
    /// Returns `Err(SafeguardDenied)` if the handler denies.
    fn handle_safeguard_event(&self, event: Option<SafeguardEvent>) -> Result<()> {
        let event = match event {
            Some(e) => e,
            None => return Ok(()),
        };

        let handler = match &self.safeguard_handler {
            Some(h) => h,
            None => return Ok(()),
        };

        let step_id = event.step_id;
        let safeguard_id = event.safeguard_id;
        let kind = event.kind.clone();

        let decision = handler.on_safeguard_triggered(event);

        match decision {
            SafeguardDecision::Allow => {
                let mut inner = self.inner.lock().unwrap();
                inner.safeguard_tracker.mark_allowed(&kind);
                Ok(())
            }
            SafeguardDecision::Deny => {
                self.rollback_current_step()?;
                Err(CodeAgentError::SafeguardDenied {
                    safeguard_id,
                    step_id,
                })
            }
        }
    }

    /// Get the forward-slash-normalized relative path string for a path.
    fn relative_path_str(&self, path: &Path) -> String {
        path.strip_prefix(&self.working_root)
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default()
    }

    fn wal_in_progress_dir(&self) -> PathBuf {
        self.undo_dir.join("wal").join("in_progress")
    }

    fn step_dir(&self, id: StepId) -> PathBuf {
        self.undo_dir.join("steps").join(id.to_string())
    }

    /// Ensure the preimage for an existing path is captured on first touch.
    /// Returns true if this was the first touch (preimage was captured).
    /// Skips capture if the step is already marked unprotected.
    fn ensure_preimage(&self, file_path: &Path) -> Result<bool> {
        let relative = file_path.strip_prefix(&self.working_root).map_err(|_| {
            CodeAgentError::Preimage {
                path: file_path.to_path_buf(),
                message: "path outside working root".to_string(),
            }
        })?;
        let relative_str = normalized_relative_path(relative);

        if let Some(ref filter) = self.gitignore_filter {
            let is_dir = file_path.symlink_metadata().map(|m| m.is_dir()).unwrap_or(false);
            if filter.is_ignored(&relative_str, is_dir) {
                return Ok(false);
            }
        }

        let mut inner = self.inner.lock().unwrap();

        // Skip if step is already unprotected (exceeded size limit)
        if inner.step_unprotected {
            return Ok(false);
        }

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

        let (meta, data_size) = capture_preimage(file_path, &self.working_root, &wal_preimage_dir)?;
        if let Some(ref mut manifest) = inner.current_manifest {
            manifest.add_entry(&relative_str, &hash, true, meta.file_type.as_str());
        }

        inner.touched_paths.insert(relative_str);

        // Track cumulative data size and check threshold
        inner.current_step_data_size += data_size;
        let limits = self.resource_limits.lock().unwrap();
        if let Some(max_size) = limits.max_single_step_size_bytes {
            if inner.current_step_data_size > max_size {
                inner.step_unprotected = true;
            }
        }

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

        if let Some(ref filter) = self.gitignore_filter {
            let is_dir = file_path.is_dir();
            if filter.is_ignored(&relative_str, is_dir) {
                return Ok(());
            }
        }

        let mut inner = self.inner.lock().unwrap();

        // Skip if step is already unprotected
        if inner.step_unprotected {
            return Ok(());
        }

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

            // Skip ignored subtrees early to avoid unnecessary I/O
            if let Some(ref filter) = self.gitignore_filter {
                if let Ok(relative) = path.strip_prefix(&self.working_root) {
                    let relative_str = normalized_relative_path(relative);
                    if filter.is_ignored(&relative_str, path.is_dir()) {
                        continue;
                    }
                }
            }

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
        if let Some(step_id) = self.step_tracker.current_step() {
            let file_size = path.metadata().map(|m| m.len()).ok();
            self.ensure_preimage(path)?;

            if let Some(size) = file_size {
                let relative = self.relative_path_str(path);
                let event = {
                    let mut inner = self.inner.lock().unwrap();
                    inner.safeguard_tracker.check_overwrite(&relative, size, step_id)
                };
                self.handle_safeguard_event(event)?;
            }
        }
        Ok(())
    }

    fn pre_unlink(&self, path: &Path, is_dir: bool) -> Result<()> {
        if let Some(step_id) = self.step_tracker.current_step() {
            self.ensure_preimage(path)?;
            if is_dir {
                self.capture_tree_preimages(path)?;
            }

            let relative = self.relative_path_str(path);
            let event = {
                let mut inner = self.inner.lock().unwrap();
                inner.safeguard_tracker.check_delete(&relative, step_id)
            };
            self.handle_safeguard_event(event)?;
        }
        Ok(())
    }

    fn pre_rename(&self, from: &Path, to: &Path) -> Result<()> {
        if let Some(step_id) = self.step_tracker.current_step() {
            let destination_exists = to.symlink_metadata().is_ok();
            self.ensure_preimage(from)?;
            if destination_exists {
                self.ensure_preimage(to)?;
            }
            if from.is_dir() {
                self.capture_tree_preimages(from)?;
            }

            if destination_exists {
                let source_rel = self.relative_path_str(from);
                let dest_rel = self.relative_path_str(to);
                let event = {
                    let mut inner = self.inner.lock().unwrap();
                    inner
                        .safeguard_tracker
                        .check_rename_over(&source_rel, &dest_rel, step_id)
                };
                self.handle_safeguard_event(event)?;
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
        if let Some(step_id) = self.step_tracker.current_step() {
            let file_size = path.metadata().map(|m| m.len()).ok();
            self.ensure_preimage(path)?;

            if let Some(size) = file_size {
                let relative = self.relative_path_str(path);
                let event = {
                    let mut inner = self.inner.lock().unwrap();
                    inner.safeguard_tracker.check_overwrite(&relative, size, step_id)
                };
                self.handle_safeguard_event(event)?;
            }
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
