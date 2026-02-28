use crate::error::ControlChannelError;
use crate::protocol::{HostMessage, VmMessage};

/// Maximum allowed message size in bytes (1 MB).
pub const MAX_MESSAGE_SIZE: usize = 1_048_576;

/// Parse a single JSONL line as a VM→host message.
///
/// Rejects oversized messages before attempting deserialization (CC-04).
/// Returns `UnknownMessageType` for valid JSON with an unrecognized `type` field (CC-03).
pub fn parse_vm_message(line: &str) -> Result<VmMessage, ControlChannelError> {
    if line.len() > MAX_MESSAGE_SIZE {
        return Err(ControlChannelError::OversizedMessage {
            max_size: MAX_MESSAGE_SIZE,
            actual_size: line.len(),
        });
    }

    serde_json::from_str::<VmMessage>(line).map_err(|source| {
        if is_unknown_type_error(line, &source) {
            ControlChannelError::UnknownMessageType {
                line: truncate_for_display(line),
            }
        } else {
            ControlChannelError::MalformedJson { source }
        }
    })
}

/// Parse a single JSONL line as a host→VM message.
///
/// Rejects oversized messages before attempting deserialization.
pub fn parse_host_message(line: &str) -> Result<HostMessage, ControlChannelError> {
    if line.len() > MAX_MESSAGE_SIZE {
        return Err(ControlChannelError::OversizedMessage {
            max_size: MAX_MESSAGE_SIZE,
            actual_size: line.len(),
        });
    }

    serde_json::from_str::<HostMessage>(line).map_err(|source| {
        if is_unknown_type_error(line, &source) {
            ControlChannelError::UnknownMessageType {
                line: truncate_for_display(line),
            }
        } else {
            ControlChannelError::MalformedJson { source }
        }
    })
}

/// Heuristic: if the JSON is syntactically valid but the `type` field doesn't match
/// any known variant, serde produces an error mentioning "type". We distinguish this
/// from truly malformed JSON by checking whether the input is valid JSON at all.
fn is_unknown_type_error(line: &str, _source: &serde_json::Error) -> bool {
    // If the line parses as valid generic JSON with a "type" field, but our
    // tagged enum rejected it, it's an unknown message type.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
        value.get("type").is_some()
    } else {
        false
    }
}

/// Truncate a line for inclusion in error messages, to avoid storing oversized input.
fn truncate_for_display(line: &str) -> String {
    const MAX_DISPLAY_LEN: usize = 200;
    if line.len() <= MAX_DISPLAY_LEN {
        line.to_string()
    } else {
        format!("{}...", &line[..MAX_DISPLAY_LEN])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::OutputStream;

    #[test]
    fn parse_valid_vm_step_started() {
        let line = r#"{"type":"step_started","id":42}"#;
        let msg = parse_vm_message(line).unwrap();
        assert_eq!(msg, VmMessage::StepStarted { id: 42 });
    }

    #[test]
    fn parse_valid_vm_output() {
        let line = r#"{"type":"output","id":1,"stream":"stderr","data":"error!\n"}"#;
        let msg = parse_vm_message(line).unwrap();
        assert_eq!(
            msg,
            VmMessage::Output {
                id: 1,
                stream: OutputStream::Stderr,
                data: "error!\n".to_string(),
            }
        );
    }

    #[test]
    fn parse_valid_vm_step_completed() {
        let line = r#"{"type":"step_completed","id":42,"exit_code":1}"#;
        let msg = parse_vm_message(line).unwrap();
        assert_eq!(msg, VmMessage::StepCompleted { id: 42, exit_code: 1 });
    }

    #[test]
    fn parse_malformed_json_returns_error() {
        let line = "not valid json {{{";
        let err = parse_vm_message(line).unwrap_err();
        assert!(matches!(err, ControlChannelError::MalformedJson { .. }));
    }

    #[test]
    fn parse_unknown_type_returns_error() {
        let line = r#"{"type":"unknown_thing","id":1}"#;
        let err = parse_vm_message(line).unwrap_err();
        assert!(matches!(err, ControlChannelError::UnknownMessageType { .. }));
    }

    #[test]
    fn parse_oversized_message_rejected() {
        let line = "x".repeat(MAX_MESSAGE_SIZE + 1);
        let err = parse_vm_message(&line).unwrap_err();
        assert!(matches!(err, ControlChannelError::OversizedMessage { .. }));
    }

    #[test]
    fn parse_valid_host_exec() {
        let line = r#"{"type":"exec","id":1,"command":"ls -la","cwd":"/tmp"}"#;
        let msg = parse_host_message(line).unwrap();
        assert_eq!(
            msg,
            HostMessage::Exec {
                id: 1,
                command: "ls -la".to_string(),
                env: None,
                cwd: Some("/tmp".to_string()),
            }
        );
    }

    #[test]
    fn parse_valid_host_cancel() {
        let line = r#"{"type":"cancel","id":1}"#;
        let msg = parse_host_message(line).unwrap();
        assert_eq!(msg, HostMessage::Cancel { id: 1 });
    }

    #[test]
    fn truncate_for_display_short_line() {
        let short = "hello";
        assert_eq!(truncate_for_display(short), "hello");
    }

    #[test]
    fn truncate_for_display_long_line() {
        let long = "a".repeat(300);
        let result = truncate_for_display(&long);
        assert!(result.len() < 300);
        assert!(result.ends_with("..."));
    }
}
