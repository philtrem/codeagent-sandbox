use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::json;
use tokio::sync::mpsc;

use codeagent_common::{BarrierReason, SafeguardConfig, SafeguardDecision};
use codeagent_control::InFlightTracker;
use codeagent_interceptor::undo_interceptor::UndoInterceptor;
use codeagent_interceptor::write_interceptor::WriteInterceptor;
use codeagent_mcp::McpError;
use codeagent_stdio::protocol::{
    AgentExecutePayload, AgentPromptPayload, FsListPayload, FsReadPayload,
    SafeguardConfirmPayload, SafeguardConfigurePayload, SessionStartPayload,
    UndoConfigurePayload, UndoHistoryPayload, UndoRollbackPayload,
};
use codeagent_stdio::{Event, RequestHandler, StdioError};

use crate::cli::CliArgs;
use crate::command_classifier::{self, CommandClassifier, CommandClassifierConfig, SanitizeResult};
use crate::command_waiter::CommandWaiter;
use crate::config::FileWatcherConfig;
use crate::control_bridge;
use crate::error::AgentError;
use crate::fs_watcher;
use crate::qemu::{QemuConfig, QemuProcess};
use crate::recent_writes::RecentBackendWrites;
use crate::safeguard_bridge::PendingSafeguard;
use crate::session::{Session, SessionState};

/// Compute a stable subdirectory name for a working directory's undo data.
///
/// Uses the first 16 hex characters of a blake3 hash of the canonicalized,
/// forward-slash-normalized path. This ensures the same working directory
/// always maps to the same undo subdirectory regardless of ordering.
pub fn undo_subdir_name(working_dir: &Path) -> String {
    let canonical = std::fs::canonicalize(working_dir)
        .unwrap_or_else(|_| working_dir.to_path_buf());
    let normalized = canonical.to_string_lossy().replace('\\', "/");
    let hash = blake3::hash(normalized.as_bytes());
    hash.to_hex()[..16].to_string()
}

/// Check that two paths do not contain each other.
/// Both paths must exist (so canonicalization works).
fn check_paths_overlap(working_dir: &std::path::Path, undo_dir: &std::path::Path) -> Result<(), AgentError> {
    let canonical_working = match std::fs::canonicalize(working_dir) {
        Ok(p) => p,
        Err(_) => return Ok(()), // non-existent paths can't overlap
    };
    let canonical_undo = match std::fs::canonicalize(undo_dir) {
        Ok(p) => p,
        Err(_) => return Ok(()),
    };

    if canonical_undo.starts_with(&canonical_working) || canonical_working.starts_with(&canonical_undo) {
        return Err(AgentError::UndoDirectoryOverlap {
            working_dir: working_dir.display().to_string(),
            undo_dir: undo_dir.display().to_string(),
        });
    }

    Ok(())
}

/// Central orchestrator that implements both `RequestHandler` (STDIO API)
/// and `McpHandler` (MCP server) by delegating to shared session state.
pub struct Orchestrator {
    state: Arc<Mutex<SessionState>>,
    cli_args: CliArgs,
    event_sender: mpsc::UnboundedSender<Event>,
    /// Populated when the filesystem backend connects and safeguards are enabled.
    #[allow(dead_code)]
    safeguard_receiver: Mutex<Option<mpsc::UnboundedReceiver<PendingSafeguard>>>,
    /// Shared with the event bridge so MCP `Bash` tool can block
    /// until a VM command completes and collect its output.
    command_waiter: Arc<CommandWaiter>,
    /// Pre-computed command classifier from config.
    classifier: CommandClassifier,
    /// Filesystem watcher configuration from TOML config.
    file_watcher_config: FileWatcherConfig,
}

