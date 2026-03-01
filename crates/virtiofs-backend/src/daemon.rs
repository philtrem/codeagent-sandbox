use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;

use log::{error, info};
use vhost::vhost_user::Listener;
use vhost_user_backend::VhostUserDaemon;
use virtiofsd::passthrough::{CachePolicy, Config, PassthroughFs};
use virtiofsd::vhost_user::VhostUserFsBackendBuilder;
use vm_memory::{GuestMemoryAtomic, GuestMemoryMmap};

use codeagent_control::InFlightTracker;
use codeagent_interceptor::write_interceptor::WriteInterceptor;

use crate::error::VirtioFsBackendError;
use crate::intercepted_fs::InterceptedFs;

/// In-process virtiofsd daemon with WriteInterceptor hooks.
///
/// Replaces the external `VirtioFsBackend` (which spawns upstream virtiofsd
/// as a child process) with an in-process daemon that intercepts all mutating
/// filesystem operations for undo tracking.
///
/// The sandbox crate wraps this in an adapter that implements its
/// `FilesystemBackend` trait, converting errors as needed.
pub struct InterceptedVirtioFsBackend {
    shared_dir: PathBuf,
    socket_path: PathBuf,
    interceptor: Arc<dyn WriteInterceptor>,
    in_flight: InFlightTracker,
    daemon_handle: Option<JoinHandle<()>>,
}

impl InterceptedVirtioFsBackend {
    pub fn new(
        shared_dir: PathBuf,
        socket_path: PathBuf,
        interceptor: Arc<dyn WriteInterceptor>,
        in_flight: InFlightTracker,
    ) -> Self {
        Self {
            shared_dir,
            socket_path,
            interceptor,
            in_flight,
            daemon_handle: None,
        }
    }

    /// Build the virtiofsd Config for the shared directory.
    fn build_config(&self) -> Config {
        Config {
            root_dir: self.shared_dir.to_string_lossy().to_string(),
            cache_policy: CachePolicy::Never,
            xattr: true,
            ..Default::default()
        }
    }

    /// Start the in-process virtiofsd daemon on a background thread.
    ///
    /// Creates a `PassthroughFs`, wraps it in `InterceptedFs`, builds the
    /// vhost-user daemon, and spawns a thread to serve requests.
    pub fn start(&mut self) -> Result<(), VirtioFsBackendError> {
        if self.daemon_handle.is_some() {
            return Ok(());
        }

        // 1. Create PassthroughFs with our config
        let cfg = self.build_config();
        let passthrough = PassthroughFs::new(cfg).map_err(|error| {
            VirtioFsBackendError::Daemon {
                reason: format!("failed to create PassthroughFs: {error}"),
            }
        })?;

        // 2. Open root node (required before serving requests)
        passthrough.open_root_node().map_err(|error| {
            VirtioFsBackendError::Daemon {
                reason: format!("failed to open root node: {error}"),
            }
        })?;

        // 3. Wrap in InterceptedFs
        let intercepted = InterceptedFs::new(
            passthrough,
            self.interceptor.clone(),
            self.in_flight.clone(),
            self.shared_dir.clone(),
        );

        // 4. Create vhost-user socket listener
        let listener = Listener::new(&self.socket_path, true).map_err(|error| {
            VirtioFsBackendError::Daemon {
                reason: format!("failed to create vhost-user listener: {error}"),
            }
        })?;

        // 5. Build VhostUserFsBackend
        let fs_backend = Arc::new(
            VhostUserFsBackendBuilder::default()
                .set_thread_pool_size(0)
                .build(intercepted)
                .map_err(|error| VirtioFsBackendError::Daemon {
                    reason: format!("failed to build vhost-user backend: {error}"),
                })?,
        );

        // 6. Spawn daemon on a background thread
        let handle = std::thread::spawn(move || {
            let mut daemon = match VhostUserDaemon::new(
                String::from("codeagent-virtiofsd"),
                fs_backend,
                GuestMemoryAtomic::new(GuestMemoryMmap::new()),
            ) {
                Ok(d) => d,
                Err(error) => {
                    error!("Failed to create VhostUserDaemon: {error:?}");
                    return;
                }
            };

            info!("virtiofsd: waiting for vhost-user connection...");

            if let Err(error) = daemon.start(listener) {
                error!("Failed to start virtiofsd daemon: {error:?}");
                return;
            }

            info!("virtiofsd: client connected, serving requests");

            if let Err(error) = daemon.wait() {
                info!("virtiofsd daemon exited: {error:?}");
            }
        });

        self.daemon_handle = Some(handle);
        Ok(())
    }

    /// Stop the daemon and clean up the socket file.
    pub fn stop(&mut self) -> Result<(), VirtioFsBackendError> {
        if let Some(handle) = self.daemon_handle.take() {
            // The daemon exits when the QEMU process disconnects from the
            // vhost-user socket. Dropping the handle is sufficient.
            drop(handle);
        }
        let _ = std::fs::remove_file(&self.socket_path);
        Ok(())
    }

    /// Whether the daemon thread is still running.
    pub fn is_running(&self) -> bool {
        self.daemon_handle
            .as_ref()
            .is_some_and(|h| !h.is_finished())
    }
}

impl Drop for InterceptedVirtioFsBackend {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
