use crate::config::SandboxConfig;
use crate::paths;
use serde::Serialize;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::process::Child;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};

/// VM status reported to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct VmStatus {
    pub state: String,
    pub pid: Option<u32>,
    pub error: Option<String>,
    /// Whether we're connected to a sandbox via the side-channel socket (MCP mode).
    pub socket_connected: bool,
}

/// A single line from the sandbox process stderr.
#[derive(Debug, Clone, Serialize)]
pub struct DebugLogLine {
    pub index: usize,
    pub timestamp: String,
    pub line: String,
}

/// Result of executing a terminal command.
#[derive(Debug, Clone, Serialize)]
pub struct TerminalOutput {
    pub exit_code: Option<i32>,
    pub output: String,
    pub status: String,
}

/// A connected side-channel socket to the sandbox (MCP mode).
struct SocketConnection {
    writer: std::io::BufWriter<TcpStream>,
    reader: BufReader<TcpStream>,
}

/// Shared state holding the sandbox child process and its I/O handles.
pub struct VmState {
    // Manual mode: desktop spawns sandbox directly
    pub process: Mutex<Option<Child>>,
    pub stdin: Arc<Mutex<Option<std::process::ChildStdin>>>,
    pub stdout: Arc<Mutex<Option<BufReader<std::process::ChildStdout>>>>,
    pub debug_log: Arc<Mutex<Vec<DebugLogLine>>>,
    pub stderr_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    // MCP mode: desktop connects to sandbox via side-channel socket
    socket: Mutex<Option<SocketConnection>>,
}

impl Default for VmState {
    fn default() -> Self {
        Self {
            process: Mutex::new(None),
            stdin: Arc::new(Mutex::new(None)),
            stdout: Arc::new(Mutex::new(None)),
            debug_log: Arc::new(Mutex::new(Vec::new())),
            stderr_handle: Mutex::new(None),
            socket: Mutex::new(None),
        }
    }
}

/// Atomic counter for terminal command request IDs.
static TERMINAL_REQUEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Check that a guest image file exists and is non-empty (0-byte files are invalid).
fn is_valid_guest_image(path: &std::path::Path) -> bool {
    path.exists() && std::fs::metadata(path).map_or(false, |m| m.len() > 0)
}

/// Resolve guest image paths: prefer user config, fall back to bundled resources,
/// then development paths.
fn resolve_guest_images(app: &AppHandle, config: &SandboxConfig) -> (String, String) {
    let mut kernel = config.vm.kernel_path.clone();
    let mut initrd = config.vm.initrd_path.clone();

    // Skip user-configured paths if the files don't actually exist or are empty
    if !kernel.is_empty() && !is_valid_guest_image(std::path::Path::new(&kernel)) {
        kernel.clear();
    }
    if !initrd.is_empty() && !is_valid_guest_image(std::path::Path::new(&initrd)) {
        initrd.clear();
    }

    // Collect candidate directories to search for guest images
    let mut candidate_dirs: Vec<std::path::PathBuf> = Vec::new();

    // 1. Bundled resources (production installs)
    if let Ok(resource_dir) = app.path().resource_dir() {
        candidate_dirs.push(resource_dir.join("guest"));
    }

    // 2. Development fallbacks relative to the executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidate_dirs.push(exe_dir.join("guest"));
            if let Some(target_dir) = exe_dir.parent() {
                let arch = if cfg!(target_arch = "aarch64") {
                    "aarch64"
                } else {
                    "x86_64"
                };
                candidate_dirs.push(target_dir.join("guest").join(arch));
            }
        }
    }

    for dir in &candidate_dirs {
        if kernel.is_empty() {
            let bundled = dir.join("vmlinuz");
            if is_valid_guest_image(&bundled) {
                kernel = bundled.to_string_lossy().into_owned();
            }
        }
        if initrd.is_empty() {
            let bundled = dir.join("initrd.img");
            if is_valid_guest_image(&bundled) {
                initrd = bundled.to_string_lossy().into_owned();
            }
        }
        if !kernel.is_empty() && !initrd.is_empty() {
            break;
        }
    }

    (kernel, initrd)
}