impl Orchestrator {
    pub fn new(
        cli_args: CliArgs,
        event_sender: mpsc::UnboundedSender<Event>,
        classifier_config: CommandClassifierConfig,
        file_watcher_config: FileWatcherConfig,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(SessionState::Idle)),
            cli_args,
            event_sender,
            safeguard_receiver: Mutex::new(None),
            command_waiter: CommandWaiter::new(),
            classifier: CommandClassifier::new(classifier_config),
            file_watcher_config,
        }
    }

    /// Resolve guest image paths: CLI args first, then auto-detect next to the binary.
    fn resolve_guest_images(&self) -> (Option<PathBuf>, Option<PathBuf>) {
        let mut kernel = self.cli_args.kernel_path.clone();
        let mut initrd = self.cli_args.initrd_path.clone();

        if kernel.is_some() && initrd.is_some() {
            return (kernel, initrd);
        }

        // Auto-detect: look for guest/ directory next to the sandbox binary
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                if kernel.is_none() {
                    let candidate = exe_dir.join("guest").join("vmlinuz");
                    if candidate.is_file() {
                        kernel = Some(candidate);
                    }
                }
                if initrd.is_none() {
                    let candidate = exe_dir.join("guest").join("initrd.img");
                    if candidate.is_file() {
                        initrd = Some(candidate);
                    }
                }
            }
        }

        (kernel, initrd)
    }

    /// Create a session from a `session.start` payload.
    fn do_session_start(
        &self,
        payload: SessionStartPayload,
    ) -> Result<serde_json::Value, AgentError> {
        let mut state = self.state.lock().unwrap();
        if matches!(*state, SessionState::Active(_)) {
            return Err(AgentError::SessionAlreadyActive);
        }

        let working_dirs: Vec<PathBuf> = if payload.working_directories.is_empty() {
            self.cli_args.working_dirs.clone()
        } else {
            payload
                .working_directories
                .iter()
                .map(|d| PathBuf::from(&d.path))
                .collect()
        };

        // Validate all working directories exist
        for dir in &working_dirs {
            if !dir.exists() {
                return Err(AgentError::InvalidWorkingDir {
                    path: dir.display().to_string(),
                });
            }
        }

        // Validate undo directory does not overlap with any working directory
        for dir in &working_dirs {
            check_paths_overlap(dir, &self.cli_args.undo_dir)?;
        }

        let mut interceptors = Vec::with_capacity(working_dirs.len());
        let mut undo_dirs = Vec::with_capacity(working_dirs.len());

        for working_dir in &working_dirs {
            let undo_dir = self.cli_args.undo_dir.join(undo_subdir_name(working_dir));
            let interceptor = UndoInterceptor::new(working_dir.clone(), undo_dir.clone());

            // Run crash recovery
            if let Ok(Some(recovery)) = interceptor.recover() {
                let _ = self.event_sender.send(Event::Recovery {
                    paths_restored: recovery.paths_restored,
                    paths_deleted: recovery.paths_deleted,
                });
            }

            // Check for version mismatch
            if let Some((expected, found)) = interceptor.version_mismatch() {
                let _ = self.event_sender.send(Event::UndoVersionMismatch {
                    expected_version: expected,
                    found_version: found,
                });
            }

            // Place a barrier if previous session steps exist, preventing accidental
            // rollback across session boundaries where untracked changes may have occurred.
            if !interceptor.is_undo_disabled() && !interceptor.completed_steps().is_empty() {
                if let Ok(Some(barrier)) = interceptor.notify_external_modification(
                    vec![working_dir.clone()],
                    BarrierReason::SessionStart,
                )
                {
                    let _ = self.event_sender.send(Event::ExternalModification {
                        affected_paths: vec![working_dir.to_string_lossy().into_owned()],
                        barrier_id: Some(barrier.barrier_id),
                    });
                }
            }

            interceptors.push(Arc::new(interceptor));
            undo_dirs.push(undo_dir);
        }

        // Compute the next command ID from the highest existing step ID across all
        // interceptors. This prevents step ID collisions when a session is restarted
        // — without this, IDs would reset to 1 and overwrite previous session steps.
        let max_existing_step_id = interceptors
            .iter()
            .flat_map(|i| i.completed_steps())
            .filter(|id| *id > 0)
            .max()
            .unwrap_or(0) as u64;
        let initial_command_id = max_existing_step_id + 1;

        // Create RecentBackendWrites tracker and spawn filesystem watcher.
        let recent_writes_ttl =
            std::time::Duration::from_millis(self.file_watcher_config.recent_write_ttl_ms);
        let recent_writes = Arc::new(RecentBackendWrites::new(recent_writes_ttl));

        let watcher_config = {
            let mut config = fs_watcher::FsWatcherConfig {
                debounce: std::time::Duration::from_millis(self.file_watcher_config.debounce_ms),
                exclude_patterns: fs_watcher::FsWatcherConfig::default().exclude_patterns,
                enabled: self.file_watcher_config.enabled,
            };
            config
                .exclude_patterns
                .extend(self.file_watcher_config.exclude_patterns.clone());
            config
        };

        let fs_watcher_handle = fs_watcher::spawn_fs_watcher(
            working_dirs.clone(),
            undo_dirs.clone(),
            recent_writes.clone(),
            self.event_sender.clone(),
            watcher_config,
        );

        // Determine VM availability and launch if configured
        let (resolved_kernel, resolved_initrd) = self.resolve_guest_images();
        let vm_available = resolved_kernel.is_some() && resolved_initrd.is_some();
        let (vm_status, backend_name) = if vm_available {
            match self.launch_vm(
                &working_dirs,
                &interceptors,
                &recent_writes,
                resolved_kernel.unwrap(),
                resolved_initrd.unwrap(),
            ) {
                Ok(vm_session_parts) => {
                    let session = Session {
                        interceptors,
                        working_dirs: working_dirs.clone(),
                        undo_dirs,
                        vm_mode: payload.vm_mode.clone(),
                        safeguard_config: SafeguardConfig::default(),
                        pending_safeguards: Default::default(),
                        last_start_payload: Some(payload),
                        qemu_process: vm_session_parts.qemu_process,
                        fs_backends: vm_session_parts.fs_backends,
                        in_flight_tracker: vm_session_parts.in_flight_tracker,
                        control_writer: vm_session_parts.control_writer,
                        control_handler: vm_session_parts.control_handler,
                        event_bridge_handle: vm_session_parts.event_bridge_handle,
                        control_reader_handle: vm_session_parts.control_reader_handle,
                        control_writer_handle: vm_session_parts.control_writer_handle,
                        socket_dir: vm_session_parts.socket_dir,
                        next_command_id: Arc::new(AtomicU64::new(initial_command_id)),
                        fs_watcher_handle,
                        recent_writes: Some(recent_writes),
                    };

                    *state = SessionState::Active(Box::new(session));

                    let backend = if cfg!(target_os = "windows") { "9p" } else { "virtiofsd" };
                    ("running", backend)
                }
                Err(error) => {
                    // VM launch failed — fall back to non-VM mode and report
                    let _ = self.event_sender.send(Event::Warning {
                        code: "vm_launch_failed".to_string(),
                        message: format!("VM launch failed, falling back to host-only mode: {error}"),
                    });
                    let session = Self::create_non_vm_session(
                        interceptors, working_dirs.clone(), undo_dirs, payload,
                        fs_watcher_handle, Some(recent_writes), initial_command_id,
                    );
                    *state = SessionState::Active(Box::new(session));
                    ("unavailable", "none")
                }
            }
        } else {
            // No VM components configured — run in host-only mode
            let missing: Vec<&str> = [
                self.cli_args.kernel_path.is_none().then_some("kernel"),
                self.cli_args.initrd_path.is_none().then_some("initrd"),
            ]
            .into_iter()
            .flatten()
            .collect();
            let _ = self.event_sender.send(Event::Warning {
                code: "vm_not_configured".to_string(),
                message: format!(
                    "VM not configured (missing: {}), running in host-only mode. \
                     Pass --kernel-path and --initrd-path to enable VM mode.",
                    missing.join(", ")
                ),
            });
            let session = Self::create_non_vm_session(
                interceptors, working_dirs.clone(), undo_dirs, payload,
                fs_watcher_handle, Some(recent_writes), initial_command_id,
            );
            *state = SessionState::Active(Box::new(session));
            ("unavailable", "none")
        };

        Ok(json!({
            "status": "ok",
            "vm_status": vm_status,
            "backend": backend_name,
            "mount_points": working_dirs.iter().enumerate().map(|(i, d)| {
                json!({
                    "index": i,
                    "path": d.display().to_string(),
                    "mount_path": format!("/mnt/working/{i}"),
                })
            }).collect::<Vec<_>>(),
        }))
    }

    /// Create a session without VM components.
    fn create_non_vm_session(
        interceptors: Vec<Arc<UndoInterceptor>>,
        working_dirs: Vec<PathBuf>,
        undo_dirs: Vec<PathBuf>,
        payload: SessionStartPayload,
        fs_watcher_handle: Option<tokio::task::JoinHandle<()>>,
        recent_writes: Option<Arc<RecentBackendWrites>>,
        initial_command_id: u64,
    ) -> Session {
        Session {
            interceptors,
            working_dirs,
            undo_dirs,
            vm_mode: payload.vm_mode.clone(),
            safeguard_config: SafeguardConfig::default(),
            pending_safeguards: Default::default(),
            last_start_payload: Some(payload),
            qemu_process: None,
            fs_backends: vec![],
            in_flight_tracker: None,
            control_writer: None,
            control_handler: None,
            event_bridge_handle: None,
            control_reader_handle: None,
            control_writer_handle: None,
            socket_dir: None,
            next_command_id: Arc::new(AtomicU64::new(initial_command_id)),
            fs_watcher_handle,
            recent_writes,
        }
    }

    /// Launch VM components: filesystem backends, QEMU, control channel.
    fn launch_vm(
        &self,
        working_dirs: &[PathBuf],
        interceptors: &[Arc<UndoInterceptor>],
        recent_writes: &Arc<RecentBackendWrites>,
        kernel_path: PathBuf,
        initrd_path: PathBuf,
    ) -> Result<VmSessionParts, AgentError> {
        let socket_dir = self.cli_args.undo_dir.join(".sockets");
        std::fs::create_dir_all(&socket_dir)?;

        let control_socket_path = socket_dir.join("control.sock");

        // Create InFlightTracker before backends so they can share it with
        // the control channel handler for quiescence detection.
        let in_flight_tracker = InFlightTracker::new();

        // 1. Start filesystem backends
        let mut fs_backends: Vec<Box<dyn crate::fs_backend::FilesystemBackend>> = Vec::new();
        let mut fs_socket_paths = Vec::new();

        #[cfg(unix)]
        {
            use crate::fs_backend::{FilesystemBackend, InterceptedBackend};
            use crate::recent_writes::WriteTrackingInterceptor;
            for (index, working_dir) in working_dirs.iter().enumerate() {
                let fs_socket = socket_dir.join(format!("vfs{index}.sock"));
                let tracking_interceptor: Arc<dyn codeagent_interceptor::write_interceptor::WriteInterceptor> =
                    Arc::new(WriteTrackingInterceptor::new(
                        interceptors[index].clone(),
                        recent_writes.clone(),
                    ));
                let mut backend = InterceptedBackend::new(
                    working_dir.clone(),
                    fs_socket.clone(),
                    tracking_interceptor,
                    in_flight_tracker.clone(),
                );
                backend.start()?;
                fs_socket_paths.push(fs_socket);
                fs_backends.push(Box::new(backend));
            }
        }

        #[cfg(target_os = "windows")]
        {
            use crate::fs_backend::{FilesystemBackend, P9Backend};
            use crate::recent_writes::WriteTrackingInterceptor;
            for (index, working_dir) in working_dirs.iter().enumerate() {
                let fs_socket = socket_dir.join(format!("p9fs{index}.addr"));
                let tracking_interceptor: Arc<dyn codeagent_interceptor::write_interceptor::WriteInterceptor> =
                    Arc::new(WriteTrackingInterceptor::new(
                        interceptors[index].clone(),
                        recent_writes.clone(),
                    ));
                let mut backend = P9Backend::new(
                    working_dir.clone(),
                    fs_socket.clone(),
                    tracking_interceptor,
                    in_flight_tracker.clone(),
                );
                backend.start()?;
                fs_socket_paths.push(fs_socket);
                fs_backends.push(Box::new(backend));
            }
        }

        // 2. On Windows, bind a TCP listener for the control channel before
        //    QEMU starts. QEMU will connect to this address as a client.
        //    The address file must exist before build_args() reads it.
        #[cfg(target_os = "windows")]
        let control_listener = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0")
                .map_err(|error| AgentError::ControlChannelFailed {
                    reason: format!("failed to bind control channel listener: {error}"),
                })?;
            let addr = listener.local_addr()
                .map_err(|error| AgentError::ControlChannelFailed {
                    reason: format!("failed to get control listener address: {error}"),
                })?;
            std::fs::write(&control_socket_path, addr.to_string())?;
            listener
        };

        // 3. Build QEMU config and spawn
        let config = QemuConfig {
            qemu_binary: self.cli_args.qemu_binary.clone(),
            kernel_path,
            initrd_path,
            rootfs_path: self.cli_args.rootfs_path.clone(),
            memory_mb: self.cli_args.memory_mb,
            cpus: self.cli_args.cpus,
            working_dirs: working_dirs.to_vec(),
            control_socket_path: control_socket_path.clone(),
            fs_socket_paths,
            vm_mode: self.cli_args.vm_mode.clone(),
            extra_args: vec![],
        };

        let qemu_process = QemuProcess::spawn(config)?;

        // 4. Connect to control channel (platform-specific transport)
        //
        // Unix: connect to the Unix domain socket that QEMU created (server mode).
        // Windows: accept the TCP connection from QEMU (client mode).
        // Both produce a (reader, writer) pair implementing AsyncRead/AsyncWrite.

        #[cfg(unix)]
        let (reader, writer) = {
            let std_stream = std::os::unix::net::UnixStream::connect(&control_socket_path)
                .map_err(|error| AgentError::ControlChannelFailed {
                    reason: format!("failed to connect to control socket: {error}"),
                })?;
            std_stream
                .set_nonblocking(true)
                .map_err(|error| AgentError::ControlChannelFailed {
                    reason: format!("failed to set socket non-blocking: {error}"),
                })?;
            let tokio_stream = tokio::net::UnixStream::from_std(std_stream)
                .map_err(|error| AgentError::ControlChannelFailed {
                    reason: format!("failed to convert socket: {error}"),
                })?;
            tokio_stream.into_split()
        };

        #[cfg(target_os = "windows")]
        let (reader, writer) = {
            // Accept one connection from QEMU with a polling timeout.
            // QEMU connects to our TCP listener as a client (server=off).
            control_listener
                .set_nonblocking(true)
                .map_err(|error| AgentError::ControlChannelFailed {
                    reason: format!("failed to set listener non-blocking: {error}"),
                })?;

            let start = std::time::Instant::now();
            let timeout = std::time::Duration::from_secs(30);
            let poll_interval = std::time::Duration::from_millis(100);

            let stream = loop {
                match control_listener.accept() {
                    Ok((stream, _addr)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if start.elapsed() > timeout {
                            return Err(AgentError::ControlChannelFailed {
                                reason: "QEMU did not connect to the control channel within 30s"
                                    .to_string(),
                            });
                        }
                        std::thread::sleep(poll_interval);
                    }
                    Err(error) => {
                        return Err(AgentError::ControlChannelFailed {
                            reason: format!(
                                "failed to accept control channel connection: {error}"
                            ),
                        });
                    }
                }
            };

            stream
                .set_nonblocking(true)
                .map_err(|error| AgentError::ControlChannelFailed {
                    reason: format!("failed to set stream non-blocking: {error}"),
                })?;
            let tokio_stream = tokio::net::TcpStream::from_std(stream)
                .map_err(|error| AgentError::ControlChannelFailed {
                    reason: format!("failed to convert TCP stream: {error}"),
                })?;
            tokio_stream.into_split()
        };

        // 5. Create control channel handler
        use codeagent_control::{ControlChannelHandler, QuiescenceConfig};
        use crate::event_bridge::run_event_bridge;
        use crate::step_adapter::StepManagerAdapter;

        let step_manager = Arc::new(StepManagerAdapter::new(interceptors[0].clone()));
        let quiescence_config = QuiescenceConfig::default();

        let (handler, handler_events) = ControlChannelHandler::new(
            step_manager,
            in_flight_tracker.clone(),
            quiescence_config,
        );
        let handler = Arc::new(handler);

        // 6. Spawn event bridge (control events → STDIO events + command waiter)
        let event_bridge_handle = tokio::spawn(run_event_bridge(
            handler_events,
            self.event_sender.clone(),
            Some(self.command_waiter.clone()),
        ));

        // 7. Spawn control channel writer and reader tasks
        let (control_writer_sender, control_writer_handle) =
            control_bridge::spawn_control_writer(writer);

        let control_reader_handle = control_bridge::spawn_control_reader(
            reader,
            handler.clone(),
            self.event_sender.clone(),
        );

        Ok(VmSessionParts {
            qemu_process: Some(qemu_process),
            fs_backends,
            in_flight_tracker: Some(in_flight_tracker),
            control_writer: Some(control_writer_sender),
            control_handler: Some(handler),
            event_bridge_handle: Some(event_bridge_handle),
            control_reader_handle: Some(control_reader_handle),
            control_writer_handle: Some(control_writer_handle),
            socket_dir: Some(socket_dir),
        })
    }

    fn do_session_stop(&self) -> Result<serde_json::Value, AgentError> {
        let mut state = self.state.lock().unwrap();
        match &mut *state {
            SessionState::Idle => Err(AgentError::SessionNotActive),
            SessionState::Active(session) => {
                // Stop filesystem watcher
                if let Some(handle) = session.fs_watcher_handle.take() {
                    handle.abort();
                }

                // Stop background tasks
                if let Some(handle) = session.control_reader_handle.take() {
                    handle.abort();
                }
                if let Some(handle) = session.control_writer_handle.take() {
                    handle.abort();
                }
                if let Some(handle) = session.event_bridge_handle.take() {
                    handle.abort();
                }

                // Drop the control writer sender so the writer task exits
                session.control_writer.take();

                // Stop QEMU
                if let Some(mut qemu) = session.qemu_process.take() {
                    let _ = qemu.stop();
                }

                // Stop filesystem backends
                for backend in &mut session.fs_backends {
                    let _ = backend.stop();
                }

                // Clean up socket directory
                if let Some(socket_dir) = &session.socket_dir {
                    let _ = std::fs::remove_dir_all(socket_dir);
                }

                *state = SessionState::Idle;
                Ok(json!({}))
            }
        }
    }

    fn do_session_reset(&self) -> Result<serde_json::Value, AgentError> {
        let payload = {
            let state = self.state.lock().unwrap();
            match &*state {
                SessionState::Idle => return Err(AgentError::SessionNotActive),
                SessionState::Active(session) => session.last_start_payload.clone(),
            }
        };

        self.do_session_stop()?;

        match payload {
            Some(p) => self.do_session_start(p),
            None => Err(AgentError::SessionNotActive),
        }
    }

    fn do_session_status(&self) -> Result<serde_json::Value, AgentError> {
        let state = self.state.lock().unwrap();
        match &*state {
            SessionState::Idle => Ok(json!({
                "state": "idle",
            })),
            SessionState::Active(session) => {
                let vm_status = if session.qemu_process.is_some() {
                    "running"
                } else {
                    "unavailable"
                };

                Ok(json!({
                    "state": "active",
                    "vm_mode": session.vm_mode,
                    "vm_status": vm_status,
                    "working_directories": session.working_dirs.iter().enumerate().map(|(i, d)| {
                        json!({
                            "index": i,
                            "path": d.display().to_string(),
                        })
                    }).collect::<Vec<_>>(),
                    "undo_steps": session.interceptors.iter().map(|interceptor| {
                        interceptor.completed_steps().len()
                    }).collect::<Vec<_>>(),
                }))
            }
        }
    }

    /// Get the primary (index 0) interceptor, or the one matching the
    /// optional directory selector.
    fn resolve_interceptor(
        &self,
        directory: Option<&str>,
    ) -> Result<Arc<UndoInterceptor>, AgentError> {
        let state = self.state.lock().unwrap();
        let session = match &*state {
            SessionState::Idle => return Err(AgentError::SessionNotActive),
            SessionState::Active(s) => s,
        };

        let index = match directory {
            None => 0,
            Some(s) => {
                if let Ok(i) = s.parse::<usize>() {
                    i
                } else {
                    // Try matching by label
                    session
                        .working_dirs
                        .iter()
                        .position(|d| {
                            d.file_name()
                                .and_then(|n| n.to_str())
                                .is_some_and(|n| n == s)
                        })
                        .unwrap_or(0)
                }
            }
        };

        session
            .interceptors
            .get(index)
            .cloned()
            .ok_or(AgentError::InvalidWorkingDir {
                path: format!("directory index {index} out of range"),
            })
    }

    /// Get the primary working directory path.
    fn primary_working_dir(&self) -> Result<PathBuf, AgentError> {
        let state = self.state.lock().unwrap();
        match &*state {
            SessionState::Idle => Err(AgentError::SessionNotActive),
            SessionState::Active(s) => {
                Ok(s.working_dirs.first().cloned().unwrap_or_default())
            }
        }
    }

    fn require_active(&self) -> Result<(), AgentError> {
        let state = self.state.lock().unwrap();
        match &*state {
            SessionState::Idle => Err(AgentError::SessionNotActive),
            SessionState::Active(_) => Ok(()),
        }
    }

    fn agent_error_to_stdio(err: AgentError) -> StdioError {
        StdioError::InvalidField {
            field: "session".to_string(),
            message: err.to_string(),
        }
    }

    fn agent_error_to_mcp(err: AgentError) -> McpError {
        McpError::InternalError {
            message: err.to_string(),
        }
    }

    /// Get the recent writes tracker from the active session, if available.
    fn recent_writes(&self) -> Option<Arc<RecentBackendWrites>> {
        let state = self.state.lock().unwrap();
        match &*state {
            SessionState::Active(session) => session.recent_writes.clone(),
            _ => None,
        }
    }

    fn next_api_step_id(&self) -> Result<i64, McpError> {
        let state = self.state.lock().unwrap();
        match &*state {
            SessionState::Active(session) => {
                Ok((session.interceptors[0].completed_steps().len() as i64) + 1_000_000)
            }
            _ => Err(Self::agent_error_to_mcp(AgentError::SessionNotActive)),
        }
    }
}

