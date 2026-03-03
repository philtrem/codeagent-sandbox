use crate::config::SandboxConfig;
use crate::paths;
use serde::Serialize;
use std::io::{BufRead, BufReader, Write};
use std::process::Child;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, State};

/// VM status reported to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct VmStatus {
    pub state: String,
    pub pid: Option<u32>,
    pub error: Option<String>,
}

/// Shared state holding the sandbox child process and its I/O handles.
pub struct VmState {
    pub process: Mutex<Option<Child>>,
    pub stdin: Mutex<Option<std::process::ChildStdin>>,
    pub stdout: Mutex<Option<BufReader<std::process::ChildStdout>>>,
}

impl Default for VmState {
    fn default() -> Self {
        Self {
            process: Mutex::new(None),
            stdin: Mutex::new(None),
            stdout: Mutex::new(None),
        }
    }
}

/// Resolve guest image paths: prefer user config, fall back to bundled resources.
fn resolve_guest_images(app: &AppHandle, config: &SandboxConfig) -> (String, String) {
    let mut kernel = config.vm.kernel_path.clone();
    let mut initrd = config.vm.initrd_path.clone();

    if kernel.is_empty() || initrd.is_empty() {
        if let Ok(resource_dir) = app.path().resource_dir() {
            let guest_dir = resource_dir.join("guest");
            if kernel.is_empty() {
                let bundled = guest_dir.join("vmlinuz");
                if bundled.exists() {
                    kernel = bundled.to_string_lossy().into_owned();
                }
            }
            if initrd.is_empty() {
                let bundled = guest_dir.join("initrd.img");
                if bundled.exists() {
                    initrd = bundled.to_string_lossy().into_owned();
                }
            }
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

    let mut child = std::process::Command::new(&binary)
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start sandbox: {e}"))?;

    let pid = child.id();

    // Extract stdin/stdout before storing the Child
    let child_stdin = child.stdin.take();
    let child_stdout = child.stdout.take().map(BufReader::new);

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
