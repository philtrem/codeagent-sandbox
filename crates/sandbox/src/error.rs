use codeagent_common::CodeAgentError;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("no active session")]
    SessionNotActive,

    #[error("session already active")]
    SessionAlreadyActive,

    #[error("invalid working directory: {path}")]
    InvalidWorkingDir { path: String },

    #[error("undo directory overlaps with working directory: undo={undo_dir}, working={working_dir}")]
    UndoDirectoryOverlap { working_dir: String, undo_dir: String },

    #[error("VM not available: QEMU and guest image are not yet built")]
    QemuUnavailable,

    #[error("QEMU spawn failed: {reason}")]
    QemuSpawnFailed { reason: String },

    #[error("control channel connection failed: {reason}")]
    ControlChannelFailed { reason: String },

    #[error("virtiofsd failed: {reason}")]
    VirtioFsFailed { reason: String },

    #[error("not implemented: {feature}")]
    NotImplemented { feature: String },

    #[error(transparent)]
    Undo(#[from] CodeAgentError),

    #[error("file watcher failed: {reason}")]
    FileWatcherFailed { reason: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