/// Parts of a session that come from VM launch.
struct VmSessionParts {
    qemu_process: Option<QemuProcess>,
    fs_backends: Vec<Box<dyn crate::fs_backend::FilesystemBackend>>,
    in_flight_tracker: Option<InFlightTracker>,
    control_writer: Option<mpsc::UnboundedSender<String>>,
    control_handler: Option<Arc<codeagent_control::ControlChannelHandler<crate::step_adapter::StepManagerAdapter>>>,
    event_bridge_handle: Option<tokio::task::JoinHandle<()>>,
    control_reader_handle: Option<tokio::task::JoinHandle<()>>,
    control_writer_handle: Option<tokio::task::JoinHandle<()>>,
    socket_dir: Option<PathBuf>,
}

impl RequestHandler for Orchestrator {
    fn session_start(
        &self,
        payload: SessionStartPayload,
    ) -> Result<serde_json::Value, StdioError> {
        self.do_session_start(payload)
            .map_err(Self::agent_error_to_stdio)
    }

    fn session_stop(&self) -> Result<serde_json::Value, StdioError> {
        self.do_session_stop()
            .map_err(Self::agent_error_to_stdio)
    }

    fn session_reset(&self) -> Result<serde_json::Value, StdioError> {
        self.do_session_reset()
            .map_err(Self::agent_error_to_stdio)
    }