/// Build CLI args from config for the sandbox binary (manual mode only).
fn build_sandbox_args(
    config: &SandboxConfig,
    kernel_path: &str,
    initrd_path: &str,
) -> Vec<String> {
    let mut args = Vec::new();

    for dir in &config.sandbox.working_dirs {
        if !dir.is_empty() {
            args.push("--working-dir".into());
            args.push(dir.clone());
        }
    }
    if !config.sandbox.undo_dir.is_empty() {
        args.push("--undo-dir".into());
        args.push(config.sandbox.undo_dir.clone());
    }

    args.push("--protocol".into());
    args.push(config.sandbox.protocol.clone());

    args.push("--vm-mode".into());
    args.push(config.sandbox.vm_mode.clone());

    args.push("--log-level".into());
    args.push(config.sandbox.log_level.clone());

    args.push("--memory-mb".into());
    args.push(config.vm.memory_mb.to_string());

    args.push("--cpus".into());
    args.push(config.vm.cpus.to_string());

    if !config.vm.qemu_binary.is_empty() {
        args.push("--qemu-binary".into());
        args.push(config.vm.qemu_binary.clone());
    }
    if !kernel_path.is_empty() {
        args.push("--kernel-path".into());
        args.push(kernel_path.to_string());
    }
    if !initrd_path.is_empty() {
        args.push("--initrd-path".into());
        args.push(initrd_path.to_string());
    }
    if !config.vm.rootfs_path.is_empty() {
        args.push("--rootfs-path".into());
        args.push(config.vm.rootfs_path.clone());
    }
    if !config.vm.virtiofsd_binary.is_empty() {
        args.push("--virtiofsd-binary".into());
        args.push(config.vm.virtiofsd_binary.clone());
    }

    args
}

/// Kill any orphaned sandbox.exe from a previous session using the PID file.
pub fn kill_orphaned_sandbox() {
    let Some(pid_path) = paths::pid_file_path() else {
        return;
    };
    let Ok(contents) = std::fs::read_to_string(&pid_path) else {
        return;
    };
    let Ok(pid) = contents.trim().parse::<u32>() else {
        let _ = std::fs::remove_file(&pid_path);
        return;
    };

    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    #[cfg(not(windows))]
    {
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
    }

    let _ = std::fs::remove_file(&pid_path);
}

/// Resolve the sandbox binary path.
///
/// Search order: Tauri sidecar (triple-suffixed) next to executable,
/// plain name next to executable, workspace target directory, then PATH.
pub(super) fn find_sandbox_binary() -> Result<String, String> {
    let sandbox_name = if cfg!(windows) {
        "sandbox.exe"
    } else {
        "sandbox"
    };

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sidecar_name = if cfg!(windows) {
                format!("sandbox-{}.exe", env!("TARGET_TRIPLE"))
            } else {
                format!("sandbox-{}", env!("TARGET_TRIPLE"))
            };
            let candidate = dir.join(&sidecar_name);
            if candidate.exists() {
                return Ok(candidate.to_string_lossy().into_owned());
            }

            let candidate = dir.join(sandbox_name);
            if candidate.exists() {
                return Ok(candidate.to_string_lossy().into_owned());
            }
        }

        if let Ok(exe) = std::env::current_exe() {
            if let Some(target_dir) = exe.parent().and_then(|p| p.parent()) {
                for profile in &["release", "debug"] {
                    let candidate = target_dir.join(profile).join(sandbox_name);
                    if candidate.exists() {
                        return Ok(candidate.to_string_lossy().into_owned());
                    }
                }
            }
            if let Some(workspace_root) = exe
                .parent()
                .and_then(|p| p.parent()) // target/
                .and_then(|p| p.parent()) // src-tauri/
                .and_then(|p| p.parent()) // desktop/
            {
                for profile in &["release", "debug"] {
                    let candidate =
                        workspace_root.join("target").join(profile).join(sandbox_name);
                    if candidate.exists() {
                        return Ok(candidate.to_string_lossy().into_owned());
                    }
                }
            }
        }
    }

    if let Ok(path) = which::which("sandbox") {
        return Ok(path.to_string_lossy().into_owned());
    }

    Err("Could not find the sandbox binary. Build it with 'cargo build -p codeagent-sandbox' or place it on PATH.".into())
}

