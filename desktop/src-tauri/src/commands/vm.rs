use crate::config::SandboxConfig;
use crate::paths;
use serde::Serialize;
use std::io::{BufRead, BufReader, Write};
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

/// Shared state holding the sandbox child process and its I/O handles.
pub struct VmState {
    pub process: Mutex<Option<Child>>,
    pub stdin: Mutex<Option<std::process::ChildStdin>>,
    pub stdout: Mutex<Option<BufReader<std::process::ChildStdout>>>,
    pub debug_log: Arc<Mutex<Vec<DebugLogLine>>>,
    pub stderr_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Default for VmState {
    fn default() -> Self {
        Self {
            process: Mutex::new(None),
            stdin: Mutex::new(None),
            stdout: Mutex::new(None),
            debug_log: Arc::new(Mutex::new(Vec::new())),
            stderr_handle: Mutex::new(None),
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
            // Installed app: guest/ next to the executable
            candidate_dirs.push(exe_dir.join("guest"));

            // Dev mode: exe is in target/debug/, guest images in target/guest/x86_64/
            if let Some(target_dir) = exe_dir.parent() {
                let arch = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" };
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

/// Build CLI args from config for the sandbox binary.
fn build_sandbox_args(config: &SandboxConfig, kernel_path: &str, initrd_path: &str) -> Vec<String> {
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
    let sandbox_name = if cfg!(windows) { "sandbox.exe" } else { "sandbox" };

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // Tauri sidecar binary (target triple suffix added by bundler)
            let sidecar_name = if cfg!(windows) {
                format!("sandbox-{}.exe", env!("TARGET_TRIPLE"))
            } else {
                format!("sandbox-{}", env!("TARGET_TRIPLE"))
            };
            let candidate = dir.join(&sidecar_name);
            if candidate.exists() {
                return Ok(candidate.to_string_lossy().into_owned());
            }

            // Plain name (dev mode or manual placement)
            let candidate = dir.join(sandbox_name);
            if candidate.exists() {
                return Ok(candidate.to_string_lossy().into_owned());
            }
        }

        // Development fallback: workspace target directory
        // In dev, the Tauri exe is in target/debug/desktop.exe
        // The sandbox binary is in target/release/sandbox.exe or target/debug/sandbox.exe
        if let Ok(exe) = std::env::current_exe() {
            if let Some(target_dir) = exe.parent().and_then(|p| p.parent()) {
                for profile in &["release", "debug"] {
                    let candidate = target_dir.join(profile).join(sandbox_name);
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

/// Start the sandbox VM as a child process.
#[tauri::command]
pub fn start_vm(
    app: AppHandle,
    config: SandboxConfig,
    state: State<'_, VmState>,
) -> Result<VmStatus, String> {
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

    // Extract stdin/stdout/stderr before storing the Child
    let child_stdin = child.stdin.take();
    let child_stdout = child.stdout.take().map(BufReader::new);
    let child_stderr = child.stderr.take();

    // Write PID file for persistence
    if let Some(pid_path) = paths::pid_file_path() {
        if let Some(parent) = pid_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&pid_path, pid.to_string());
    }

    *process_guard = Some(child);
    drop(process_guard);

    // Store I/O handles separately
    if let Ok(mut guard) = state.stdin.lock() {
        *guard = child_stdin;
    }
    if let Ok(mut guard) = state.stdout.lock() {
        *guard = child_stdout;
    }

    // Spawn a background thread to capture stderr and emit events
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

    // Register MCP server in Claude Code config if integration is enabled
    super::claude::register_mcp_server(&config, &binary, &kernel_path, &initrd_path);

    Ok(VmStatus {
        state: "running".into(),
        pid: Some(pid),
        error: None,
    })
}

/// Stop the sandbox VM.
#[tauri::command]
pub fn stop_vm(state: State<'_, VmState>) -> Result<VmStatus, String> {
    // Clear I/O handles first
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

    // Clean up debug log buffer and stderr reader thread
    if let Ok(mut log) = state.debug_log.lock() {
        log.clear();
    }
    if let Ok(mut guard) = state.stderr_handle.lock() {
        // The thread exits when stderr closes (process killed)
        guard.take();
    }

    // Unregister MCP server and restore Claude's built-in tools
    super::claude::unregister_mcp_server();

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
            })
        }
        None => Ok(VmStatus {
            state: "stopped".into(),
            pid: None,
            error: None,
        }),
    }
}

/// Get the current VM status.
#[tauri::command]
pub fn get_vm_status(state: State<'_, VmState>) -> Result<VmStatus, String> {
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

                // Also clear I/O handles on process exit
                if let Ok(mut stdin_guard) = state.stdin.lock() {
                    stdin_guard.take();
                }
                if let Ok(mut stdout_guard) = state.stdout.lock() {
                    stdout_guard.take();
                }

                if let Some(pid_path) = paths::pid_file_path() {
                    let _ = std::fs::remove_file(&pid_path);
                }

                // Process exited unexpectedly — clean up MCP registration
                super::claude::unregister_mcp_server();

                Ok(VmStatus {
                    state: "stopped".into(),
                    pid: None,
                    error,
                })
            }
            Ok(None) => Ok(VmStatus {
                state: "running".into(),
                pid: Some(child.id()),
                error: None,
            }),
            Err(e) => Ok(VmStatus {
                state: "error".into(),
                pid: None,
                error: Some(format!("Failed to check process status: {e}")),
            }),
        },
        None => Ok(VmStatus {
            state: "stopped".into(),
            pid: None,
            error: None,
        }),
    }
}