    fn session_status(&self) -> Result<serde_json::Value, StdioError> {
        self.do_session_status()
            .map_err(Self::agent_error_to_stdio)
    }

    fn undo_rollback(
        &self,
        payload: UndoRollbackPayload,
    ) -> Result<serde_json::Value, StdioError> {
        let interceptor = self
            .resolve_interceptor(payload.directory.as_deref())
            .map_err(Self::agent_error_to_stdio)?;

        let result = interceptor
            .rollback(payload.count as usize, payload.force)
            .map_err(AgentError::from)
            .map_err(Self::agent_error_to_stdio)?;

        Ok(json!({
            "steps_rolled_back": result.steps_rolled_back,
            "barriers_crossed": result.barriers_crossed.len(),
        }))
    }

    fn undo_history(
        &self,
        payload: UndoHistoryPayload,
    ) -> Result<serde_json::Value, StdioError> {
        let interceptor = self
            .resolve_interceptor(payload.directory.as_deref())
            .map_err(Self::agent_error_to_stdio)?;

        let steps = interceptor.completed_steps();
        Ok(json!({
            "steps": steps,
        }))
    }

    fn undo_configure(
        &self,
        _payload: UndoConfigurePayload,
    ) -> Result<serde_json::Value, StdioError> {
        self.require_active()
            .map_err(Self::agent_error_to_stdio)?;
        Ok(json!({}))
    }

