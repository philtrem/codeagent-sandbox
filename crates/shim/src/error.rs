#[derive(Debug, thiserror::Error)]
pub enum ShimError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("control channel closed")]
    ChannelClosed,

    #[error("command {id} not found")]
    CommandNotFound { id: u64 },

    #[error("malformed message: {reason}")]
    MalformedMessage { reason: String },
}
