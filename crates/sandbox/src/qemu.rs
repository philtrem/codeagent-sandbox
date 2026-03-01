use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::Duration;

use crate::error::AgentError;

/// Timeout for waiting for the control socket to appear after QEMU starts.
const CONTROL_SOCKET_TIMEOUT: Duration = Duration::from_secs(30);

/// Polling interval while waiting for the control socket.
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
    /// - Windows: q35 + WHPX + virtfs (9P) — no shared memory needed
    pub fn build_args(&self) -> Result<(PathBuf, Vec<OsString>), AgentError> {
        let binary = self.resolve_qemu_binary()?;
        let mut args: Vec<OsString> = Vec::new();

        self.add_platform_args(&mut args);
        self.add_common_args(&mut args);
        self.add_filesystem_args(&mut args);
        self.add_control_channel_args(&mut args);
        self.add_boot_args(&mut args);
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
            args.extend(["-machine".into(), "q35".into()]);
            args.extend([
                "-cpu".into(),
                "host".into(),
                "-accel".into(),
                "whpx".into(),
            ]);
        }
    }

    /// Common arguments: memory, CPUs, network, display.
    fn add_common_args(&self, args: &mut Vec<OsString>) {
        args.extend(["-m".into(), format!("{}M", self.memory_mb).into()]);
        args.extend(["-smp".into(), self.cpus.to_string().into()]);
        args.extend(["-netdev".into(), "user,id=net0".into()]);
        args.extend(["-device".into(), "virtio-net-pci,netdev=net0".into()]);
        args.push("-nographic".into());
    }

    /// Filesystem sharing devices (virtiofs on Linux/macOS, 9P on Windows).
    fn add_filesystem_args(&self, args: &mut Vec<OsString>) {
        for (index, _socket_path) in self.fs_socket_paths.iter().enumerate() {
            let chardev_id = format!("vfs{index}");
            let tag = if index == 0 {
                "working".to_string()
            } else {
                format!("working{index}")
            };

            #[cfg(not(target_os = "windows"))]
            {
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
                let working_dir = &self.working_dirs[index];
                args.extend([
                    "-virtfs".into(),
                    format!(
                        "local,path={},mount_tag={tag},security_model=none,id={chardev_id}",
                        working_dir.display()
                    )
                    .into(),
                ]);
            }
        }
    }

    /// Control channel: virtio-serial device connected via a chardev socket.
    fn add_control_channel_args(&self, args: &mut Vec<OsString>) {
        args.extend([
            "-chardev".into(),
            format!(
                "socket,id=ctrl,path={},server=on,wait=off",
                self.control_socket_path.display()
            )
            .into(),
        ]);
        args.extend(["-device".into(), "virtio-serial-pci".into()]);
        args.extend([
            "-device".into(),
            "virtconsole,chardev=ctrl,name=control".into(),
        ]);
    }

    /// Kernel, initrd, and rootfs boot arguments.
    fn add_boot_args(&self, args: &mut Vec<OsString>) {
        args.extend([
            "-kernel".into(),
            self.kernel_path.as_os_str().to_owned(),
        ]);
        args.extend([
            "-initrd".into(),
            self.initrd_path.as_os_str().to_owned(),
        ]);
        args.extend(["-append".into(), "console=hvc0 root=/dev/vda".into()]);

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

    /// Resolve the QEMU binary path from the override or PATH.
    fn resolve_qemu_binary(&self) -> Result<PathBuf, AgentError> {
        if let Some(path) = &self.qemu_binary {
            return Ok(path.clone());
        }

        let default_name = default_qemu_binary_name();

        which::which(default_name).map_err(|_| AgentError::QemuUnavailable)
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

/// Handle to a running QEMU process.
pub struct QemuProcess {
    config: QemuConfig,
    child: Child,
}

impl QemuProcess {
    /// Spawn a QEMU VM.
    ///
    /// Builds the platform-specific command line, spawns the process,
    /// and waits for the control socket to appear (indicating the VM
    /// is ready to accept connections).
    pub fn spawn(config: QemuConfig) -> Result<Self, AgentError> {
        let (binary, args) = config.build_args()?;

        let child = std::process::Command::new(&binary)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| AgentError::QemuSpawnFailed {
                reason: format!(
                    "failed to start {}: {error}",
                    binary.display()
                ),
            })?;

        let process = Self { config, child };

        // Wait for the control socket to appear
        Self::wait_for_ready(&process.config.control_socket_path)?;

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

    /// Wait for the control socket to appear, indicating VM readiness.
    fn wait_for_ready(control_socket: &Path) -> Result<(), AgentError> {
        let start = std::time::Instant::now();
        while start.elapsed() < CONTROL_SOCKET_TIMEOUT {
            if control_socket.exists() {
                return Ok(());
            }
            std::thread::sleep(SOCKET_POLL_INTERVAL);
        }
        Err(AgentError::QemuSpawnFailed {
            reason: format!(
                "control socket {} did not appear within {}s",
                control_socket.display(),
                CONTROL_SOCKET_TIMEOUT.as_secs()
            ),
        })
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
        assert!(args.contains(&"q35".to_string()));
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

        let has_ctrl_chardev = args.iter().any(|a| {
            a.contains("socket,id=ctrl") && a.contains("/tmp/control.sock")
        });
        assert!(has_ctrl_chardev, "missing control chardev: {args:?}");

        assert!(args.contains(&"virtio-serial-pci".to_string()));
        assert!(args.contains(&"virtconsole,chardev=ctrl,name=control".to_string()));
    }

    /// QC-04: build_args includes one filesystem device per working dir.
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

        let fs_device_count = args
            .iter()
            .filter(|a| a.contains("vfs0") || a.contains("vfs1"))
            .count();

        // Each working dir should produce at least a chardev + device pair (or virtfs)
        assert!(
            fs_device_count >= 2,
            "expected >=2 filesystem device args, got {fs_device_count}: {args:?}"
        );
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
}
