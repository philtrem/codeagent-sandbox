use serde::{Deserialize, Serialize};

/// Structured error detail included in error responses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

/// Errors that can occur during STDIO API message processing.
#[derive(Debug, thiserror::Error)]
pub enum StdioError {
    #[error("malformed JSON: {source}")]
    MalformedJson {
        #[source]
        source: serde_json::Error,
    },

    #[error("unknown operation type: {operation}")]
    UnknownOperation { operation: String },

    #[error("missing required field: {field}")]
    MissingField { field: String },

    #[error("invalid field value for {field}: {message}")]
    InvalidField { field: String, message: String },

    #[error("message exceeds maximum size of {max_size} bytes (got {actual_size})")]
    OversizedMessage { max_size: usize, actual_size: usize },

    #[error("unsupported protocol version {version} (supported: {min}..={max})")]
    UnsupportedProtocolVersion { version: u32, min: u32, max: u32 },

    #[error("path outside root: {path}")]
    PathOutsideRoot { path: String },

    #[error("missing request_id")]
    MissingRequestId,

    #[error("I/O error: {source}")]
    Io {
        #[from]
        source: std::io::Error,
    },
}

impl StdioError {
    /// Convert this error to a structured `ErrorDetail` for inclusion in responses.
    pub fn to_error_detail(&self) -> ErrorDetail {
        match self {
            StdioError::MalformedJson { source } => ErrorDetail {
                code: "malformed_json".to_string(),
                message: source.to_string(),
                field: None,
            },
            StdioError::UnknownOperation { operation } => ErrorDetail {
                code: "unknown_operation".to_string(),
                message: format!("unknown operation type: {operation}"),
                field: None,
            },
            StdioError::MissingField { field } => ErrorDetail {
                code: "missing_field".to_string(),
                message: format!("missing required field: {field}"),
                field: Some(field.clone()),
            },
            StdioError::InvalidField { field, message } => ErrorDetail {
                code: "invalid_field".to_string(),
                message: message.clone(),
                field: Some(field.clone()),
            },
            StdioError::OversizedMessage {
                max_size,
                actual_size,
            } => ErrorDetail {
                code: "oversized_message".to_string(),
                message: format!(
                    "message exceeds maximum size of {max_size} bytes (got {actual_size})"
                ),
                field: None,
            },
            StdioError::UnsupportedProtocolVersion { version, min, max } => ErrorDetail {
                code: "unsupported_protocol_version".to_string(),
                message: format!(
                    "unsupported protocol version {version} (supported: {min}..={max})"
                ),
                field: None,
            },
            StdioError::PathOutsideRoot { path } => ErrorDetail {
                code: "path_outside_root".to_string(),
                message: format!("path is outside working directory root: {path}"),
                field: None,
            },
            StdioError::MissingRequestId => ErrorDetail {
                code: "missing_request_id".to_string(),
                message: "missing required field: request_id".to_string(),
                field: Some("request_id".to_string()),
            },
            StdioError::Io { source } => ErrorDetail {
                code: "io_error".to_string(),
                message: source.to_string(),
                field: None,
            },
        }
    }
}
