use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::McpError;
use crate::parser::extract_missing_field;
use crate::path_validation::validate_path;
use crate::protocol::{
    EditFileArgs, ExecuteCommandArgs, GetUndoHistoryArgs, GlobArgs, GrepArgs, JsonRpcRequest,
    JsonRpcResponse, ListDirectoryArgs, ReadFileArgs, ToolCallParams, ToolCallResult,
    ToolDefinition, UndoArgs, WriteFileArgs,
};

/// Trait abstracting the handling of MCP tool invocations.
///
/// Each method receives typed arguments and returns either a success value
/// or an `McpError`. For contract tests, a stub implementation provides
/// canned responses. Real implementations are added in later TDD steps.
pub trait McpHandler: Send + Sync {
    fn execute_command(&self, args: ExecuteCommandArgs) -> Result<serde_json::Value, McpError>;
    fn read_file(&self, args: ReadFileArgs) -> Result<serde_json::Value, McpError>;
    fn write_file(&self, args: WriteFileArgs) -> Result<serde_json::Value, McpError>;
    fn edit_file(&self, args: EditFileArgs) -> Result<serde_json::Value, McpError>;
    fn list_directory(&self, args: ListDirectoryArgs) -> Result<serde_json::Value, McpError>;
    fn glob(&self, args: GlobArgs) -> Result<serde_json::Value, McpError>;
    fn grep(&self, args: GrepArgs) -> Result<serde_json::Value, McpError>;
    fn undo(&self, args: UndoArgs) -> Result<serde_json::Value, McpError>;
    fn get_undo_history(&self, args: GetUndoHistoryArgs) -> Result<serde_json::Value, McpError>;
    fn get_session_status(&self) -> Result<serde_json::Value, McpError>;
}

/// Returns the server capabilities advertised in the `initialize` response.
fn server_info(instructions: &str) -> serde_json::Value {
    serde_json::json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {
            "tools": {}
        },
        "serverInfo": {
            "name": "codeagent-mcp",
            "version": "0.1.0"
        },
        "instructions": instructions
    })
}

