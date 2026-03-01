use std::time::Duration;

/// Configuration for output buffering between the child process and
/// the control channel. Prevents flooding the channel with per-byte messages
/// by coalescing rapid output into larger chunks.
#[derive(Debug, Clone)]
pub struct OutputBufferConfig {
    /// Maximum buffer size in bytes before a forced flush. Default: 4096.
    pub max_buffer_size: usize,
    /// Maximum time between flushes. Default: 50ms.
    pub flush_interval: Duration,
}

impl Default for OutputBufferConfig {
    fn default() -> Self {
        Self {
            max_buffer_size: 4096,
            flush_interval: Duration::from_millis(50),
        }
    }
}
