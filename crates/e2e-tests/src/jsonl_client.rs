use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, Notify, oneshot};

use crate::constants::{AGENT_BIN_ENV, DEFAULT_BINARY_NAME};

#[derive(Debug, Error)]
pub enum E2eError {
    #[error("agent binary not found: {0}")]
    BinaryNotFound(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("timeout waiting for response to request {request_id}")]
    ResponseTimeout { request_id: String },

    #[error("timeout waiting for event {event_type}")]
    EventTimeout { event_type: String },

    #[error("agent process exited unexpectedly: {0}")]
    ProcessExited(String),

    #[error("JSON serialization error: {0}")]
    JsonSerialize(#[from] serde_json::Error),

    #[error("child stdin closed")]
    StdinClosed,
}

/// Internal shared state for the background reader tasks.
struct ReaderState {
    /// Pending response waiters: request_id -> oneshot sender.
    response_waiters: Mutex<HashMap<String, oneshot::Sender<Value>>>,
    /// Buffered events: event_type -> Vec<event payloads>.
    event_buffers: Mutex<HashMap<String, Vec<Value>>>,
    /// Notifies event waiters when a new event arrives.
    event_notify: Notify,
    /// Captured stderr lines for log validation.
    stderr_lines: Mutex<Vec<String>>,
}

/// STDIO API test client that spawns the agent as a child process.
///
/// Sends JSON Lines to the agent's stdin, reads responses and events from
/// stdout in a background task, and captures stderr for log validation.
///
/// Responses (lines with `"type":"response"`) are delivered to callers by
/// matching `request_id`. Events (lines with `"type":"event.*"`) are buffered
/// per event type and delivered in FIFO order.
pub struct JsonlClient {
    pub child: Child,
    stdin: Option<tokio::process::ChildStdin>,
    state: Arc<ReaderState>,
    _stdout_task: tokio::task::JoinHandle<()>,
    _stderr_task: tokio::task::JoinHandle<()>,
}

impl JsonlClient {
    /// Resolve the agent binary path from environment or default locations.
    fn resolve_binary() -> Result<PathBuf, E2eError> {
        if let Ok(path) = std::env::var(AGENT_BIN_ENV) {
            let candidate = PathBuf::from(&path);
            if candidate.exists() {
                return Ok(candidate);
            }
            return Err(E2eError::BinaryNotFound(format!(
                "{AGENT_BIN_ENV}={path} does not exist"
            )));
        }

        for profile in ["debug", "release"] {
            let candidate = PathBuf::from(format!("target/{profile}/{DEFAULT_BINARY_NAME}"));
            if candidate.exists() {
                return Ok(candidate);
            }
            let candidate_exe =
                PathBuf::from(format!("target/{profile}/{DEFAULT_BINARY_NAME}.exe"));
            if candidate_exe.exists() {
                return Ok(candidate_exe);
            }
        }

        Err(E2eError::BinaryNotFound(format!(
            "set {AGENT_BIN_ENV} or build the agent binary"
        )))
    }

    /// Spawn the agent binary with the given working directory and undo directory.
    ///
    /// The agent is started with `--working-dir`, `--undo-dir`, and `--vm-mode`
    /// arguments. Additional arguments can be passed via `extra_args`.
    pub async fn spawn(
        working_dir: &Path,
        undo_dir: &Path,
        vm_mode: &str,
        extra_args: &[&str],
    ) -> Result<Self, E2eError> {
        let bin = Self::resolve_binary()?;

        let mut cmd = Command::new(&bin);
        cmd.arg("--working-dir")
            .arg(working_dir)
            .arg("--undo-dir")
            .arg(undo_dir)
            .arg("--vm-mode")
            .arg(vm_mode)
            .args(extra_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn()?;

        let stdin = child.stdin.take().expect("stdin was not captured");
        let stdout = child.stdout.take().expect("stdout was not captured");
        let stderr = child.stderr.take().expect("stderr was not captured");

        let state = Arc::new(ReaderState {
            response_waiters: Mutex::new(HashMap::new()),
            event_buffers: Mutex::new(HashMap::new()),
            event_notify: Notify::new(),
            stderr_lines: Mutex::new(Vec::new()),
        });

        let stdout_state = Arc::clone(&state);
        let stdout_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(parsed) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let Some(msg_type) = parsed.get("type").and_then(|v| v.as_str()) else {
                    continue;
                };

                if msg_type == "response" {
                    if let Some(request_id) = parsed.get("request_id").and_then(|v| v.as_str()) {
                        let mut waiters = stdout_state.response_waiters.lock().await;
                        if let Some(sender) = waiters.remove(request_id) {
                            let _ = sender.send(parsed);
                        }
                    }
                } else if msg_type.starts_with("event.") {
                    let mut buffers = stdout_state.event_buffers.lock().await;
                    buffers
                        .entry(msg_type.to_string())
                        .or_default()
                        .push(parsed);
                    stdout_state.event_notify.notify_waiters();
                }
            }
        });

