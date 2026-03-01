use std::path::PathBuf;

use crate::error::AgentError;

/// Configuration for launching a QEMU virtual machine.
#[derive(Debug, Clone)]
pub struct QemuConfig {
    pub vm_mode: String,
    pub working_dirs: Vec<PathBuf>,
    pub control_socket_path: PathBuf,
    pub fs_socket_paths: Vec<PathBuf>,
}

/// Handle to a running QEMU process.
///
/// Currently a stub — the guest image and VM-side shim do not exist yet.
/// `spawn()` always returns `QemuUnavailable`. When the VM components are
/// built, this will manage a real `tokio::process::Child`.
pub struct QemuProcess {
    _config: QemuConfig,
}

impl QemuProcess {
    /// Attempt to spawn a QEMU VM. Currently always returns an error
    /// because the guest image and VM-side shim are not yet built.
    pub fn spawn(_config: QemuConfig) -> Result<Self, AgentError> {
        Err(AgentError::QemuUnavailable)
    }

    pub fn stop(&mut self) -> Result<(), AgentError> {
        Ok(())
    }
}