    fn undo_discard(&self) -> Result<serde_json::Value, StdioError> {
        let interceptor = self
            .resolve_interceptor(None)
            .map_err(Self::agent_error_to_stdio)?;

        interceptor
            .discard()
            .map_err(AgentError::from)
            .map_err(Self::agent_error_to_stdio)?;

        Ok(json!({}))
    }

    fn agent_execute(
        &self,
        payload: AgentExecutePayload,
    ) -> Result<serde_json::Value, StdioError> {
        self.require_active()
            .map_err(Self::agent_error_to_stdio)?;

        let state = self.state.lock().unwrap();
        let session = match &*state {
            SessionState::Active(s) => s,
            _ => return Err(Self::agent_error_to_stdio(AgentError::SessionNotActive)),
        };

        // Check if VM is available
        let control_writer = match &session.control_writer {
            Some(writer) => writer.clone(),
            None => return Err(Self::agent_error_to_stdio(AgentError::QemuUnavailable)),
        };

        let command_id = session.next_command_id.fetch_add(1, Ordering::Relaxed);

        // Build and serialize the exec message
        let exec_msg = codeagent_control::HostMessage::Exec {
            id: command_id,
            command: payload.command.clone(),
            cwd: payload.cwd,
            env: payload.env,
        };

        let json_str = control_bridge::serialize_host_message(&exec_msg)
            .map_err(|error| Self::agent_error_to_stdio(AgentError::Io(
                std::io::Error::other(error),
            )))?;

        control_writer
            .send(json_str)
            .map_err(|_| Self::agent_error_to_stdio(
                AgentError::ControlChannelFailed {
                    reason: "control channel closed".to_string(),
                },
            ))?;

        Ok(json!({
            "command_id": command_id,
            "status": "started",
        }))
    }

    fn agent_prompt(
        &self,
        _payload: AgentPromptPayload,
    ) -> Result<serde_json::Value, StdioError> {
        self.require_active()
            .map_err(Self::agent_error_to_stdio)?;

        Err(Self::agent_error_to_stdio(AgentError::NotImplemented {
            feature: "agent.prompt".to_string(),
        }))
    }

