use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Identifies an undo step. Positive IDs are command steps; negative IDs are ambient steps.
pub type StepId = i64;

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
}