/// Start the sandbox VM as a child process (manual mode only).
///
/// Returns an error if MCP mode is enabled — in MCP mode, Claude Code spawns
/// the sandbox and the desktop app connects via side-channel socket instead.
#[tauri::command]
pub fn start_vm(
    app: AppHandle,
    config: SandboxConfig,
    state: State<'_, VmState>,
) -> Result<VmStatus, String> {
    if config.claude_code.enabled {
        return Err(
            "Sandbox is managed by Claude Code. Disable MCP integration to use manual mode."
                .into(),
        );
    }

    let mut process_guard = state.process.lock().map_err(|e| e.to_string())?;

    if process_guard.is_some() {
        return Err("VM is already running".into());
    }

    let binary = find_sandbox_binary()?;
    let (kernel_path, initrd_path) = resolve_guest_images(&app, &config);
    let args = build_sandbox_args(&config, &kernel_path, &initrd_path);

    let mut command = std::process::Command::new(&binary);
    command
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = command
        .spawn()
        .map_err(|e| format!("Failed to start sandbox: {e}"))?;

    let pid = child.id();

    let child_stdin = child.stdin.take();
    let child_stdout = child.stdout.take().map(BufReader::new);
    let child_stderr = child.stderr.take();

    if let Some(pid_path) = paths::pid_file_path() {
        if let Some(parent) = pid_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&pid_path, pid.to_string());
    }

    *process_guard = Some(child);
    drop(process_guard);

    if let Ok(mut guard) = state.stdin.lock() {
        *guard = child_stdin;
    }
    if let Ok(mut guard) = state.stdout.lock() {
        *guard = child_stdout;
    }

    if let Some(stderr) = child_stderr {
        let app_handle = app.clone();
        let debug_log = Arc::clone(&state.debug_log);

        let handle = std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            let mut index = 0usize;
            for line in reader.lines().map_while(Result::ok) {
                let entry = DebugLogLine {
                    index,
                    timestamp: chrono::Local::now().to_rfc3339(),
                    line: line.clone(),
                };
                if let Ok(mut log) = debug_log.lock() {
                    log.push(entry.clone());
                    if log.len() > 10_000 {
                        log.drain(..1000);
                    }
                }
                let _ = app_handle.emit("vm-debug-log", &entry);
                index += 1;
            }
        });
        if let Ok(mut guard) = state.stderr_handle.lock() {
            *guard = Some(handle);
        }
    }

    Ok(VmStatus {
        state: "running".into(),
        pid: Some(pid),
        error: None,
        socket_connected: false,
    })
}