    fn fs_list(&self, payload: FsListPayload) -> Result<serde_json::Value, StdioError> {
        let working_dir = self
            .primary_working_dir()
            .map_err(Self::agent_error_to_stdio)?;

        let target = working_dir.join(&payload.path);

        let entries: Vec<serde_json::Value> = std::fs::read_dir(&target)
            .map_err(|e| StdioError::Io { source: e })?
            .filter_map(|entry| entry.ok())
            .map(|entry| {
                let file_type = entry.file_type().ok();
                json!({
                    "name": entry.file_name().to_string_lossy(),
                    "type": if file_type.as_ref().is_some_and(|ft| ft.is_dir()) {
                        "directory"
                    } else if file_type.as_ref().is_some_and(|ft| ft.is_symlink()) {
                        "symlink"
                    } else {
                        "file"
                    },
                })
            })
            .collect();

        Ok(json!({
            "working_directory": target.display().to_string(),
            "entries": entries,
        }))
    }

    fn fs_read(&self, payload: FsReadPayload) -> Result<serde_json::Value, StdioError> {
        let working_dir = self
            .primary_working_dir()
            .map_err(Self::agent_error_to_stdio)?;

        let target = working_dir.join(&payload.path);
        let content =
            std::fs::read_to_string(&target).map_err(|e| StdioError::Io { source: e })?;

        Ok(json!({ "content": content }))
    }

    fn fs_status(&self) -> Result<serde_json::Value, StdioError> {
        self.require_active()
            .map_err(Self::agent_error_to_stdio)?;

        let state = self.state.lock().unwrap();
        let session = match &*state {
            SessionState::Active(s) => s,
            _ => return Err(Self::agent_error_to_stdio(AgentError::SessionNotActive)),
        };

        if session.qemu_process.is_some() {
            Ok(json!({
                "backend": if cfg!(target_os = "windows") { "9p" } else { "virtiofsd" },
                "vm_status": "running",
                "vm_pid": session.qemu_process.as_ref().and_then(|p| p.pid()),
            }))
        } else {
            Ok(json!({
                "backend": "none",
                "vm_status": "unavailable",
            }))
        }
    }

    fn safeguard_configure(
        &self,
        payload: SafeguardConfigurePayload,
    ) -> Result<serde_json::Value, StdioError> {
        let mut state = self.state.lock().unwrap();
        match &mut *state {
            SessionState::Idle => Err(Self::agent_error_to_stdio(AgentError::SessionNotActive)),
            SessionState::Active(session) => {
                if let Some(threshold) = payload.delete_threshold {
                    session.safeguard_config.delete_threshold = Some(threshold);
                }
                if let Some(threshold) = payload.overwrite_file_size_threshold {
                    session.safeguard_config.overwrite_file_size_threshold = Some(threshold);
                }
                session.safeguard_config.rename_over_existing = payload.rename_over_existing;
                Ok(json!({}))
            }
        }
    }

