pub mod error;
pub mod executor;
pub mod output_buffer;

use std::collections::HashMap;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use codeagent_control::{HostMessage, VmMessage, parse_host_message, MAX_MESSAGE_SIZE};

use error::ShimError;
use executor::CommandHandle;
use output_buffer::OutputBufferConfig;

/// The shim's runtime state, tracking currently executing commands.
struct Shim {
    running_commands: HashMap<u64, CommandHandle>,
    message_sender: mpsc::UnboundedSender<VmMessage>,
    buffer_config: OutputBufferConfig,
}

impl Shim {
    fn new(
        message_sender: mpsc::UnboundedSender<VmMessage>,
        buffer_config: OutputBufferConfig,
    ) -> Self {
        Self {
            running_commands: HashMap::new(),
            message_sender,
            buffer_config,
        }
    }

    /// Dispatch a single host message.
    fn handle_message(&mut self, msg: HostMessage) -> Result<(), ShimError> {
        match msg {
            HostMessage::Exec {
                id,
                command,
                cwd,
                env,
            } => {
                let handle = executor::spawn_command(
                    id,
                    &command,
                    cwd.as_deref(),
                    env.as_ref(),
                    self.message_sender.clone(),
                    self.buffer_config.clone(),
                )?;
                self.running_commands.insert(id, handle);
                Ok(())
            }
            HostMessage::Cancel { id } => {
                if let Some(handle) = self.running_commands.remove(&id) {
                    // Spawn cancel as a task so we don't block the message loop.
                    tokio::spawn(executor::cancel_command(handle));
                }
                Ok(())
            }
            HostMessage::RollbackNotify { .. } => {
                // Informational only — the host already rolled back the filesystem.
                Ok(())
            }
        }
    }

    /// Remove entries for commands that have finished.
    fn reap_completed(&mut self) {
        self.running_commands.retain(|_, handle| !handle.is_finished());
    }

    /// Cancel all running commands (used during shutdown).
    async fn cancel_all(&mut self) {
        let ids: Vec<u64> = self.running_commands.keys().copied().collect();
        for id in ids {
            if let Some(handle) = self.running_commands.remove(&id) {
                executor::cancel_command(handle).await;
            }
        }
    }
}

/// Run the shim: read host messages from `reader`, write VM messages to `writer`.
///
/// Generic over `AsyncRead`/`AsyncWrite` so integration tests can use
/// `tokio::io::duplex()` instead of real device files.
pub async fn run<R, W>(reader: R, writer: W) -> Result<(), ShimError>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (message_sender, mut message_receiver) = mpsc::unbounded_channel::<VmMessage>();
    let mut shim = Shim::new(message_sender, OutputBufferConfig::default());

    let mut lines = BufReader::new(reader).lines();

    // Writer task: serialize VmMessages as JSON Lines to the output.
    let writer_handle: JoinHandle<Result<(), ShimError>> = tokio::spawn(async move {
        let mut writer = tokio::io::BufWriter::new(writer);
        while let Some(msg) = message_receiver.recv().await {
            let json = serde_json::to_string(&msg)?;
            writer.write_all(json.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
        }
        Ok(())
    });

    // Reader loop: parse host messages and dispatch.
    while let Ok(Some(line)) = lines.next_line().await {
        if line.len() > MAX_MESSAGE_SIZE {
            eprintln!("skipping oversized message ({} bytes)", line.len());
            continue;
        }
        match parse_host_message(&line) {
            Ok(msg) => {
                if let Err(error) = shim.handle_message(msg) {
                    eprintln!("error handling message: {error}");
                }
            }
            Err(error) => {
                eprintln!("parse error: {error}");
            }
        }
        shim.reap_completed();
    }

    // Control channel closed — graceful shutdown.
    shim.cancel_all().await;

    // Drop the shim (and its message_sender) so the writer task exits.
    drop(shim);
    let _ = writer_handle.await;

    Ok(())
}
