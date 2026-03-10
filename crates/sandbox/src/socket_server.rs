use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{mpsc, watch};

use codeagent_mcp::protocol::{JsonRpcNotification, JsonRpcResponse};
use codeagent_mcp::{McpHandler, McpRouter, McpServer};

use crate::config::{load_config, SandboxTomlConfig};

/// Run a side-channel MCP server for desktop app connections.
///
/// On Unix: listens on a Unix domain socket at `socket_path`.
/// On Windows: listens on TCP `127.0.0.1` and writes the port to `socket_path`
/// (since tokio doesn't expose Unix sockets on Windows).
///
/// Each accepted connection gets its own `McpRouter` and `McpServer` with an
/// independent MCP handshake. The handler is shared via `Arc`.
///
/// Runs until `shutdown` receives a value or the task is dropped.
pub async fn run_socket_server(
    socket_path: PathBuf,
    handler: Arc<dyn McpHandler>,
    root_dir: PathBuf,
    working_dirs: Vec<PathBuf>,
    shutdown: watch::Receiver<bool>,
) {
    #[cfg(unix)]
    run_unix_socket_server(socket_path, handler, root_dir, working_dirs, shutdown).await;

    #[cfg(windows)]
    run_tcp_socket_server(socket_path, handler, root_dir, working_dirs, shutdown).await;
}

#[cfg(unix)]
async fn run_unix_socket_server(
    socket_path: PathBuf,
    handler: Arc<dyn McpHandler>,
    root_dir: PathBuf,
    working_dirs: Vec<PathBuf>,
    mut shutdown: watch::Receiver<bool>,
) {
    use tokio::net::UnixListener;

    // Remove stale socket file from a previous run
    let _ = std::fs::remove_file(&socket_path);

    // Ensure parent directory exists
    if let Some(parent) = socket_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let listener = match UnixListener::bind(&socket_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "{{\"level\":\"error\",\"message\":\"failed to bind socket {}: {e}\"}}",
                socket_path.display()
            );
            return;
        }
    };

    eprintln!(
        "{{\"level\":\"info\",\"message\":\"socket server listening on {}\"}}",
        socket_path.display()
    );

    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            result = listener.accept() => {
                match result {
                    Ok((stream, _addr)) => {
                        let handler = Arc::clone(&handler);
                        let root = root_dir.clone();
                        let dirs = working_dirs.clone();
                        tokio::spawn(async move {
                            let (reader, writer) = tokio::io::split(stream);
                            handle_connection(reader, writer, handler, root, dirs).await;
                        });
                    }
                    Err(e) => {
                        eprintln!(
                            "{{\"level\":\"warn\",\"message\":\"socket accept error: {e}\"}}"
                        );
                    }
                }
            }
        }
    }

    let _ = std::fs::remove_file(&socket_path);
}

#[cfg(windows)]
async fn run_tcp_socket_server(
    socket_path: PathBuf,
    handler: Arc<dyn McpHandler>,
    root_dir: PathBuf,
    working_dirs: Vec<PathBuf>,
    mut shutdown: watch::Receiver<bool>,
) {
    use tokio::net::TcpListener;

    // Bind to localhost with port 0 to get an OS-assigned port
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "{{\"level\":\"error\",\"message\":\"failed to bind TCP socket: {e}\"}}"
            );
            return;
        }
    };

    let local_addr = match listener.local_addr() {
        Ok(addr) => addr,
        Err(e) => {
            eprintln!(
                "{{\"level\":\"error\",\"message\":\"failed to get local address: {e}\"}}"
            );
            return;
        }
    };

    // Write the port to the socket_path file so the desktop app can find it
    if let Some(parent) = socket_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&socket_path, local_addr.port().to_string()) {
        eprintln!(
            "{{\"level\":\"error\",\"message\":\"failed to write port file {}: {e}\"}}",
            socket_path.display()
        );
        return;
    }

    eprintln!(
        "{{\"level\":\"info\",\"message\":\"socket server listening on {} (port file: {})\"}}",
        local_addr,
        socket_path.display()
    );

    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            result = listener.accept() => {
                match result {
                    Ok((stream, _addr)) => {
                        let handler = Arc::clone(&handler);
                        let root = root_dir.clone();
                        let dirs = working_dirs.clone();
                        tokio::spawn(async move {
                            let (reader, writer) = tokio::io::split(stream);
                            handle_connection(reader, writer, handler, root, dirs).await;
                        });
                    }
                    Err(e) => {
                        eprintln!(
                            "{{\"level\":\"warn\",\"message\":\"socket accept error: {e}\"}}"
                        );
                    }
                }
            }
        }
    }

    let _ = std::fs::remove_file(&socket_path);
}

