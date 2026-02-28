use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use codeagent_mcp::protocol::{
    ExecuteCommandArgs, GetUndoHistoryArgs, JsonRpcNotification, ListDirectoryArgs, ReadFileArgs,
    UndoArgs, WriteFileArgs,
};
use codeagent_mcp::{McpError, McpHandler, McpRouter, McpServer};

// ---------------------------------------------------------------------------
// StubMcpHandler — minimal canned responses for protocol-level tests
// ---------------------------------------------------------------------------

struct StubMcpHandler;

impl McpHandler for StubMcpHandler {
    fn execute_command(&self, args: ExecuteCommandArgs) -> Result<Value, McpError> {
        Ok(json!({
            "exit_code": 0,
            "stdout": format!("executed: {}", args.command),
            "stderr": ""
        }))
    }

    fn read_file(&self, args: ReadFileArgs) -> Result<Value, McpError> {
        Ok(json!({ "content": format!("contents of {}", args.path) }))
    }

    fn write_file(&self, _args: WriteFileArgs) -> Result<Value, McpError> {
        Ok(json!({ "bytes_written": 0 }))
    }

    fn list_directory(&self, _args: ListDirectoryArgs) -> Result<Value, McpError> {
        Ok(json!({ "entries": [] }))
    }

    fn undo(&self, _args: UndoArgs) -> Result<Value, McpError> {
        Ok(json!({ "steps_rolled_back": 0 }))
    }

    fn get_undo_history(&self, _args: GetUndoHistoryArgs) -> Result<Value, McpError> {
        Ok(json!({ "steps": [] }))
    }

    fn get_session_status(&self) -> Result<Value, McpError> {
        Ok(json!({ "state": "idle" }))
    }
}

// ---------------------------------------------------------------------------
// McpTestHarness — in-memory duplex transport for integration tests
// ---------------------------------------------------------------------------

struct McpTestHarness {
    input_writer: tokio::io::DuplexStream,
    output_reader: BufReader<tokio::io::DuplexStream>,
    notification_sender: mpsc::UnboundedSender<JsonRpcNotification>,
    _server_handle: tokio::task::JoinHandle<Result<(), McpError>>,
}

impl McpTestHarness {
    fn new() -> Self {
        Self::with_handler(Box::new(StubMcpHandler), test_root())
    }

    fn with_handler(handler: Box<dyn McpHandler>, root: PathBuf) -> Self {
        let (input_writer, input_reader) = tokio::io::duplex(8192);
        let (output_writer, output_reader) = tokio::io::duplex(8192);

        let (notification_sender, notification_receiver) = mpsc::unbounded_channel();

        let router = McpRouter::new(root, handler);
        let mut server = McpServer::new(router, notification_receiver);

        let server_handle = tokio::spawn(async move { server.run(input_reader, output_writer).await });

        Self {
            input_writer,
            output_reader: BufReader::new(output_reader),
            notification_sender,
            _server_handle: server_handle,
        }
    }

    async fn send_line(&mut self, line: &str) {
        let data = format!("{line}\n");
        self.input_writer
            .write_all(data.as_bytes())
            .await
            .expect("failed to write to server input");
        self.input_writer
            .flush()
            .await
            .expect("failed to flush server input");
    }

    async fn recv_line(&mut self) -> String {
        let mut line = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.output_reader.read_line(&mut line),
        )
        .await
        .expect("timeout reading server output")
        .expect("I/O error reading server output");
        line.trim_end().to_string()
    }

    /// Send a JSON-RPC request and read back the response as parsed JSON.
    async fn send_request(&mut self, id: impl Into<Value>, method: &str, params: Value) -> Value {
        let request = json!({
            "jsonrpc": "2.0",
            "id": id.into(),
            "method": method,
            "params": params,
        });
        self.send_line(&serde_json::to_string(&request).unwrap())
            .await;
        let line = self.recv_line().await;
        serde_json::from_str(&line).expect("response is not valid JSON")
    }

    /// Perform the MCP initialize handshake.
    async fn initialize(&mut self) {
        // Send initialize
        let resp = self
            .send_request(
                1,
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "test-client", "version": "0.1.0" }
                }),
            )
            .await;
        assert_eq!(resp["jsonrpc"], "2.0");
        assert!(resp.get("result").is_some());

        // Send initialized notification (no response expected)
        self.send_line(
            &serde_json::to_string(&json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }))
            .unwrap(),
        )
        .await;
    }

    fn inject_notification(&self, notification: JsonRpcNotification) {
        self.notification_sender
            .send(notification)
            .expect("failed to inject notification");
    }
}

