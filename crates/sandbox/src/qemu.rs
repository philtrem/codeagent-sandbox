use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::Duration;

use crate::error::AgentError;

/// Timeout for waiting for the control socket to appear after QEMU starts.
#[cfg(not(target_os = "windows"))]
const CONTROL_SOCKET_TIMEOUT: Duration = Duration::from_secs(30);

/// Polling interval while waiting for the control socket.
#[cfg(not(target_os = "windows"))]
const SOCKET_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Timeout for graceful QEMU shutdown before sending SIGKILL.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum length for virtiofs tags and virtio-serial port names.
const MAX_MOUNT_NAME_LEN: usize = 36;

/// Length of the blake3 hash suffix used when truncating long names.
const HASH_SUFFIX_LEN: usize = 7;

/// Sanitize a single path component into a mount-safe name.
///
/// Lowercases, replaces non-alphanumeric characters with `-`, collapses
/// consecutive dashes, and strips leading/trailing dashes.
fn sanitize_path_component(component: &str) -> String {
    let lowered = component.to_lowercase();
    let replaced: String = lowered
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let collapsed = collapse_dashes(&replaced);
    collapsed.trim_matches('-').to_string()
}

/// Collapse consecutive dashes into a single dash.
fn collapse_dashes(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut prev_dash = false;
    for c in s.chars() {
        if c == '-' {
            if !prev_dash {
                result.push('-');
            }
            prev_dash = true;
        } else {
            result.push(c);
            prev_dash = false;
        }
    }
    result
}

/// Extract the sanitized parent directory component for disambiguation.
fn extract_parent_component(path: &Path) -> String {
    path.parent()
        .and_then(|p| p.file_name())
        .map(|n| sanitize_path_component(&n.to_string_lossy()))
        .unwrap_or_default()
}

/// Truncate a name to fit within [`MAX_MOUNT_NAME_LEN`] by appending a blake3 hash suffix.
///
/// If the name is already short enough, returns it unchanged.
/// Otherwise: `name[..28]-<7-char hash>`.
fn truncate_if_needed(name: &str, full_path: &Path) -> String {
    if name.len() <= MAX_MOUNT_NAME_LEN {
        return name.to_string();
    }
    let hash = blake3::hash(full_path.to_string_lossy().as_bytes());
    let hash_str = &hash.to_hex()[..HASH_SUFFIX_LEN];
    let prefix_len = MAX_MOUNT_NAME_LEN - 1 - HASH_SUFFIX_LEN; // 36 - 1 - 7 = 28
    let prefix = &name[..prefix_len];
    let prefix = prefix.trim_end_matches('-');
    format!("{prefix}-{hash_str}")
}

/// Generate self-documenting mount names from a list of working directory paths.
///
/// Each name is derived from the directory's basename:
/// 1. Sanitize (lowercase, non-alnum → `-`, collapse dashes, strip edges)
/// 2. On collision: prepend parent dir (`parent-dir`)
/// 3. If still colliding: append numeric suffix (`-2`, `-3`, ...)
/// 4. Truncate to 36 chars if needed (28-char prefix + `-` + 7-char blake3 hash)
/// 5. Empty name fallback: 7-char blake3 hash of the full path
pub fn generate_mount_names(dirs: &[PathBuf]) -> Vec<String> {
    let mut names: Vec<String> = Vec::with_capacity(dirs.len());

    // Phase 1: Generate base names
    for dir in dirs {
        let component = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let sanitized = sanitize_path_component(&component);
        let name = if sanitized.is_empty() {
            let hash = blake3::hash(dir.to_string_lossy().as_bytes());
            hash.to_hex()[..HASH_SUFFIX_LEN].to_string()
        } else {
            sanitized
        };
        names.push(name);
    }

    // Phase 2: Resolve collisions with parent prefix
    let collisions = find_collisions(&names);
    for &idx in &collisions {
        let parent = extract_parent_component(&dirs[idx]);
        if !parent.is_empty() {
            let prefixed = format!("{parent}-{}", names[idx]);
            names[idx] = prefixed;
        }
    }

    // Phase 3: Resolve remaining collisions with numeric suffix
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut final_names: Vec<String> = Vec::with_capacity(dirs.len());
    for (i, name) in names.iter().enumerate() {
        let count = counts.entry(name.clone()).or_insert(0);
        *count += 1;
        let unique = if *count > 1 {
            format!("{name}-{count}")
        } else {
            // Check if this name appears more than once total
            let total = names.iter().filter(|n| *n == name).count();
            if total > 1 {
                format!("{name}-1")
            } else {
                name.clone()
            }
        };
        final_names.push(truncate_if_needed(&unique, &dirs[i]));
    }

    final_names
}

/// Find indices of names that appear more than once.
fn find_collisions(names: &[String]) -> Vec<usize> {
    let mut counts: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, name) in names.iter().enumerate() {
        counts.entry(name.as_str()).or_default().push(i);
    }
    let mut colliding = Vec::new();
    for indices in counts.values() {
        if indices.len() > 1 {
            colliding.extend(indices);
        }
    }
    colliding.sort();
    colliding
}