async fn handle_connection<R, W>(
    reader: R,
    writer: W,
    handler: Arc<dyn McpHandler>,
    root_dir: PathBuf,
    working_dirs: Vec<PathBuf>,
) where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let (_notification_sender, notification_receiver) =
        mpsc::unbounded_channel::<JsonRpcNotification>();
    let mut router = McpRouter::with_working_dirs(root_dir, &working_dirs, handler);
    router.set_custom_method_handler(Box::new(handle_sandbox_method));
    let mut server = McpServer::new(router, notification_receiver);

    if let Err(e) = server.run(reader, writer).await {
        eprintln!("{{\"level\":\"warn\",\"message\":\"socket connection closed: {e}\"}}");
    }
}

/// Handle `sandbox/*` JSON-RPC methods (only available on the side-channel socket).
///
/// Returns `Some` for recognized sandbox methods, `None` to fall through to
/// the standard MCP dispatch.
fn handle_sandbox_method(
    method: &str,
    id: Option<serde_json::Value>,
    params: serde_json::Value,
) -> Option<JsonRpcResponse> {
    let response = match method {
        "sandbox/get_config" => {
            let config_path = params
                .get("config_path")
                .and_then(|v| v.as_str())
                .map(std::path::Path::new);
            let config = load_config(config_path);
            match serde_json::to_value(&config) {
                Ok(value) => JsonRpcResponse::success(id, value),
                Err(e) => JsonRpcResponse::error(
                    id,
                    codeagent_mcp::JsonRpcError {
                        code: -32603,
                        message: format!("failed to serialize config: {e}"),
                        data: None,
                    },
                ),
            }
        }
        "sandbox/set_config" => {
            let config: SandboxTomlConfig = match serde_json::from_value(
                params.get("config").cloned().unwrap_or_default(),
            ) {
                Ok(c) => c,
                Err(e) => {
                    return Some(JsonRpcResponse::error(
                        id,
                        codeagent_mcp::JsonRpcError {
                            code: -32602,
                            message: format!("invalid config: {e}"),
                            data: None,
                        },
                    ));
                }
            };

            let config_path = params
                .get("config_path")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .or_else(crate::config::default_config_file_path);

            let Some(path) = config_path else {
                return Some(JsonRpcResponse::error(
                    id,
                    codeagent_mcp::JsonRpcError {
                        code: -32603,
                        message: "cannot determine config file path".into(),
                        data: None,
                    },
                ));
            };

            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            match toml::to_string_pretty(&config) {
                Ok(contents) => match std::fs::write(&path, &contents) {
                    Ok(()) => {
                        JsonRpcResponse::success(id, serde_json::json!({"written": true}))
                    }
                    Err(e) => JsonRpcResponse::error(
                        id,
                        codeagent_mcp::JsonRpcError {
                            code: -32603,
                            message: format!("failed to write config: {e}"),
                            data: None,
                        },
                    ),
                },
                Err(e) => JsonRpcResponse::error(
                    id,
                    codeagent_mcp::JsonRpcError {
                        code: -32603,
                        message: format!("failed to serialize config: {e}"),
                        data: None,
                    },
                ),
            }
        }
        _ => return None,
    };
    Some(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codeagent_mcp::protocol::{
        BashArgs, DiscardUndoHistoryArgs, EditFileArgs, GetUndoHistoryArgs, GlobArgs, GrepArgs,
        ReadFileArgs, UndoArgs, WriteFileArgs,
    };
    use codeagent_mcp::McpError;
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    struct StubHandler;

    impl McpHandler for StubHandler {
        fn bash(&self, _: BashArgs) -> Result<serde_json::Value, McpError> {
            Ok(json!({"exit_code": 0, "stdout": "ok", "stderr": ""}))
        }
        fn read_file(&self, _: ReadFileArgs) -> Result<serde_json::Value, McpError> {
            Ok(json!({"content": "test"}))
        }
        fn write_file(&self, _: WriteFileArgs) -> Result<serde_json::Value, McpError> {
            Ok(json!({"bytes_written": 0}))
        }
        fn edit_file(&self, _: EditFileArgs) -> Result<serde_json::Value, McpError> {
            Ok(json!("ok"))
        }
        fn glob(&self, _: GlobArgs) -> Result<serde_json::Value, McpError> {
            Ok(json!(""))
        }
        fn grep(&self, _: GrepArgs) -> Result<serde_json::Value, McpError> {
            Ok(json!(""))
        }
        fn undo(&self, _: UndoArgs) -> Result<serde_json::Value, McpError> {
            Ok(json!({"steps_rolled_back": 0}))
        }
        fn get_undo_history(&self, _: GetUndoHistoryArgs) -> Result<serde_json::Value, McpError> {
            Ok(json!({"steps": []}))
        }
        fn get_session_status(&self) -> Result<serde_json::Value, McpError> {
            Ok(json!({"state": "active"}))
        }
        fn discard_undo_history(
            &self,
            _: DiscardUndoHistoryArgs,
        ) -> Result<serde_json::Value, McpError> {
            Ok(json!({}))
        }
    }

    #[tokio::test]
    async fn socket_server_accepts_and_responds() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("test.sock");

        let handler: Arc<dyn McpHandler> = Arc::new(StubHandler);
        let root = dir.path().to_path_buf();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let server_socket = socket_path.clone();
        let server_root = root.clone();
        let server_handle = tokio::spawn(async move {
            run_socket_server(
                server_socket,
                handler,
                server_root.clone(),
                vec![server_root],
                shutdown_rx,
            )
            .await;
        });

        // Wait for the server to be ready
        for _ in 0..50 {
            #[cfg(unix)]
            let ready = socket_path.exists();
            #[cfg(windows)]
            let ready = socket_path.exists();

            if ready {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // Connect to the server
        #[cfg(unix)]
        let stream = tokio::net::UnixStream::connect(&socket_path).await.unwrap();

        #[cfg(windows)]
        let stream = {
            let port_str = std::fs::read_to_string(&socket_path).unwrap();
            let port: u16 = port_str.trim().parse().unwrap();
            tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
                .await
                .unwrap()
        };

        let (reader, mut writer) = tokio::io::split(stream);
        let mut reader = BufReader::new(reader);

        // Send initialize
        let init_req = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        });
        writer
            .write_all(format!("{}\n", init_req).as_bytes())
            .await
            .unwrap();
        writer.flush().await.unwrap();

        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let resp: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(resp["id"], 1);
        assert!(resp["result"]["protocolVersion"].is_string());

        // Send tools/list
        let list_req = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        });
        writer
            .write_all(format!("{}\n", list_req).as_bytes())
            .await
            .unwrap();
        writer.flush().await.unwrap();

        let mut line2 = String::new();
        reader.read_line(&mut line2).await.unwrap();
        let resp2: serde_json::Value = serde_json::from_str(&line2).unwrap();
        assert_eq!(resp2["id"], 2);
        assert!(resp2["result"]["tools"].is_array());

        // Send get_session_status via tools/call
        let status_req = json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "get_session_status",
                "arguments": {}
            }
        });
        writer
            .write_all(format!("{}\n", status_req).as_bytes())
            .await
            .unwrap();
        writer.flush().await.unwrap();

        let mut line3 = String::new();
        reader.read_line(&mut line3).await.unwrap();
        let resp3: serde_json::Value = serde_json::from_str(&line3).unwrap();
        assert_eq!(resp3["id"], 3);

        // Send sandbox/get_config (custom method handled via pre-dispatch hook)
        let config_req = json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "sandbox/get_config",
            "params": {}
        });
        writer
            .write_all(format!("{}\n", config_req).as_bytes())
            .await
            .unwrap();
        writer.flush().await.unwrap();

        let mut line4 = String::new();
        reader.read_line(&mut line4).await.unwrap();
        let resp4: serde_json::Value = serde_json::from_str(&line4).unwrap();
        assert_eq!(resp4["id"], 4);
        assert!(resp4["result"].is_object(), "sandbox/get_config should return config");

        // Clean up
        drop(writer);
        let _ = shutdown_tx.send(true);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), server_handle).await;
    }
}
