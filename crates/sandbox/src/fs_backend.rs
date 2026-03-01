use crate::error::AgentError;

/// Abstraction over the filesystem backend (virtiofsd or 9P server).
pub trait FilesystemBackend: Send + Sync {
    fn start(&mut self) -> Result<(), AgentError>;
    fn stop(&mut self) -> Result<(), AgentError>;
    fn is_running(&self) -> bool;
}

/// Placeholder backend used when no VM is available.
pub struct NullBackend;

impl FilesystemBackend for NullBackend {
    fn start(&mut self) -> Result<(), AgentError> {
        Ok(())
    }

    fn stop(&mut self) -> Result<(), AgentError> {
        Ok(())
    }

    fn is_running(&self) -> bool {
        false
    }
}

/// Filesystem backend that spawns a virtiofsd process.
///
/// Available on Linux and macOS only. On Windows, the 9P server
/// (Phase 3) will serve as the filesystem backend instead.
///
/// Currently launches the upstream (unmodified) virtiofsd. When the
/// forked virtiofsd with `WriteInterceptor` hooks is built, it will
/// be a drop-in replacement using the same CLI interface.
#[cfg(not(target_os = "windows"))]
use std::path::PathBuf;

#[cfg(not(target_os = "windows"))]
pub struct VirtioFsBackend {
    shared_dir: PathBuf,
    socket_path: PathBuf,
    virtiofsd_binary: Option<PathBuf>,
    child: Option<std::process::Child>,
}

#[cfg(not(target_os = "windows"))]
impl VirtioFsBackend {
    pub fn new(
        shared_dir: PathBuf,
        socket_path: PathBuf,
        virtiofsd_binary: Option<PathBuf>,
    ) -> Self {
        Self {
            shared_dir,
            socket_path,
            virtiofsd_binary,
            child: None,
        }
    }

    fn resolve_virtiofsd_binary(&self) -> Result<PathBuf, AgentError> {
        if let Some(path) = &self.virtiofsd_binary {
            return Ok(path.clone());
        }

        // Search common installation paths first
        for candidate in ["/usr/libexec/virtiofsd", "/usr/lib/virtiofsd"] {
            if std::path::Path::new(candidate).exists() {
                return Ok(PathBuf::from(candidate));
            }
        }

        which::which("virtiofsd").map_err(|_| AgentError::VirtioFsFailed {
            reason: "virtiofsd binary not found in PATH or standard locations".to_string(),
        })
    }
}

#[cfg(not(target_os = "windows"))]
impl FilesystemBackend for VirtioFsBackend {
    fn start(&mut self) -> Result<(), AgentError> {
        let binary = self.resolve_virtiofsd_binary()?;

        let child = std::process::Command::new(&binary)
            .arg("--shared-dir")
            .arg(&self.shared_dir)
            .arg("--socket-path")
            .arg(&self.socket_path)
            .arg("--cache=never")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| AgentError::VirtioFsFailed {
                reason: format!("failed to start {}: {error}", binary.display()),
            })?;

        self.child = Some(child);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), AgentError> {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_file(&self.socket_path);
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.child.is_some()
    }
}

#[cfg(not(target_os = "windows"))]
impl Drop for VirtioFsBackend {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// In-process virtiofsd backend with WriteInterceptor hooks.
///
/// Wraps `InterceptedVirtioFsBackend` from the virtiofs-backend crate,
/// adapting its error type to `AgentError` for use in the Orchestrator.
#[cfg(target_os = "linux")]
pub struct InterceptedBackend {
    inner: codeagent_virtiofs_backend::daemon::InterceptedVirtioFsBackend,
}

#[cfg(target_os = "linux")]
impl InterceptedBackend {
    pub fn new(
        shared_dir: PathBuf,
        socket_path: PathBuf,
        interceptor: std::sync::Arc<dyn codeagent_interceptor::write_interceptor::WriteInterceptor>,
        in_flight: codeagent_control::InFlightTracker,
    ) -> Self {
        Self {
            inner: codeagent_virtiofs_backend::daemon::InterceptedVirtioFsBackend::new(
                shared_dir,
                socket_path,
                interceptor,
                in_flight,
            ),
        }
    }
}

#[cfg(target_os = "linux")]
impl FilesystemBackend for InterceptedBackend {
    fn start(&mut self) -> Result<(), AgentError> {
        self.inner.start().map_err(|error| AgentError::VirtioFsFailed {
            reason: error.to_string(),
        })
    }

    fn stop(&mut self) -> Result<(), AgentError> {
        self.inner.stop().map_err(|error| AgentError::VirtioFsFailed {
            reason: error.to_string(),
        })
    }

    fn is_running(&self) -> bool {
        self.inner.is_running()
    }
}

#[cfg(target_os = "linux")]
impl Drop for InterceptedBackend {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
