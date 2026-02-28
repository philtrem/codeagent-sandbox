/// Errors that can occur during control channel message processing.
#[derive(Debug, thiserror::Error)]
pub enum ControlChannelError {
    #[error("malformed JSON: {source}")]
    MalformedJson {
        #[source]
        source: serde_json::Error,
    },

    #[error("unknown message type: {line}")]
    UnknownMessageType { line: String },

    #[error(
        "message exceeds maximum size of {max_size} bytes (got {actual_size})"
    )]
    OversizedMessage { max_size: usize, actual_size: usize },

    #[error("step_completed for unknown command {id}: no matching step_started")]
    UnexpectedStepCompleted { id: u64 },

    #[error("duplicate step_started for command {id}")]
    DuplicateStepStarted { id: u64 },

    #[error("output for unknown command {id}")]
    OutputForUnknownCommand { id: u64 },

    #[error("step_started for unknown command {id}: no matching exec")]
    UnexpectedStepStarted { id: u64 },

    #[error("cancel for unknown command {id}")]
    CancelUnknownCommand { id: u64 },
}
