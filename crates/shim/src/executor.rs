use std::collections::HashMap;

#[cfg(unix)]
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use codeagent_control::{OutputStream, VmMessage};

use crate::error::ShimError;
use crate::output_buffer::OutputBufferConfig;

/// Timeout between SIGTERM and SIGKILL during cancellation.
#[cfg(unix)]
const CANCEL_KILL_TIMEOUT: Duration = Duration::from_secs(5);

/// Handle to a running command, used for cancellation.
pub struct CommandHandle {
    /// Sender to signal cancellation to the command task.
    cancel_sender: Option<oneshot::Sender<()>>,
    /// Join handle for the command task (sends StepCompleted on exit).
    task_handle: JoinHandle<()>,
}

impl CommandHandle {
    /// Returns true if the command task has finished.
    pub fn is_finished(&self) -> bool {
        self.task_handle.is_finished()
    }
}

/// Spawn a shell command and stream output as `VmMessage`s.
///
/// Immediately sends `StepStarted`, then streams `Output` messages for
/// stdout and stderr, and finally sends `StepCompleted` when the process
/// exits. Returns a `CommandHandle` that allows cancellation.
pub fn spawn_command(
    id: u64,
    command: &str,
    cwd: Option<&str>,
    env: Option<&HashMap<String, String>>,
    message_sender: mpsc::UnboundedSender<VmMessage>,
    buffer_config: OutputBufferConfig,
) -> Result<CommandHandle, ShimError> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command);

    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    if let Some(env_vars) = env {
        cmd.envs(env_vars);
    }

    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    // Spawn in a new process group so cancel can kill the whole tree.
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            libc::setpgid(0, 0);
            Ok(())
        });
    }

    let mut child = cmd.spawn()?;

    // Take the output handles before moving child into the task.
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let (cancel_sender, cancel_receiver) = oneshot::channel::<()>();

    // Send step_started immediately
    let _ = message_sender.send(VmMessage::StepStarted { id });

    let task_handle = tokio::spawn(run_command(
        id,
        child,
        stdout,
        stderr,
        message_sender,
        cancel_receiver,
        buffer_config,
    ));

    Ok(CommandHandle {
        cancel_sender: Some(cancel_sender),
        task_handle,
    })
}

/// Core command lifecycle: stream output, wait for exit, handle cancel.
async fn run_command(
    id: u64,
    mut child: Child,
    stdout: Option<tokio::process::ChildStdout>,
    stderr: Option<tokio::process::ChildStderr>,
    message_sender: mpsc::UnboundedSender<VmMessage>,
    cancel_receiver: oneshot::Receiver<()>,
    buffer_config: OutputBufferConfig,
) {
    // Capture the PID before the child is consumed (needed for process group kill on Unix).
    #[cfg(unix)]
    let child_pid = child.id();
    // Spawn output reader tasks
    let stdout_handle = stdout.map(|out| {
        let sender = message_sender.clone();
        let config = buffer_config.clone();
        tokio::spawn(stream_output(id, OutputStream::Stdout, out, sender, config))
    });

    let stderr_handle = stderr.map(|err| {
        let sender = message_sender.clone();
        let config = buffer_config.clone();
        tokio::spawn(stream_output(id, OutputStream::Stderr, err, sender, config))
    });

    // Wait for either child exit or cancel signal
    let cancelled;
    let exit_code = tokio::select! {
        status = child.wait() => {
            cancelled = false;
            match status {
                Ok(s) => s.code().unwrap_or(-1),
                Err(_) => -1,
            }
        }
        _ = cancel_receiver => {
            cancelled = true;
            // On Unix, kill the entire process group first
            #[cfg(unix)]
            terminate_process_group(child_pid).await;

            // Cross-platform: kill the direct child process
            let _ = child.kill().await;
            match child.wait().await {
                Ok(s) => s.code().unwrap_or(-1),
                Err(_) => -1,
            }
        }
    };

    if cancelled {
        // On cancel, abort output readers immediately — orphaned subprocesses
        // (e.g., MSYS2 sleep on Windows) may keep pipes open indefinitely.
        if let Some(handle) = stdout_handle {
            handle.abort();
        }
        if let Some(handle) = stderr_handle {
            handle.abort();
        }
    } else {
        // Normal exit — wait for output streams to drain.
        if let Some(handle) = stdout_handle {
            let _ = handle.await;
        }
        if let Some(handle) = stderr_handle {
            let _ = handle.await;
        }
    }

    let _ = message_sender.send(VmMessage::StepCompleted { id, exit_code });
}

/// Read from a child output stream and send buffered output messages.
async fn stream_output<R: AsyncReadExt + Unpin>(
    id: u64,
    stream: OutputStream,
    mut reader: R,
    sender: mpsc::UnboundedSender<VmMessage>,
    config: OutputBufferConfig,
) {
    let mut buffer = vec![0u8; config.max_buffer_size];
    let mut pending = Vec::new();
    let mut flush_interval = tokio::time::interval(config.flush_interval);
    // The first tick completes immediately; consume it so we start waiting.
    flush_interval.tick().await;

    loop {
        tokio::select! {
            result = reader.read(&mut buffer) => {
                match result {
                    Ok(0) => {
                        // EOF: flush remaining data and exit
                        if !pending.is_empty() {
                            flush_output(id, stream, &mut pending, &sender);
                        }
                        return;
                    }
                    Ok(n) => {
                        pending.extend_from_slice(&buffer[..n]);
                        if pending.len() >= config.max_buffer_size {
                            flush_output(id, stream, &mut pending, &sender);
                        }
                    }
                    Err(_) => {
                        if !pending.is_empty() {
                            flush_output(id, stream, &mut pending, &sender);
                        }
                        return;
                    }
                }
            }
            _ = flush_interval.tick() => {
                if !pending.is_empty() {
                    flush_output(id, stream, &mut pending, &sender);
                }
            }
        }
    }
}

/// Flush the pending buffer as a single output message.
fn flush_output(
    id: u64,
    stream: OutputStream,
    pending: &mut Vec<u8>,
    sender: &mpsc::UnboundedSender<VmMessage>,
) {
    let data = String::from_utf8_lossy(pending).into_owned();
    pending.clear();
    let _ = sender.send(VmMessage::Output { id, stream, data });
}

/// Terminate a process group: SIGTERM first, then SIGKILL after timeout.
///
/// Only available on Unix. On other platforms, the caller uses `child.kill()`
/// directly (cross-platform fallback).
#[cfg(unix)]
async fn terminate_process_group(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    let pgid = -(pid as i32);

    unsafe {
        libc::kill(pgid, libc::SIGTERM);
    }

    tokio::time::sleep(CANCEL_KILL_TIMEOUT).await;

    unsafe {
        libc::kill(pgid, libc::SIGKILL);
    }
}

/// Cancel a running command by sending the cancel signal.
pub async fn cancel_command(mut handle: CommandHandle) {
    if let Some(sender) = handle.cancel_sender.take() {
        let _ = sender.send(());
    }
    // Wait for the task to finish (it will send StepCompleted)
    let _ = handle.task_handle.await;
}