/// Stop the sandbox VM (manual mode only).
#[tauri::command]
pub fn stop_vm(state: State<'_, VmState>) -> Result<VmStatus, String> {
    // In MCP mode, just disconnect the socket — don't try to kill the process
    if let Ok(mut guard) = state.socket.lock() {
        if guard.is_some() {
            guard.take();
            return Ok(VmStatus {
                state: "stopped".into(),
                pid: None,
                error: None,
                socket_connected: false,
            });
        }
    }

    // Manual mode: kill the child process
    if let Ok(mut guard) = state.stdin.lock() {
        guard.take();
    }
    if let Ok(mut guard) = state.stdout.lock() {
        guard.take();
    }

    let child = {
        let mut guard = state.process.lock().map_err(|e| e.to_string())?;
        guard.take()
    };

    if let Ok(mut log) = state.debug_log.lock() {
        log.clear();
    }
    if let Ok(mut guard) = state.stderr_handle.lock() {
        guard.take();
    }

    match child {
        Some(mut child) => {
            let _ = child.kill();
            let _ = child.wait();

            if let Some(pid_path) = paths::pid_file_path() {
                let _ = std::fs::remove_file(&pid_path);
            }

            Ok(VmStatus {
                state: "stopped".into(),
                pid: None,
                error: None,
                socket_connected: false,
            })
        }
        None => Ok(VmStatus {
            state: "stopped".into(),
            pid: None,
            error: None,
            socket_connected: false,
        }),
    }
}

/// Get the current VM status.
#[tauri::command]
pub fn get_vm_status(state: State<'_, VmState>) -> Result<VmStatus, String> {
    // Check socket connection (MCP mode)
    if let Ok(guard) = state.socket.lock() {
        if guard.is_some() {
            return Ok(VmStatus {
                state: "running".into(),
                pid: None,
                error: None,
                socket_connected: true,
            });
        }
    }

    // Check child process (manual mode)
    let mut guard = state.process.lock().map_err(|e| e.to_string())?;

    match guard.as_mut() {
        Some(child) => match child.try_wait() {
            Ok(Some(status)) => {
                let error = if status.success() {
                    None
                } else {
                    Some(format!("Process exited with status: {status}"))
                };

                guard.take();

                if let Ok(mut stdin_guard) = state.stdin.lock() {
                    stdin_guard.take();
                }
                if let Ok(mut stdout_guard) = state.stdout.lock() {
                    stdout_guard.take();
                }

                if let Some(pid_path) = paths::pid_file_path() {
                    let _ = std::fs::remove_file(&pid_path);
                }

                Ok(VmStatus {
                    state: "stopped".into(),
                    pid: None,
                    error,
                    socket_connected: false,
                })
            }
            Ok(None) => Ok(VmStatus {
                state: "running".into(),
                pid: Some(child.id()),
                error: None,
                socket_connected: false,
            }),
            Err(e) => Ok(VmStatus {
                state: "error".into(),
                pid: None,
                error: Some(format!("Failed to check process status: {e}")),
                socket_connected: false,
            }),
        },
        None => Ok(VmStatus {
            state: "stopped".into(),
            pid: None,
            error: None,
            socket_connected: false,
        }),
    }
}

/// Connect to the sandbox via the side-channel socket (MCP mode).
///
/// On Unix: connects to a Unix domain socket at the socket_path.
/// On Windows: reads the TCP port from socket_path and connects to localhost.
#[tauri::command]
pub fn connect_to_sandbox(state: State<'_, VmState>) -> Result<VmStatus, String> {
    let socket_path = paths::socket_path().ok_or("Could not determine socket path")?;

    if !socket_path.exists() {
        return Err("Sandbox socket not available. Is Claude Code running with the MCP server?".into());
    }

    // On Windows, the socket_path file contains a TCP port number.
    // On Unix, it's the actual socket file — but we use TCP for cross-platform consistency
    // since std::os::unix::net is not available in the Tauri context on all platforms.
    // Actually, we use TCP on Windows and Unix domain sockets on Unix.
    let stream = connect_to_socket(&socket_path)?;

    // Clone the stream for reader/writer
    let reader_stream = stream.try_clone().map_err(|e| format!("Failed to clone stream: {e}"))?;

    let mut conn = SocketConnection {
        writer: std::io::BufWriter::new(stream),
        reader: BufReader::new(reader_stream),
    };

    // Perform MCP initialize handshake
    let init_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "desktop-init",
        "method": "initialize",
        "params": {}
    });
    let init_str = serde_json::to_string(&init_req).map_err(|e| format!("JSON error: {e}"))?;

    conn.writer
        .write_all(init_str.as_bytes())
        .map_err(|e| format!("Failed to write to socket: {e}"))?;
    conn.writer
        .write_all(b"\n")
        .map_err(|e| format!("Failed to write newline: {e}"))?;
    conn.writer
        .flush()
        .map_err(|e| format!("Failed to flush socket: {e}"))?;

    let mut line = String::new();
    conn.reader
        .read_line(&mut line)
        .map_err(|e| format!("Failed to read from socket: {e}"))?;

    if line.is_empty() {
        return Err("Socket closed during handshake".into());
    }

    // Verify it's a valid response
    let _resp: serde_json::Value =
        serde_json::from_str(&line).map_err(|e| format!("Invalid handshake response: {e}"))?;

    if let Ok(mut guard) = state.socket.lock() {
        *guard = Some(conn);
    }

    Ok(VmStatus {
        state: "running".into(),
        pid: None,
        error: None,
        socket_connected: true,
    })
}

