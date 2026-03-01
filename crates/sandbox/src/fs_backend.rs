use crate::error::AgentError;

/// Abstraction over the filesystem backend (virtiofsd or 9P server).
///
/// The real implementations will hold an `Arc<dyn WriteInterceptor>` and an
/// `InFlightTracker` to intercept POSIX syscalls and track in-flight operations.
/// For now, only `NullBackend` exists as a placeholder.
pub trait FilesystemBackend: Send + Sync {
    fn start(&mut self) -> Result<(), AgentError>;
    fn stop(&mut self) -> Result<(), AgentError>;
    fn is_running(&self) -> bool;
}

/// Placeholder backend used until virtiofsd or 9P server is built.
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
