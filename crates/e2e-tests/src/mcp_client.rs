use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::jsonl_client::E2eError;

/// MCP test client that connects to a Unix domain socket and speaks JSON-RPC 2.0.
pub struct McpClient {
    writer: tokio::io::WriteHalf<UnixStream>,
    reader: BufReader<tokio::io::ReadHalf<UnixStream>>,
    next_id: u64,
}

impl McpClient {
    /// Connect to the MCP socket at the given path.
    pub async fn connect(socket_path: &Path) -> Result<Self, E2eError> {
        let stream = UnixStream::connect(socket_path).await?;
        let (reader, writer) = tokio::io::split(stream);
        Ok(Self {
            writer,
            reader: BufReader::new(reader),
            next_id: 1,
        })
    }

    /// Send a JSON-RPC 2.0 request and receive the response.
    ///
    /// Notifications (messages without an `id` field) received before the
    /// response are silently skipped.
    pub async fn call(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, E2eError> {
        let id = self.next_id;
        self.next_id += 1;

        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let line = serde_json::to_string(&request)?;
        self.writer.write_all(line.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await?;

        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let mut line = String::new();
            match tokio::time::timeout_at(deadline, self.reader.read_line(&mut line)).await {
                Ok(Ok(0)) => {
                    return Err(E2eError::ProcessExited("MCP socket closed".into()));
                }
                Ok(Ok(_)) => {
                    let parsed: Value = serde_json::from_str(line.trim())?;
                    // Skip notifications (no id field)
                    if parsed.get("id").is_some() {
                        return Ok(parsed);
                    }
                }
                Ok(Err(e)) => return Err(E2eError::Io(e)),
                Err(_) => {
                    return Err(E2eError::ResponseTimeout {
                        request_id: id.to_string(),
                    });
                }
            }
        }
    }

    /// Perform the MCP initialize handshake (initialize + initialized notification).
    pub async fn initialize(&mut self, timeout: Duration) -> Result<Value, E2eError> {
        let resp = self
            .call(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "e2e-test", "version": "0.1.0" }
                }),
                timeout,
            )
            .await?;

        // Send the initialized notification (no response expected)
        let notification = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        let line = serde_json::to_string(&notification)?;
        self.writer.write_all(line.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await?;

        Ok(resp)
    }
}