/// Send a JSON-RPC line to the sandbox stdin and read one response line from stdout.
#[tauri::command]
pub fn send_mcp_request(
    request_json: String,
    state: State<'_, VmState>,
) -> Result<String, String> {
    let mut stdin_guard = state.stdin.lock().map_err(|e| e.to_string())?;
    let stdin = stdin_guard
        .as_mut()
        .ok_or_else(|| "Sandbox process is not running".to_string())?;

    // Write the JSON-RPC line
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

    // Read one response line from stdout
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
#[tauri::command]
pub fn get_debug_log(
    since_index: usize,
    state: State<'_, VmState>,
) -> Result<Vec<DebugLogLine>, String> {
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

/// Execute a shell command in the VM via the MCP execute_command tool.
#[tauri::command]
pub fn execute_terminal_command(
    command: String,
    timeout: Option<u32>,
    state: State<'_, VmState>,
) -> Result<TerminalOutput, String> {
    let request_id = TERMINAL_REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": format!("term-{request_id}"),
        "method": "tools/call",
        "params": {
            "name": "execute_command",
            "arguments": {
                "command": command,
                "timeout": timeout.unwrap_or(120),
            }
        }
    });

    let request_str = request.to_string();

    // Reuse the same stdin/stdout logic as send_mcp_request
    let mut stdin_guard = state.stdin.lock().map_err(|e| e.to_string())?;
    let stdin = stdin_guard
        .as_mut()
        .ok_or_else(|| "Sandbox process is not running".to_string())?;

    stdin
        .write_all(request_str.as_bytes())
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

    parse_terminal_response(&line)
}

/// Parse a JSON-RPC response from execute_command into a TerminalOutput.
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

    // The execute_command tool returns JSON with command_id, exit_code, output
    if let Ok(result) = serde_json::from_str::<serde_json::Value>(text) {
        let exit_code = result.get("exit_code").and_then(|c| c.as_i64()).map(|c| c as i32);
        let output = result
            .get("output")
            .and_then(|o| o.as_str())
            .unwrap_or("")
            .to_string();
        let status = if result.get("timed_out").and_then(|t| t.as_bool()).unwrap_or(false) {
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