/// Connect to the sandbox socket, platform-specific.
fn connect_to_socket(socket_path: &std::path::Path) -> Result<TcpStream, String> {
    #[cfg(unix)]
    {
        // On Unix, connect to the Unix domain socket via std::os::unix::net,
        // but then we need a TCP bridge. For simplicity and cross-platform
        // consistency in the desktop app, we'll read the socket path differently.
        // If the file is a socket, connect via Unix socket.
        // Actually, std::os::unix::net::UnixStream returns a different type
        // than TcpStream. Let's use a simpler approach: always use TCP on all
        // platforms (the sandbox TCP server writes the port to the file).
        // This means on Unix the socket_path file also contains a port number.
        let port_str = std::fs::read_to_string(socket_path)
            .map_err(|e| format!("Failed to read socket file: {e}"))?;
        let port: u16 = port_str
            .trim()
            .parse()
            .map_err(|e| format!("Invalid port in socket file: {e}"))?;
        TcpStream::connect(format!("127.0.0.1:{port}"))
            .map_err(|e| format!("Failed to connect to sandbox: {e}"))
    }

    #[cfg(windows)]
    {
        let port_str = std::fs::read_to_string(socket_path)
            .map_err(|e| format!("Failed to read socket file: {e}"))?;
        let port: u16 = port_str
            .trim()
            .parse()
            .map_err(|e| format!("Invalid port in socket file: {e}"))?;
        TcpStream::connect(format!("127.0.0.1:{port}"))
            .map_err(|e| format!("Failed to connect to sandbox: {e}"))
    }
}

/// Disconnect from the sandbox side-channel socket.
#[tauri::command]
pub fn disconnect_from_sandbox(state: State<'_, VmState>) -> Result<(), String> {
    if let Ok(mut guard) = state.socket.lock() {
        guard.take();
    }
    Ok(())
}

