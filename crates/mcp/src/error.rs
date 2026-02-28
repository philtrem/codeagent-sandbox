use serde::{Deserialize, Serialize};

// JSON-RPC 2.0 standard error codes.
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const INTERNAL_ERROR: i32 = -32603;

// Application-specific error codes (within -32000..-32099 server error range).
pub const PATH_OUTSIDE_ROOT: i32 = -32001;
#[allow(dead_code)]
pub const ROLLBACK_BLOCKED: i32 = -32002;
#[allow(dead_code)]
pub const SAFEGUARD_DENIED: i32 = -32003;

/// Structured JSON-RPC 2.0 error object sent in error responses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// Errors that can occur during MCP message processing.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("parse error: {source}")]
    ParseError {
        #[source]
        source: serde_json::Error,
    },

    #[error("invalid request: {message}")]
    InvalidRequest { message: String },

    #[error("method not found: {method}")]
    MethodNotFound { method: String },

    #[error("invalid params: {message}")]
    InvalidParams { message: String },

    #[error("invalid params: missing field `{field}`")]
    MissingField { field: String },

    #[error("path outside root: {path}")]
    PathOutsideRoot { path: String },

    #[error("internal error: {message}")]
    InternalError { message: String },

    #[error("message exceeds maximum size of {max_size} bytes (got {actual_size})")]
    OversizedMessage { max_size: usize, actual_size: usize },

    #[error("I/O error: {source}")]
    Io {
        #[from]
        source: std::io::Error,
    },
}

impl McpError {
    /// Convert to a JSON-RPC 2.0 error object for wire transmission.
    pub fn to_jsonrpc_error(&self) -> JsonRpcError {
        match self {
            McpError::ParseError { source } => JsonRpcError {
                code: PARSE_ERROR,
                message: format!("Parse error: {source}"),
                data: None,
            },
            McpError::InvalidRequest { message } => JsonRpcError {
                code: INVALID_REQUEST,
                message: message.clone(),
                data: None,
            },
            McpError::MethodNotFound { method } => JsonRpcError {
                code: METHOD_NOT_FOUND,
                message: format!("Method not found: {method}"),
                data: None,
            },
            McpError::InvalidParams { message } => JsonRpcError {
                code: INVALID_PARAMS,
                message: message.clone(),
                data: None,
            },
            McpError::MissingField { field } => JsonRpcError {
                code: INVALID_PARAMS,
                message: format!("Missing required parameter: {field}"),
                data: Some(serde_json::json!({ "field": field })),
            },
            McpError::PathOutsideRoot { path } => JsonRpcError {
                code: PATH_OUTSIDE_ROOT,
                message: format!("Path outside root: {path}"),
                data: None,
            },
            McpError::InternalError { message } => JsonRpcError {
                code: INTERNAL_ERROR,
                message: message.clone(),
                data: None,
            },
            McpError::OversizedMessage {
                max_size,
                actual_size,
            } => JsonRpcError {
                code: INVALID_REQUEST,
                message: format!(
                    "Message exceeds maximum size of {max_size} bytes (got {actual_size})"
                ),
                data: None,
            },
            McpError::Io { source } => JsonRpcError {
                code: INTERNAL_ERROR,
                message: format!("I/O error: {source}"),
                data: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_error_has_correct_code() {
        let err = McpError::ParseError {
            source: serde_json::from_str::<serde_json::Value>("not json").unwrap_err(),
        };
        assert_eq!(err.to_jsonrpc_error().code, PARSE_ERROR);
    }

    #[test]
    fn method_not_found_has_correct_code() {
        let err = McpError::MethodNotFound {
            method: "unknown".to_string(),
        };
        let rpc_err = err.to_jsonrpc_error();
        assert_eq!(rpc_err.code, METHOD_NOT_FOUND);
        assert!(rpc_err.message.contains("unknown"));
    }

    #[test]
    fn missing_field_includes_field_in_data() {
        let err = McpError::MissingField {
            field: "command".to_string(),
        };
        let rpc_err = err.to_jsonrpc_error();
        assert_eq!(rpc_err.code, INVALID_PARAMS);
        assert_eq!(rpc_err.data.unwrap()["field"], "command");
    }

    #[test]
    fn path_outside_root_has_correct_code() {
        let err = McpError::PathOutsideRoot {
            path: "../../etc/passwd".to_string(),
        };
        assert_eq!(err.to_jsonrpc_error().code, PATH_OUTSIDE_ROOT);
    }
}