/// Configuration for launching a QEMU virtual machine.
#[derive(Debug, Clone)]
pub struct QemuConfig {
    /// Path to the QEMU binary. If None, auto-detected by platform.
    pub qemu_binary: Option<PathBuf>,

    /// Path to the guest kernel image.
    pub kernel_path: PathBuf,

    /// Path to the guest initrd image.
    pub initrd_path: PathBuf,

    /// Path to the guest rootfs image (optional, used for `-drive`).
    pub rootfs_path: Option<PathBuf>,

    /// VM memory in megabytes.
    pub memory_mb: u32,

    /// Number of virtual CPUs.
    pub cpus: u32,

    /// Working directory paths to share with the guest.
    pub working_dirs: Vec<PathBuf>,

    /// Path for the control channel socket (host-side).
    pub control_socket_path: PathBuf,

    /// Paths for filesystem sockets (one per working dir, host-side).
    pub fs_socket_paths: Vec<PathBuf>,

    /// VM lifecycle mode ("ephemeral" or "persistent").
    pub vm_mode: String,

    /// Sanitized mount names for each working directory (same length as `working_dirs`).
    /// Used as virtiofs tags (Unix) and virtio-serial port names (Windows).
    pub mount_names: Vec<String>,

    /// Path to a tools disk image to attach as a read-only virtio drive.
    pub tools_image_path: Option<PathBuf>,

    /// Extra QEMU command-line arguments.
    pub extra_args: Vec<String>,
}

impl QemuConfig {
    /// Build the QEMU command-line arguments.
    ///
    /// Returns `(binary_path, args)`. The arguments are platform-specific:
    /// - Linux: q35 + KVM + memory-backend-memfd + vhost-user-fs-pci
    /// - macOS: virt + HVF + memory-backend-shm + vhost-user-fs-pci
    /// - Windows: q35 + WHPX + 9P over virtio-serial — no shared memory needed
    pub fn build_args(&self) -> Result<(PathBuf, Vec<OsString>), AgentError> {
        let binary = self.resolve_qemu_binary()?;
        let mut args: Vec<OsString> = Vec::new();
        let mut extra_kernel_params: Vec<String> = Vec::new();

        self.add_platform_args(&mut args);
        self.add_common_args(&mut args);
        self.add_filesystem_args(&mut args, &mut extra_kernel_params);
        self.add_control_channel_args(&mut args);
        self.add_tools_drive_args(&mut args, &mut extra_kernel_params);
        self.add_boot_args(&mut args, &extra_kernel_params);
        self.add_extra_args(&mut args);

        Ok((binary, args))
    }

    /// Platform-specific machine type, accelerator, and memory backend.
    fn add_platform_args(&self, args: &mut Vec<OsString>) {
        #[cfg(target_os = "linux")]
        {
            args.extend(["-machine".into(), "q35".into()]);
            args.extend(["-cpu".into(), "host".into(), "-accel".into(), "kvm".into()]);
            args.extend([
                "-object".into(),
                format!(
                    "memory-backend-memfd,id=mem,size={}M,share=on",
                    self.memory_mb
                )
                .into(),
            ]);
            args.extend(["-numa".into(), "node,memdev=mem".into()]);
        }

        #[cfg(target_os = "macos")]
        {
            args.extend(["-machine".into(), "virt".into()]);
            args.extend(["-cpu".into(), "host".into(), "-accel".into(), "hvf".into()]);
            args.extend([
                "-object".into(),
                format!(
                    "memory-backend-shm,id=mem,size={}M,share=on",
                    self.memory_mb
                )
                .into(),
            ]);
            args.extend(["-numa".into(), "node,memdev=mem".into()]);
        }

        #[cfg(target_os = "windows")]
        {
            // Try WHPX (hardware virtualization) first, fall back to TCG
            // (software emulation) if WHPX is unavailable. Use `-cpu qemu64`
            // instead of `-cpu max` because WHPX cannot handle certain advanced
            // CPU features exposed by `max` (causes "Unexpected VP exit code 4").
            args.extend(["-machine".into(), "q35,accel=whpx:tcg".into()]);
            args.extend(["-cpu".into(), "qemu64".into()]);
        }
    }

    /// Common arguments: memory, CPUs, network, display, virtio-serial bus.
    fn add_common_args(&self, args: &mut Vec<OsString>) {
        args.extend(["-m".into(), format!("{}M", self.memory_mb).into()]);
        args.extend(["-smp".into(), self.cpus.to_string().into()]);
        args.extend(["-netdev".into(), "user,id=net0".into()]);
        args.extend(["-device".into(), "virtio-net-pci,netdev=net0".into()]);
        args.push("-nographic".into());

        // On Windows, both filesystem (virtserialport) and control channel
        // (virtserialport) devices need the virtio-serial-pci bus. Add it
        // here so it's available before any port devices are added.
        #[cfg(target_os = "windows")]
        {
            args.extend(["-device".into(), "virtio-serial-pci".into()]);
            // Disable MSI-X for all virtio PCI devices to work around WHPX
            // MSI injection failures. Falls back to legacy INTx interrupts.
            args.extend(["-global".into(), "virtio-pci.vectors=0".into()]);
        }
    }