fn test_root() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\sandbox\working")
    } else {
        PathBuf::from("/sandbox/working")
    }
}

// ===========================================================================
// MC-01: JSON-RPC Compliance
// ===========================================================================

#[tokio::test]
async fn mc01_response_has_jsonrpc_version() {
    let mut harness = McpTestHarness::new();
    let resp = harness
        .send_request(1, "initialize", json!({"protocolVersion": "2024-11-05", "capabilities": {}}))
        .await;
    assert_eq!(resp["jsonrpc"], "2.0");
}

#[tokio::test]
async fn mc01_response_id_matches_request() {
    let mut harness = McpTestHarness::new();
    harness.initialize().await;

    let resp = harness.send_request(42, "tools/list", json!({})).await;
    assert_eq!(resp["id"], 42);

    let resp = harness
        .send_request("req-abc", "tools/list", json!({}))
        .await;
    assert_eq!(resp["id"], "req-abc");
}

#[tokio::test]
async fn mc01_unknown_method_returns_method_not_found() {
    let mut harness = McpTestHarness::new();
    let resp = harness
        .send_request(1, "nonexistent/method", json!({}))
        .await;
    assert_eq!(resp["error"]["code"], -32601);
    assert!(resp.get("result").is_none());
}

#[tokio::test]
async fn mc01_malformed_json_returns_parse_error() {
    let mut harness = McpTestHarness::new();
    harness.send_line("not valid json {{").await;
    let line = harness.recv_line().await;
    let resp: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(resp["error"]["code"], -32700);
}

#[tokio::test]
async fn mc01_wrong_jsonrpc_version_returns_invalid_request() {
    let mut harness = McpTestHarness::new();
    harness
        .send_line(r#"{"jsonrpc":"1.0","id":1,"method":"tools/list"}"#)
        .await;
    let line = harness.recv_line().await;
    let resp: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(resp["error"]["code"], -32600);
}

#[tokio::test]
async fn mc01_notification_gets_no_response() {
    let mut harness = McpTestHarness::new();

    // Send a notification (no id)
    harness
        .send_line(
            &serde_json::to_string(&json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }))
            .unwrap(),
        )
        .await;

    // Send a normal request to verify server is still alive
    let resp = harness
        .send_request(99, "initialize", json!({"protocolVersion": "2024-11-05", "capabilities": {}}))
        .await;
    assert_eq!(resp["id"], 99);
    assert!(resp.get("result").is_some());
}

#[tokio::test]
async fn mc01_initialize_returns_capabilities() {
    let mut harness = McpTestHarness::new();
    let resp = harness
        .send_request(1, "initialize", json!({"protocolVersion": "2024-11-05", "capabilities": {}}))
        .await;
    let result = &resp["result"];
    assert!(result.get("capabilities").is_some());
    assert!(result["capabilities"].get("tools").is_some());
    assert!(result.get("serverInfo").is_some());
    assert_eq!(result["serverInfo"]["name"], "codeagent-mcp");
}

#[tokio::test]
async fn mc01_tools_list_returns_seven_tools() {
    let mut harness = McpTestHarness::new();
    harness.initialize().await;

    let resp = harness.send_request(2, "tools/list", json!({})).await;
    let tools = resp["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 7);

    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"execute_command"));
    assert!(names.contains(&"read_file"));
    assert!(names.contains(&"write_file"));
    assert!(names.contains(&"list_directory"));
    assert!(names.contains(&"undo"));
    assert!(names.contains(&"get_undo_history"));
    assert!(names.contains(&"get_session_status"));
}

// ===========================================================================
// MC-02: execute_command
// ===========================================================================

