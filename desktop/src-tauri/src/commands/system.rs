/// Return the current platform: "windows", "macos", or "linux".
#[tauri::command]
pub fn get_platform() -> String {
    if cfg!(target_os = "windows") {
        "windows".into()
    } else if cfg!(target_os = "macos") {
        "macos".into()
    } else {
        "linux".into()
    }
}

/// Check whether the given path is an existing directory.
#[tauri::command]
pub fn validate_directory(path: String) -> bool {
    std::path::Path::new(&path).is_dir()
}

/// Create a directory (and parents) if it doesn't exist. Returns true on success.
#[tauri::command]
pub fn ensure_directory(path: String) -> Result<(), String> {
    std::fs::create_dir_all(&path)
        .map_err(|e| format!("Failed to create directory: {e}"))
}

/// Check whether the undo directory overlaps with any working directory.
/// Returns `None` if no overlap, or an error message if they overlap.
#[tauri::command]
pub fn validate_paths_overlap(working_dirs: Vec<String>, undo_dir: String) -> Option<String> {
    let undo_path = std::path::Path::new(&undo_dir);
    let canonical_undo = match std::fs::canonicalize(undo_path) {
        Ok(p) => p,
        Err(_) => return None,
    };

    for dir in &working_dirs {
        if dir.is_empty() {
            continue;
        }
        let working_path = std::path::Path::new(dir);
        let canonical_working = match std::fs::canonicalize(working_path) {
            Ok(p) => p,
            Err(_) => continue,
        };

        if canonical_undo.starts_with(&canonical_working) {
            return Some(format!(
                "Undo directory is inside working directory \"{}\"",
                dir
            ));
        }
        if canonical_working.starts_with(&canonical_undo) {
            return Some(format!(
                "Working directory \"{}\" is inside undo directory",
                dir
            ));
        }
    }

    None
}

/// Return a default undo directory path, creating it if it doesn't exist.
#[tauri::command]
pub fn get_default_undo_dir() -> Result<String, String> {
    let base = dirs::data_local_dir()
        .ok_or_else(|| "Could not determine local data directory".to_string())?;
    let undo_dir = base.join("CodeAgent").join("undo");
    std::fs::create_dir_all(&undo_dir)
        .map_err(|e| format!("Failed to create undo directory: {e}"))?;
    Ok(undo_dir.to_string_lossy().into_owned())
}

/// Return the number of logical CPU cores on the host.
#[tauri::command]
pub fn get_cpu_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

/// Resolve the sandbox binary path for use in MCP config entries.
///
/// Delegates to the same resolution logic used by the VM launcher.
#[tauri::command]
pub fn resolve_sandbox_binary() -> Result<String, String> {
    super::vm::find_sandbox_binary()
}

/// Return the platform-specific socket path for side-channel sandbox communication.
#[tauri::command]
pub fn get_socket_path() -> Result<String, String> {
    crate::paths::socket_path()
        .map(|p| p.to_string_lossy().into_owned())
        .ok_or_else(|| "Could not determine socket path".into())
}

/// Return the platform-specific log file path for sandbox stderr output.
#[tauri::command]
pub fn get_log_file_path() -> Result<String, String> {
    crate::paths::log_file_path()
        .map(|p| p.to_string_lossy().into_owned())
        .ok_or_else(|| "Could not determine log file path".into())
}

/// Resolve a binary name to its full path via the system PATH.
#[tauri::command]
pub fn resolve_binary(name: String) -> Result<Option<String>, String> {
    match which::which(&name) {
        Ok(path) => Ok(Some(path.to_string_lossy().into_owned())),
        Err(which::Error::CannotFindBinaryPath) => Ok(None),
        Err(e) => Err(format!("Failed to resolve binary '{name}': {e}")),
    }
}

/// Find running sandbox processes and return their PIDs.
#[tauri::command]
pub fn find_sandbox_processes() -> Vec<u32> {
    #[cfg(windows)]
    {
        find_sandbox_processes_windows()
    }
    #[cfg(not(windows))]
    {
        find_sandbox_processes_unix()
    }
}

#[cfg(windows)]
fn find_sandbox_processes_windows() -> Vec<u32> {
    let output = std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq sandbox.exe", "/FO", "CSV", "/NH"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output();

    let Ok(output) = output else {
        return Vec::new();
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut pids = Vec::new();

    for line in stdout.lines() {
        // CSV format: "sandbox.exe","1234","Console","1","12,345 K"
        let fields: Vec<&str> = line.split(',').collect();
        if fields.len() >= 2 {
            let pid_field = fields[1].trim().trim_matches('"');
            if let Ok(pid) = pid_field.parse::<u32>() {
                pids.push(pid);
            }
        }
    }

    pids
}

#[cfg(not(windows))]
fn find_sandbox_processes_unix() -> Vec<u32> {
    let output = std::process::Command::new("pgrep")
        .args(["-x", "sandbox"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output();

    let Ok(output) = output else {
        return Vec::new();
    };

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse::<u32>().ok())
        .collect()
}

/// Kill sandbox processes by PID.
#[tauri::command]
pub fn kill_sandbox_processes(pids: Vec<u32>) -> Result<(), String> {
    for pid in &pids {
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
                libc::kill(*pid as i32, libc::SIGTERM);
            }
        }
    }

    #[cfg(not(windows))]
    {
        std::thread::sleep(std::time::Duration::from_millis(500));
        for pid in &pids {
            unsafe {
                libc::kill(*pid as i32, libc::SIGKILL);
            }
        }
    }

    Ok(())
}