/// Send a JSON-RPC line to the sandbox and read one response line.
///
/// Routes through stdin/stdout (manual mode) or the side-channel socket (MCP mode).
#[tauri::command]
pub fn send_mcp_request(
    request_json: String,
    state: State<'_, VmState>,
) -> Result<String, String> {
    // Try socket connection first (MCP mode)
    if let Ok(mut guard) = state.socket.lock() {
        if let Some(conn) = guard.as_mut() {
            conn.writer
                .write_all(request_json.as_bytes())
                .map_err(|e| format!("Failed to write to socket: {e}"))?;
            conn.writer
                .write_all(b"\n")
                .map_err(|e| format!("Failed to write newline: {e}"))?;
            conn.writer
                .flush()
                .map_err(|e| format!("Failed to flush socket: {e}"))?;

            let mut line = String::new();
            conn.reader
                .read_line(&mut line)
                .map_err(|e| format!("Failed to read from socket: {e}"))?;

            if line.is_empty() {
                // Socket closed — drop the connection
                guard.take();
                return Err("Sandbox socket closed".into());
            }

            return Ok(line.trim().to_string());
        }
    }

    // Fall back to stdin/stdout (manual mode)
    let mut stdin_guard = state.stdin.lock().map_err(|e| e.to_string())?;
    let stdin = stdin_guard
        .as_mut()
        .ok_or_else(|| "Sandbox process is not running".to_string())?;

    stdin
        .write_all(request_json.as_bytes())
        .map_err(|e| format!("Failed to write to sandbox stdin: {e}"))?;
    stdin
        .write_all(b"\n")
        .map_err(|e| format!("Failed to write newline: {e}"))?;
    stdin
        .flush()
        .map_err(|e| format!("Failed to flush stdin: {e}"))?;
    drop(stdin_guard);

    let mut stdout_guard = state.stdout.lock().map_err(|e| e.to_string())?;
    let stdout = stdout_guard
        .as_mut()
        .ok_or_else(|| "Sandbox process stdout unavailable".to_string())?;

    let mut line = String::new();
    stdout
        .read_line(&mut line)
        .map_err(|e| format!("Failed to read from sandbox stdout: {e}"))?;

    if line.is_empty() {
        return Err("Sandbox process closed stdout".into());
    }

    Ok(line.trim().to_string())
}

/// Fetch debug log lines since a given index.
///
/// In MCP mode, reads from the log file written by the sandbox process.
/// In manual mode, reads from the in-memory buffer fed by the stderr reader thread.
#[tauri::command]
pub fn get_debug_log(
    since_index: usize,
    state: State<'_, VmState>,
) -> Result<Vec<DebugLogLine>, String> {
    // Check if we're in MCP mode (socket connected but no child process)
    let is_mcp_mode = state
        .socket
        .lock()
        .map(|g| g.is_some())
        .unwrap_or(false);

    if is_mcp_mode {
        // Read from the log file
        if let Some(log_path) = paths::log_file_path() {
            if log_path.exists() {
                let content = std::fs::read_to_string(&log_path).unwrap_or_default();
                let lines: Vec<DebugLogLine> = content
                    .lines()
                    .enumerate()
                    .filter(|(i, _)| *i >= since_index)
                    .map(|(i, line)| DebugLogLine {
                        index: i,
                        timestamp: String::new(),
                        line: line.to_string(),
                    })
                    .collect();
                return Ok(lines);
            }
        }
        return Ok(Vec::new());
    }

    // Manual mode: read from in-memory buffer
    let log = state.debug_log.lock().map_err(|e| e.to_string())?;
    Ok(log
        .iter()
        .filter(|l| l.index >= since_index)
        .cloned()
        .collect())
}

/// Clear the debug log buffer.
#[tauri::command]
pub fn clear_debug_log(state: State<'_, VmState>) -> Result<(), String> {
    let mut log = state.debug_log.lock().map_err(|e| e.to_string())?;
    log.clear();
    Ok(())
}