        let stderr_state = Arc::clone(&state);
        let stderr_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                stderr_state.stderr_lines.lock().await.push(line);
            }
        });

        Ok(Self {
            child,
            stdin: Some(stdin),
            state,
            _stdout_task: stdout_task,
            _stderr_task: stderr_task,
        })
    }

    /// Send a JSON message to the agent's stdin.
    pub async fn send(&mut self, msg: &Value) -> Result<(), E2eError> {
        let stdin = self.stdin.as_mut().ok_or(E2eError::StdinClosed)?;
        let line = serde_json::to_string(msg)?;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|_| E2eError::StdinClosed)?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|_| E2eError::StdinClosed)?;
        stdin.flush().await.map_err(|_| E2eError::StdinClosed)?;
        Ok(())
    }

    /// Wait for a response matching the given request_id within the timeout.
    pub async fn recv_response(
        &self,
        request_id: &str,
        timeout: Duration,
    ) -> Result<Value, E2eError> {
        let (tx, rx) = oneshot::channel();
        {
            let mut waiters = self.state.response_waiters.lock().await;
            waiters.insert(request_id.to_string(), tx);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(value)) => Ok(value),
            _ => {
                // Clean up the waiter on timeout or channel error
                let mut waiters = self.state.response_waiters.lock().await;
                waiters.remove(request_id);
                Err(E2eError::ResponseTimeout {
                    request_id: request_id.to_string(),
                })
            }
        }
    }

    /// Wait for an event of the given type within the timeout.
    ///
    /// If a matching event is already buffered, it is returned immediately.
    /// Events are returned in FIFO order per event type.
    pub async fn recv_event(
        &self,
        event_type: &str,
        timeout: Duration,
    ) -> Result<Value, E2eError> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            // Check buffer first
            {
                let mut buffers = self.state.event_buffers.lock().await;
                if let Some(events) = buffers.get_mut(event_type) {
                    if !events.is_empty() {
                        return Ok(events.remove(0));
                    }
                }
            }
            // Wait for notification or timeout
            if tokio::time::timeout_at(deadline, self.state.event_notify.notified())
                .await
                .is_err()
            {
                return Err(E2eError::EventTimeout {
                    event_type: event_type.to_string(),
                });
            }
        }
    }

    /// Send a message and wait for the response in one call.
    pub async fn request(
        &mut self,
        msg: &Value,
        request_id: &str,
        timeout: Duration,
    ) -> Result<Value, E2eError> {
        self.send(msg).await?;
        self.recv_response(request_id, timeout).await
    }

    /// Get all captured stderr lines so far.
    pub async fn stderr_lines(&self) -> Vec<String> {
        self.state.stderr_lines.lock().await.clone()
    }

    /// Send `session.stop` and wait for the process to exit.
    pub async fn shutdown(&mut self, timeout: Duration) -> Result<(), E2eError> {
        let (msg, id) = crate::messages::session_stop();
        self.send(&msg).await?;
        let _ = self.recv_response(&id, timeout).await;
        // Drop stdin to signal EOF
        self.stdin.take();
        let _ = tokio::time::timeout(timeout, self.child.wait()).await;
        Ok(())
    }

    /// Kill the child process immediately.
    pub async fn kill(&mut self) -> Result<(), E2eError> {
        self.child.kill().await?;
        Ok(())
    }
}
