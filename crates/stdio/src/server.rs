use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::error::StdioError;
use crate::parser::{extract_request_id, parse_request};
use crate::protocol::{Event, LogEntry, ResponseEnvelope};
use crate::router::Router;

/// Async STDIO API server that reads JSON Lines from an input, dispatches
/// through a `Router`, and writes responses/events to an output.
///
/// Log messages are written to a separate output (stderr in production).
pub struct StdioServer {
    router: Router,
    event_receiver: mpsc::UnboundedReceiver<Event>,
    log_sender: Option<LogSender>,
}

type LogSender = Box<dyn Fn(LogEntry) + Send + Sync>;

impl StdioServer {
    pub fn new(router: Router, event_receiver: mpsc::UnboundedReceiver<Event>) -> Self {
        Self {
            router,
            event_receiver,
            log_sender: None,
        }
    }

    /// Set a log handler that will be called for each log entry.
    pub fn with_log_sender(mut self, sender: impl Fn(LogEntry) + Send + Sync + 'static) -> Self {
        self.log_sender = Some(Box::new(sender));
        self
    }

    /// Run the server loop.
    ///
    /// Reads JSON Lines from `input`, dispatches requests through the router,
    /// and writes responses and events to `output`. Log entries are written
    /// to `log_output`.
    ///
    /// The loop terminates when the input stream closes (EOF).
    pub async fn run<R, W, L>(
        &mut self,
        input: R,
        mut output: W,
        mut log_output: L,
    ) -> Result<(), StdioError>
    where
        R: tokio::io::AsyncRead + Unpin,
        W: tokio::io::AsyncWrite + Unpin,
        L: tokio::io::AsyncWrite + Unpin,
    {
        let mut lines = BufReader::new(input).lines();

        loop {
            tokio::select! {
                line_result = lines.next_line() => {
                    match line_result {
                        Ok(Some(line)) => {
                            self.emit_log(
                                &mut log_output,
                                "debug",
                                "stdio_api",
                                None,
                                &format!("received: {}", truncate_for_log(&line)),
                            ).await;

                            let response = match parse_request(&line) {
                                Ok(request) => {
                                    let request_id = request.request_id().to_string();
                                    self.emit_log(
                                        &mut log_output,
                                        "info",
                                        "stdio_api",
                                        Some(&request_id),
                                        &format!("dispatching request type: {}", request_type_name(&request)),
                                    ).await;
                                    self.router.dispatch(request)
                                }
                                Err(error) => {
                                    let request_id = extract_request_id(&line)
                                        .unwrap_or_default();
                                    self.emit_log(
                                        &mut log_output,
                                        "warn",
                                        "stdio_api",
                                        if request_id.is_empty() { None } else { Some(&request_id) },
                                        &format!("parse error: {error}"),
                                    ).await;
                                    ResponseEnvelope::error(request_id, error.to_error_detail())
                                }
                            };

                            write_jsonl(&mut output, &response).await?;
                        }
                        Ok(None) => break, // EOF
                        Err(e) => return Err(StdioError::Io { source: e }),
                    }
                }

                Some(event) = self.event_receiver.recv() => {
                    let envelope = event.to_envelope();
                    write_jsonl(&mut output, &envelope).await?;
                }
            }
        }

        Ok(())
    }

    async fn emit_log<L: tokio::io::AsyncWrite + Unpin>(
        &self,
        log_output: &mut L,
        level: &str,
        component: &str,
        request_id: Option<&str>,
        message: &str,
    ) {
        let entry = LogEntry {
            timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
            level: level.to_string(),
            component: component.to_string(),
            request_id: request_id.map(String::from),
            step_id: None,
            message: message.to_string(),
        };

        if let Some(ref sender) = self.log_sender {
            sender(entry.clone());
        }

        if let Ok(json) = serde_json::to_string(&entry) {
            let _ = log_output.write_all(json.as_bytes()).await;
            let _ = log_output.write_all(b"\n").await;
            let _ = log_output.flush().await;
        }
    }
}

/// Write a serializable value as a single JSON line.
async fn write_jsonl<W: tokio::io::AsyncWrite + Unpin, T: serde::Serialize>(
    writer: &mut W,
    value: &T,
) -> Result<(), StdioError> {
    let json = serde_json::to_string(value).map_err(|e| StdioError::Io {
        source: std::io::Error::other(e),
    })?;
    writer
        .write_all(json.as_bytes())
        .await
        .map_err(|source| StdioError::Io { source })?;
    writer
        .write_all(b"\n")
        .await
        .map_err(|source| StdioError::Io { source })?;
    writer.flush().await.map_err(|source| StdioError::Io { source })?;
    Ok(())
}

fn request_type_name(request: &crate::protocol::Request) -> &'static str {
    match request {
        crate::protocol::Request::SessionStart { .. } => "session.start",
        crate::protocol::Request::SessionStop { .. } => "session.stop",
        crate::protocol::Request::SessionReset { .. } => "session.reset",
        crate::protocol::Request::SessionStatus { .. } => "session.status",
        crate::protocol::Request::UndoRollback { .. } => "undo.rollback",
        crate::protocol::Request::UndoHistory { .. } => "undo.history",
        crate::protocol::Request::UndoConfigure { .. } => "undo.configure",
        crate::protocol::Request::UndoDiscard { .. } => "undo.discard",
        crate::protocol::Request::AgentExecute { .. } => "agent.execute",
        crate::protocol::Request::AgentPrompt { .. } => "agent.prompt",
        crate::protocol::Request::FsList { .. } => "fs.list",
        crate::protocol::Request::FsRead { .. } => "fs.read",
        crate::protocol::Request::FsStatus { .. } => "fs.status",
        crate::protocol::Request::SafeguardConfigure { .. } => "safeguard.configure",
        crate::protocol::Request::SafeguardConfirm { .. } => "safeguard.confirm",
    }
}

fn truncate_for_log(line: &str) -> &str {
    const MAX_LOG_LEN: usize = 200;
    if line.len() <= MAX_LOG_LEN {
        line
    } else {
        &line[..MAX_LOG_LEN]
    }
}
