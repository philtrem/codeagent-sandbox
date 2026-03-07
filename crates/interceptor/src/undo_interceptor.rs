use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use codeagent_common::{
    AffectedPath, BarrierInfo, BarrierReason, CodeAgentError, ExternalModificationPolicy,
    ResourceLimitsConfig, Result, RollbackResult, SafeguardConfig, SafeguardDecision,
    SafeguardEvent, StepId, StepManager, SymlinkPolicy,
};
use serde::{Deserialize, Serialize};

use crate::gitignore::build_gitignore;
use ignore::gitignore::Gitignore;
use crate::manifest::StepManifest;
use crate::preimage::{capture_creation_marker, capture_preimage, path_hash};
use crate::resource_limits;
use crate::rollback;
use crate::safeguard::{SafeguardHandler, SafeguardTracker};
use crate::write_interceptor::WriteInterceptor;

/// The current on-disk format version. Compared against the `version` file
/// inside the undo directory on startup.
const CURRENT_VERSION: &str = "1";

/// A single barrier entry stored in a step's `barriers.json` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct BarrierEntry {
    pub(crate) timestamp: DateTime<Utc>,
    pub(crate) affected_paths: Vec<AffectedPath>,
    pub(crate) reason: BarrierReason,
}

/// Read barrier entries from a step directory's `barriers.json`.
/// Returns an empty vec if the file does not exist or is corrupt.
pub(crate) fn read_step_barriers(step_dir: &Path) -> Vec<BarrierEntry> {
    let path = step_dir.join("barriers.json");
    if !path.exists() {
        return Vec::new();
    }
    match fs::read_to_string(&path) {
        Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// Write barrier entries to a step directory's `barriers.json`.
fn write_step_barriers(step_dir: &Path, entries: &[BarrierEntry]) -> Result<()> {
    let path = step_dir.join("barriers.json");
    let json = serde_json::to_string_pretty(entries)?;
    fs::write(&path, json)?;
    Ok(())
}

/// Synthesize a barrier_id from a step ID and entry index within that step.
/// Produces unique values suitable for display (no operational significance).
pub(crate) fn synthesize_barrier_id(step_id: StepId, index: usize) -> u64 {
    step_id as u64 * 1000 + index as u64
}

/// Load barriers from per-step files for the given step IDs, returning
/// `BarrierInfo` objects with synthesized barrier IDs.
fn load_barriers_for_steps(steps_dir: &Path, step_ids: &[StepId]) -> Vec<BarrierInfo> {
    let mut result = Vec::new();
    for &step_id in step_ids {
        let step_dir = steps_dir.join(step_id.to_string());
        let entries = read_step_barriers(&step_dir);
        for (index, entry) in entries.into_iter().enumerate() {
            result.push(BarrierInfo {
                barrier_id: synthesize_barrier_id(step_id, index),
                after_step_id: step_id,
                timestamp: entry.timestamp,
                affected_paths: entry.affected_paths,
                reason: entry.reason,
            });
        }
    }
    result
}

/// Configuration for constructing an `UndoInterceptor`.
///
/// All fields have sensible defaults via `Default`. Use struct update syntax
/// to override only the fields you need:
/// ```ignore
/// UndoInterceptor::new(root, dir, UndoConfig {
///     gitignore: true,
///     ..Default::default()
/// });
/// ```
#[derive(Default)]
pub struct UndoConfig {
    pub policy: ExternalModificationPolicy,
    pub safeguard_config: SafeguardConfig,
    pub safeguard_handler: Option<Box<dyn SafeguardHandler>>,
    pub resource_limits: ResourceLimitsConfig,
    pub symlink_policy: SymlinkPolicy,
    pub gitignore: bool,
}

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
    policy: ExternalModificationPolicy,
    resource_limits: Mutex<ResourceLimitsConfig>,
    safeguard_handler: Option<Box<dyn SafeguardHandler>>,
    symlink_policy: SymlinkPolicy,
    gitignore_filter: Option<Gitignore>,
    /// When true, undo operations are disabled due to a version mismatch.
    undo_disabled: Mutex<bool>,
    /// (expected, found) version strings when a mismatch is detected.
    version_mismatch_info: Mutex<Option<(String, String)>>,
    /// Counter for assigning sequential step IDs at close time, so that
    /// read-only commands (empty steps) don't create gaps in numbering.
    next_step_id: Mutex<StepId>,
    inner: Mutex<UndoInterceptorInner>,
}

struct UndoInterceptorInner {
    /// The currently active (in-progress) step, if any.
    active_step: Option<StepId>,
    /// Completed step IDs in chronological order.
    completed_steps: Vec<StepId>,
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
    /// Create an `UndoInterceptor` with the given configuration.
    pub fn new(working_root: PathBuf, undo_dir: PathBuf, config: UndoConfig) -> Self {
        Self::build(working_root, undo_dir, config)
    }

    /// Create an `UndoInterceptor` with default configuration.
    pub fn new_default(working_root: PathBuf, undo_dir: PathBuf) -> Self {
        Self::build(working_root, undo_dir, UndoConfig::default())
    }

    fn build(working_root: PathBuf, undo_dir: PathBuf, config: UndoConfig) -> Self {
        let UndoConfig {
            policy,
            safeguard_config,
            safeguard_handler,
            resource_limits,
            symlink_policy,
            gitignore: respect_gitignore,
        } = config;
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

        // Reconstruct completed steps from on-disk steps/ directory
        let mut completed_steps: Vec<StepId> = Vec::new();
        let mut max_step_id: StepId = 0;
        if !undo_disabled {
            let steps_dir = undo_dir.join("steps");
            if steps_dir.exists() {
                if let Ok(entries) = fs::read_dir(&steps_dir) {
                    for entry in entries.flatten() {
                        if let Some(name) = entry.file_name().to_str() {
                            if let Ok(id) = name.parse::<StepId>() {
                                completed_steps.push(id);
                            }
                        }
                    }
                }
                completed_steps.sort();
                if let Some(&last) = completed_steps.iter().rfind(|id| **id > 0) {
                    max_step_id = last;
                }
            }
        }

        // Migrate legacy global barriers.json to per-step files
        if !undo_disabled {
            migrate_global_barriers(&undo_dir);
        }

        let gitignore_filter = if respect_gitignore {
            build_gitignore(&working_root)
        } else {
            None
        };

        Self {
            working_root,
            undo_dir,
            policy,
            resource_limits: Mutex::new(resource_limits),
            safeguard_handler,
            symlink_policy,
            gitignore_filter,
            undo_disabled: Mutex::new(undo_disabled),
            version_mismatch_info: Mutex::new(version_mismatch_info),
            next_step_id: Mutex::new(max_step_id + 1),
            inner: Mutex::new(UndoInterceptorInner {
                active_step: None,
                completed_steps,
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

        // Create WAL directory BEFORE marking the step as active in the
        // inner state. If filesystem setup fails, the state remains clean and
        // subsequent open_step calls won't fail with StepAlreadyActive.
        let wal_dir = self.wal_in_progress_dir();
        if wal_dir.exists() {
            fs::remove_dir_all(&wal_dir)?;
        }
        fs::create_dir_all(wal_dir.join("preimages"))?;

        let mut inner = self.inner.lock().unwrap();
        if let Some(active) = inner.active_step {
            return Err(CodeAgentError::StepAlreadyActive { step_id: active });
        }
        inner.active_step = Some(id);
        inner.touched_paths.clear();
        inner.current_manifest = Some(StepManifest::new(id));
        inner.safeguard_tracker.reset();
        inner.current_step_data_size = 0;
        inner.step_unprotected = false;

        Ok(())
    }

    /// Store the command string associated with the current step in the manifest.
    pub fn set_step_command(&self, command: String) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(ref mut manifest) = inner.current_manifest {
            manifest.command = Some(command);
        }
    }

    /// Close the current step, promoting WAL to steps/.
    ///
    /// Steps that touched no files (read-only commands) are silently discarded:
    /// the WAL is cleaned up, no step ID is consumed, and nothing is persisted.
    /// This keeps step IDs contiguous for the user-visible history.
    ///
    /// Non-empty steps are assigned a sequential step ID from an internal
    /// monotonic counter, decoupled from the caller-provided ID. This ensures
    /// unique IDs even after rollback (which removes steps but doesn't reset
    /// the counter).
    ///
    /// Returns the list of step IDs that were evicted due to resource limits.
    pub fn close_step(&self, _id: StepId) -> Result<Vec<StepId>> {
        // Check if the step has any manifest entries (files touched).
        // If empty, discard the step: cancel without adding to completed list,
        // clean up WAL, and don't consume a step ID.
        {
            let mut inner = self.inner.lock().unwrap();
            let is_empty = inner
                .current_manifest
                .as_ref()
                .is_none_or(|m| m.entries.is_empty());
            if is_empty {
                if inner.active_step.is_none() {
                    return Err(CodeAgentError::NoActiveStep);
                }
                inner.active_step = None;
                inner.touched_paths.clear();
                inner.current_manifest = None;
                inner.current_step_data_size = 0;
                inner.step_unprotected = false;
                drop(inner);

                let wal_dir = self.wal_in_progress_dir();
                if wal_dir.exists() {
                    let _ = fs::remove_dir_all(&wal_dir);
                }
                return Ok(vec![]);
            }
        }

        let final_id = {
            let mut counter = self.next_step_id.lock().unwrap();
            let allocated = *counter;
            *counter += 1;
            allocated
        };

        // Update the manifest's step_id to the final ID before writing,
        // then close the active step and record as completed.
        let completed_steps_snapshot = {
            let mut inner = self.inner.lock().unwrap();
            if let Some(ref mut manifest) = inner.current_manifest {
                manifest.step_id = final_id;
                let mut manifest_to_write = manifest.clone();
                if inner.step_unprotected {
                    manifest_to_write.unprotected = true;
                }
                manifest_to_write.write_to(&self.wal_in_progress_dir())?;
            }
            // Close the active step and clear inner state BEFORE filesystem
            // promotion. If fs::rename fails, the step is recorded as completed
            // (so subsequent open_step calls succeed) but missing on disk -- the
            // next session won't find it, which is a harmless loss.
            if inner.active_step.is_none() {
                return Err(CodeAgentError::NoActiveStep);
            }
            inner.active_step = None;
            inner.completed_steps.push(final_id);
            inner.touched_paths.clear();
            inner.current_manifest = None;
            inner.current_step_data_size = 0;
            inner.step_unprotected = false;
            inner.completed_steps.clone()
        };

        // Promote WAL to steps/{final_id}/
        let wal_dir = self.wal_in_progress_dir();
        let steps_parent = self.undo_dir.join("steps");
        let step_dir = self.step_dir(final_id);
        if wal_dir.exists() {
            // Ensure parent directory exists (may have been removed externally).
            if let Err(error) = fs::create_dir_all(&steps_parent) {
                eprintln!(
                    "{{\"level\":\"error\",\"component\":\"undo\",\"message\":\"failed to create steps dir: {error}\"}}",
                );
            }
            if step_dir.exists() {
                if let Err(error) = fs::remove_dir_all(&step_dir) {
                    eprintln!(
                        "{{\"level\":\"error\",\"component\":\"undo\",\"message\":\"failed to remove old step dir {}: {error}\"}}",
                        step_dir.display()
                    );
                }
            }
            if let Err(error) = fs::rename(&wal_dir, &step_dir) {
                eprintln!(
                    "{{\"level\":\"error\",\"component\":\"undo\",\"message\":\"failed to promote WAL to step {final_id}: {error}\"}}",
                );
            }
        }

        // Run eviction after step promotion
        let evicted = self.evict_if_needed(&completed_steps_snapshot)?;

        Ok(evicted)
    }

    /// Rollback the most recent N steps (pop semantics -- removed from history).
    ///
    /// If `force` is false and any undo barriers exist between the current state
    /// and the target, the rollback is rejected with `RollbackBlocked`.
    /// If `force` is true, barriers are crossed and removed.
    /// If any step in the rollback range is unprotected, returns `StepUnprotected`.
    pub fn rollback(&self, count: usize, force: bool) -> Result<RollbackResult> {
        let completed = self.inner.lock().unwrap().completed_steps.clone();
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

        // Check for blocking barriers by reading per-step barrier files
        let steps_dir = self.undo_dir.join("steps");
        let blocking = load_barriers_for_steps(&steps_dir, &steps_to_rollback);

        if !blocking.is_empty() && !force {
            return Err(CodeAgentError::RollbackBlocked {
                count: blocking.len(),
                barriers: blocking,
            });
        }

        // Perform the rollback (inner lock is NOT held during filesystem I/O).
        // fs::remove_dir_all deletes the step dir including any barriers.json.
        for step_id in &steps_to_rollback {
            let step_dir = self.step_dir(*step_id);
            if step_dir.exists() {
                rollback::rollback_step(&step_dir, &self.working_root, self.symlink_policy)?;
                fs::remove_dir_all(&step_dir)?;
            }
        }

        // Batch-remove rolled-back steps from the in-memory list
        {
            let mut inner = self.inner.lock().unwrap();
            inner
                .completed_steps
                .retain(|s| !steps_to_rollback.contains(s));
        }

        Ok(RollbackResult {
            steps_rolled_back: steps_to_rollback.len(),
            barriers_crossed: blocking,
        })
    }

    /// Get the list of completed step IDs.
    pub fn completed_steps(&self) -> Vec<StepId> {
        self.inner.lock().unwrap().completed_steps.clone()
    }

    /// Record an external modification, optionally creating an undo barrier.
    ///
    /// Under `Barrier` policy, creates a barrier and returns it.
    /// Under `Warn` policy, returns `None` (no barrier created).
    pub fn notify_external_modification(
        &self,
        affected_paths: Vec<AffectedPath>,
        reason: BarrierReason,
    ) -> Result<Option<BarrierInfo>> {
        match self.policy {
            ExternalModificationPolicy::Barrier => {
                let completed = self.inner.lock().unwrap().completed_steps.clone();
                // Use step 0 as sentinel when no steps exist yet, so
                // pre-step host modifications are still visible in history.
                let after_step_id = completed.last().copied().unwrap_or(0);

                let step_dir = self.step_dir(after_step_id);
                // Ensure the step directory exists (step 0 is never
                // opened via open_step, so its directory may not exist).
                if !step_dir.exists() {
                    fs::create_dir_all(&step_dir)?;
                }
                let mut entries = read_step_barriers(&step_dir);

                // Merge into the last barrier if it has the same reason.
                // This coalesces watcher ticks between the same VM steps into
                // one barrier instead of creating a separate barrier per tick.
                if let Some(last) = entries.last_mut() {
                    if last.reason == reason {
                        for ap in &affected_paths {
                            if !last.affected_paths.iter().any(|existing| existing.path == ap.path) {
                                last.affected_paths.push(ap.clone());
                            }
                        }
                        last.timestamp = Utc::now();
                        write_step_barriers(&step_dir, &entries)?;

                        let index = entries.len() - 1;
                        return Ok(Some(BarrierInfo {
                            barrier_id: synthesize_barrier_id(after_step_id, index),
                            after_step_id,
                            timestamp: entries[index].timestamp,
                            affected_paths: entries[index].affected_paths.clone(),
                            reason,
                        }));
                    }
                }

                let index = entries.len();
                entries.push(BarrierEntry {
                    timestamp: Utc::now(),
                    affected_paths: affected_paths.clone(),
                    reason,
                });
                write_step_barriers(&step_dir, &entries)?;

                let barrier = BarrierInfo {
                    barrier_id: synthesize_barrier_id(after_step_id, index),
                    after_step_id,
                    timestamp: entries[index].timestamp,
                    affected_paths,
                    reason,
                };
                Ok(Some(barrier))
            }
            ExternalModificationPolicy::Warn => Ok(None),
        }
    }

    /// Return all current undo barriers.
    pub fn barriers(&self) -> Vec<BarrierInfo> {
        let completed = self.inner.lock().unwrap().completed_steps.clone();
        let steps_dir = self.undo_dir.join("steps");
        let mut barriers = Vec::new();
        // Include pre-step barriers (step 0 sentinel) if any exist.
        let step_0_dir = steps_dir.join("0");
        if step_0_dir.exists() {
            for (i, entry) in read_step_barriers(&step_0_dir).into_iter().enumerate() {
                barriers.push(BarrierInfo {
                    barrier_id: synthesize_barrier_id(0, i),
                    after_step_id: 0,
                    timestamp: entry.timestamp,
                    affected_paths: entry.affected_paths,
                    reason: entry.reason,
                });
            }
        }
        barriers.extend(load_barriers_for_steps(&steps_dir, &completed));
        barriers
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

        // Clear in-memory state (step tracking + per-step state in one lock)
        {
            let mut inner = self.inner.lock().unwrap();
            inner.active_step = None;
            inner.completed_steps.clear();
            inner.touched_paths.clear();
            inner.current_manifest = None;
            inner.current_step_data_size = 0;
            inner.step_unprotected = false;
        }

        // Reset step ID counter
        *self.next_step_id.lock().unwrap() = 1;

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
            rollback::rollback_step(&wal_dir, &self.working_root, self.symlink_policy)?;
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
    /// Used when an error occurs mid-step or when a safeguard denies the
    /// current operation -- undoes all operations already applied in this step.
    pub fn rollback_current_step(&self) -> Result<()> {
        let wal_dir = self.wal_in_progress_dir();

        // Write manifest, cancel the active step, and clear inner state.
        // The lock is released before filesystem I/O (rollback + WAL removal).
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.active_step.is_none() {
                return Err(CodeAgentError::NoActiveStep);
            }
            if let Some(ref manifest) = inner.current_manifest {
                let _ = manifest.write_to(&wal_dir);
            }
            inner.active_step = None;
            inner.touched_paths.clear();
            inner.current_manifest = None;
            inner.safeguard_tracker.reset();
            inner.current_step_data_size = 0;
            inner.step_unprotected = false;
        }

        // Best-effort rollback using the WAL data. Even if this fails the step
        // is already cancelled so subsequent operations are not blocked.
        let mut rollback_error = None;
        if wal_dir.exists() {
            if let Err(e) = rollback::rollback_step(&wal_dir, &self.working_root, self.symlink_policy) {
                rollback_error = Some(e);
            }
            let _ = fs::remove_dir_all(&wal_dir);
        }

        match rollback_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
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

    /// Evict oldest completed steps to satisfy resource limits.
    ///
    /// Takes a snapshot of completed steps (caller must not hold inner lock).
    /// Returns the list of evicted step IDs and removes them from the in-memory list.
    fn evict_if_needed(&self, completed_steps: &[StepId]) -> Result<Vec<StepId>> {
        let limits = self.resource_limits.lock().unwrap().clone();
        let steps_dir = self.undo_dir.join("steps");
        let mut evicted: Vec<StepId> = Vec::new();
        let mut remaining = completed_steps.to_vec();

        // Phase 1: Evict by step count
        if let Some(max_count) = limits.max_step_count {
            while remaining.len() > max_count {
                let oldest = remaining[0];
                let step_dir = steps_dir.join(oldest.to_string());
                if step_dir.exists() {
                    fs::remove_dir_all(&step_dir)?;
                }
                remaining.remove(0);
                evicted.push(oldest);
            }
        }

        // Phase 2: Evict by total log size
        if let Some(max_size) = limits.max_log_size_bytes {
            let mut current_size =
                resource_limits::calculate_total_log_size(&steps_dir, &remaining)?;
            while current_size > max_size && !remaining.is_empty() {
                let oldest = remaining[0];
                let step_dir = steps_dir.join(oldest.to_string());
                let step_size = resource_limits::calculate_step_size(&step_dir)?;
                if step_dir.exists() {
                    fs::remove_dir_all(&step_dir)?;
                }
                remaining.remove(0);
                current_size = current_size.saturating_sub(step_size);
                evicted.push(oldest);
            }
        }

        // Remove evicted steps from the in-memory list
        if !evicted.is_empty() {
            let mut inner = self.inner.lock().unwrap();
            inner.completed_steps.retain(|s| !evicted.contains(s));
        }

        Ok(evicted)
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
            if filter.matched_path_or_any_parents(&relative_str, is_dir).is_ignore() {
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
        let symlink_meta = file_path.symlink_metadata();
        if symlink_meta.is_err() {
            return Ok(false);
        }

        // Skip symlinks when policy is Ignore
        if self.symlink_policy == SymlinkPolicy::Ignore
            && symlink_meta.unwrap().is_symlink()
        {
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
        // Skip symlinks when policy is Ignore
        if self.symlink_policy == SymlinkPolicy::Ignore
            && file_path
                .symlink_metadata()
                .map(|m| m.is_symlink())
                .unwrap_or(false)
        {
            return Ok(());
        }

        let relative = file_path.strip_prefix(&self.working_root).map_err(|_| {
            CodeAgentError::Preimage {
                path: file_path.to_path_buf(),
                message: "path outside working root".to_string(),
            }
        })?;
        let relative_str = normalized_relative_path(relative);

        if let Some(ref filter) = self.gitignore_filter {
            let is_dir = file_path.is_dir();
            if filter.matched_path_or_any_parents(&relative_str, is_dir).is_ignore() {
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

            // Skip symlinks when policy is Ignore
            if self.symlink_policy == SymlinkPolicy::Ignore
                && path
                    .symlink_metadata()
                    .map(|m| m.is_symlink())
                    .unwrap_or(false)
            {
                continue;
            }

            // Skip ignored subtrees early to avoid unnecessary I/O
            if let Some(ref filter) = self.gitignore_filter {
                if let Ok(relative) = path.strip_prefix(&self.working_root) {
                    let relative_str = normalized_relative_path(relative);
                    if filter.matched_path_or_any_parents(&relative_str, path.is_dir()).is_ignore() {
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

/// Migrate a legacy global `barriers.json` to per-step barrier files.
///
/// If `{undo_dir}/barriers.json` exists, reads all entries, distributes
/// each to the corresponding `steps/{after_step_id}/barriers.json`, then
/// deletes the global file.
fn migrate_global_barriers(undo_dir: &Path) {
    let global_path = undo_dir.join("barriers.json");
    if !global_path.exists() {
        return;
    }

    let json = match fs::read_to_string(&global_path) {
        Ok(j) => j,
        Err(_) => {
            let _ = fs::remove_file(&global_path);
            return;
        }
    };

    let legacy_barriers: Vec<BarrierInfo> = match serde_json::from_str(&json) {
        Ok(b) => b,
        Err(_) => {
            let _ = fs::remove_file(&global_path);
            return;
        }
    };

    // Group legacy barriers by after_step_id
    let mut by_step: std::collections::HashMap<StepId, Vec<BarrierEntry>> =
        std::collections::HashMap::new();
    for barrier in legacy_barriers {
        by_step.entry(barrier.after_step_id).or_default().push(BarrierEntry {
            timestamp: barrier.timestamp,
            affected_paths: barrier.affected_paths,
            reason: barrier.reason,
        });
    }

    // Write per-step barrier files
    let steps_dir = undo_dir.join("steps");
    for (step_id, entries) in by_step {
        let step_dir = steps_dir.join(step_id.to_string());
        if step_dir.exists() {
            // Merge with any existing per-step barriers (unlikely but safe)
            let mut existing = read_step_barriers(&step_dir);
            existing.extend(entries);
            let _ = write_step_barriers(&step_dir, &existing);
        }
    }

    let _ = fs::remove_file(&global_path);
}

impl StepManager for UndoInterceptor {
    fn open_step(&self, id: StepId) -> Result<()> {
        UndoInterceptor::open_step(self, id)
    }

    fn close_step(&self, id: StepId) -> Result<Vec<StepId>> {
        UndoInterceptor::close_step(self, id)
    }

    fn current_step(&self) -> Option<StepId> {
        self.inner.lock().unwrap().active_step
    }

    fn set_step_command(&self, _id: StepId, command: String) {
        UndoInterceptor::set_step_command(self, command);
    }
}

/// Normalize path separators to forward slashes for consistent comparison.
fn normalized_relative_path(relative: &Path) -> String {
    relative.to_string_lossy().replace('\\', "/")
}

impl WriteInterceptor for UndoInterceptor {
    fn pre_write(&self, path: &Path) -> Result<()> {
        let active = self.inner.lock().unwrap().active_step;
        if let Some(step_id) = active {
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
        let active = self.inner.lock().unwrap().active_step;
        if let Some(step_id) = active {
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
        let active = self.inner.lock().unwrap().active_step;
        if let Some(step_id) = active {
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
        let has_active = self.inner.lock().unwrap().active_step.is_some();
        if has_active {
            self.record_creation(path)?;
        }
        Ok(())
    }

    fn post_mkdir(&self, path: &Path) -> Result<()> {
        let has_active = self.inner.lock().unwrap().active_step.is_some();
        if has_active {
            self.record_creation(path)?;
        }
        Ok(())
    }

    fn pre_setattr(&self, path: &Path) -> Result<()> {
        let has_active = self.inner.lock().unwrap().active_step.is_some();
        if has_active {
            self.ensure_preimage(path)?;
        }
        Ok(())
    }

    fn pre_link(&self, target: &Path, _link_path: &Path) -> Result<()> {
        if self.symlink_policy == SymlinkPolicy::Ignore {
            return Ok(());
        }
        let has_active = self.inner.lock().unwrap().active_step.is_some();
        if has_active {
            self.ensure_preimage(target)?;
        }
        Ok(())
    }

    fn post_symlink(&self, _target: &Path, link_path: &Path) -> Result<()> {
        if self.symlink_policy == SymlinkPolicy::Ignore {
            return Ok(());
        }
        let has_active = self.inner.lock().unwrap().active_step.is_some();
        if has_active {
            self.record_creation(link_path)?;
        }
        Ok(())
    }

    fn pre_xattr(&self, path: &Path) -> Result<()> {
        let has_active = self.inner.lock().unwrap().active_step.is_some();
        if has_active {
            self.ensure_preimage(path)?;
        }
        Ok(())
    }

    fn pre_open_trunc(&self, path: &Path) -> Result<()> {
        let active = self.inner.lock().unwrap().active_step;
        if let Some(step_id) = active {
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
        let has_active = self.inner.lock().unwrap().active_step.is_some();
        if has_active {
            self.ensure_preimage(path)?;
        }
        Ok(())
    }

    fn pre_copy_file_range(&self, dst_path: &Path) -> Result<()> {
        let has_active = self.inner.lock().unwrap().active_step.is_some();
        if has_active {
            self.ensure_preimage(dst_path)?;
        }
        Ok(())
    }

    fn current_step(&self) -> Option<StepId> {
        self.inner.lock().unwrap().active_step
    }
}