    /// Filesystem sharing devices (virtiofs on Linux/macOS, 9P on Windows).
    ///
    /// On Linux/macOS: uses vhost-user-fs-pci with a socket chardev for
    /// each working directory (connects to virtiofsd).
    ///
    /// On Windows: uses a virtio-serial chardev connecting to the host-side
    /// P9Backend TCP listener, with a virtserialport device named `p9fsN`.
    /// Inside the guest, the p9proxy binary bridges the serial port to a
    /// Unix socketpair for the kernel's 9P `trans=fd` transport.
    fn add_filesystem_args(
        &self,
        args: &mut Vec<OsString>,
        extra_kernel_params: &mut Vec<String>,
    ) {
        for (index, socket_path) in self.fs_socket_paths.iter().enumerate() {
            let mount_name = &self.mount_names[index];

            #[cfg(not(target_os = "windows"))]
            {
                let chardev_id = format!("vfs{index}");
                args.extend([
                    "-chardev".into(),
                    format!("socket,id={chardev_id},path={}", socket_path.display()).into(),
                ]);
                args.extend([
                    "-device".into(),
                    format!("vhost-user-fs-pci,chardev={chardev_id},tag={mount_name}").into(),
                ]);
            }

            #[cfg(target_os = "windows")]
            {
                let addr = std::fs::read_to_string(socket_path).unwrap_or_default();
                let addr = addr.trim().to_string();
                let (host, port) = addr.rsplit_once(':').unwrap_or((&addr, "0"));
                let chardev_id = format!("p9fs{index}");

                args.extend([
                    "-chardev".into(),
                    format!("socket,id={chardev_id},host={host},port={port},server=off").into(),
                ]);
                args.extend([
                    "-device".into(),
                    format!("virtserialport,chardev={chardev_id},name={mount_name}").into(),
                ]);
            }
        }

        // Pass mount names to guest via kernel cmdline
        if !self.mount_names.is_empty() {
            extra_kernel_params.push(format!("mount_names={}", self.mount_names.join(",")));
        }
    }

    /// Control channel: virtio-serial device connected via a chardev socket.
    ///
    /// On Unix: QEMU creates a Unix domain socket (server mode).
    /// On Windows: QEMU connects to a host-side TCP listener (client mode).
    /// The TCP address is read from the file at `control_socket_path`.
    fn add_control_channel_args(&self, args: &mut Vec<OsString>) {
        #[cfg(not(target_os = "windows"))]
        {
            args.extend([
                "-chardev".into(),
                format!(
                    "socket,id=ctrl,path={},server=on,wait=off",
                    self.control_socket_path.display()
                )
                .into(),
            ]);
        }

        #[cfg(target_os = "windows")]
        {
            // QEMU requires host and port as separate parameters:
            //   socket,id=ctrl,host=IP,port=PORT,server=off
            let addr = std::fs::read_to_string(&self.control_socket_path)
                .unwrap_or_default();
            let addr = addr.trim();
            let (host, port) = addr.rsplit_once(':').unwrap_or((addr, "0"));
            args.extend([
                "-chardev".into(),
                format!("socket,id=ctrl,host={host},port={port},server=off").into(),
            ]);
        }

        // On Unix, the virtio-serial-pci bus is only needed for the control
        // channel. On Windows it's added in add_common_args() since filesystem
        // ports also use it.
        #[cfg(not(target_os = "windows"))]
        args.extend(["-device".into(), "virtio-serial-pci".into()]);

        args.extend([
            "-device".into(),
            "virtserialport,chardev=ctrl,name=control".into(),
        ]);
    }

    /// Tools disk image: attached as a read-only virtio drive.
    fn add_tools_drive_args(
        &self,
        args: &mut Vec<OsString>,
        extra_kernel_params: &mut Vec<String>,
    ) {
        if let Some(path) = &self.tools_image_path {
            args.extend([
                "-drive".into(),
                format!("file={},format=raw,if=virtio,readonly=on", path.display()).into(),
            ]);
            extra_kernel_params.push("tools_image=1".to_string());
        }
    }

    /// Kernel, initrd, and rootfs boot arguments.
    fn add_boot_args(&self, args: &mut Vec<OsString>, extra_kernel_params: &[String]) {
        args.extend([
            "-kernel".into(),
            self.kernel_path.as_os_str().to_owned(),
        ]);
        args.extend([
            "-initrd".into(),
            self.initrd_path.as_os_str().to_owned(),
        ]);
        // On Windows, add ttyS0 alongside hvc0 so boot messages appear on
        // the serial port (captured via -nographic → stdout) for debugging,
        // in case the virtio console (hvc0) is not yet functional.
        #[cfg(target_os = "windows")]
        let mut append = if self.rootfs_path.is_some() {
            "console=ttyS0 console=hvc0 root=/dev/vda".to_string()
        } else {
            "console=ttyS0 console=hvc0".to_string()
        };
        #[cfg(not(target_os = "windows"))]
        let mut append = if self.rootfs_path.is_some() {
            "console=hvc0 root=/dev/vda".to_string()
        } else {
            "console=hvc0".to_string()
        };
        for param in extra_kernel_params {
            append.push(' ');
            append.push_str(param);
        }
        args.extend(["-append".into(), append.into()]);

        if let Some(rootfs) = &self.rootfs_path {
            args.extend([
                "-drive".into(),
                format!("file={},format=raw,if=virtio", rootfs.display()).into(),
            ]);
        }
    }

