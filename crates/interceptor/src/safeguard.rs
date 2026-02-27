use std::collections::HashSet;

use codeagent_common::{
    SafeguardConfig, SafeguardDecision, SafeguardEvent, SafeguardId, SafeguardKind, StepId,
};

/// Handler called when a safeguard threshold is crossed.
///
/// The implementation blocks until a decision is available. In production this
/// bridges to the STDIO API; in tests an immediate-response handler is used.
pub trait SafeguardHandler: Send + Sync {
    fn on_safeguard_triggered(&self, event: SafeguardEvent) -> SafeguardDecision;
}

/// Tracks per-step safeguard counters and checks thresholds.
pub struct SafeguardTracker {
    config: SafeguardConfig,
    /// Monotonically increasing ID for safeguard events.
    next_safeguard_id: SafeguardId,
    /// Delete operations counted in the current step.
    delete_count: u64,
    /// Paths deleted so far in the current step (for sample_paths in events).
    deleted_paths: Vec<String>,
    /// Safeguard kinds that have already been allowed for the current step,
    /// keyed by a discriminant string. Prevents re-triggering after Allow.
    allowed_kinds: HashSet<String>,
}

impl SafeguardTracker {
    pub fn new(config: SafeguardConfig) -> Self {
        Self {
            config,
            next_safeguard_id: 1,
            delete_count: 0,
            deleted_paths: Vec::new(),
            allowed_kinds: HashSet::new(),
        }
    }

    /// Reset per-step counters. Called when a new step is opened.
    pub fn reset(&mut self) {
        self.delete_count = 0;
        self.deleted_paths.clear();
        self.allowed_kinds.clear();
    }

    /// Record a delete operation and check the threshold.
    /// Returns `Some(event)` if the threshold was just reached.
    pub fn check_delete(&mut self, path: &str, step_id: StepId) -> Option<SafeguardEvent> {
        self.delete_count += 1;
        self.deleted_paths.push(path.to_string());

        let threshold = self.config.delete_threshold?;

        if self.delete_count < threshold {
            return None;
        }

        if self.allowed_kinds.contains("delete_threshold") {
            return None;
        }

        let event = SafeguardEvent {
            safeguard_id: self.next_id(),
            step_id,
            kind: SafeguardKind::DeleteThreshold {
                count: self.delete_count,
                threshold,
            },
            sample_paths: self.deleted_paths.clone(),
        };
        Some(event)
    }

    /// Check whether overwriting a file of the given size triggers the safeguard.
    pub fn check_overwrite(
        &mut self,
        path: &str,
        file_size: u64,
        step_id: StepId,
    ) -> Option<SafeguardEvent> {
        let threshold = self.config.overwrite_file_size_threshold?;

        if file_size < threshold {
            return None;
        }

        if self.allowed_kinds.contains("overwrite_large_file") {
            return None;
        }

        let event = SafeguardEvent {
            safeguard_id: self.next_id(),
            step_id,
            kind: SafeguardKind::OverwriteLargeFile {
                path: path.to_string(),
                file_size,
                threshold,
            },
            sample_paths: vec![path.to_string()],
        };
        Some(event)
    }

    /// Check whether a rename-over-existing triggers the safeguard.
    pub fn check_rename_over(
        &mut self,
        source: &str,
        destination: &str,
        step_id: StepId,
    ) -> Option<SafeguardEvent> {
        if !self.config.rename_over_existing {
            return None;
        }

        if self.allowed_kinds.contains("rename_over_existing") {
            return None;
        }

        let event = SafeguardEvent {
            safeguard_id: self.next_id(),
            step_id,
            kind: SafeguardKind::RenameOverExisting {
                source: source.to_string(),
                destination: destination.to_string(),
            },
            sample_paths: vec![source.to_string(), destination.to_string()],
        };
        Some(event)
    }

    /// Mark a safeguard kind as allowed for the current step (prevents re-triggering).
    pub fn mark_allowed(&mut self, kind: &SafeguardKind) {
        let key = match kind {
            SafeguardKind::DeleteThreshold { .. } => "delete_threshold",
            SafeguardKind::OverwriteLargeFile { .. } => "overwrite_large_file",
            SafeguardKind::RenameOverExisting { .. } => "rename_over_existing",
        };
        self.allowed_kinds.insert(key.to_string());
    }

    fn next_id(&mut self) -> SafeguardId {
        let id = self.next_safeguard_id;
        self.next_safeguard_id += 1;
        id
    }
}