/// Returns the definitions for all 10 MCP tools.
pub fn tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "execute_command".to_string(),
            description: "Run a terminal command inside the VM".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Command to execute" },
                    "env": { "type": "object", "description": "Environment variables", "additionalProperties": { "type": "string" } },
                    "cwd": { "type": "string", "description": "Working directory for the command" }
                },
                "required": ["command"]
            }),
        },
        ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file's contents from the working folder".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path to the file" }
                },
                "required": ["path"]
            }),
        },
        ToolDefinition {
            name: "write_file".to_string(),
            description: "Write content to a file in the working folder".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path to the file" },
                    "content": { "type": "string", "description": "File content to write" }
                },
                "required": ["path", "content"]
            }),
        },
        ToolDefinition {
            name: "edit_file".to_string(),
            description: "Perform exact string replacement in a file".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path to the file" },
                    "old_string": { "type": "string", "description": "Exact text to find" },
                    "new_string": { "type": "string", "description": "Replacement text" },
                    "replace_all": { "type": "boolean", "description": "Replace all occurrences (default: false)", "default": false }
                },
                "required": ["path", "old_string", "new_string"]
            }),
        },
        ToolDefinition {
            name: "list_directory".to_string(),
            description: "List directory contents".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path to the directory" }
                },
                "required": ["path"]
            }),
        },
        ToolDefinition {
            name: "glob".to_string(),
            description: "Find files matching a glob pattern. Results sorted by modification time (newest first).".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern (e.g. **/*.rs, src/**/*.ts)" },
                    "path": { "type": "string", "description": "Directory to search in (relative to working dir)" },
                    "limit": { "type": "integer", "description": "Max results to return (default: 200)" }
                },
                "required": ["pattern"]
            }),
        },
        ToolDefinition {
            name: "grep".to_string(),
            description: "Search file contents with a regex pattern".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Regex pattern to search for" },
                    "path": { "type": "string", "description": "File or directory to search (relative to working dir)" },
                    "include": { "type": "string", "description": "Glob pattern to filter files (e.g. *.rs)" },
                    "output_mode": { "type": "string", "description": "Output format: files_with_matches (default), content, or count", "default": "files_with_matches" },
                    "context_lines": { "type": "integer", "description": "Lines of context around matches (content mode)" },
                    "case_insensitive": { "type": "boolean", "description": "Case-insensitive matching", "default": false }
                },
                "required": ["pattern"]
            }),
        },
        ToolDefinition {
            name: "undo".to_string(),
            description: "Roll back the most recent N steps".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "count": { "type": "integer", "description": "Number of steps to undo", "default": 1 },
                    "force": { "type": "boolean", "description": "Force rollback across barriers", "default": false }
                }
            }),
        },
        ToolDefinition {
            name: "get_undo_history".to_string(),
            description: "List recent steps with metadata".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDefinition {
            name: "get_session_status".to_string(),
            description: "Query current session state".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDefinition {
            name: "get_working_directory".to_string(),
            description: "Get the sandbox working directory. This is the root directory for all file operations — NOT the project directory open in your editor.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        },
    ]
}

/// Routes JSON-RPC requests to the `McpHandler`, performing path validation
/// for filesystem operations and implementing the MCP lifecycle (initialize,
/// tools/list, tools/call).
pub struct McpRouter {
    root_dir: PathBuf,
    working_dirs: Vec<PathBuf>,
    handler: Box<dyn McpHandler>,
    initialized: AtomicBool,
    instructions: String,
}

impl McpRouter {
    pub fn new(root_dir: PathBuf, handler: Box<dyn McpHandler>) -> Self {
        let instructions = format!(
            "IMPORTANT: You are operating inside a sandboxed environment. \
             Your working directory is NOT the project open in your editor. \
             Call get_working_directory to find the real sandbox working directory. \
             All file paths must be relative to that sandbox directory.\n\n\
             Sandbox root: {}\n\n\
             Use ONLY this server's tools for ALL file and command operations. \
             Built-in tools (Read, Edit, Write, Glob, Grep, Bash, NotebookEdit) have been \
             disabled — do not attempt to use them.",
            root_dir.display()
        );
        let working_dirs = vec![root_dir.clone()];
        Self {
            root_dir,
            working_dirs,
            handler,
            initialized: AtomicBool::new(false),
            instructions,
        }
    }

    /// Create a router with multiple working directories listed in instructions.
    pub fn with_working_dirs(
        root_dir: PathBuf,
        all_dirs: &[PathBuf],
        handler: Box<dyn McpHandler>,
    ) -> Self {
        let dir_list: Vec<String> = all_dirs.iter().map(|d| format!("  - {}", d.display())).collect();
        let instructions = format!(
            "IMPORTANT: You are operating inside a sandboxed environment. \
             Your working directory is NOT the project open in your editor. \
             Call get_working_directory to find the real sandbox working directory. \
             All file paths must be relative to that sandbox directory.\n\n\
             Sandbox roots:\n{}\n\n\
             Use ONLY this server's tools for ALL file and command operations. \
             Built-in tools (Read, Edit, Write, Glob, Grep, Bash, NotebookEdit) have been \
             disabled — do not attempt to use them.",
            dir_list.join("\n")
        );
        let working_dirs = all_dirs.to_vec();
        Self {
            root_dir,
            working_dirs,
            handler,
            initialized: AtomicBool::new(false),
            instructions,
        }
    }

    /// Dispatch a parsed JSON-RPC request, returning a response.
    ///
    /// Returns `None` for notifications (messages without an `id`), as
    /// JSON-RPC 2.0 requires no response for those.
    pub fn dispatch(&self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let id = request.id.clone();
        let is_notification = id.is_none();

        match request.method.as_str() {
            "initialize" => {
                self.initialized.store(true, Ordering::SeqCst);
                Some(JsonRpcResponse::success(id, server_info(&self.instructions)))
            }

            "notifications/initialized" => {
                // Notification: no response
                None
            }

            "tools/list" => {
                let tools = tool_definitions();
                Some(JsonRpcResponse::success(
                    id,
                    serde_json::json!({ "tools": tools }),
                ))
            }

            "tools/call" => {
                let result = self.dispatch_tool_call(request.params);
                Some(match result {
                    Ok(tool_result) => {
                        JsonRpcResponse::success(id, serde_json::to_value(tool_result).unwrap())
                    }
                    Err(error) => JsonRpcResponse::error(id, error.to_jsonrpc_error()),
                })
            }

            _ => {
                if is_notification {
                    // Unknown notifications are silently ignored per JSON-RPC 2.0
                    None
                } else {
                    Some(JsonRpcResponse::error(
                        id,
                        McpError::MethodNotFound {
                            method: request.method,
                        }
                        .to_jsonrpc_error(),
                    ))
                }
            }
        }
    }

    fn dispatch_tool_call(
        &self,
        params: serde_json::Value,
    ) -> Result<ToolCallResult, McpError> {
        let tool_params: ToolCallParams =
            serde_json::from_value(params).map_err(|e| McpError::InvalidParams {
                message: format!("Invalid tools/call params: {e}"),
            })?;

        match tool_params.name.as_str() {
            "execute_command" => {
                let args = parse_tool_args::<ExecuteCommandArgs>(tool_params.arguments)?;
                let value = self.handler.execute_command(args)?;
                Ok(ToolCallResult::text(serde_json::to_string(&value).unwrap()))
            }
            "read_file" => {
                let args = parse_tool_args::<ReadFileArgs>(tool_params.arguments)?;
                validate_path(&args.path, &self.root_dir)?;
                let value = self.handler.read_file(args)?;
                Ok(ToolCallResult::text(serde_json::to_string(&value).unwrap()))
            }
            "write_file" => {
                let args = parse_tool_args::<WriteFileArgs>(tool_params.arguments)?;
                validate_path(&args.path, &self.root_dir)?;
                let value = self.handler.write_file(args)?;
                Ok(ToolCallResult::text(serde_json::to_string(&value).unwrap()))
            }
            "edit_file" => {
                let args = parse_tool_args::<EditFileArgs>(tool_params.arguments)?;
                validate_path(&args.path, &self.root_dir)?;
                let value = self.handler.edit_file(args)?;
                Ok(ToolCallResult::text(serde_json::to_string(&value).unwrap()))
            }
            "list_directory" => {
                let args = parse_tool_args::<ListDirectoryArgs>(tool_params.arguments)?;
                validate_path(&args.path, &self.root_dir)?;
                let value = self.handler.list_directory(args)?;
                Ok(ToolCallResult::text(serde_json::to_string(&value).unwrap()))
            }
            "glob" => {
                let args = parse_tool_args::<GlobArgs>(tool_params.arguments)?;
                if let Some(ref path) = args.path {
                    validate_path(path, &self.root_dir)?;
                }
                let value = self.handler.glob(args)?;
                Ok(ToolCallResult::text(serde_json::to_string(&value).unwrap()))
            }
            "grep" => {
                let args = parse_tool_args::<GrepArgs>(tool_params.arguments)?;
                if let Some(ref path) = args.path {
                    validate_path(path, &self.root_dir)?;
                }
                let value = self.handler.grep(args)?;
                Ok(ToolCallResult::text(serde_json::to_string(&value).unwrap()))
            }
            "undo" => {
                let args = parse_tool_args::<UndoArgs>(tool_params.arguments)?;
                let value = self.handler.undo(args)?;
                Ok(ToolCallResult::text(serde_json::to_string(&value).unwrap()))
            }
            "get_undo_history" => {
                let args = parse_tool_args::<GetUndoHistoryArgs>(tool_params.arguments)?;
                let value = self.handler.get_undo_history(args)?;
                Ok(ToolCallResult::text(serde_json::to_string(&value).unwrap()))
            }
            "get_session_status" => {
                let value = self.handler.get_session_status()?;
                Ok(ToolCallResult::text(serde_json::to_string(&value).unwrap()))
            }
            "get_working_directory" => {
                let dirs: Vec<serde_json::Value> = self
                    .working_dirs
                    .iter()
                    .map(|d| serde_json::json!(d.display().to_string()))
                    .collect();
                let value = serde_json::json!({
                    "working_directory": self.root_dir.display().to_string(),
                    "all_working_directories": dirs,
                });
                Ok(ToolCallResult::text(serde_json::to_string(&value).unwrap()))
            }
            unknown => Err(McpError::MethodNotFound {
                method: format!("tools/call: unknown tool `{unknown}`"),
            }),
        }
    }
}

/// Parse tool-specific arguments from a JSON value, classifying serde errors
/// into `MissingField` or `InvalidParams` as appropriate.
fn parse_tool_args<T: serde::de::DeserializeOwned>(
    value: serde_json::Value,
) -> Result<T, McpError> {
    serde_json::from_value::<T>(value).map_err(|e| {
        let message = e.to_string();
        if let Some(field) = extract_missing_field(&message) {
            McpError::MissingField { field }
        } else {
            McpError::InvalidParams { message }
        }
    })
}
