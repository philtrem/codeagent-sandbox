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

/// Return the total physical memory in megabytes.
#[tauri::command]
pub fn get_total_memory_mb() -> u64 {
    get_total_memory_mb_impl()
}

#[cfg(windows)]
fn get_total_memory_mb_impl() -> u64 {
    use std::mem;

    #[repr(C)]
    struct MemoryStatusEx {
        dw_length: u32,
        dw_memory_load: u32,
        ull_total_phys: u64,
        ull_avail_phys: u64,
        ull_total_page_file: u64,
        ull_avail_page_file: u64,
        ull_total_virtual: u64,
        ull_avail_virtual: u64,
        ull_avail_extended_virtual: u64,
    }

    extern "system" {
        fn GlobalMemoryStatusEx(lp_buffer: *mut MemoryStatusEx) -> i32;
    }

    unsafe {
        let mut status: MemoryStatusEx = mem::zeroed();
        status.dw_length = mem::size_of::<MemoryStatusEx>() as u32;
        if GlobalMemoryStatusEx(&mut status) != 0 {
            status.ull_total_phys / (1024 * 1024)
        } else {
            8192
        }
    }
}

#[cfg(not(windows))]
fn get_total_memory_mb_impl() -> u64 {
    // Read from /proc/meminfo on Linux, sysctl on macOS
    #[cfg(target_os = "linux")]
    {
        if let Ok(contents) = std::fs::read_to_string("/proc/meminfo") {
            for line in contents.lines() {
                if let Some(rest) = line.strip_prefix("MemTotal:") {
                    let kb_str = rest.trim().trim_end_matches("kB").trim();
                    if let Ok(kb) = kb_str.parse::<u64>() {
                        return kb / 1024;
                    }
                }
            }
        }
        8192
    }
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("sysctl")
            .arg("-n")
            .arg("hw.memsize")
            .output();
        if let Ok(output) = output {
            let s = String::from_utf8_lossy(&output.stdout);
            if let Ok(bytes) = s.trim().parse::<u64>() {
                return bytes / (1024 * 1024);
            }
        }
        8192
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        8192
    }
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
    use std::ffi::CStr;
    use std::mem;
    use std::os::raw::c_char;

    const TH32CS_SNAPPROCESS: u32 = 0x00000002;
    const MAX_PATH: usize = 260;

    #[repr(C)]
    struct ProcessEntry32 {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [c_char; MAX_PATH],
    }

    extern "system" {
        fn CreateToolhelp32Snapshot(dw_flags: u32, th32_process_id: u32) -> isize;
        fn Process32First(h_snapshot: isize, lppe: *mut ProcessEntry32) -> i32;
        fn Process32Next(h_snapshot: isize, lppe: *mut ProcessEntry32) -> i32;
        fn CloseHandle(h_object: isize) -> i32;
    }

    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == -1 {
            return Vec::new();
        }

        let mut entry: ProcessEntry32 = mem::zeroed();
        entry.dw_size = mem::size_of::<ProcessEntry32>() as u32;

        let mut pids = Vec::new();

        if Process32First(snap, &mut entry) != 0 {
            loop {
                let exe = CStr::from_ptr(entry.sz_exe_file.as_ptr()).to_string_lossy();
                if exe.eq_ignore_ascii_case("sandbox.exe") {
                    pids.push(entry.th32_process_id);
                }
                if Process32Next(snap, &mut entry) == 0 {
                    break;
                }
            }
        }

        CloseHandle(snap);
        pids
    }
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
            kill_process_windows(*pid);
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

#[cfg(windows)]
pub(super) fn kill_process_windows(pid: u32) {
    const PROCESS_TERMINATE: u32 = 0x0001;

    extern "system" {
        fn OpenProcess(dw_desired_access: u32, b_inherit_handle: i32, dw_process_id: u32) -> isize;
        fn TerminateProcess(h_process: isize, u_exit_code: u32) -> i32;
        fn CloseHandle(h_object: isize) -> i32;
    }

    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if handle != 0 {
            TerminateProcess(handle, 1);
            CloseHandle(handle);
        }
    }
}
