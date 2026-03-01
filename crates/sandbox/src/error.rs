use codeagent_common::CodeAgentError;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("no active session")]
    SessionNotActive,

    #[error("session already active")]
    SessionAlreadyActive,

    #[error("invalid working directory: {path}")]
    InvalidWorkingDir { path: String },

    #[error("VM not available: QEMU and guest image are not yet built")]
    QemuUnavailable,

    #[error("not implemented: {feature}")]
    NotImplemented { feature: String },

    #[error(transparent)]
    Undo(#[from] CodeAgentError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
