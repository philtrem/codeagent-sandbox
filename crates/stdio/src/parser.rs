use crate::error::StdioError;
use crate::protocol::{
    AgentExecutePayload, AgentPromptPayload, FsListPayload, FsReadPayload, Request,
    RequestEnvelope, SafeguardConfirmPayload, SafeguardConfigurePayload, SessionStartPayload,
    UndoConfigurePayload, UndoHistoryPayload, UndoRollbackPayload,
};

/// Maximum allowed message size in bytes (1 MB).
pub const MAX_MESSAGE_SIZE: usize = 1_048_576;

/// Parse a single JSONL line into a typed `Request`.
///
/// 1. Rejects oversized messages before any JSON parsing.
/// 2. Parses the envelope to extract `type` and `request_id`.
/// 3. Dispatches on `type` to parse the typed payload.
/// 4. Returns structured errors for unknown types, missing fields, etc.
pub fn parse_request(line: &str) -> Result<Request, StdioError> {
    if line.len() > MAX_MESSAGE_SIZE {
        return Err(StdioError::OversizedMessage {
            max_size: MAX_MESSAGE_SIZE,
            actual_size: line.len(),
        });
    }

    let envelope: RequestEnvelope =
        serde_json::from_str(line).map_err(|source| classify_envelope_error(line, source))?;

    parse_typed_request(envelope)
}

/// Attempt to extract a `request_id` from a raw JSON line, even if parsing
/// the full envelope fails. Used to produce useful error responses.
pub fn extract_request_id(line: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|v| v.get("request_id")?.as_str().map(String::from))
}

/// Classify an envelope parsing error as either malformed JSON or missing request_id.
fn classify_envelope_error(line: &str, source: serde_json::Error) -> StdioError {
    // If the JSON is valid but missing request_id, report that specifically.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
        if value.get("request_id").is_none() && value.get("type").is_some() {
            return StdioError::MissingRequestId;
        }
    }
    StdioError::MalformedJson { source }
}

/// Dispatch on the envelope's `type` field to parse the typed payload.
fn parse_typed_request(envelope: RequestEnvelope) -> Result<Request, StdioError> {
    let request_id = envelope.request_id;
    let payload = envelope.payload;

    match envelope.message_type.as_str() {
        "session.start" => {
            let p = parse_payload::<SessionStartPayload>(payload, "session.start")?;
            Ok(Request::SessionStart {
                request_id,
                payload: p,
            })
        }
        "session.stop" => Ok(Request::SessionStop { request_id }),
        "session.reset" => Ok(Request::SessionReset { request_id }),
        "session.status" => Ok(Request::SessionStatus { request_id }),

        "undo.rollback" => {
            let p = parse_payload::<UndoRollbackPayload>(payload, "undo.rollback")?;
            Ok(Request::UndoRollback {
                request_id,
                payload: p,
            })
        }
        "undo.history" => {
            let p = parse_payload_or_default::<UndoHistoryPayload>(payload);
            Ok(Request::UndoHistory {
                request_id,
                payload: p,
            })
        }
        "undo.configure" => {
            let p = parse_payload_or_default::<UndoConfigurePayload>(payload);
            Ok(Request::UndoConfigure {
                request_id,
                payload: p,
            })
        }
        "undo.discard" => Ok(Request::UndoDiscard { request_id }),

        "agent.execute" => {
            let p = parse_payload::<AgentExecutePayload>(payload, "agent.execute")?;
            Ok(Request::AgentExecute {
                request_id,
                payload: p,
            })
        }
        "agent.prompt" => {
            let p = parse_payload::<AgentPromptPayload>(payload, "agent.prompt")?;
            Ok(Request::AgentPrompt {
                request_id,
                payload: p,
            })
        }

        "fs.list" => {
            let p = parse_payload::<FsListPayload>(payload, "fs.list")?;
            Ok(Request::FsList {
                request_id,
                payload: p,
            })
        }
        "fs.read" => {
            let p = parse_payload::<FsReadPayload>(payload, "fs.read")?;
            Ok(Request::FsRead {
                request_id,
                payload: p,
            })
        }
        "fs.status" => Ok(Request::FsStatus { request_id }),

        "safeguard.configure" => {
            let p = parse_payload_or_default::<SafeguardConfigurePayload>(payload);
            Ok(Request::SafeguardConfigure {
                request_id,
                payload: p,
            })
        }
        "safeguard.confirm" => {
            let p = parse_payload::<SafeguardConfirmPayload>(payload, "safeguard.confirm")?;
            Ok(Request::SafeguardConfirm {
                request_id,
                payload: p,
            })
        }

        unknown => Err(StdioError::UnknownOperation {
            operation: unknown.to_string(),
        }),
    }
}

