use std::ffi::OsString;
use std::path::PathBuf;
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
        _extra_kernel_params: &mut Vec<String>,
    ) {
        for (index, socket_path) in self.fs_socket_paths.iter().enumerate() {
            #[cfg(not(target_os = "windows"))]
            {
                let chardev_id = format!("vfs{index}");
                let tag = if index == 0 {
                    "working".to_string()
                } else {
                    format!("working{index}")
                };
                args.extend([
                    "-chardev".into(),
                    format!("socket,id={chardev_id},path={}", socket_path.display()).into(),
                ]);
                args.extend([
                    "-device".into(),
                    format!("vhost-user-fs-pci,chardev={chardev_id},tag={tag}").into(),
                ]);
            }

            #[cfg(target_os = "windows")]
            {
                // Read the TCP address from the socket_path file written by
                // P9Backend::start(). QEMU connects to it via a chardev, and
                // exposes a virtserialport named p9fsN to the guest.
                let addr = std::fs::read_to_string(socket_path).unwrap_or_default();
                let addr = addr.trim().to_string();
                let (host, port) = addr.rsplit_once(':').unwrap_or((&addr, "0"));
                let chardev_id = format!("p9fs{index}");
                let port_name = format!("p9fs{index}");

                args.extend([
                    "-chardev".into(),
                    format!("socket,id={chardev_id},host={host},port={port},server=off").into(),
                ]);
                args.extend([
                    "-device".into(),
                    format!("virtserialport,chardev={chardev_id},name={port_name}").into(),
                ]);
            }
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

        let child = std::process::Command::new(&binary)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(stdout_mode)
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| AgentError::QemuSpawnFailed {
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
        QemuConfig {
            qemu_binary: Some(PathBuf::from("/usr/bin/qemu-system-x86_64")),
            kernel_path: PathBuf::from("/boot/vmlinuz"),
            initrd_path: PathBuf::from("/boot/initrd.img"),
            rootfs_path: None,
            memory_mb: 2048,
            cpus: 2,
            working_dirs: vec![PathBuf::from("/tmp/work")],
            control_socket_path: PathBuf::from("/tmp/control.sock"),
            fs_socket_paths: vec![PathBuf::from("/tmp/vfs0.sock")],
            vm_mode: "ephemeral".to_string(),
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
        config.fs_socket_paths = vec![
            PathBuf::from("/tmp/vfs0.sock"),
            PathBuf::from("/tmp/vfs1.sock"),
        ];

        let (_binary, args) = config.build_args().unwrap();
        let args = args_to_strings(&args);

        #[cfg(not(target_os = "windows"))]
        {
            let fs_device_count = args
                .iter()
                .filter(|a| a.contains("vfs0") || a.contains("vfs1"))
                .count();
            assert!(
                fs_device_count >= 2,
                "expected >=2 filesystem device args, got {fs_device_count}: {args:?}"
            );
        }

        #[cfg(target_os = "windows")]
        {
            // On Windows, filesystem ports use chardev + virtserialport
            let fs_chardev_count = args
                .iter()
                .filter(|a| a.contains("id=p9fs0") || a.contains("id=p9fs1"))
                .count();
            assert!(
                fs_chardev_count >= 2,
                "expected >=2 filesystem chardev args, got {fs_chardev_count}: {args:?}"
            );
            let fs_port_count = args
                .iter()
                .filter(|a| a.contains("name=p9fs0") || a.contains("name=p9fs1"))
                .count();
            assert!(
                fs_port_count >= 2,
                "expected >=2 filesystem virtserialport args, got {fs_port_count}: {args:?}"
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

    /// QC-09: append line does not include root= when rootfs_path is None.
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

        // Filesystem virtserialport should be named p9fs0
        assert!(
            args.iter().any(|a| a.contains("virtserialport,chardev=p9fs0,name=p9fs0")),
            "expected virtserialport named p9fs0: {args:?}"
        );
    }
}