    /// User-provided extra arguments.
    fn add_extra_args(&self, args: &mut Vec<OsString>) {
        for arg in &self.extra_args {
            args.push(arg.into());
        }
    }

    /// Resolve the QEMU binary path from the override, PATH, or common install locations.
    fn resolve_qemu_binary(&self) -> Result<PathBuf, AgentError> {
        if let Some(path) = &self.qemu_binary {
            return Ok(path.clone());
        }

        let default_name = default_qemu_binary_name();

        // Try PATH first
        if let Ok(path) = which::which(default_name) {
            return Ok(path);
        }

        // Check common installation paths
        for candidate in common_qemu_paths(default_name) {
            if candidate.exists() {
                return Ok(candidate);
            }
        }

        Err(AgentError::QemuUnavailable)
    }
}

/// Returns the default QEMU binary name for the current platform.
fn default_qemu_binary_name() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "qemu-system-aarch64"
    }
    #[cfg(not(target_os = "macos"))]
    {
        "qemu-system-x86_64"
    }
}

/// Returns common QEMU installation paths to check when PATH lookup fails.
fn common_qemu_paths(binary_name: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    #[cfg(target_os = "windows")]
    {
        paths.push(PathBuf::from(format!("C:\\Program Files\\qemu\\{binary_name}.exe")));
        paths.push(PathBuf::from(format!("C:\\Program Files (x86)\\qemu\\{binary_name}.exe")));
        if let Ok(scoop) = std::env::var("SCOOP") {
            paths.push(PathBuf::from(format!("{scoop}\\shims\\{binary_name}.exe")));
        }
    }

    #[cfg(target_os = "macos")]
    {
        paths.push(PathBuf::from(format!("/opt/homebrew/bin/{binary_name}")));
        paths.push(PathBuf::from(format!("/usr/local/bin/{binary_name}")));
    }

    #[cfg(target_os = "linux")]
    {
        paths.push(PathBuf::from(format!("/usr/bin/{binary_name}")));
        paths.push(PathBuf::from(format!("/usr/local/bin/{binary_name}")));
    }

    paths
}

/// Handle to a running QEMU process.
pub struct QemuProcess {
    config: QemuConfig,
    child: Child,
    /// On Windows, a job object that kills QEMU when the sandbox exits.
    /// Kept alive for the lifetime of QemuProcess; closing it kills the job.
    #[cfg(target_os = "windows")]
    _job: Option<OwnedHandle>,
}

/// Wrapper for a Windows HANDLE that closes it on drop.
#[cfg(target_os = "windows")]
struct OwnedHandle(windows_sys::Win32::Foundation::HANDLE);

#[cfg(target_os = "windows")]
unsafe impl Send for OwnedHandle {}

#[cfg(target_os = "windows")]
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

/// Assign a child process to a Windows job object with
/// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. This ensures QEMU is killed
/// if the sandbox process exits (even if killed abruptly).
#[cfg(target_os = "windows")]
fn create_kill_on_close_job(child: &Child) -> Option<OwnedHandle> {
    use std::mem::zeroed;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::*;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_ALL_ACCESS};

    unsafe {
        let job: HANDLE = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return None;
        }

        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

        let ok = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if ok == 0 {
            CloseHandle(job);
            return None;
        }

        let process_handle = OpenProcess(PROCESS_ALL_ACCESS, 0, child.id());
        if process_handle.is_null() {
            CloseHandle(job);
            return None;
        }

        let assigned = AssignProcessToJobObject(job, process_handle);
        CloseHandle(process_handle);

        if assigned == 0 {
            CloseHandle(job);
            return None;
        }

        Some(OwnedHandle(job))
    }
}

impl QemuProcess {
    /// Spawn a QEMU VM.
    ///
    /// Builds the platform-specific command line, spawns the process,
    /// and waits for readiness. On Unix, waits for the control socket
    /// file to appear. On Windows, verifies QEMU hasn't exited early
    /// (the orchestrator handles TCP connection acceptance separately).
    pub fn spawn(config: QemuConfig) -> Result<Self, AgentError> {
        let (binary, args) = config.build_args()?;

        // On Windows, pipe stdout to capture serial console output
        // (-nographic maps the serial port to stdout). On other platforms
        // the virtio console (hvc0) is used and serial output is not needed.
        let stdout_mode = if cfg!(target_os = "windows") {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        };

        let mut command = std::process::Command::new(&binary);
        command
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(stdout_mode)
            .stderr(std::process::Stdio::piped());

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x00004000;
            command.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
        }

        let child = command.spawn().map_err(|error| AgentError::QemuSpawnFailed {
            reason: format!(
                "failed to start {}: {error}",
                binary.display()
            ),
        })?;

