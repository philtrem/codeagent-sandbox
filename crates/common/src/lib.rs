use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Identifies an undo step. Positive IDs are command steps; negative IDs are ambient steps.
pub type StepId = i64;

/// Identifies an undo barrier. Monotonically increasing within a session.
pub type BarrierId = u64;

/// Identifies a safeguard trigger instance. Monotonically increasing within a session.
pub type SafeguardId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepType {
    Command,
    Ambient,
    Api,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepInfo {
    pub id: StepId,
    pub step_type: StepType,
    pub timestamp: DateTime<Utc>,
    pub command: Option<String>,
    pub affected_paths: Vec<PathBuf>,
}

/// Policy for handling external modifications to the working directory.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalModificationPolicy {
    /// Create an undo barrier that blocks rollback (default).
    #[default]
    Barrier,
    /// Emit a warning but do not create a barrier.
    Warn,
}

/// Controls how the undo interceptor handles symlinks.
///
/// Symlinks can point outside the working root, creating security risks:
/// - **Read risk**: preimage capture follows a symlink and stores content from
///   outside the sandbox.
/// - **Write risk**: rollback restores a symlink that points outside the sandbox,
///   then subsequent operations write through it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymlinkPolicy {
    /// Do not capture or restore symlinks. Symlink paths are silently skipped
    /// during preimage capture, creation recording, and rollback restore.
    #[default]
    Ignore,
    /// Capture symlink preimages (read through symlinks) but do not restore
    /// them during rollback (no write through symlinks).
    ReadOnly,
    /// Full symlink support: capture preimages and restore on rollback.
    ReadWrite,
}

/// A marker in the undo history that prevents rollback from crossing it.
///
/// Barriers are created when external modifications are detected in the working
/// directory. They prevent rollback from silently destroying user edits that
/// happened outside the sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BarrierInfo {
    pub barrier_id: BarrierId,
    /// The most recently completed step when this barrier was created.
    /// Rolling back this step would cross the barrier.
    pub after_step_id: StepId,
    pub timestamp: DateTime<Utc>,
    pub affected_paths: Vec<PathBuf>,
}

/// Result of a successful rollback operation.
#[derive(Debug, Clone)]
pub struct RollbackResult {
    /// Number of steps that were rolled back.
    pub steps_rolled_back: usize,
    /// Barriers that were crossed (only non-empty when `force: true` was used).
    pub barriers_crossed: Vec<BarrierInfo>,
}

/// The kind of safeguard that was triggered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SafeguardKind {
    /// The number of delete operations in a step reached the configured threshold.
    DeleteThreshold { count: u64, threshold: u64 },
    /// An existing file larger than the configured size threshold is being overwritten.
    OverwriteLargeFile {
        path: String,
        file_size: u64,
        threshold: u64,
    },
    /// A rename operation would overwrite an existing destination file.
    RenameOverExisting {
        source: String,
        destination: String,
    },
}

/// Configuration for undo log resource limits. Each limit is optional — `None` means
/// no limit is enforced for that dimension.
#[derive(Debug, Clone, Default)]
pub struct ResourceLimitsConfig {
    /// Maximum total size of the undo log in bytes. When exceeded, oldest steps
    /// are evicted (FIFO) until the log fits within budget.
    pub max_log_size_bytes: Option<u64>,
    /// Maximum number of completed steps to retain. When exceeded, oldest steps
    /// are evicted in FIFO order.
    pub max_step_count: Option<usize>,
    /// Maximum cumulative preimage data size for a single step. When exceeded,
    /// the step stops capturing preimages and is marked "unprotected" — it cannot
    /// be rolled back individually, but subsequent steps can still be.
    pub max_single_step_size_bytes: Option<u64>,
}

/// Configuration for safeguard thresholds. Each threshold is optional — `None` means
/// the safeguard is disabled for that kind.
#[derive(Debug, Clone)]
pub struct SafeguardConfig {
    /// Maximum number of delete operations in a single step before triggering.
    pub delete_threshold: Option<u64>,
    /// Trigger when overwriting an existing file larger than this many bytes.
    pub overwrite_file_size_threshold: Option<u64>,
    /// Trigger when a rename would overwrite an existing destination file.
    pub rename_over_existing: bool,
    /// Seconds to wait for a confirmation before auto-denying.
    pub timeout_seconds: u64,
}

impl Default for SafeguardConfig {
    fn default() -> Self {
        Self {
            delete_threshold: None,
            overwrite_file_size_threshold: None,
            rename_over_existing: false,
            timeout_seconds: 30,
        }
    }
}

/// Information about a triggered safeguard, sent to the handler for a decision.
#[derive(Debug, Clone)]
pub struct SafeguardEvent {
    pub safeguard_id: SafeguardId,
    pub step_id: StepId,
    pub kind: SafeguardKind,
    /// Representative paths involved in the trigger.
    pub sample_paths: Vec<String>,
}

/// The user's decision in response to a safeguard trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafeguardDecision {
    Allow,
    Deny,
}

#[derive(Debug, thiserror::Error)]
pub enum CodeAgentError {
    #[error("I/O error: {source}")]
    Io {
        #[from]
        source: std::io::Error,
    },

    #[error("step {step_id} is not active")]
    StepNotActive { step_id: StepId },

    #[error("no active step")]
    NoActiveStep,

    #[error("step {step_id} already active")]
    StepAlreadyActive { step_id: StepId },

    #[error("manifest error: {message}")]
    Manifest { message: String },

    #[error("rollback error: {message}")]
    Rollback { message: String },

