use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::JsonRpcError;

/// Raw JSON-RPC 2.0 request envelope (first-pass deserialization target).
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<serde_json::Value>,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// JSON-RPC 2.0 response envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    /// Create a success response.
    pub fn success(id: Option<serde_json::Value>, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Create an error response.
    pub fn error(id: Option<serde_json::Value>, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// JSON-RPC 2.0 notification (no id, no response expected).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

// --- MCP-specific types ---

/// Tool definition returned by `tools/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

/// A single content item in a tool call result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: String,
}

/// Result of a `tools/call` invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub content: Vec<ToolContent>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[serde(rename = "isError")]
    pub is_error: bool,
}

impl ToolCallResult {
    /// Create a successful text result.
    pub fn text(text: String) -> Self {
        Self {
            content: vec![ToolContent {
                content_type: "text".to_string(),
                text,
            }],
            is_error: false,
        }
    }

    /// Create an error text result.
    pub fn error_text(text: String) -> Self {
        Self {
            content: vec![ToolContent {
                content_type: "text".to_string(),
                text,
            }],
            is_error: true,
        }
    }
}

/// Parameters for `tools/call`.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolCallParams {
    pub name: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
}

// --- Tool argument structs ---

/// Arguments for the `execute_command` tool.
#[derive(Debug, Clone, Deserialize)]
pub struct ExecuteCommandArgs {
    pub command: String,
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    #[serde(default)]
    pub cwd: Option<String>,
}

/// Arguments for the `read_file` tool.
#[derive(Debug, Clone, Deserialize)]
pub struct ReadFileArgs {
    pub path: String,
}

/// Arguments for the `write_file` tool.
#[derive(Debug, Clone, Deserialize)]
pub struct WriteFileArgs {
    pub path: String,
    pub content: String,
}

/// Arguments for the `list_directory` tool.
#[derive(Debug, Clone, Deserialize)]
pub struct ListDirectoryArgs {
    pub path: String,
}

/// Arguments for the `undo` tool.
#[derive(Debug, Clone, Deserialize)]
pub struct UndoArgs {
    #[serde(default = "default_undo_count")]
    pub count: u32,
    #[serde(default)]
    pub force: bool,
}

fn default_undo_count() -> u32 {
    1
}

/// Arguments for the `get_undo_history` tool (no required fields).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GetUndoHistoryArgs {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_response_structure() {
        let resp = JsonRpcResponse::success(Some(serde_json::json!(1)), serde_json::json!({"ok": true}));
        assert_eq!(resp.jsonrpc, "2.0");
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
        assert_eq!(resp.id, Some(serde_json::json!(1)));
    }

    #[test]
    fn error_response_structure() {
        let err = JsonRpcError {
            code: -32600,
            message: "Invalid Request".to_string(),
            data: None,
        };
        let resp = JsonRpcResponse::error(Some(serde_json::json!(2)), err);
        assert_eq!(resp.jsonrpc, "2.0");
        assert!(resp.result.is_none());
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32600);
    }

    #[test]
    fn tool_call_result_text() {
        let result = ToolCallResult::text("hello".to_string());
        assert_eq!(result.content.len(), 1);
        assert_eq!(result.content[0].content_type, "text");
        assert_eq!(result.content[0].text, "hello");
        assert!(!result.is_error);
    }

    #[test]
    fn tool_call_result_error_text() {
        let result = ToolCallResult::error_text("failed".to_string());
        assert!(result.is_error);
    }

    #[test]
    fn tool_call_result_serializes_correctly() {
        let result = ToolCallResult::text("hello".to_string());
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["content"][0]["type"], "text");
        assert_eq!(json["content"][0]["text"], "hello");
        // is_error should be omitted when false
        assert!(json.get("isError").is_none());
    }

    #[test]
    fn undo_args_default_count() {
        let args: UndoArgs = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(args.count, 1);
        assert!(!args.force);
    }

    #[test]
    fn execute_command_args_required_field() {
        let result = serde_json::from_str::<ExecuteCommandArgs>(r#"{"command": "ls -la"}"#);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().command, "ls -la");
    }

    #[test]
    fn notification_serializes_without_params() {
        let notif = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: "notifications/initialized".to_string(),
            params: None,
        };
        let json = serde_json::to_value(&notif).unwrap();
        assert!(json.get("params").is_none());
    }
}
