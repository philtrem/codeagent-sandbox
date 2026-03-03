use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Accumulated output and exit status for a completed command.
#[derive(Debug, Default)]
pub struct CommandResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    completed: bool,
}

/// Thread-safe bridge that collects async command events and allows
/// synchronous callers to block until a command completes.
///
/// The event bridge task calls `append_output` / `mark_completed` as
/// events arrive from the VM control channel. The MCP `execute_command`
/// handler calls `wait_for_completion` to block until the command
/// finishes (or times out).
pub struct CommandWaiter {
    results: Mutex<HashMap<u64, CommandResult>>,
    notify: Condvar,
}

impl CommandWaiter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            results: Mutex::new(HashMap::new()),
            notify: Condvar::new(),
        })
    }

    /// Register a command ID before sending it, so events that arrive
    /// before `wait_for_completion` is called are not lost.
    pub fn register(&self, command_id: u64) {
        let mut results = self.results.lock().unwrap();
        results.insert(command_id, CommandResult::default());
    }

    /// Append output data for a running command.
    pub fn append_output(&self, command_id: u64, stream: &str, data: &str) {
        let mut results = self.results.lock().unwrap();
        if let Some(result) = results.get_mut(&command_id) {
            match stream {
                "stderr" => result.stderr.push_str(data),
                _ => result.stdout.push_str(data),
            }
        }
    }

    /// Mark a command as completed with its exit code.
    pub fn mark_completed(&self, command_id: u64, exit_code: i32) {
        let mut results = self.results.lock().unwrap();
        if let Some(result) = results.get_mut(&command_id) {
            result.exit_code = Some(exit_code);
            result.completed = true;
        }
        self.notify.notify_all();
    }

    /// Block until the command completes or the timeout expires.
    /// Returns `None` if the command was never registered.
    pub fn wait_for_completion(
        &self,
        command_id: u64,
        timeout: Duration,
    ) -> Option<CommandResult> {
        let mut results = self.results.lock().unwrap();
        let deadline = std::time::Instant::now() + timeout;

        loop {
            if let Some(result) = results.get(&command_id) {
                if result.completed {
                    return results.remove(&command_id);
                }
            } else {
                return None;
            }

            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                // Timed out — return partial result
                return results.remove(&command_id);
            }

            let (guard, _timeout_result) =
                self.notify.wait_timeout(results, remaining).unwrap();
            results = guard;
        }
    }
}