        #[cfg(target_os = "windows")]
        let _job = {
            let job = create_kill_on_close_job(&child);
            if job.is_some() {
                eprintln!("[sandbox] QEMU PID {} assigned to kill-on-close job object", child.id());
            } else {
                eprintln!("[sandbox] WARNING: failed to create kill-on-close job object for QEMU PID {}", child.id());
            }
            job
        };

        let mut process = Self {
            config,
            child,
            #[cfg(target_os = "windows")]
            _job,
        };

        process.wait_for_ready()?;

        Ok(process)
    }

    /// Stop the QEMU VM.
    ///
    /// Kills the child process and waits for it to exit.
    pub fn stop(&mut self) -> Result<(), AgentError> {
        let _ = self.child.kill();

        // Wait for the process to exit with a timeout
        let start = std::time::Instant::now();
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if start.elapsed() > SHUTDOWN_TIMEOUT {
                        // Already killed above, just break
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => break,
            }
        }

        // Clean up socket files
        let _ = std::fs::remove_file(&self.config.control_socket_path);
        for socket in &self.config.fs_socket_paths {
            let _ = std::fs::remove_file(socket);
        }

        Ok(())
    }

    /// Returns the process ID of the QEMU process.
    pub fn pid(&self) -> Option<u32> {
        Some(self.child.id())
    }

    /// Wait for the VM to be ready after spawn.
    ///
    /// On Unix: waits for the control socket file to appear (QEMU creates it
    /// in server mode).
    ///
    /// On Windows: the control socket address file is written by the
    /// orchestrator BEFORE QEMU starts, so file existence is meaningless.
    /// Instead, we briefly verify QEMU hasn't crashed on startup (e.g.,
    /// due to invalid arguments). The actual connection is handled by the
    /// orchestrator's TCP accept loop with its own timeout.
    fn wait_for_ready(&mut self) -> Result<(), AgentError> {
        #[cfg(not(target_os = "windows"))]
        {
            let start = std::time::Instant::now();
            while start.elapsed() < CONTROL_SOCKET_TIMEOUT {
                if self.config.control_socket_path.exists() {
                    return Ok(());
                }
                std::thread::sleep(SOCKET_POLL_INTERVAL);
            }
            return Err(AgentError::QemuSpawnFailed {
                reason: format!(
                    "control socket {} did not appear within {}s",
                    self.config.control_socket_path.display(),
                    CONTROL_SOCKET_TIMEOUT.as_secs()
                ),
            });
        }

        #[cfg(target_os = "windows")]
        {
            // Spawn threads to drain QEMU's stdout (serial console) and
            // stderr (QEMU diagnostics) for debugging.
            if let Some(stdout) = self.child.stdout.take() {
                std::thread::spawn(move || {
                    use std::io::{BufRead, BufReader};
                    let reader = BufReader::new(stdout);
                    for line in reader.lines() {
                        match line {
                            Ok(line) => eprintln!("[qemu serial] {line}"),
                            Err(_) => break,
                        }
                    }
                });
            }
            if let Some(stderr) = self.child.stderr.take() {
                std::thread::spawn(move || {
                    use std::io::{BufRead, BufReader};
                    let reader = BufReader::new(stderr);
                    for line in reader.lines() {
                        match line {
                            Ok(line) => eprintln!("[qemu stderr] {line}"),
                            Err(_) => break,
                        }
                    }
                });
            }

            // Give QEMU a moment to start, then check if it crashed immediately
            // (e.g., invalid arguments, missing files, accelerator failure).
            std::thread::sleep(Duration::from_millis(500));
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    let stderr_output = "(see [qemu stderr] lines above)";
                    Err(AgentError::QemuSpawnFailed {
                        reason: format!(
                            "QEMU exited immediately with {status}. stderr: {stderr_output}"
                        ),
                    })
                }
                Ok(None) => Ok(()), // Still running — good
                Err(error) => Err(AgentError::QemuSpawnFailed {
                    reason: format!("failed to check QEMU process status: {error}"),
                }),
            }
        }
    }
}

impl Drop for QemuProcess {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> QemuConfig {
        let working_dirs = vec![PathBuf::from("/tmp/work")];
        let mount_names = generate_mount_names(&working_dirs);
        QemuConfig {
            qemu_binary: Some(PathBuf::from("/usr/bin/qemu-system-x86_64")),
            kernel_path: PathBuf::from("/boot/vmlinuz"),
            initrd_path: PathBuf::from("/boot/initrd.img"),
            rootfs_path: None,
            memory_mb: 2048,
            cpus: 2,
            working_dirs,
            control_socket_path: PathBuf::from("/tmp/control.sock"),
            fs_socket_paths: vec![PathBuf::from("/tmp/vfs0.sock")],
            vm_mode: "ephemeral".to_string(),
            mount_names,
            tools_image_path: None,
            extra_args: vec![],
        }
    }