#[tokio::test]
async fn mc02_execute_command_returns_exit_code_stdout_stderr() {
    let mut harness = McpTestHarness::new();
    harness.initialize().await;

    let resp = harness
        .send_request(
            10,
            "tools/call",
            json!({
                "name": "execute_command",
                "arguments": { "command": "echo hello" }
            }),
        )
        .await;

    let result = &resp["result"];
    let content_text = result["content"][0]["text"].as_str().unwrap();
    let parsed: Value = serde_json::from_str(content_text).unwrap();
    assert_eq!(parsed["exit_code"], 0);
    assert_eq!(parsed["stdout"], "executed: echo hello");
    assert_eq!(parsed["stderr"], "");
}

#[tokio::test]
async fn mc02_execute_command_missing_command_returns_invalid_params() {
    let mut harness = McpTestHarness::new();
    harness.initialize().await;

    let resp = harness
        .send_request(
            11,
            "tools/call",
            json!({
                "name": "execute_command",
                "arguments": {}
            }),
        )
        .await;

    assert_eq!(resp["error"]["code"], -32602);
    assert!(resp["error"]["data"]["field"]
        .as_str()
        .unwrap()
        .contains("command"));
}

#[tokio::test]
async fn mc02_unknown_tool_returns_method_not_found() {
    let mut harness = McpTestHarness::new();
    harness.initialize().await;

    let resp = harness
        .send_request(
            12,
            "tools/call",
            json!({
                "name": "nonexistent_tool",
                "arguments": {}
            }),
        )
        .await;

    assert_eq!(resp["error"]["code"], -32601);
}

// ===========================================================================
// MC-05: write_file / read_file / list_directory — path containment
// ===========================================================================

#[tokio::test]
async fn mc05_write_file_path_traversal_returns_error() {
    let mut harness = McpTestHarness::new();
    harness.initialize().await;

    let resp = harness
        .send_request(
            20,
            "tools/call",
            json!({
                "name": "write_file",
                "arguments": {
                    "path": "../../etc/passwd",
                    "content": "malicious"
                }
            }),
        )
        .await;

    assert_eq!(resp["error"]["code"], -32001);
}

#[tokio::test]
async fn mc05_read_file_path_outside_root_returns_error() {
    let mut harness = McpTestHarness::new();
    harness.initialize().await;

    let outside = if cfg!(windows) {
        r"C:\Windows\System32\config"
    } else {
        "/etc/passwd"
    };

    let resp = harness
        .send_request(
            21,
            "tools/call",
            json!({
                "name": "read_file",
                "arguments": { "path": outside }
            }),
        )
        .await;

    assert_eq!(resp["error"]["code"], -32001);
}

#[tokio::test]
async fn mc05_list_directory_path_traversal_returns_error() {
    let mut harness = McpTestHarness::new();
    harness.initialize().await;

    let resp = harness
        .send_request(
            22,
            "tools/call",
            json!({
                "name": "list_directory",
                "arguments": { "path": "subdir/../../../etc" }
            }),
        )
        .await;

    assert_eq!(resp["error"]["code"], -32001);
}

// ===========================================================================
// MC-07: Connection without auth token (placeholder)
// ===========================================================================

#[tokio::test]
async fn mc07_auth_not_yet_implemented() {
    // Authentication on the MCP socket is not yet implemented.
    // When added, this test should verify that unauthenticated
    // connections are rejected. For now, verify that a connection
    // without any token succeeds (current behavior).
    let mut harness = McpTestHarness::new();
    let resp = harness
        .send_request(1, "initialize", json!({"protocolVersion": "2024-11-05", "capabilities": {}}))
        .await;
    assert!(resp.get("result").is_some());
}

// ===========================================================================
// MC-03: write_file creates synthetic API step
// ===========================================================================

use codeagent_interceptor::undo_interceptor::UndoInterceptor;
use codeagent_interceptor::write_interceptor::WriteInterceptor;
use codeagent_test_support::TempWorkspace;

/// Convert any error with Display into an McpError::InternalError.
fn to_internal<E: std::fmt::Display>(error: E) -> McpError {
    McpError::InternalError {
        message: error.to_string(),
    }
}

