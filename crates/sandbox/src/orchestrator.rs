use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::json;
use tokio::sync::mpsc;

use codeagent_common::{SafeguardConfig, SafeguardDecision};
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
use crate::control_bridge;
use crate::error::AgentError;
use crate::qemu::{QemuConfig, QemuProcess};
use crate::safeguard_bridge::PendingSafeguard;
use crate::session::{Session, SessionState};

/// Central orchestrator that implements both `RequestHandler` (STDIO API)
/// and `McpHandler` (MCP server) by delegating to shared session state.
pub struct Orchestrator {
    state: Arc<Mutex<SessionState>>,
    cli_args: CliArgs,
    event_sender: mpsc::UnboundedSender<Event>,
    /// Populated when the filesystem backend connects and safeguards are enabled.
    #[allow(dead_code)]
    safeguard_receiver: Mutex<Option<mpsc::UnboundedReceiver<PendingSafeguard>>>,
}

impl Orchestrator {
    pub fn new(
        cli_args: CliArgs,
        event_sender: mpsc::UnboundedSender<Event>,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(SessionState::Idle)),
            cli_args,
            event_sender,
            safeguard_receiver: Mutex::new(None),
        }
    }

    /// Returns true if VM components (kernel + initrd) are configured.
    fn is_vm_available(&self) -> bool {
        self.cli_args.kernel_path.is_some() && self.cli_args.initrd_path.is_some()
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
            vec![self.cli_args.working_dir.clone()]
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

        let mut interceptors = Vec::with_capacity(working_dirs.len());
        let mut undo_dirs = Vec::with_capacity(working_dirs.len());

        for (index, working_dir) in working_dirs.iter().enumerate() {
            let undo_dir = self.cli_args.undo_dir.join(index.to_string());
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

            interceptors.push(Arc::new(interceptor));
            undo_dirs.push(undo_dir);
        }

        // Determine VM availability and launch if configured
        let (vm_status, backend_name) = if self.is_vm_available() {
            // VM components are configured — attempt to launch.
            // For now, the actual launch is deferred to when `launch_vm`
            // is called from `do_session_start`. The launch_vm method
            // will populate the VM fields on the session.
            match self.launch_vm(&working_dirs, &interceptors) {
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
                        event_bridge_handle: vm_session_parts.event_bridge_handle,
                        control_reader_handle: vm_session_parts.control_reader_handle,
                        control_writer_handle: vm_session_parts.control_writer_handle,
                        socket_dir: vm_session_parts.socket_dir,
                        next_command_id: Arc::new(AtomicU64::new(1)),
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
                    );
                    *state = SessionState::Active(Box::new(session));
                    ("unavailable", "none")
                }
            }
        } else {
            // No VM components configured — run in host-only mode
            let session = Self::create_non_vm_session(
                interceptors, working_dirs.clone(), undo_dirs, payload,
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
            event_bridge_handle: None,
            control_reader_handle: None,
            control_writer_handle: None,
            socket_dir: None,
            next_command_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Launch VM components: virtiofsd backends, QEMU, control channel.
    #[allow(unused_variables, unused_mut)]
    fn launch_vm(
        &self,
        working_dirs: &[PathBuf],
        interceptors: &[Arc<UndoInterceptor>],
    ) -> Result<VmSessionParts, AgentError> {
        let socket_dir = self.cli_args.undo_dir.join(".sockets");
        std::fs::create_dir_all(&socket_dir)?;

        let control_socket_path = socket_dir.join("control.sock");

        // Create InFlightTracker before backends so they can share it with
        // the control channel handler for quiescence detection.
        let in_flight_tracker = InFlightTracker::new();

        // 1. Start filesystem backends (Linux/macOS only)
        let mut fs_backends: Vec<Box<dyn crate::fs_backend::FilesystemBackend>> = Vec::new();
        let mut fs_socket_paths = Vec::new();

        #[cfg(target_os = "linux")]
        {
            use crate::fs_backend::{FilesystemBackend, InterceptedBackend};
            for (index, working_dir) in working_dirs.iter().enumerate() {
                let fs_socket = socket_dir.join(format!("vfs{index}.sock"));
                let mut backend = InterceptedBackend::new(
                    working_dir.clone(),
                    fs_socket.clone(),
                    interceptors[index].clone(),
                    in_flight_tracker.clone(),
                );
                backend.start()?;
                fs_socket_paths.push(fs_socket);
                fs_backends.push(Box::new(backend));
            }
        }

        #[cfg(all(not(target_os = "linux"), not(target_os = "windows")))]
        {
            use crate::fs_backend::{FilesystemBackend, VirtioFsBackend};
            for (index, working_dir) in working_dirs.iter().enumerate() {
                let fs_socket = socket_dir.join(format!("vfs{index}.sock"));
                let mut backend = VirtioFsBackend::new(
                    working_dir.clone(),
                    fs_socket.clone(),
                    self.cli_args.virtiofsd_binary.clone(),
                );
                backend.start()?;
                fs_socket_paths.push(fs_socket);
                fs_backends.push(Box::new(backend));
            }
        }

        // 2. Build QEMU config and spawn
        let config = QemuConfig {
            qemu_binary: self.cli_args.qemu_binary.clone(),
            kernel_path: self.cli_args.kernel_path.clone().unwrap(),
            initrd_path: self.cli_args.initrd_path.clone().unwrap(),
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

        // 3. Connect to control socket and set up channel handler
        #[cfg(not(unix))]
        {
            // On Windows, control channel will use named pipes (Phase 3).
            Err(AgentError::ControlChannelFailed {
                reason: "control channel not yet supported on Windows".to_string(),
            })
        }

        #[cfg(unix)]
        {
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

            // 4. Create control channel handler
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

            // 5. Spawn event bridge (control events → STDIO events)
            let event_bridge_handle = tokio::spawn(run_event_bridge(
                handler_events,
                self.event_sender.clone(),
            ));

            // 6. Spawn control channel writer and reader tasks
            let (control_writer_sender, control_writer_handle) =
                control_bridge::spawn_control_writer(writer);

            let control_reader_handle = control_bridge::spawn_control_reader(
                reader,
                handler,
                self.event_sender.clone(),
            );

            Ok(VmSessionParts {
                qemu_process: Some(qemu_process),
                fs_backends,
                in_flight_tracker: Some(in_flight_tracker),
                control_writer: Some(control_writer_sender),
                event_bridge_handle: Some(event_bridge_handle),
                control_reader_handle: Some(control_reader_handle),
                control_writer_handle: Some(control_writer_handle),
                socket_dir: Some(socket_dir),
            })
        }
    }

    fn do_session_stop(&self) -> Result<serde_json::Value, AgentError> {
        let mut state = self.state.lock().unwrap();
        match &mut *state {
            SessionState::Idle => Err(AgentError::SessionNotActive),
            SessionState::Active(session) => {
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
}

/// Parts of a session that come from VM launch.
struct VmSessionParts {
    qemu_process: Option<QemuProcess>,
    fs_backends: Vec<Box<dyn crate::fs_backend::FilesystemBackend>>,
    in_flight_tracker: Option<InFlightTracker>,
    control_writer: Option<mpsc::UnboundedSender<String>>,
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

        Ok(json!({ "entries": entries }))
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
    ExecuteCommandArgs, GetUndoHistoryArgs, ListDirectoryArgs, ReadFileArgs, UndoArgs,
    WriteFileArgs,
};

impl codeagent_mcp::McpHandler for Orchestrator {
    fn execute_command(
        &self,
        _args: ExecuteCommandArgs,
    ) -> Result<serde_json::Value, McpError> {
        self.require_active()
            .map_err(Self::agent_error_to_mcp)?;

        Err(Self::agent_error_to_mcp(AgentError::QemuUnavailable))
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
        let step_id = {
            let state = self.state.lock().unwrap();
            match &*state {
                SessionState::Active(session) => {
                    // Use a large positive step ID for API steps to avoid
                    // collision with control channel step IDs (which start
                    // at 1) and ambient step IDs (which are negative).
                    (session.interceptors[0].completed_steps().len() as i64) + 1_000_000
                }
                _ => return Err(Self::agent_error_to_mcp(AgentError::SessionNotActive)),
            }
        };

        interceptor
            .open_step(step_id)
            .map_err(|e| McpError::InternalError {
                message: e.to_string(),
            })?;

        // Capture preimage before writing
        if target.exists() {
            let _ = interceptor.pre_write(&target);
        } else {
            // Ensure parent directory exists
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| McpError::InternalError {
                    message: e.to_string(),
                })?;
            }
        }

        std::fs::write(&target, &args.content).map_err(|e| McpError::InternalError {
            message: e.to_string(),
        })?;

        if !target.exists() {
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

        Ok(json!({ "entries": entries }))
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