    /// Convert OsString vec to string vec for easier assertion.
    fn args_to_strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().to_string())
            .collect()
    }

    /// QC-01: build_args includes correct machine type for the current platform.
    #[test]
    fn qc_01_platform_machine_type() {
        let config = test_config();
        let (_binary, args) = config.build_args().unwrap();
        let args = args_to_strings(&args);

        assert!(args.contains(&"-machine".to_string()));

        #[cfg(target_os = "linux")]
        assert!(args.contains(&"q35".to_string()));

        #[cfg(target_os = "macos")]
        assert!(args.contains(&"virt".to_string()));

        #[cfg(target_os = "windows")]
        assert!(args.iter().any(|a| a.starts_with("q35")));
    }

    /// QC-02: build_args includes kernel and initrd paths.
    #[test]
    fn qc_02_kernel_initrd_paths() {
        let config = test_config();
        let (_binary, args) = config.build_args().unwrap();
        let args = args_to_strings(&args);

        let kernel_idx = args.iter().position(|a| a == "-kernel").unwrap();
        assert_eq!(args[kernel_idx + 1], "/boot/vmlinuz");

        let initrd_idx = args.iter().position(|a| a == "-initrd").unwrap();
        assert_eq!(args[initrd_idx + 1], "/boot/initrd.img");
    }

    /// QC-03: build_args includes control socket chardev and virtio-serial device.
    #[test]
    fn qc_03_control_socket() {
        let config = test_config();
        let (_binary, args) = config.build_args().unwrap();
        let args = args_to_strings(&args);

        let has_ctrl_chardev = args.iter().any(|a| a.contains("socket,id=ctrl"));
        assert!(has_ctrl_chardev, "missing control chardev: {args:?}");

        // Unix uses path= (QEMU is server), Windows uses host= (QEMU is client)
        #[cfg(unix)]
        {
            let has_path = args.iter().any(|a| a.contains("/tmp/control.sock"));
            assert!(has_path, "missing control socket path: {args:?}");
        }

        assert!(args.contains(&"virtio-serial-pci".to_string()));
        assert!(args.contains(&"virtserialport,chardev=ctrl,name=control".to_string()));
    }

    /// QC-04: build_args includes one filesystem entry per working dir.
    /// On Unix: chardev + device pairs. On Windows: kernel cmdline p9portN= params.
    #[test]
    fn qc_04_multiple_working_dirs() {
        let mut config = test_config();
        config.working_dirs = vec![
            PathBuf::from("/tmp/work0"),
            PathBuf::from("/tmp/work1"),
        ];
        config.mount_names = generate_mount_names(&config.working_dirs);
        config.fs_socket_paths = vec![
            PathBuf::from("/tmp/vfs0.sock"),
            PathBuf::from("/tmp/vfs1.sock"),
        ];

        let (_binary, args) = config.build_args().unwrap();
        let args = args_to_strings(&args);

        #[cfg(not(target_os = "windows"))]
        {
            // Chardev IDs remain index-based (vfs0, vfs1)
            let fs_device_count = args
                .iter()
                .filter(|a| a.contains("vfs0") || a.contains("vfs1"))
                .count();
            assert!(
                fs_device_count >= 2,
                "expected >=2 filesystem device args, got {fs_device_count}: {args:?}"
            );
            // Tags use mount names derived from directory basenames
            assert!(
                args.iter().any(|a| a.contains("tag=work0")),
                "expected virtiofs tag 'work0': {args:?}"
            );
            assert!(
                args.iter().any(|a| a.contains("tag=work1")),
                "expected virtiofs tag 'work1': {args:?}"
            );
        }

        #[cfg(target_os = "windows")]
        {
            let fs_chardev_count = args
                .iter()
                .filter(|a| a.contains("id=p9fs0") || a.contains("id=p9fs1"))
                .count();
            assert!(
                fs_chardev_count >= 2,
                "expected >=2 filesystem chardev args, got {fs_chardev_count}: {args:?}"
            );
            // Port names use mount names derived from directory basenames
            assert!(
                args.iter().any(|a| a.contains("name=work0")),
                "expected virtserialport named 'work0': {args:?}"
            );
            assert!(
                args.iter().any(|a| a.contains("name=work1")),
                "expected virtserialport named 'work1': {args:?}"
            );
        }
    }

    /// QC-05: build_args includes correct memory and CPU settings.
    #[test]
    fn qc_05_memory_and_cpus() {
        let mut config = test_config();
        config.memory_mb = 4096;
        config.cpus = 4;

        let (_binary, args) = config.build_args().unwrap();
        let args = args_to_strings(&args);

        let mem_idx = args.iter().position(|a| a == "-m").unwrap();
        assert_eq!(args[mem_idx + 1], "4096M");

        let smp_idx = args.iter().position(|a| a == "-smp").unwrap();
        assert_eq!(args[smp_idx + 1], "4");
    }

    /// QC-06: custom qemu_binary uses the override path.
    #[test]
    fn qc_06_custom_qemu_binary() {
        let mut config = test_config();
        config.qemu_binary = Some(PathBuf::from("/custom/qemu"));

        let (binary, _args) = config.build_args().unwrap();
        assert_eq!(binary, PathBuf::from("/custom/qemu"));
    }

    /// QC-07: with rootfs includes `-drive` argument.
    #[test]
    fn qc_07_with_rootfs() {
        let mut config = test_config();
        config.rootfs_path = Some(PathBuf::from("/boot/rootfs.img"));

        let (_binary, args) = config.build_args().unwrap();
        let args = args_to_strings(&args);

        let has_drive = args.iter().any(|a| {
            a.contains("file=/boot/rootfs.img") && a.contains("format=raw")
        });
        assert!(has_drive, "missing rootfs drive arg: {args:?}");
    }

    /// QC-09: append line does not include root= when rootfs_path is None,
    /// but does include mount_names= with the generated names.
    #[test]
    fn qc_09_no_root_without_rootfs() {
        let config = test_config(); // rootfs_path: None
        let (_binary, args) = config.build_args().unwrap();
        let args = args_to_strings(&args);

        let append_idx = args.iter().position(|a| a == "-append").unwrap();
        let append_val = &args[append_idx + 1];
        assert!(
            !append_val.contains("root="),
            "append should not contain root= without rootfs: {append_val}"
        );
        assert!(append_val.contains("console=hvc0"));
        assert!(
            append_val.contains("mount_names=work"),
            "append should contain mount_names=: {append_val}"
        );
    }

    /// QC-10: append line includes root=/dev/vda when rootfs_path is set.
    #[test]
    fn qc_10_root_with_rootfs() {
        let mut config = test_config();
        config.rootfs_path = Some(PathBuf::from("/boot/rootfs.img"));
        let (_binary, args) = config.build_args().unwrap();
        let args = args_to_strings(&args);

        let append_idx = args.iter().position(|a| a == "-append").unwrap();
        let append_val = &args[append_idx + 1];
        assert!(
            append_val.contains("root=/dev/vda"),
            "append should contain root=/dev/vda with rootfs: {append_val}"
        );
    }

    /// QC-08: extra_args are appended to the command line.
    #[test]
    fn qc_08_extra_args() {
        let mut config = test_config();
        config.extra_args = vec!["-nodefaults".to_string(), "-S".to_string()];

        let (_binary, args) = config.build_args().unwrap();
        let args = args_to_strings(&args);

        // Extra args should appear at the end
        assert!(args.contains(&"-nodefaults".to_string()));
        assert!(args.contains(&"-S".to_string()));
    }

    /// QC-11: Windows control chardev uses separate host/port; filesystem
    /// uses virtio-serial chardev + virtserialport connecting to the P9Backend.
    /// Port names use the generated mount name (not index-based p9fsN).
    #[cfg(target_os = "windows")]
    #[test]
    fn qc_11_windows_chardev_transport() {
        let dir = tempfile::tempdir().unwrap();

        // Write a TCP address file for the control channel
        let ctrl_addr_path = dir.path().join("control.addr");
        std::fs::write(&ctrl_addr_path, "127.0.0.1:54321").unwrap();

        // Write a TCP address file for the filesystem channel
        let fs_addr_path = dir.path().join("p9fs0.addr");
        std::fs::write(&fs_addr_path, "127.0.0.1:54322").unwrap();

        let mut config = test_config();
        config.control_socket_path = ctrl_addr_path;
        config.fs_socket_paths = vec![fs_addr_path];
        // mount_names already set by test_config() from working_dirs

        let (_binary, args) = config.build_args().unwrap();
        let args = args_to_strings(&args);

        // Control channel chardev should have host=...,port=... separately
        let ctrl_chardev = args.iter().find(|a| a.contains("id=ctrl")).unwrap();
        assert!(
            ctrl_chardev.contains("host=127.0.0.1,port=54321"),
            "expected separate host and port in control chardev: {ctrl_chardev}"
        );
        assert!(
            !ctrl_chardev.contains("host=127.0.0.1:54321"),
            "should not use colon-separated host:port format: {ctrl_chardev}"
        );

        // Filesystem chardev should connect to the P9Backend TCP address
        let fs_chardev = args.iter().find(|a| a.contains("id=p9fs0")).unwrap();
        assert!(
            fs_chardev.contains("host=127.0.0.1,port=54322"),
            "expected P9Backend address in filesystem chardev: {fs_chardev}"
        );

        // Filesystem virtserialport should use the mount name
        let mount_name = &config.mount_names[0];
        assert!(
            args.iter().any(|a| a.contains(&format!("virtserialport,chardev=p9fs0,name={mount_name}"))),
            "expected virtserialport named '{mount_name}': {args:?}"
        );
    }

    // --- Mount name generation tests (MN-01..MN-10) ---

    /// MN-01: Single directory produces sanitized basename.
    #[test]
    fn mn_01_single_dir_basename() {
        let names = generate_mount_names(&[PathBuf::from("/home/user/my-project")]);
        assert_eq!(names, vec!["my-project"]);
    }

    /// MN-02: Non-alphanumeric characters are replaced with dashes and collapsed.
    #[test]
    fn mn_02_sanitize_special_chars() {
        let names = generate_mount_names(&[PathBuf::from("/home/user/My___Project!!!")]);
        assert_eq!(names, vec!["my-project"]);
    }

    /// MN-03: Colliding basenames get parent prefix.
    #[test]
    fn mn_03_collision_parent_prefix() {
        let names = generate_mount_names(&[
            PathBuf::from("/home/user/project"),
            PathBuf::from("/home/work/project"),
        ]);
        assert_eq!(names, vec!["user-project", "work-project"]);
    }

    /// MN-04: Collisions surviving parent prefix get numeric suffix.
    #[test]
    fn mn_04_collision_numeric_suffix() {
        let names = generate_mount_names(&[
            PathBuf::from("/home/user/project"),
            PathBuf::from("/opt/user/project"),
        ]);
        // Both have parent "user", so after parent prefix they're both "user-project"
        assert_eq!(names, vec!["user-project-1", "user-project-2"]);
    }

    /// MN-05: Long names are truncated with blake3 hash suffix.
    #[test]
    fn mn_05_truncation() {
        let long_name = "a".repeat(50);
        let dir = PathBuf::from(format!("/home/user/{long_name}"));
        let names = generate_mount_names(&[dir]);
        assert!(
            names[0].len() <= MAX_MOUNT_NAME_LEN,
            "name too long: {} ({})",
            names[0],
            names[0].len()
        );
        assert!(
            names[0].contains('-'),
            "truncated name should have hash suffix: {}",
            names[0]
        );
    }

    /// MN-06: Empty basename (root path) falls back to hash.
    #[test]
    fn mn_06_empty_basename_hash_fallback() {
        let names = generate_mount_names(&[PathBuf::from("/")]);
        assert_eq!(names[0].len(), HASH_SUFFIX_LEN);
        assert!(names[0].chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// MN-07: Three distinct directories produce unique names without suffixes.
    #[test]
    fn mn_07_no_collision_no_suffix() {
        let names = generate_mount_names(&[
            PathBuf::from("/home/alice"),
            PathBuf::from("/home/bob"),
            PathBuf::from("/home/charlie"),
        ]);
        assert_eq!(names, vec!["alice", "bob", "charlie"]);
    }

    /// MN-08: Names are lowercased.
    #[test]
    fn mn_08_lowercase() {
        let names = generate_mount_names(&[PathBuf::from("/home/user/MyProject")]);
        assert_eq!(names, vec!["myproject"]);
    }

    /// MN-09: Leading/trailing special chars are stripped.
    #[test]
    fn mn_09_strip_edges() {
        let names = generate_mount_names(&[PathBuf::from("/home/user/---test---")]);
        assert_eq!(names, vec!["test"]);
    }

    /// QC-12: tools drive args present when tools_image_path is set.
    #[test]
    fn qc_12_tools_drive_present() {
        let mut config = test_config();
        config.tools_image_path = Some(PathBuf::from("/path/to/tools.img"));

        let (_binary, args) = config.build_args().unwrap();
        let args = args_to_strings(&args);

        let has_drive = args.iter().any(|a| {
            a.contains("file=/path/to/tools.img")
                && a.contains("format=raw")
                && a.contains("if=virtio")
                && a.contains("readonly=on")
        });
        assert!(has_drive, "missing tools drive arg: {args:?}");
    }

    /// QC-13: tools drive args absent when tools_image_path is None.
    #[test]
    fn qc_13_tools_drive_absent() {
        let config = test_config(); // tools_image_path: None
        let (_binary, args) = config.build_args().unwrap();
        let args = args_to_strings(&args);

        let has_tools_drive = args.iter().any(|a| a.contains("tools.img"));
        assert!(!has_tools_drive, "unexpected tools drive arg: {args:?}");
    }

    /// QC-14: tools_image=1 kernel param added when tools image is set.
    #[test]
    fn qc_14_tools_kernel_param() {
        let mut config = test_config();
        config.tools_image_path = Some(PathBuf::from("/path/to/tools.img"));

        let (_binary, args) = config.build_args().unwrap();
        let args = args_to_strings(&args);

        let append_idx = args.iter().position(|a| a == "-append").unwrap();
        let append_val = &args[append_idx + 1];
        assert!(
            append_val.contains("tools_image=1"),
            "append should contain tools_image=1: {append_val}"
        );
    }

    /// MN-10: mount_names= appears in kernel cmdline.
    #[test]
    fn mn_10_mount_names_in_kernel_cmdline() {
        let mut config = test_config();
        config.working_dirs = vec![
            PathBuf::from("/tmp/alpha"),
            PathBuf::from("/tmp/beta"),
        ];
        config.mount_names = generate_mount_names(&config.working_dirs);
        config.fs_socket_paths = vec![
            PathBuf::from("/tmp/vfs0.sock"),
            PathBuf::from("/tmp/vfs1.sock"),
        ];

        let (_binary, args) = config.build_args().unwrap();
        let args = args_to_strings(&args);

        let append_idx = args.iter().position(|a| a == "-append").unwrap();
        let append_val = &args[append_idx + 1];
        assert!(
            append_val.contains("mount_names=alpha,beta"),
            "expected mount_names=alpha,beta in append: {append_val}"
        );
    }
}
