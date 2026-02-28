use crate::error::McpError;
use crate::protocol::JsonRpcRequest;

/// Maximum message size in bytes (1 MB). Messages exceeding this limit are
/// rejected before any JSON parsing to prevent unbounded allocation.
pub const MAX_MESSAGE_SIZE: usize = 1_048_576;

/// Parse a JSON-RPC 2.0 request from a single JSON line.
///
/// Validates the `jsonrpc` version field after deserializing.
pub fn parse_jsonrpc(line: &str) -> Result<JsonRpcRequest, McpError> {
    if line.len() > MAX_MESSAGE_SIZE {
        return Err(McpError::OversizedMessage {
            max_size: MAX_MESSAGE_SIZE,
            actual_size: line.len(),
        });
    }

    let request: JsonRpcRequest =
        serde_json::from_str(line).map_err(|source| McpError::ParseError { source })?;

    if request.jsonrpc != "2.0" {
        return Err(McpError::InvalidRequest {
            message: format!(
                "Expected jsonrpc version \"2.0\", got \"{}\"",
                request.jsonrpc
            ),
        });
    }

    Ok(request)
}

/// Try to extract the `id` field from a raw JSON line for use in error
/// responses when full parsing fails.
pub fn extract_id(line: &str) -> Option<serde_json::Value> {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|v| v.get("id").cloned())
}

/// Extract a missing field name from a serde deserialization error message.
///
/// Serde produces error strings like "missing field `command` at line 1 column 23".
pub fn extract_missing_field(message: &str) -> Option<String> {
    let prefix = "missing field `";
    if let Some(start) = message.find(prefix) {
        let after = &message[start + prefix.len()..];
        if let Some(end) = after.find('`') {
            return Some(after[..end].to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_request_parses() {
        let line = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let request = parse_jsonrpc(line).unwrap();
        assert_eq!(request.jsonrpc, "2.0");
        assert_eq!(request.method, "tools/list");
        assert_eq!(request.id, Some(serde_json::json!(1)));
    }

    #[test]
    fn notification_parses_without_id() {
        let line = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let request = parse_jsonrpc(line).unwrap();
        assert!(request.id.is_none());
        assert_eq!(request.method, "notifications/initialized");
    }

    #[test]
    fn malformed_json_returns_parse_error() {
        let err = parse_jsonrpc("not json {{").unwrap_err();
        assert!(matches!(err, McpError::ParseError { .. }));
    }

    #[test]
    fn wrong_version_returns_invalid_request() {
        let line = r#"{"jsonrpc":"1.0","id":1,"method":"tools/list"}"#;
        let err = parse_jsonrpc(line).unwrap_err();
        assert!(matches!(err, McpError::InvalidRequest { .. }));
    }

    #[test]
    fn oversized_message_rejected_before_parsing() {
        let line = "x".repeat(MAX_MESSAGE_SIZE + 1);
        let err = parse_jsonrpc(&line).unwrap_err();
        assert!(matches!(err, McpError::OversizedMessage { .. }));
    }

    #[test]
    fn extract_id_from_valid_json() {
        let line = r#"{"jsonrpc":"2.0","id":42,"method":"test"}"#;
        assert_eq!(extract_id(line), Some(serde_json::json!(42)));
    }

    #[test]
    fn extract_id_from_malformed_json() {
        assert_eq!(extract_id("not json"), None);
    }

    #[test]
    fn extract_id_returns_string_id() {
        let line = r#"{"jsonrpc":"2.0","id":"req-1","method":"test"}"#;
        assert_eq!(extract_id(line), Some(serde_json::json!("req-1")));
    }

    #[test]
    fn extract_missing_field_from_serde_error() {
        assert_eq!(
            extract_missing_field("missing field `command` at line 1 column 23"),
            Some("command".to_string())
        );
    }

    #[test]
    fn extract_missing_field_returns_none_for_other_errors() {
        assert_eq!(extract_missing_field("some other error"), None);
    }
}
