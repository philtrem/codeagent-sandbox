use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use codeagent_common::SafeguardConfig;
use codeagent_interceptor::undo_interceptor::UndoInterceptor;
use tokio::sync::oneshot;

use codeagent_common::SafeguardDecision;

/// Lifecycle state of the sandbox session.
pub enum SessionState {
    /// No session active. Only `session.start` is valid.
    Idle,
    /// Session is active with all resources initialized.
    Active(Box<Session>),
}

/// An active session with all per-session resources.
pub struct Session {
    /// Per-working-directory undo interceptors (indexed by directory position).
    pub interceptors: Vec<Arc<UndoInterceptor>>,

    /// Absolute paths of shared working directories.
    pub working_dirs: Vec<PathBuf>,

    /// Absolute paths of per-directory undo log directories.
    pub undo_dirs: Vec<PathBuf>,

    /// VM lifecycle mode for this session.
    pub vm_mode: String,

    /// Current safeguard configuration.
    pub safeguard_config: SafeguardConfig,

    /// Pending safeguard confirmations: safeguard_id → oneshot sender.
    pub pending_safeguards: HashMap<String, oneshot::Sender<SafeguardDecision>>,

    /// The last `SessionStartPayload` used, stored for `session.reset`.
    pub last_start_payload: Option<codeagent_stdio::protocol::SessionStartPayload>,
}
