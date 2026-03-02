use std::io;

/// Errors specific to the intercepted virtiofs backend.
#[derive(Debug, thiserror::Error)]
pub enum VirtioFsBackendError {
    #[error("IO error: {source}")]
    Io {
        #[from]
        source: io::Error,
    },

    #[error("interceptor error: {source}")]
    Interceptor {
        #[from]
        source: codeagent_common::CodeAgentError,
    },

    #[error("virtiofsd daemon error: {reason}")]
    Daemon { reason: String },
}