    fn safeguard_confirm(
        &self,
        payload: SafeguardConfirmPayload,
    ) -> Result<serde_json::Value, StdioError> {
        let mut state = self.state.lock().unwrap();
        let session = match &mut *state {
            SessionState::Idle => {
                return Err(Self::agent_error_to_stdio(AgentError::SessionNotActive));
            }
            SessionState::Active(s) => s,
        };

        let decision = match payload.action.as_str() {
            "allow" => SafeguardDecision::Allow,
            _ => SafeguardDecision::Deny,
        };

        if let Some(sender) = session.pending_safeguards.remove(&payload.safeguard_id) {
            let _ = sender.send(decision);
            Ok(json!({}))
        } else {
            Err(StdioError::InvalidField {
                field: "safeguard_id".to_string(),
                message: format!("no pending safeguard with id '{}'", payload.safeguard_id),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// McpHandler implementation
// ---------------------------------------------------------------------------

use codeagent_mcp::protocol::{
    BashArgs, EditFileArgs, GetUndoHistoryArgs, GlobArgs, GrepArgs, ListDirectoryArgs,
    ReadFileArgs, UndoArgs, WriteFileArgs,
};

impl codeagent_mcp::McpHandler for Orchestrator {
    fn bash(
        &self,
        args: BashArgs,
    ) -> Result<serde_json::Value, McpError> {
        self.require_active()
            .map_err(Self::agent_error_to_mcp)?;

        // Sanitize: reject inherently dangerous commands before they reach the VM
        if let SanitizeResult::Rejected { reason } = command_classifier::sanitize(&args.command) {
            return Err(McpError::InvalidParams {
                message: format!("command rejected: {reason}"),
            });
        }

        // Classify for response metadata (informational — no gate)
        let classification = self.classifier.classify(&args.command);

        let (control_writer, control_handler, command_id) = {
            let state = self.state.lock().unwrap();
            let session = match &*state {
                SessionState::Active(s) => s,
                _ => return Err(Self::agent_error_to_mcp(AgentError::SessionNotActive)),
            };

            let writer = match &session.control_writer {
                Some(writer) => writer.clone(),
                None => return Err(Self::agent_error_to_mcp(AgentError::QemuUnavailable)),
            };

            let handler = match &session.control_handler {
                Some(handler) => handler.clone(),
                None => return Err(Self::agent_error_to_mcp(AgentError::QemuUnavailable)),
            };

            let id = session.next_command_id.fetch_add(1, Ordering::Relaxed);
            (writer, handler, id)
        };

        // Register with the waiter before sending so early events are captured
        self.command_waiter.register(command_id);

        // Register the command with the control channel handler's state machine
        // (so it expects the StepStarted response from the VM) and get the
        // serializable HostMessage back. Uses block_in_place because send_exec
        // is async (may close an ambient step).
        let host_msg = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(
                control_handler.send_exec(
                    command_id,
                    args.command.clone(),
                    None,
                    None,
                ),
            )
        });

        let json_str = control_bridge::serialize_host_message(&host_msg)
            .map_err(|error| McpError::InternalError {
                message: format!("failed to serialize exec message: {error}"),
            })?;

        control_writer
            .send(json_str)
            .map_err(|_| McpError::InternalError {
                message: "control channel closed".to_string(),
            })?;

        // Block until the command completes or times out.
        // Use block_in_place so tokio can spawn a replacement worker thread
        // while this one is blocked on the Condvar — otherwise async tasks
        // (control reader, event bridge, P9 server) may starve.
        let timeout_ms = args.timeout.unwrap_or(120_000).min(600_000);
        let timeout = std::time::Duration::from_millis(timeout_ms);
        eprintln!(
            "{{\"level\":\"debug\",\"component\":\"mcp\",\"message\":\"bash: waiting for command {} (timeout {}ms)\"}}",
            command_id,
            timeout_ms
        );
        let result = tokio::task::block_in_place(|| {
            self.command_waiter.wait_for_completion(command_id, timeout)
        });

        match result {
            Some(r) if r.exit_code.is_some() => {
                let exit_code = r.exit_code.unwrap();
                eprintln!(
                    "{{\"level\":\"debug\",\"component\":\"mcp\",\"message\":\"bash: command {} completed with exit code {}\"}}",
                    command_id, exit_code
                );
                let mut output = r.stdout;
                if !r.stderr.is_empty() {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(&r.stderr);
                }
                Ok(json!({
                    "command_id": command_id,
                    "exit_code": exit_code,
                    "output": output,
                    "classification": classification.to_string(),
                }))
            }
            Some(r) => {
                eprintln!(
                    "{{\"level\":\"warn\",\"component\":\"mcp\",\"message\":\"bash: command {} timed out\"}}",
                    command_id
                );
                let mut output = r.stdout;
                if !r.stderr.is_empty() {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(&r.stderr);
                }
                Ok(json!({
                    "command_id": command_id,
                    "status": "timeout",
                    "output": output,
                    "classification": classification.to_string(),
                }))
            }
            None => Err(McpError::InternalError {
                message: "command was not registered".to_string(),
            }),
        }
    }

    fn read_file(&self, args: ReadFileArgs) -> Result<serde_json::Value, McpError> {
        let working_dir = self
            .primary_working_dir()
            .map_err(Self::agent_error_to_mcp)?;

        let target = working_dir.join(&args.path);
        let content = std::fs::read_to_string(&target)
            .map_err(|e| McpError::InternalError {
                message: e.to_string(),
            })?;

        Ok(json!({ "content": content }))
    }

    fn write_file(&self, args: WriteFileArgs) -> Result<serde_json::Value, McpError> {
        let interceptor = self
            .resolve_interceptor(None)
            .map_err(Self::agent_error_to_mcp)?;

        let working_dir = self
            .primary_working_dir()
            .map_err(Self::agent_error_to_mcp)?;

        let target = working_dir.join(&args.path);

        // Open a synthetic API step for undo tracking
        let step_id = self.next_api_step_id()?;

        interceptor
            .open_step(step_id)
            .map_err(|e| McpError::InternalError {
                message: e.to_string(),
            })?;

        interceptor.set_step_command(format!("write_file {}", args.path));

        let existed_before = target.exists();

        if existed_before {
            let _ = interceptor.pre_write(&target);
        } else {
            // Collect directories that need to be created so we can track them
            let mut dirs_to_track = Vec::new();
            if let Some(parent) = target.parent() {
                let mut ancestor = parent.to_path_buf();
                while !ancestor.exists() {
                    dirs_to_track.push(ancestor.clone());
                    if !ancestor.pop() {
                        break;
                    }
                }
                std::fs::create_dir_all(parent).map_err(|e| McpError::InternalError {
                    message: e.to_string(),
                })?;
                // Record created directories shallowest-first
                for dir in dirs_to_track.iter().rev() {
                    let _ = interceptor.post_mkdir(dir);
                }
            }
        }

        std::fs::write(&target, &args.content).map_err(|e| McpError::InternalError {
            message: e.to_string(),
        })?;

        // Record the write so the filesystem watcher doesn't treat it as external.
        if let Some(rw) = self.recent_writes() {
            rw.record(&target);
        }

        if !existed_before {
            let _ = interceptor.post_create(&target);
        }

        interceptor
            .close_step(step_id)
            .map_err(|e| McpError::InternalError {
                message: e.to_string(),
            })?;

        Ok(json!({ "written": true, "step_id": step_id }))
    }

    fn list_directory(
        &self,
        args: ListDirectoryArgs,
    ) -> Result<serde_json::Value, McpError> {
        let working_dir = self
            .primary_working_dir()
            .map_err(Self::agent_error_to_mcp)?;

        let target = working_dir.join(&args.path);

        let entries: Vec<serde_json::Value> = std::fs::read_dir(&target)
            .map_err(|e| McpError::InternalError {
                message: e.to_string(),
            })?
            .filter_map(|entry| entry.ok())
            .map(|entry| {
                let file_type = entry.file_type().ok();
                json!({
                    "name": entry.file_name().to_string_lossy(),
                    "type": if file_type.as_ref().is_some_and(|ft| ft.is_dir()) {
                        "directory"
                    } else if file_type.as_ref().is_some_and(|ft| ft.is_symlink()) {
                        "symlink"
                    } else {
                        "file"
                    },
                })
            })
            .collect();

        Ok(json!({
            "working_directory": target.display().to_string(),
            "entries": entries,
        }))
    }

    fn edit_file(&self, args: EditFileArgs) -> Result<serde_json::Value, McpError> {
        let interceptor = self
            .resolve_interceptor(None)
            .map_err(Self::agent_error_to_mcp)?;

        let working_dir = self
            .primary_working_dir()
            .map_err(Self::agent_error_to_mcp)?;

        let target = working_dir.join(&args.path);

        let content = std::fs::read_to_string(&target).map_err(|e| McpError::InternalError {
            message: e.to_string(),
        })?;

        // Validate that old_string exists and is unique (unless replace_all)
        let match_count = content.matches(&args.old_string).count();
        if match_count == 0 {
            return Err(McpError::InvalidParams {
                message: "old_string not found in file".to_string(),
            });
        }
        if match_count > 1 && !args.replace_all {
            return Err(McpError::InvalidParams {
                message: format!(
                    "old_string is not unique in the file (found {} occurrences). \
                     Provide more context or use replace_all.",
                    match_count
                ),
            });
        }

        let new_content = if args.replace_all {
            content.replace(&args.old_string, &args.new_string)
        } else {
            content.replacen(&args.old_string, &args.new_string, 1)
        };

        // Open a synthetic API step for undo tracking
        let step_id = self.next_api_step_id()?;

        interceptor
            .open_step(step_id)
            .map_err(|e| McpError::InternalError {
                message: e.to_string(),
            })?;

        interceptor.set_step_command(format!("edit_file {}", args.path));

        let _ = interceptor.pre_write(&target);

        std::fs::write(&target, &new_content).map_err(|e| McpError::InternalError {
            message: e.to_string(),
        })?;

        // Record the write so the filesystem watcher doesn't treat it as external.
        if let Some(rw) = self.recent_writes() {
            rw.record(&target);
        }

        interceptor
            .close_step(step_id)
            .map_err(|e| McpError::InternalError {
                message: e.to_string(),
            })?;

        Ok(json!(format!("The file {} has been updated successfully.", args.path)))
    }

    fn glob(&self, args: GlobArgs) -> Result<serde_json::Value, McpError> {
        let working_dir = self
            .primary_working_dir()
            .map_err(Self::agent_error_to_mcp)?;

        let search_dir = match &args.path {
            Some(p) => working_dir.join(p),
            None => working_dir.clone(),
        };

        let pattern_str = search_dir
            .join(&args.pattern)
            .to_string_lossy()
            .replace('\\', "/");

        let mut entries: Vec<(String, std::time::SystemTime)> =
            glob::glob(&pattern_str)
                .map_err(|e| McpError::InvalidParams {
                    message: format!("Invalid glob pattern: {e}"),
                })?
                .filter_map(|entry| entry.ok())
                .filter_map(|path| {
                    let mtime = path.metadata().ok()?.modified().ok()?;
                    let relative = path
                        .strip_prefix(&working_dir)
                        .ok()?
                        .to_string_lossy()
                        .replace('\\', "/");
                    Some((relative, mtime))
                })
                .collect();

        // Sort by modification time, newest first
        entries.sort_by(|a, b| b.1.cmp(&a.1));

        let limit = args.limit.unwrap_or(200);
        let total = entries.len();
        let truncated = total > limit;
        entries.truncate(limit);

        let result: Vec<&str> = entries.iter().map(|(path, _)| path.as_str()).collect();
        let mut output = result.join("\n");
        if truncated {
            output.push_str(&format!("\n\n[Truncated: showing {limit} of {total} matches]"));
        }
        Ok(json!(output))
    }

    fn grep(&self, args: GrepArgs) -> Result<serde_json::Value, McpError> {
        let working_dir = self
            .primary_working_dir()
            .map_err(Self::agent_error_to_mcp)?;

        let search_path = match &args.path {
            Some(p) => working_dir.join(p),
            None => working_dir.clone(),
        };

        let regex = regex::RegexBuilder::new(&args.pattern)
            .case_insensitive(args.case_insensitive)
            .build()
            .map_err(|e| McpError::InvalidParams {
                message: format!("Invalid regex pattern: {e}"),
            })?;

        let include_glob = args
            .include
            .as_ref()
            .map(|p| glob::Pattern::new(p))
            .transpose()
            .map_err(|e| McpError::InvalidParams {
                message: format!("Invalid include pattern: {e}"),
            })?;

        let context = args.context_lines.unwrap_or(0);
        let mut output = String::new();

        // Collect files to search
        let files: Vec<std::path::PathBuf> = if search_path.is_file() {
            vec![search_path.clone()]
        } else {
            walkdir::WalkDir::new(&search_path)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_file())
                .filter(|e| {
                    if let Some(ref pat) = include_glob {
                        pat.matches(
                            &e.path()
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy(),
                        )
                    } else {
                        true
                    }
                })
                .map(|e| e.into_path())
                .collect()
        };

        for file_path in &files {
            let content = match std::fs::read_to_string(file_path) {
                Ok(c) => c,
                Err(_) => continue, // skip binary/unreadable files
            };

            let lines: Vec<&str> = content.lines().collect();
            let matching_lines: Vec<usize> = lines
                .iter()
                .enumerate()
                .filter(|(_, line)| regex.is_match(line))
                .map(|(i, _)| i)
                .collect();

            if matching_lines.is_empty() {
                continue;
            }

            let relative = file_path
                .strip_prefix(&working_dir)
                .unwrap_or(file_path)
                .to_string_lossy()
                .replace('\\', "/");

            match args.output_mode.as_str() {
                "files_with_matches" => {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(&relative);
                }
                "count" => {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(&format!("{}:{}", relative, matching_lines.len()));
                }
                _ => {
                    if !output.is_empty() {
                        output.push_str("\n\n");
                    }
                    output.push_str(&relative);

                    // Build set of lines to show (matches + context)
                    let mut visible: std::collections::BTreeSet<usize> =
                        std::collections::BTreeSet::new();
                    for &line_idx in &matching_lines {
                        let start = line_idx.saturating_sub(context);
                        let end = (line_idx + context + 1).min(lines.len());
                        for i in start..end {
                            visible.insert(i);
                        }
                    }

                    for &i in &visible {
                        output.push_str(&format!("\n{}:{}", i + 1, lines[i]));
                    }
                }
            }
        }

        Ok(json!(output))
    }

    fn undo(&self, args: UndoArgs) -> Result<serde_json::Value, McpError> {
        let interceptor = self
            .resolve_interceptor(None)
            .map_err(Self::agent_error_to_mcp)?;

        let count = args.count as usize;
        let force = args.force;

        let result = interceptor
            .rollback(count, force)
            .map_err(|e| McpError::InternalError {
                message: e.to_string(),
            })?;

        Ok(json!({
            "steps_rolled_back": result.steps_rolled_back,
            "barriers_crossed": result.barriers_crossed.len(),
        }))
    }

    fn get_undo_history(
        &self,
        _args: GetUndoHistoryArgs,
    ) -> Result<serde_json::Value, McpError> {
        let interceptor = self
            .resolve_interceptor(None)
            .map_err(Self::agent_error_to_mcp)?;

        let steps = interceptor.completed_steps();
        Ok(json!({ "steps": steps }))
    }

    fn get_session_status(&self) -> Result<serde_json::Value, McpError> {
        self.do_session_status()
            .map_err(Self::agent_error_to_mcp)
    }
}
