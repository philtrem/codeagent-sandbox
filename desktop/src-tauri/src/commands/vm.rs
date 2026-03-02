use crate::config::SandboxConfig;
use crate::paths;
use serde::Serialize;
use std::sync::Mutex;
use tauri::State;
use tokio::process::Child;

/// VM status reported to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct VmStatus {
    pub state: String,
    pub pid: Option<u32>,
    pub error: Option<String>,
}

/// Shared state holding the sandbox child process handle.
pub struct VmState {
    pub process: Mutex<Option<Child>>,
}

impl Default for VmState {
    fn default() -> Self {
        Self {
            process: Mutex::new(None),
        }
    }
}

/// Build CLI args from config for the sandbox binary.
fn build_sandbox_args(config: &SandboxConfig) -> Vec<String> {
    let mut args = Vec::new();

    if !config.sandbox.working_dir.is_empty() {
        args.push("--working-dir".into());
        args.push(config.sandbox.working_dir.clone());
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
    if !config.vm.kernel_path.is_empty() {
        args.push("--kernel-path".into());
        args.push(config.vm.kernel_path.clone());
    }
    if !config.vm.initrd_path.is_empty() {
        args.push("--initrd-path".into());
        args.push(config.vm.initrd_path.clone());
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
fn find_sandbox_binary() -> Result<String, String> {
    // 1. Check next to the current executable (bundled)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(if cfg!(windows) {
                "sandbox.exe"
            } else {
                "sandbox"
            });
            if candidate.exists() {
                return Ok(candidate.to_string_lossy().into_owned());
            }
        }
    }

    // 2. Check PATH
    if let Ok(path) = which::which("sandbox") {
        return Ok(path.to_string_lossy().into_owned());
    }

    Err("Could not find the sandbox binary. Build it with `cargo build -p codeagent-sandbox` or place it on PATH.".into())
}

/// Start the sandbox VM as a child process.
#[tauri::command]
pub async fn start_vm(
    config: SandboxConfig,
    state: State<'_, VmState>,
) -> Result<VmStatus, String> {
    let mut process_guard = state.process.lock().map_err(|e| e.to_string())?;

    if process_guard.is_some() {
        return Err("VM is already running".into());
    }

    let binary = find_sandbox_binary()?;
    let args = build_sandbox_args(&config);

    let child = tokio::process::Command::new(&binary)
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start sandbox: {e}"))?;

    let pid = child.id();

    // Write PID file for persistence
    if let Some(pid_path) = paths::pid_file_path() {
        if let Some(parent) = pid_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Some(pid) = pid {
            let _ = std::fs::write(&pid_path, pid.to_string());
        }
    }

    *process_guard = Some(child);

    Ok(VmStatus {
        state: "running".into(),
        pid,
        error: None,
    })
}

/// Stop the sandbox VM.
#[tauri::command]
pub async fn stop_vm(state: State<'_, VmState>) -> Result<VmStatus, String> {
    // Take the child out of the mutex so we can drop the guard before awaiting.
    let child = {
        let mut guard = state.process.lock().map_err(|e| e.to_string())?;
        guard.take()
    };

    match child {
        Some(mut child) => {
            let _ = child.kill().await;

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
                pid: child.id(),
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