/// Parse a payload from a JSON value, returning a `MissingField` error
/// when serde reports a missing required field.
fn parse_payload<T: serde::de::DeserializeOwned>(
    value: serde_json::Value,
    operation: &str,
) -> Result<T, StdioError> {
    serde_json::from_value::<T>(value).map_err(|source| classify_payload_error(source, operation))
}

/// Parse a payload with a `Default` fallback for null/missing payloads.
fn parse_payload_or_default<T: serde::de::DeserializeOwned + Default>(
    value: serde_json::Value,
) -> T {
    if value.is_null() {
        T::default()
    } else {
        serde_json::from_value::<T>(value).unwrap_or_default()
    }
}

/// Classify a payload parsing error, extracting field names from serde messages.
fn classify_payload_error(source: serde_json::Error, _operation: &str) -> StdioError {
    let message = source.to_string();
    if let Some(field) = extract_missing_field(&message) {
        return StdioError::MissingField { field };
    }
    StdioError::InvalidField {
        field: "payload".to_string(),
        message,
    }
}

/// Extract the field name from a serde "missing field `xyz`" error message.
fn extract_missing_field(message: &str) -> Option<String> {
    // serde_json errors for missing fields say: "missing field `fieldname`"
    let prefix = "missing field `";
    let start = message.find(prefix)? + prefix.len();
    let end = message[start..].find('`')? + start;
    Some(message[start..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_session_start() {
        let line = r#"{"type":"session.start","request_id":"1","payload":{"working_directories":[{"path":"/tmp/project"}]}}"#;
        let request = parse_request(line).unwrap();
        match request {
            Request::SessionStart {
                request_id,
                payload,
            } => {
                assert_eq!(request_id, "1");
                assert_eq!(payload.working_directories.len(), 1);
                assert_eq!(payload.working_directories[0].path, "/tmp/project");
            }
            other => panic!("Expected SessionStart, got: {other:?}"),
        }
    }

    #[test]
    fn parse_agent_execute() {
        let line = r#"{"type":"agent.execute","request_id":"9","payload":{"command":"npm install","cwd":"/mnt"}}"#;
        let request = parse_request(line).unwrap();
        match request {
            Request::AgentExecute {
                request_id,
                payload,
            } => {
                assert_eq!(request_id, "9");
                assert_eq!(payload.command, "npm install");
                assert_eq!(payload.cwd, Some("/mnt".to_string()));
            }
            other => panic!("Expected AgentExecute, got: {other:?}"),
        }
    }

    #[test]
    fn unknown_type_error() {
        let line = r#"{"type":"foo.bar","request_id":"1","payload":{}}"#;
        let err = parse_request(line).unwrap_err();
        assert!(matches!(err, StdioError::UnknownOperation { .. }));
    }

    #[test]
    fn missing_required_field_command() {
        let line = r#"{"type":"agent.execute","request_id":"1","payload":{}}"#;
        let err = parse_request(line).unwrap_err();
        match &err {
            StdioError::MissingField { field } => assert_eq!(field, "command"),
            other => panic!("Expected MissingField, got: {other:?}"),
        }
    }

    #[test]
    fn missing_request_id() {
        let line = r#"{"type":"session.status"}"#;
        let err = parse_request(line).unwrap_err();
        assert!(matches!(err, StdioError::MissingRequestId));
    }

    #[test]
    fn oversized_message_rejected() {
        let line = "x".repeat(MAX_MESSAGE_SIZE + 1);
        let err = parse_request(&line).unwrap_err();
        assert!(matches!(err, StdioError::OversizedMessage { .. }));
    }

    #[test]
    fn malformed_json() {
        let line = "not valid json {{{";
        let err = parse_request(line).unwrap_err();
        assert!(matches!(err, StdioError::MalformedJson { .. }));
    }

    #[test]
    fn extract_request_id_from_valid_json() {
        let line = r#"{"type":"session.status","request_id":"abc"}"#;
        assert_eq!(extract_request_id(line), Some("abc".to_string()));
    }

    #[test]
    fn extract_request_id_from_invalid_json() {
        let line = "not json";
        assert_eq!(extract_request_id(line), None);
    }

    #[test]
    fn null_payload_uses_default() {
        let line = r#"{"type":"undo.history","request_id":"1"}"#;
        let request = parse_request(line).unwrap();
        assert!(matches!(request, Request::UndoHistory { .. }));
    }

    #[test]
    fn extract_missing_field_from_serde_message() {
        let msg = r#"missing field `command` at line 1 column 2"#;
        assert_eq!(extract_missing_field(msg), Some("command".to_string()));
    }

    #[test]
    fn extract_missing_field_no_match() {
        let msg = "unexpected token";
        assert_eq!(extract_missing_field(msg), None);
    }
}