/// Handler backed by a real UndoInterceptor for testing undo integration.
struct UndoMcpHandler {
    interceptor: Arc<UndoInterceptor>,
    root_dir: PathBuf,
    next_step_id: AtomicI64,
}

impl UndoMcpHandler {
    fn new(interceptor: Arc<UndoInterceptor>, root_dir: PathBuf) -> Self {
        Self {
            interceptor,
            root_dir,
            next_step_id: AtomicI64::new(1000),
        }
    }
}

impl McpHandler for UndoMcpHandler {
    fn execute_command(&self, _args: ExecuteCommandArgs) -> Result<Value, McpError> {
        Ok(json!({ "exit_code": 0, "stdout": "", "stderr": "" }))
    }

    fn read_file(&self, args: ReadFileArgs) -> Result<Value, McpError> {
        let full_path = self.root_dir.join(&args.path);
        let content = std::fs::read_to_string(&full_path)
            .map_err(to_internal)?;
        Ok(json!({ "content": content }))
    }

    fn write_file(&self, args: WriteFileArgs) -> Result<Value, McpError> {
        let step_id = self.next_step_id.fetch_add(1, Ordering::SeqCst);
        let full_path = self.root_dir.join(&args.path);

        self.interceptor
            .open_step(step_id)
            .map_err(to_internal)?;

        let file_exists = full_path.exists();

        if file_exists {
            // Existing file: capture preimage before writing
            self.interceptor
                .pre_write(&full_path)
                .map_err(to_internal)?;
            std::fs::write(&full_path, &args.content)
                .map_err(to_internal)?;
        } else {
            // New file: create parent dirs if needed, write, record creation
            if let Some(parent) = full_path.parent() {
                if !parent.exists() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| McpError::InternalError {
                            message: e.to_string(),
                        })?;
                }
            }
            std::fs::write(&full_path, &args.content)
                .map_err(to_internal)?;
            self.interceptor
                .post_create(&full_path)
                .map_err(to_internal)?;
        }

        self.interceptor
            .close_step(step_id)
            .map_err(to_internal)?;

        Ok(json!({ "bytes_written": args.content.len(), "step_id": step_id }))
    }

    fn list_directory(&self, _args: ListDirectoryArgs) -> Result<Value, McpError> {
        Ok(json!({ "entries": [] }))
    }

    fn undo(&self, args: UndoArgs) -> Result<Value, McpError> {
        let result = self
            .interceptor
            .rollback(args.count as usize, args.force)
            .map_err(to_internal)?;
        Ok(json!({
            "steps_rolled_back": result.steps_rolled_back,
            "barriers_crossed": result.barriers_crossed,
        }))
    }

    fn get_undo_history(&self, _args: GetUndoHistoryArgs) -> Result<Value, McpError> {
        let steps = self.interceptor.completed_steps();
        Ok(json!({ "steps": steps }))
    }

    fn get_session_status(&self) -> Result<Value, McpError> {
        Ok(json!({ "state": "active" }))
    }
}

/// Create an UndoInterceptor + handler pair for a TempWorkspace.
fn make_undo_harness(ws: &TempWorkspace) -> (Arc<UndoInterceptor>, UndoMcpHandler) {
    let interceptor = Arc::new(UndoInterceptor::new(
        ws.working_dir.clone(),
        ws.undo_dir.clone(),
    ));
    let handler = UndoMcpHandler::new(Arc::clone(&interceptor), ws.working_dir.clone());
    (interceptor, handler)
}

