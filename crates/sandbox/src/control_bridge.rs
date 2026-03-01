use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use codeagent_control::{ControlChannelHandler, StepManager, parse_vm_message};
use codeagent_stdio::Event;

/// Spawn a background task that writes host messages to the control channel.
///
/// Returns a sender that the orchestrator uses to enqueue serialized messages.
/// Each message is written as a JSON Line (with trailing newline and flush).
pub fn spawn_control_writer<W>(writer: W) -> (mpsc::UnboundedSender<String>, JoinHandle<()>)
where
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (sender, mut receiver) = mpsc::unbounded_channel::<String>();

    let handle = tokio::spawn(async move {
        let mut writer = tokio::io::BufWriter::new(writer);
        while let Some(line) = receiver.recv().await {
            if writer.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            if writer.write_all(b"\n").await.is_err() {
                break;
            }
            if writer.flush().await.is_err() {
                break;
            }
        }
    });

    (sender, handle)
}

/// Spawn a background task that reads VM messages from the control channel
/// and dispatches them through the handler.
///
/// On parse errors or channel close, emits error events via the event sender.
pub fn spawn_control_reader<R, S>(
    reader: R,
    handler: Arc<ControlChannelHandler<S>>,
    event_sender: mpsc::UnboundedSender<Event>,
) -> JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    S: StepManager + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();

        while let Ok(Some(line)) = lines.next_line().await {
            match parse_vm_message(&line) {
                Ok(msg) => {
                    handler.handle_vm_message(msg).await;
                }
                Err(error) => {
                    let _ = event_sender.send(Event::Error {
                        code: "control_channel_parse_error".to_string(),
                        message: error.to_string(),
                    });
                }
            }
        }

        // Control channel closed
        let _ = event_sender.send(Event::Error {
            code: "control_channel_closed".to_string(),
            message: "VM control channel disconnected".to_string(),
        });
    })
}

/// Serialize a `VmMessage` as JSON for sending through the control writer.
pub fn serialize_host_message(msg: &codeagent_control::HostMessage) -> Result<String, serde_json::Error> {
    serde_json::to_string(msg)
}
