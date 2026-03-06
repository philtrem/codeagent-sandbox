use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64};

use codeagent_common::SafeguardConfig;
use codeagent_interceptor::undo_interceptor::UndoInterceptor;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::recent_writes::RecentBackendWrites;

use codeagent_common::SafeguardDecision;
use codeagent_control::{ControlChannelHandler, InFlightTracker};

use crate::fs_backend::FilesystemBackend;
use crate::qemu::QemuProcess;
use crate::step_adapter::StepManagerAdapter;

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

    // --- VM-related fields (all None/empty in non-VM mode) ---

    /// Handle to the running QEMU VM process.
    pub qemu_process: Option<QemuProcess>,

    /// Filesystem backends (one per working dir).
    pub fs_backends: Vec<Box<dyn FilesystemBackend>>,

    /// In-flight tracker shared between filesystem backend and control handler.
    pub in_flight_tracker: Option<InFlightTracker>,

    /// Sender for enqueuing host messages to the control channel writer task.
    pub control_writer: Option<mpsc::UnboundedSender<String>>,

    /// Control channel handler for registering outgoing commands.
    pub control_handler: Option<Arc<ControlChannelHandler<StepManagerAdapter>>>,

    /// Background task for the event bridge (control events → STDIO events).
    pub event_bridge_handle: Option<JoinHandle<()>>,

    /// Background task for reading VM messages from the control channel.
    pub control_reader_handle: Option<JoinHandle<()>>,

    /// Background task for writing host messages to the control channel.
    pub control_writer_handle: Option<JoinHandle<()>>,

    /// Path to the temporary socket directory (cleaned up on stop).
    pub socket_dir: Option<PathBuf>,

    /// Atomic counter for generating command IDs for `agent.execute`.
    pub next_command_id: Arc<AtomicU64>,

    /// Monotonic counter for API step IDs (write_file/edit_file synthetic steps).
    pub next_api_step_id: Arc<AtomicI64>,

    // --- Filesystem watcher fields ---

    /// Background task running the filesystem watcher.
    pub fs_watcher_handle: Option<JoinHandle<()>>,

    /// Shared tracker for paths recently written by this sandbox's backends.
    pub recent_writes: Option<Arc<RecentBackendWrites>>,

    // --- Safeguard bridge fields ---

    /// Background task consuming safeguard events from interceptors.
    pub safeguard_bridge_handle: Option<JoinHandle<()>>,
}
