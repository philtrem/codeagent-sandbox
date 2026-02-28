use std::collections::HashMap;

use codeagent_common::{BarrierId, StepId};
use serde::{Deserialize, Serialize};

use crate::error::ErrorDetail;

// ---------------------------------------------------------------------------
// Inbound: requests from frontend
// ---------------------------------------------------------------------------

/// Raw envelope for all inbound STDIO API messages.
///
/// The two-step parsing approach first deserializes this envelope to extract
/// the `type` and `request_id`, then dispatches on `type` to parse the typed
/// payload. This allows producing useful error responses that include the
/// `request_id` even when the payload is malformed.
#[derive(Debug, Clone, Deserialize)]
pub struct RequestEnvelope {
    #[serde(rename = "type")]
    pub message_type: String,
    pub request_id: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// Parsed request with typed payload.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    SessionStart {
        request_id: String,
        payload: SessionStartPayload,
    },
    SessionStop {
        request_id: String,
    },
    SessionReset {
        request_id: String,
    },
    SessionStatus {
        request_id: String,
    },
    UndoRollback {
        request_id: String,
        payload: UndoRollbackPayload,
    },
    UndoHistory {
        request_id: String,
        payload: UndoHistoryPayload,
    },
    UndoConfigure {
        request_id: String,
        payload: UndoConfigurePayload,
    },
    UndoDiscard {
        request_id: String,
    },
    AgentExecute {
        request_id: String,
        payload: AgentExecutePayload,
    },
    AgentPrompt {
        request_id: String,
        payload: AgentPromptPayload,
    },
    FsList {
        request_id: String,
        payload: FsListPayload,
    },
    FsRead {
        request_id: String,
        payload: FsReadPayload,
    },
    FsStatus {
        request_id: String,
    },
    SafeguardConfigure {
        request_id: String,
        payload: SafeguardConfigurePayload,
    },
    SafeguardConfirm {
        request_id: String,
        payload: SafeguardConfirmPayload,
    },
}

impl Request {
    /// Returns the `request_id` for this request.
    pub fn request_id(&self) -> &str {
        match self {
            Request::SessionStart { request_id, .. }
            | Request::SessionStop { request_id }
            | Request::SessionReset { request_id }
            | Request::SessionStatus { request_id }
            | Request::UndoRollback { request_id, .. }
            | Request::UndoHistory { request_id, .. }
            | Request::UndoConfigure { request_id, .. }
            | Request::UndoDiscard { request_id }
            | Request::AgentExecute { request_id, .. }
            | Request::AgentPrompt { request_id, .. }
            | Request::FsList { request_id, .. }
            | Request::FsRead { request_id, .. }
            | Request::FsStatus { request_id }
            | Request::SafeguardConfigure { request_id, .. }
            | Request::SafeguardConfirm { request_id, .. } => request_id,
        }
    }
}