/// Execute a shell command in the VM via the MCP Bash tool.
#[tauri::command]
pub async fn execute_terminal_command(
    command: String,
    timeout: Option<u32>,
    state: State<'_, VmState>,
) -> Result<TerminalOutput, String> {
    // Check if we're in MCP mode
    let is_mcp_mode = state
        .socket
        .lock()
        .map(|g| g.is_some())
        .unwrap_or(false);

    if is_mcp_mode {
        // Send through socket connection
        let request_id = TERMINAL_REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let timeout_ms = timeout.unwrap_or(120) as u64 * 1000;
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": format!("term-{request_id}"),
            "method": "tools/call",
            "params": {
                "name": "Bash",
                "arguments": {
                    "command": command,
                    "timeout": timeout_ms,
                }
            }
        });
        let request_str = request.to_string();

        let mut guard = state.socket.lock().map_err(|e| e.to_string())?;
        let conn = guard
            .as_mut()
            .ok_or("Socket connection lost")?;

        conn.writer
            .write_all(request_str.as_bytes())
            .map_err(|e| format!("Failed to write to socket: {e}"))?;
        conn.writer
            .write_all(b"\n")
            .map_err(|e| format!("Failed to write newline: {e}"))?;
        conn.writer
            .flush()
            .map_err(|e| format!("Failed to flush socket: {e}"))?;

        let mut line = String::new();
        conn.reader
            .read_line(&mut line)
            .map_err(|e| format!("Failed to read from socket: {e}"))?;

        if line.is_empty() {
            guard.take();
            return Err("Socket closed".into());
        }

        return parse_terminal_response(&line);
    }

    // Manual mode: send through stdin/stdout
    let stdin = Arc::clone(&state.stdin);
    let stdout = Arc::clone(&state.stdout);

    tauri::async_runtime::spawn_blocking(move || {
        let request_id = TERMINAL_REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let timeout_ms = timeout.unwrap_or(120) as u64 * 1000;
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": format!("term-{request_id}"),
            "method": "tools/call",
            "params": {
                "name": "Bash",
                "arguments": {
                    "command": command,
                    "timeout": timeout_ms,
                }
            }
        });

        let request_str = request.to_string();

        let mut stdin_guard = stdin.lock().map_err(|e| e.to_string())?;
        let stdin_handle = stdin_guard
            .as_mut()
            .ok_or_else(|| "Sandbox process is not running".to_string())?;

        stdin_handle
            .write_all(request_str.as_bytes())
            .map_err(|e| format!("Failed to write to sandbox stdin: {e}"))?;
        stdin_handle
            .write_all(b"\n")
            .map_err(|e| format!("Failed to write newline: {e}"))?;
        stdin_handle
            .flush()
            .map_err(|e| format!("Failed to flush stdin: {e}"))?;
        drop(stdin_guard);

        let mut stdout_guard = stdout.lock().map_err(|e| e.to_string())?;
        let stdout_handle = stdout_guard
            .as_mut()
            .ok_or_else(|| "Sandbox process stdout unavailable".to_string())?;

        let mut line = String::new();
        stdout_handle
            .read_line(&mut line)
            .map_err(|e| format!("Failed to read from sandbox stdout: {e}"))?;

        if line.is_empty() {
            return Err("Sandbox process closed stdout".into());
        }

        parse_terminal_response(&line)
    })
    .await
    .map_err(|e| format!("Task failed: {e}"))?
}

/// Parse a JSON-RPC response from the Bash tool into a TerminalOutput.
fn parse_terminal_response(response: &str) -> Result<TerminalOutput, String> {
    let parsed: serde_json::Value =
        serde_json::from_str(response).map_err(|e| format!("Invalid JSON response: {e}"))?;

    if let Some(error) = parsed.get("error") {
        let message = error
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("Unknown error");
        return Ok(TerminalOutput {
            exit_code: None,
            output: message.to_string(),
            status: "error".into(),
        });
    }

    let content = parsed
        .pointer("/result/content")
        .and_then(|c| c.as_array());

    let text = content
        .and_then(|arr| arr.first())
        .and_then(|item| item.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("");

    if let Ok(result) = serde_json::from_str::<serde_json::Value>(text) {
        let exit_code = result
            .get("exit_code")
            .and_then(|c| c.as_i64())
            .map(|c| c as i32);
        let output = result
            .get("output")
            .and_then(|o| o.as_str())
            .unwrap_or("")
            .to_string();
        let status = if result
            .get("timed_out")
            .and_then(|t| t.as_bool())
            .unwrap_or(false)
        {
            "timeout"
        } else {
            "completed"
        };
        Ok(TerminalOutput {
            exit_code,
            output,
            status: status.into(),
        })
    } else {
        Ok(TerminalOutput {
            exit_code: None,
            output: text.to_string(),
            status: "completed".into(),
        })
    }
}
