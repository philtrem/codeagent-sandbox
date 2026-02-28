use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::error::McpError;
use crate::parser::{extract_id, parse_jsonrpc};
use crate::protocol::{JsonRpcNotification, JsonRpcResponse};
use crate::router::McpRouter;

/// Async MCP server that reads JSON-RPC from an input stream, dispatches
/// through the `McpRouter`, and writes responses/notifications to an output
/// stream.
///
/// Transport-agnostic: in production the input/output are connected to a Unix
/// domain socket (or named pipe on Windows); in tests they are
/// `tokio::io::DuplexStream`s.
pub struct McpServer {
    router: McpRouter,
    notification_receiver: mpsc::UnboundedReceiver<JsonRpcNotification>,
}

impl McpServer {
    pub fn new(
        router: McpRouter,
        notification_receiver: mpsc::UnboundedReceiver<JsonRpcNotification>,
    ) -> Self {
        Self {
            router,
            notification_receiver,
        }
    }

    /// Run the server loop.
    ///
    /// Reads JSON Lines from `input`, dispatches requests through the router,
    /// and writes responses and notifications to `output`. The loop terminates
    /// when the input stream closes (EOF).
    pub async fn run<R, W>(
        &mut self,
        input: R,
        mut output: W,
    ) -> Result<(), McpError>
    where
        R: tokio::io::AsyncRead + Unpin,
        W: tokio::io::AsyncWrite + Unpin,
    {
        let mut lines = BufReader::new(input).lines();

        loop {
            tokio::select! {
                line_result = lines.next_line() => {
                    match line_result {
                        Ok(Some(line)) => {
                            let response = match parse_jsonrpc(&line) {
                                Ok(request) => self.router.dispatch(request),
                                Err(error) => {
                                    let id = extract_id(&line);
                                    Some(JsonRpcResponse::error(id, error.to_jsonrpc_error()))
                                }
                            };
                            if let Some(resp) = response {
                                write_jsonl(&mut output, &resp).await?;
                            }
                        }
                        Ok(None) => break, // EOF
                        Err(e) => return Err(McpError::Io { source: e }),
                    }
                }

                Some(notification) = self.notification_receiver.recv() => {
                    write_jsonl(&mut output, &notification).await?;
                }
            }
        }

        Ok(())
    }
}

/// Write a serializable value as a single JSON line.
async fn write_jsonl<W: tokio::io::AsyncWrite + Unpin, T: serde::Serialize>(
    writer: &mut W,
    value: &T,
) -> Result<(), McpError> {
    let json = serde_json::to_string(value).map_err(|e| McpError::Io {
        source: std::io::Error::other(e),
    })?;
    writer
        .write_all(json.as_bytes())
        .await
        .map_err(|source| McpError::Io { source })?;
    writer
        .write_all(b"\n")
        .await
        .map_err(|source| McpError::Io { source })?;
    writer
        .flush()
        .await
        .map_err(|source| McpError::Io { source })?;
    Ok(())
}