// ---------------------------------------------------------------------------
// Request payloads
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkingDirectoryConfig {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionStartPayload {
    pub working_directories: Vec<WorkingDirectoryConfig>,
    #[serde(default = "default_network_policy")]
    pub network_policy: String,
    #[serde(default = "default_vm_mode")]
    pub vm_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<u32>,
}

fn default_network_policy() -> String {
    "disabled".to_string()
}

fn default_vm_mode() -> String {
    "ephemeral".to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UndoRollbackPayload {
    pub count: u32,
    #[serde(default)]
    pub force: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct UndoHistoryPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct UndoConfigurePayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_log_size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_step_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_single_step_size_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentExecutePayload {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentPromptPayload {
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FsListPayload {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FsReadPayload {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SafeguardConfigurePayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete_threshold: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overwrite_file_size_threshold: Option<u64>,
    #[serde(default)]
    pub rename_over_existing: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SafeguardConfirmPayload {
    pub safeguard_id: String,
    pub action: String,
}

// ---------------------------------------------------------------------------
// Outbound: responses and events from agent
// ---------------------------------------------------------------------------

/// Response envelope written to stdout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    #[serde(rename = "type")]
    pub message_type: String,
    pub request_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorDetail>,
}

impl ResponseEnvelope {
    /// Create a success response with an optional payload.
    pub fn ok(request_id: String, payload: Option<serde_json::Value>) -> Self {
        Self {
            message_type: "response".to_string(),
            request_id,
            status: "ok".to_string(),
            payload,
            error: None,
        }
    }

    /// Create an error response with a structured error detail.
    pub fn error(request_id: String, error: ErrorDetail) -> Self {
        Self {
            message_type: "response".to_string(),
            request_id,
            status: "error".to_string(),
            payload: None,
            error: Some(error),
        }
    }
}

/// Event envelope written to stdout (unsolicited).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    #[serde(rename = "type")]
    pub event_type: String,
    pub payload: serde_json::Value,
}

/// Typed event variants for internal construction.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    StepCompleted {
        step_id: StepId,
        affected_paths: Vec<String>,
        exit_code: i32,
    },
    AgentOutput {
        data: String,
    },
    TerminalOutput {
        stream: String,
        data: String,
    },
    Warning {
        code: String,
        message: String,
    },
    Error {
        code: String,
        message: String,
    },
    SafeguardTriggered {
        step_id: StepId,
        safeguard_id: String,
        kind: String,
        sample_paths: Vec<String>,
        message: String,
    },
    ExternalModification {
        affected_paths: Vec<String>,
        barrier_id: Option<BarrierId>,
    },
    Recovery {
        paths_restored: usize,
        paths_deleted: usize,
    },
    UndoVersionMismatch {
        expected_version: String,
        found_version: String,
    },
}

impl Event {
    /// Convert this typed event into a serializable envelope.
    pub fn to_envelope(&self) -> EventEnvelope {
        match self {
            Event::StepCompleted {
                step_id,
                affected_paths,
                exit_code,
            } => EventEnvelope {
                event_type: "event.step_completed".to_string(),
                payload: serde_json::json!({
                    "step_id": step_id,
                    "affected_paths": affected_paths,
                    "exit_code": exit_code,
                }),
            },
            Event::AgentOutput { data } => EventEnvelope {
                event_type: "event.agent_output".to_string(),
                payload: serde_json::json!({ "data": data }),
            },
            Event::TerminalOutput { stream, data } => EventEnvelope {
                event_type: "event.terminal_output".to_string(),
                payload: serde_json::json!({ "stream": stream, "data": data }),
            },
            Event::Warning { code, message } => EventEnvelope {
                event_type: "event.warning".to_string(),
                payload: serde_json::json!({ "code": code, "message": message }),
            },
            Event::Error { code, message } => EventEnvelope {
                event_type: "event.error".to_string(),
                payload: serde_json::json!({ "code": code, "message": message }),
            },
            Event::SafeguardTriggered {
                step_id,
                safeguard_id,
                kind,
                sample_paths,
                message,
            } => EventEnvelope {
                event_type: "event.safeguard_triggered".to_string(),
                payload: serde_json::json!({
                    "step_id": step_id,
                    "safeguard_id": safeguard_id,
                    "kind": kind,
                    "sample_paths": sample_paths,
                    "message": message,
                }),
            },
            Event::ExternalModification {
                affected_paths,
                barrier_id,
            } => EventEnvelope {
                event_type: "event.external_modification".to_string(),
                payload: serde_json::json!({
                    "affected_paths": affected_paths,
                    "barrier_id": barrier_id,
                }),
            },
            Event::Recovery {
                paths_restored,
                paths_deleted,
            } => EventEnvelope {
                event_type: "event.recovery".to_string(),
                payload: serde_json::json!({
                    "paths_restored": paths_restored,
                    "paths_deleted": paths_deleted,
                }),
            },
            Event::UndoVersionMismatch {
                expected_version,
                found_version,
            } => EventEnvelope {
                event_type: "event.undo_version_mismatch".to_string(),
                payload: serde_json::json!({
                    "expected_version": expected_version,
                    "found_version": found_version,
                }),
            },
        }
    }
}

/// Structured log entry written to stderr.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub component: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<StepId>,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_ok_serialization() {
        let response = ResponseEnvelope::ok("42".to_string(), Some(serde_json::json!({"state": "running"})));
        let json = serde_json::to_string(&response).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "response");
        assert_eq!(parsed["request_id"], "42");
        assert_eq!(parsed["status"], "ok");
        assert_eq!(parsed["payload"]["state"], "running");
        assert!(parsed.get("error").is_none());
    }

    #[test]
    fn response_error_serialization() {
        let error = ErrorDetail {
            code: "unknown_operation".to_string(),
            message: "unknown operation type: foo.bar".to_string(),
            field: None,
        };
        let response = ResponseEnvelope::error("1".to_string(), error);
        let json = serde_json::to_string(&response).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["status"], "error");
        assert_eq!(parsed["error"]["code"], "unknown_operation");
        assert!(parsed.get("payload").is_none());
    }

    #[test]
    fn event_step_completed_envelope() {
        let event = Event::StepCompleted {
            step_id: 7,
            affected_paths: vec!["package-lock.json".to_string()],
            exit_code: 0,
        };
        let envelope = event.to_envelope();
        assert_eq!(envelope.event_type, "event.step_completed");
        assert_eq!(envelope.payload["step_id"], 7);
        assert_eq!(envelope.payload["exit_code"], 0);
    }

    #[test]
    fn event_terminal_output_envelope() {
        let event = Event::TerminalOutput {
            stream: "stdout".to_string(),
            data: "hello world\n".to_string(),
        };
        let envelope = event.to_envelope();
        assert_eq!(envelope.event_type, "event.terminal_output");
        assert_eq!(envelope.payload["stream"], "stdout");
    }

    #[test]
    fn event_warning_envelope() {
        let event = Event::Warning {
            code: "undo_eviction".to_string(),
            message: "oldest step evicted".to_string(),
        };
        let envelope = event.to_envelope();
        assert_eq!(envelope.event_type, "event.warning");
        assert_eq!(envelope.payload["code"], "undo_eviction");
    }

    #[test]
    fn event_envelope_serialization_round_trip() {
        let event = Event::Recovery {
            paths_restored: 5,
            paths_deleted: 2,
        };
        let envelope = event.to_envelope();
        let json = serde_json::to_string(&envelope).unwrap();
        let parsed: EventEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.event_type, "event.recovery");
        assert_eq!(parsed.payload["paths_restored"], 5);
        assert_eq!(parsed.payload["paths_deleted"], 2);
    }

    #[test]
    fn log_entry_serialization() {
        let entry = LogEntry {
            timestamp: "2025-03-01T12:00:01.234567Z".to_string(),
            level: "info".to_string(),
            component: "stdio_api".to_string(),
            request_id: Some("1".to_string()),
            step_id: None,
            message: "session.start received".to_string(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["timestamp"], "2025-03-01T12:00:01.234567Z");
        assert_eq!(parsed["level"], "info");
        assert_eq!(parsed["component"], "stdio_api");
        assert_eq!(parsed["request_id"], "1");
        assert!(parsed.get("step_id").is_none());
    }

    #[test]
    fn request_envelope_deserialization() {
        let json = r#"{"type":"session.status","request_id":"abc"}"#;
        let envelope: RequestEnvelope = serde_json::from_str(json).unwrap();
        assert_eq!(envelope.message_type, "session.status");
        assert_eq!(envelope.request_id, "abc");
        assert!(envelope.payload.is_null());
    }

    #[test]
    fn request_envelope_with_payload() {
        let json = r#"{"type":"agent.execute","request_id":"1","payload":{"command":"ls"}}"#;
        let envelope: RequestEnvelope = serde_json::from_str(json).unwrap();
        assert_eq!(envelope.message_type, "agent.execute");
        assert_eq!(envelope.payload["command"], "ls");
    }

    #[test]
    fn working_directory_config_round_trip() {
        let config = WorkingDirectoryConfig {
            path: "/tmp/project".to_string(),
            label: Some("main".to_string()),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: WorkingDirectoryConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, parsed);
    }

    #[test]
    fn session_start_payload_defaults() {
        let json = r#"{"working_directories":[{"path":"/tmp"}]}"#;
        let payload: SessionStartPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.network_policy, "disabled");
        assert_eq!(payload.vm_mode, "ephemeral");
        assert_eq!(payload.protocol_version, None);
    }
}