    #[error("preimage error for path {path}: {message}")]
    Preimage { path: PathBuf, message: String },

    #[error("serialization error: {source}")]
    Serialization {
        #[from]
        source: serde_json::Error,
    },

    #[error("decompression error: {message}")]
    Decompression { message: String },

    #[error("recovery error: {message}")]
    Recovery { message: String },

    #[error("rollback blocked by {count} undo barrier(s)")]
    RollbackBlocked {
        count: usize,
        barriers: Vec<BarrierInfo>,
    },

    #[error("safeguard denied: step {step_id} rolled back (safeguard {safeguard_id})")]
    SafeguardDenied {
        safeguard_id: SafeguardId,
        step_id: StepId,
    },

    #[error("step {step_id} is unprotected (preimage capture exceeded size limit)")]
    StepUnprotected { step_id: StepId },

    #[error("undo disabled: version mismatch (expected {expected_version}, found {found_version})")]
    UndoDisabled {
        expected_version: String,
        found_version: String,
    },
}

pub type Result<T> = std::result::Result<T, CodeAgentError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_type_serde_round_trip() {
        for variant in [StepType::Command, StepType::Ambient, StepType::Api] {
            let json = serde_json::to_string(&variant).unwrap();
            let deserialized: StepType = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, deserialized);
        }
    }

    #[test]
    fn step_info_serde_round_trip() {
        let info = StepInfo {
            id: 42,
            step_type: StepType::Command,
            timestamp: Utc::now(),
            command: Some("npm install".to_string()),
            affected_paths: vec![PathBuf::from("package-lock.json")],
        };
        let json = serde_json::to_string_pretty(&info).unwrap();
        let deserialized: StepInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info.id, deserialized.id);
        assert_eq!(info.step_type, deserialized.step_type);
        assert_eq!(info.command, deserialized.command);
        assert_eq!(info.affected_paths, deserialized.affected_paths);
    }

    #[test]
    fn error_display_messages() {
        let err = CodeAgentError::StepNotActive { step_id: 5 };
        assert_eq!(err.to_string(), "step 5 is not active");

        let err = CodeAgentError::NoActiveStep;
        assert_eq!(err.to_string(), "no active step");

        let err = CodeAgentError::Preimage {
            path: PathBuf::from("src/main.rs"),
            message: "file not found".to_string(),
        };
        assert!(err.to_string().contains("src/main.rs"));
    }

    #[test]
    fn ambient_step_id_is_negative() {
        let ambient_id: StepId = -1;
        assert!(ambient_id < 0);

        let command_id: StepId = 1;
        assert!(command_id > 0);
    }

    #[test]
    fn barrier_info_serde_round_trip() {
        let info = BarrierInfo {
            barrier_id: 1,
            after_step_id: 42,
            timestamp: Utc::now(),
            affected_paths: vec![PathBuf::from("src/main.rs"), PathBuf::from("Cargo.toml")],
        };
        let json = serde_json::to_string_pretty(&info).unwrap();
        let deserialized: BarrierInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info.barrier_id, deserialized.barrier_id);
        assert_eq!(info.after_step_id, deserialized.after_step_id);
        assert_eq!(info.affected_paths, deserialized.affected_paths);
    }

    #[test]
    fn external_modification_policy_default_is_barrier() {
        assert_eq!(
            ExternalModificationPolicy::default(),
            ExternalModificationPolicy::Barrier
        );
    }

    #[test]
    fn rollback_blocked_error_display() {
        let err = CodeAgentError::RollbackBlocked {
            count: 2,
            barriers: vec![],
        };
        assert!(err.to_string().contains("2 undo barrier(s)"));
    }

    #[test]
    fn safeguard_config_default_all_disabled() {
        let config = SafeguardConfig::default();
        assert_eq!(config.delete_threshold, None);
        assert_eq!(config.overwrite_file_size_threshold, None);
        assert!(!config.rename_over_existing);
        assert_eq!(config.timeout_seconds, 30);
    }

    #[test]
    fn resource_limits_config_default_all_none() {
        let config = ResourceLimitsConfig::default();
        assert_eq!(config.max_log_size_bytes, None);
        assert_eq!(config.max_step_count, None);
        assert_eq!(config.max_single_step_size_bytes, None);
    }

    #[test]
    fn step_unprotected_error_display() {
        let err = CodeAgentError::StepUnprotected { step_id: 5 };
        let msg = err.to_string();
        assert!(msg.contains("unprotected"));
        assert!(msg.contains("5"));
    }

    #[test]
    fn undo_disabled_error_display() {
        let err = CodeAgentError::UndoDisabled {
            expected_version: "1".to_string(),
            found_version: "2".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("version mismatch"));
        assert!(msg.contains("expected 1"));
        assert!(msg.contains("found 2"));
    }

    #[test]
    fn symlink_policy_default_is_ignore() {
        assert_eq!(SymlinkPolicy::default(), SymlinkPolicy::Ignore);
    }

    #[test]
    fn symlink_policy_serde_round_trip() {
        for variant in [
            SymlinkPolicy::Ignore,
            SymlinkPolicy::ReadOnly,
            SymlinkPolicy::ReadWrite,
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            let deserialized: SymlinkPolicy = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, deserialized);
        }
    }

    #[test]
    fn safeguard_denied_error_display() {
        let err = CodeAgentError::SafeguardDenied {
            safeguard_id: 1,
            step_id: 42,
        };
        let msg = err.to_string();
        assert!(msg.contains("safeguard denied"));
        assert!(msg.contains("42"));
    }
}