#[tokio::test]
async fn mc03_write_file_creates_api_step() {
    let ws = TempWorkspace::new();

    let (_interceptor, handler) = make_undo_harness(&ws);
    let mut harness =
        McpTestHarness::with_handler(Box::new(handler), ws.working_dir.clone());
    harness.initialize().await;

    // Write a new file
    let resp = harness
        .send_request(
            30,
            "tools/call",
            json!({
                "name": "write_file",
                "arguments": { "path": "hello.txt", "content": "Hello, world!" }
            }),
        )
        .await;

    // Verify success
    let result_text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let result: Value = serde_json::from_str(result_text).unwrap();
    assert_eq!(result["bytes_written"], 13);
    assert!(result.get("step_id").is_some());

    // Verify file exists on disk
    let content = std::fs::read_to_string(ws.working_dir.join("hello.txt")).unwrap();
    assert_eq!(content, "Hello, world!");

    // Verify step appears in undo history
    let history_resp = harness
        .send_request(31, "tools/call", json!({"name": "get_undo_history", "arguments": {}}))
        .await;
    let history_text = history_resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    let history: Value = serde_json::from_str(history_text).unwrap();
    let steps = history["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 1);
}

#[tokio::test]
async fn mc03_write_file_overwrites_existing() {
    let ws = TempWorkspace::new();
    std::fs::write(ws.working_dir.join("existing.txt"), "original").unwrap();

    let (_interceptor, handler) = make_undo_harness(&ws);
    let mut harness =
        McpTestHarness::with_handler(Box::new(handler), ws.working_dir.clone());
    harness.initialize().await;

    let resp = harness
        .send_request(
            32,
            "tools/call",
            json!({
                "name": "write_file",
                "arguments": { "path": "existing.txt", "content": "modified" }
            }),
        )
        .await;

    assert!(resp.get("result").is_some());
    let content = std::fs::read_to_string(ws.working_dir.join("existing.txt")).unwrap();
    assert_eq!(content, "modified");
}

#[tokio::test]
async fn mc03_write_file_creates_parent_directories() {
    let ws = TempWorkspace::new();

    let (_interceptor, handler) = make_undo_harness(&ws);
    let mut harness =
        McpTestHarness::with_handler(Box::new(handler), ws.working_dir.clone());
    harness.initialize().await;

    let resp = harness
        .send_request(
            33,
            "tools/call",
            json!({
                "name": "write_file",
                "arguments": { "path": "subdir/nested/file.txt", "content": "nested content" }
            }),
        )
        .await;

    assert!(resp.get("result").is_some());
    let content =
        std::fs::read_to_string(ws.working_dir.join("subdir/nested/file.txt")).unwrap();
    assert_eq!(content, "nested content");
}

// ===========================================================================
// MC-04: write_file → rollback
// ===========================================================================

#[tokio::test]
async fn mc04_write_file_then_rollback_restores_original() {
    let ws = TempWorkspace::new();
    let original = "original content";
    std::fs::write(ws.working_dir.join("target.txt"), original).unwrap();

    let (_interceptor, handler) = make_undo_harness(&ws);
    let mut harness =
        McpTestHarness::with_handler(Box::new(handler), ws.working_dir.clone());
    harness.initialize().await;

    // Overwrite the file
    harness
        .send_request(
            40,
            "tools/call",
            json!({
                "name": "write_file",
                "arguments": { "path": "target.txt", "content": "modified content" }
            }),
        )
        .await;

    // Verify modification
    assert_eq!(
        std::fs::read_to_string(ws.working_dir.join("target.txt")).unwrap(),
        "modified content"
    );

    // Rollback
    let resp = harness
        .send_request(
            41,
            "tools/call",
            json!({"name": "undo", "arguments": {"count": 1}}),
        )
        .await;

    let result_text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let result: Value = serde_json::from_str(result_text).unwrap();
    assert_eq!(result["steps_rolled_back"], 1);

    // Verify restoration
    assert_eq!(
        std::fs::read_to_string(ws.working_dir.join("target.txt")).unwrap(),
        original
    );
}

#[tokio::test]
async fn mc04_write_new_file_then_rollback_removes_it() {
    let ws = TempWorkspace::new();

    let (_interceptor, handler) = make_undo_harness(&ws);
    let mut harness =
        McpTestHarness::with_handler(Box::new(handler), ws.working_dir.clone());
    harness.initialize().await;

    // Create a new file
    harness
        .send_request(
            42,
            "tools/call",
            json!({
                "name": "write_file",
                "arguments": { "path": "new_file.txt", "content": "new content" }
            }),
        )
        .await;

    assert!(ws.working_dir.join("new_file.txt").exists());

    // Rollback
    harness
        .send_request(
            43,
            "tools/call",
            json!({"name": "undo", "arguments": {"count": 1}}),
        )
        .await;

    // File should be gone
    assert!(!ws.working_dir.join("new_file.txt").exists());
}

// ===========================================================================
// MC-06: Notification forwarding (safeguard event cross-interface pattern)
// ===========================================================================

#[tokio::test]
async fn mc06_injected_notification_appears_on_output() {
    let mut harness = McpTestHarness::new();
    harness.initialize().await;

    // Inject a notification (simulating a safeguard event)
    harness.inject_notification(JsonRpcNotification {
        jsonrpc: "2.0".to_string(),
        method: "notifications/message".to_string(),
        params: Some(json!({
            "level": "warning",
            "data": {
                "type": "safeguard_triggered",
                "step_id": 42,
                "kind": "delete_threshold",
                "delete_count": 100,
            }
        })),
    });

    // Give the server loop a chance to pick up the notification
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Send a regular request to flush the output
    let request = json!({
        "jsonrpc": "2.0",
        "id": 60,
        "method": "tools/list",
        "params": {},
    });
    harness
        .send_line(&serde_json::to_string(&request).unwrap())
        .await;

    // Read lines — we should find both the notification and the response
    let mut found_notification = false;
    let mut found_response = false;
    for _ in 0..5 {
        let read_result = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            async {
                let mut line = String::new();
                self::BufReader::read_line(&mut harness.output_reader, &mut line).await.unwrap();
                line
            },
        )
        .await;

        match read_result {
            Ok(line) => {
                let parsed: Value = serde_json::from_str(line.trim()).unwrap();
                if parsed.get("method").is_some()
                    && parsed["method"] == "notifications/message"
                {
                    found_notification = true;
                    assert_eq!(parsed["params"]["data"]["type"], "safeguard_triggered");
                }
                if parsed.get("id").is_some() && parsed["id"] == 60 {
                    found_response = true;
                }
                if found_notification && found_response {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    assert!(found_notification, "notification was not received on output");
    assert!(found_response, "response was not received on output");
}

#[tokio::test]
async fn mc06_multiple_notifications_delivered() {
    let mut harness = McpTestHarness::new();
    harness.initialize().await;

    // Inject two notifications
    harness.inject_notification(JsonRpcNotification {
        jsonrpc: "2.0".to_string(),
        method: "notifications/message".to_string(),
        params: Some(json!({"level": "info", "data": {"type": "event_one"}})),
    });
    harness.inject_notification(JsonRpcNotification {
        jsonrpc: "2.0".to_string(),
        method: "notifications/message".to_string(),
        params: Some(json!({"level": "info", "data": {"type": "event_two"}})),
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Flush with a request
    let request = json!({"jsonrpc": "2.0", "id": 61, "method": "tools/list", "params": {}});
    harness
        .send_line(&serde_json::to_string(&request).unwrap())
        .await;

    let mut notification_count = 0;
    for _ in 0..5 {
        let read_result = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            async {
                let mut line = String::new();
                self::BufReader::read_line(&mut harness.output_reader, &mut line).await.unwrap();
                line
            },
        )
        .await;

        match read_result {
            Ok(line) => {
                let parsed: Value = serde_json::from_str(line.trim()).unwrap();
                if parsed.get("method").is_some()
                    && parsed["method"] == "notifications/message"
                {
                    notification_count += 1;
                }
            }
            Err(_) => break,
        }
    }

    assert_eq!(notification_count, 2);
}

// ===========================================================================
// MC-08: Concurrent MCP + STDIO operations (shared undo state)
// ===========================================================================

#[tokio::test]
async fn mc08_concurrent_operations_share_undo_state() {
    let ws = TempWorkspace::new();

    let interceptor = Arc::new(UndoInterceptor::new(
        ws.working_dir.clone(),
        ws.undo_dir.clone(),
    ));

    // Create two handlers sharing the same interceptor (simulating MCP + STDIO)
    let handler_mcp = UndoMcpHandler::new(Arc::clone(&interceptor), ws.working_dir.clone());
    let handler_stdio = UndoMcpHandler::new(Arc::clone(&interceptor), ws.working_dir.clone());

    let mut harness_mcp =
        McpTestHarness::with_handler(Box::new(handler_mcp), ws.working_dir.clone());
    let mut harness_stdio =
        McpTestHarness::with_handler(Box::new(handler_stdio), ws.working_dir.clone());

    harness_mcp.initialize().await;
    harness_stdio.initialize().await;

    // Write via MCP
    harness_mcp
        .send_request(
            80,
            "tools/call",
            json!({
                "name": "write_file",
                "arguments": { "path": "mcp_file.txt", "content": "from mcp" }
            }),
        )
        .await;

    // Write via "STDIO" (second MCP harness acting as STDIO)
    harness_stdio
        .send_request(
            81,
            "tools/call",
            json!({
                "name": "write_file",
                "arguments": { "path": "stdio_file.txt", "content": "from stdio" }
            }),
        )
        .await;

    // Both files exist
    assert!(ws.working_dir.join("mcp_file.txt").exists());
    assert!(ws.working_dir.join("stdio_file.txt").exists());

    // Query history from MCP — should see both steps
    let resp = harness_mcp
        .send_request(82, "tools/call", json!({"name": "get_undo_history", "arguments": {}}))
        .await;
    let result_text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let history: Value = serde_json::from_str(result_text).unwrap();
    let steps = history["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 2);
}

#[tokio::test]
async fn mc08_concurrent_write_and_query_no_deadlock() {
    let ws = TempWorkspace::new();

    let interceptor = Arc::new(UndoInterceptor::new(
        ws.working_dir.clone(),
        ws.undo_dir.clone(),
    ));

    let handler = UndoMcpHandler::new(Arc::clone(&interceptor), ws.working_dir.clone());
    let mut harness =
        McpTestHarness::with_handler(Box::new(handler), ws.working_dir.clone());
    harness.initialize().await;

    // Perform multiple writes sequentially (concurrent writes within a single
    // server are serialized by the tokio::select! loop, but this verifies
    // that interleaving write + query does not deadlock).
    for i in 0..5 {
        harness
            .send_request(
                90 + i,
                "tools/call",
                json!({
                    "name": "write_file",
                    "arguments": { "path": format!("file_{i}.txt"), "content": format!("content {i}") }
                }),
            )
            .await;

        // Query history after each write
        let resp = harness
            .send_request(
                100 + i,
                "tools/call",
                json!({"name": "get_undo_history", "arguments": {}}),
            )
            .await;
        assert!(resp.get("result").is_some());
    }

    // All 5 files should exist
    for i in 0..5 {
        assert!(ws.working_dir.join(format!("file_{i}.txt")).exists());
    }

    // Final history should show 5 steps
    let resp = harness
        .send_request(200, "tools/call", json!({"name": "get_undo_history", "arguments": {}}))
        .await;
    let result_text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let history: Value = serde_json::from_str(result_text).unwrap();
    assert_eq!(history["steps"].as_array().unwrap().len(), 5);
}

// ===========================================================================
// Additional edge cases
// ===========================================================================

#[tokio::test]
async fn oversized_message_rejected() {
    let mut harness = McpTestHarness::new();
    let big_payload = "x".repeat(1_048_577); // just over 1MB
    harness.send_line(&big_payload).await;
    let line = harness.recv_line().await;
    let resp: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(resp["error"]["code"], -32600); // INVALID_REQUEST (oversized)
}

#[tokio::test]
async fn tools_call_missing_name_returns_invalid_params() {
    let mut harness = McpTestHarness::new();
    harness.initialize().await;

    let resp = harness
        .send_request(
            50,
            "tools/call",
            json!({ "arguments": { "command": "ls" } }),
        )
        .await;

    assert_eq!(resp["error"]["code"], -32602);
}

#[tokio::test]
async fn get_session_status_returns_state() {
    let mut harness = McpTestHarness::new();
    harness.initialize().await;

    let resp = harness
        .send_request(
            51,
            "tools/call",
            json!({"name": "get_session_status", "arguments": {}}),
        )
        .await;

    let result_text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let result: Value = serde_json::from_str(result_text).unwrap();
    assert_eq!(result["state"], "idle");
}
