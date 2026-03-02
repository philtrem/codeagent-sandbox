use std::path::PathBuf;

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
#[cfg(unix)]
pub struct InterceptedBackend {
    inner: codeagent_virtiofs_backend::daemon::InterceptedVirtioFsBackend,
}

#[cfg(unix)]
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

#[cfg(unix)]
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

#[cfg(unix)]
impl Drop for InterceptedBackend {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// 9P2000.L filesystem backend for Windows.
///
/// Runs a host-side P9 server that listens on a socket. QEMU connects to
/// this socket via a virtio-serial chardev, and the guest kernel's v9fs
/// module sends raw 9P2000.L messages through the serial port.
///
/// The server calls `WriteInterceptor` hooks for undo tracking on all
/// mutating filesystem operations, mirroring the behavior of the
/// `InterceptedBackend` on Unix.
#[cfg(target_os = "windows")]
pub struct P9Backend {
    shared_dir: PathBuf,
    socket_path: PathBuf,
    interceptor: std::sync::Arc<dyn codeagent_interceptor::write_interceptor::WriteInterceptor>,
    in_flight: codeagent_control::InFlightTracker,
    server_handle: Option<tokio::task::JoinHandle<()>>,
    shutdown_sender: Option<tokio::sync::oneshot::Sender<()>>,
}

#[cfg(target_os = "windows")]
impl P9Backend {
    pub fn new(
        shared_dir: PathBuf,
        socket_path: PathBuf,
        interceptor: std::sync::Arc<dyn codeagent_interceptor::write_interceptor::WriteInterceptor>,
        in_flight: codeagent_control::InFlightTracker,
    ) -> Self {
        Self {
            shared_dir,
            socket_path,
            interceptor,
            in_flight,
            server_handle: None,
            shutdown_sender: None,
        }
    }
}

#[cfg(target_os = "windows")]
impl FilesystemBackend for P9Backend {
    fn start(&mut self) -> Result<(), AgentError> {
        use codeagent_p9::server::P9Server;

        let server = P9Server::new(self.shared_dir.clone())
            .with_interceptor(self.interceptor.clone())
            .with_in_flight(self.in_flight.clone());

        let socket_path = self.socket_path.clone();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let handle = tokio::spawn(async move {
            // Listen on a socket for QEMU to connect.
            let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
                Ok(l) => l,
                Err(error) => {
                    eprintln!("P9Backend: failed to bind listener: {error}");
                    return;
                }
            };

            // Write the actual bound port to the socket_path file so QEMU
            // can find it. On Windows we use TCP since Unix sockets are
            // not reliably available.
            let addr = listener.local_addr().unwrap();
            if let Err(error) = std::fs::write(&socket_path, addr.to_string()) {
                eprintln!("P9Backend: failed to write socket address: {error}");
                return;
            }

            // Accept exactly one connection (from QEMU) and run the server.
            tokio::select! {
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((stream, _addr)) => {
                            let (reader, writer) = stream.into_split();
                            let mut server = server;
                            if let Err(error) = server.run(reader, writer).await {
                                eprintln!("P9Backend: server error: {error}");
                            }
                        }
                        Err(error) => {
                            eprintln!("P9Backend: accept failed: {error}");
                        }
                    }
                }
                _ = shutdown_rx => {
                    // Graceful shutdown requested.
                }
            }
        });

        self.server_handle = Some(handle);
        self.shutdown_sender = Some(shutdown_tx);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), AgentError> {
        if let Some(tx) = self.shutdown_sender.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.server_handle.take() {
            handle.abort();
        }
        let _ = std::fs::remove_file(&self.socket_path);
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.server_handle
            .as_ref()
            .is_some_and(|h| !h.is_finished())
    }
}

#[cfg(target_os = "windows")]
impl Drop for P9Backend {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
