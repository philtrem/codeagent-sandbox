use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Info about a detected Claude config file.
#[derive(Debug, Clone, Serialize)]
pub struct ClaudeConfigInfo {
    pub path: String,
    pub exists: bool,
    pub mcp_servers: Vec<String>,
}

/// An MCP server entry to write into a Claude config file.
#[derive(Debug, Clone, Deserialize)]
pub struct McpServerEntry {
    pub server_name: String,
    pub command: String,
    pub args: Vec<String>,
}

// --- Claude Desktop config paths ---

fn claude_desktop_config_path() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        dirs::home_dir().map(|h| {
            h.join("Library")
                .join("Application Support")
                .join("Claude")
                .join("claude_desktop_config.json")
        })
    } else {
        // Windows: %APPDATA%\Claude\claude_desktop_config.json
        dirs::config_dir().map(|c| c.join("Claude").join("claude_desktop_config.json"))
    }
}

// --- Claude Code config paths ---

fn claude_code_user_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude.json"))
}

fn claude_code_project_config_path() -> PathBuf {
    PathBuf::from(".mcp.json")
}

// --- Helpers ---

fn read_json_file(path: &std::path::Path) -> serde_json::Value {
    if let Ok(contents) = fs::read_to_string(path) {
        serde_json::from_str(&contents).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    }
}

fn list_mcp_servers(value: &serde_json::Value) -> Vec<String> {
    value
        .get("mcpServers")
        .and_then(|s| s.as_object())
        .map(|obj| obj.keys().cloned().collect())
        .unwrap_or_default()
}

fn merge_mcp_entry(value: &mut serde_json::Value, entry: &McpServerEntry) {
    let servers = value
        .as_object_mut()
        .unwrap()
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));

    if let Some(obj) = servers.as_object_mut() {
        obj.insert(
            entry.server_name.clone(),
            serde_json::json!({
                "command": entry.command,
                "args": entry.args,
            }),
        );
    }
}

fn remove_mcp_entry(value: &mut serde_json::Value, server_name: &str) {
    if let Some(servers) = value.get_mut("mcpServers").and_then(|s| s.as_object_mut()) {
        servers.remove(server_name);
    }
}

fn write_json_file(path: &std::path::Path, value: &serde_json::Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directory: {e}"))?;
    }
    let contents = serde_json::to_string_pretty(value)
        .map_err(|e| format!("Failed to serialize JSON: {e}"))?;
    fs::write(path, contents).map_err(|e| format!("Failed to write file: {e}"))?;
    Ok(())
}

// --- Claude Code commands ---

fn resolve_claude_code_path(scope: &str) -> Result<PathBuf, String> {
    match scope {
        "user" => claude_code_user_config_path()
            .ok_or_else(|| "Could not determine home directory".into()),
        "project" => Ok(claude_code_project_config_path()),
        _ => Err(format!("Invalid scope: {scope}")),
    }
}

#[tauri::command]
pub fn detect_claude_code_config(scope: String) -> Result<ClaudeConfigInfo, String> {
    let path = resolve_claude_code_path(&scope)?;
    let exists = path.exists();
    let mcp_servers = if exists {
        let value = read_json_file(&path);
        list_mcp_servers(&value)
    } else {
        Vec::new()
    };

    Ok(ClaudeConfigInfo {
        path: path.to_string_lossy().into_owned(),
        exists,
        mcp_servers,
    })
}

#[tauri::command]
pub fn write_claude_code_config(entry: McpServerEntry, scope: String) -> Result<(), String> {
    let path = resolve_claude_code_path(&scope)?;
    let mut value = read_json_file(&path);
    merge_mcp_entry(&mut value, &entry);
    write_json_file(&path, &value)
}

#[tauri::command]
pub fn remove_claude_code_config(server_name: String, scope: String) -> Result<(), String> {
    let path = resolve_claude_code_path(&scope)?;
    if !path.exists() {
        return Ok(());
    }
    let mut value = read_json_file(&path);
    remove_mcp_entry(&mut value, &server_name);
    write_json_file(&path, &value)
}

// --- Claude Code: denied tools (settings.json) ---

fn claude_code_settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("settings.json"))
}

#[tauri::command]
pub fn set_claude_code_denied_tools(tools: Vec<String>) -> Result<(), String> {
    let path = claude_code_settings_path()
        .ok_or("Could not determine Claude Code settings path")?;
    let mut value = read_json_file(&path);
    let obj = value.as_object_mut().unwrap();
    let permissions = obj
        .entry("permissions")
        .or_insert_with(|| serde_json::json!({}));
    let deny = permissions
        .as_object_mut()
        .ok_or("permissions is not an object")?
        .entry("deny")
        .or_insert_with(|| serde_json::json!([]));
    if let Some(arr) = deny.as_array_mut() {
        for tool in &tools {
            if !arr.iter().any(|v| v.as_str() == Some(tool)) {
                arr.push(serde_json::Value::String(tool.clone()));
            }
        }
    }
    write_json_file(&path, &value)
}

#[tauri::command]
pub fn remove_claude_code_denied_tools(tools: Vec<String>) -> Result<(), String> {
    let path = claude_code_settings_path()
        .ok_or("Could not determine Claude Code settings path")?;
    if !path.exists() {
        return Ok(());
    }
    let mut value = read_json_file(&path);
    if let Some(arr) = value
        .pointer_mut("/permissions/deny")
        .and_then(|v| v.as_array_mut())
    {
        arr.retain(|v| {
            v.as_str()
                .map(|s| !tools.iter().any(|t| t == s))
                .unwrap_or(true)
        });
    }
    write_json_file(&path, &value)
}

// --- Denied tools list ---

const DENIED_TOOLS: &[&str] = &["Read", "Edit", "Write", "Glob", "Grep", "Bash"];

// --- Lifecycle helpers (called from start_vm, stop_vm, and exit handler) ---

/// Build the MCP server entry args from config (mirrors the frontend's buildMcpEntry).
fn build_mcp_args(
    config: &crate::config::SandboxConfig,
    kernel_path: &str,
    initrd_path: &str,
) -> Vec<String> {
    let mut args = Vec::new();
    for dir in &config.sandbox.working_dirs {
        if !dir.is_empty() {
            args.push("--working-dir".into());
            args.push(dir.clone());
        }
    }
    if !config.sandbox.undo_dir.is_empty() {
        args.push("--undo-dir".into());
        args.push(config.sandbox.undo_dir.clone());
    }
    args.push("--protocol".into());
    args.push("mcp".into());
    args.push("--memory-mb".into());
    args.push(config.vm.memory_mb.to_string());
    args.push("--cpus".into());
    args.push(config.vm.cpus.to_string());
    if !config.vm.qemu_binary.is_empty() {
        args.push("--qemu-binary".into());
        args.push(config.vm.qemu_binary.clone());
    }
    if !kernel_path.is_empty() {
        args.push("--kernel-path".into());
        args.push(kernel_path.into());
    }
    if !initrd_path.is_empty() {
        args.push("--initrd-path".into());
        args.push(initrd_path.into());
    }
    if !config.vm.rootfs_path.is_empty() {
        args.push("--rootfs-path".into());
        args.push(config.vm.rootfs_path.clone());
    }
    args
}

/// Register the MCP server in Claude Code config and set denied tools.
/// Called from `start_vm` when Claude Code integration is enabled.
///
/// `kernel_path` and `initrd_path` are the resolved (not raw config) paths
/// so that Claude Code's spawned sandbox instance can find the guest images.
pub fn register_mcp_server(
    config: &crate::config::SandboxConfig,
    binary_path: &str,
    kernel_path: &str,
    initrd_path: &str,
) {
    if !config.claude_code.enabled {
        return;
    }

    let entry = McpServerEntry {
        server_name: config.claude_code.server_name.clone(),
        command: binary_path.into(),
        args: build_mcp_args(config, kernel_path, initrd_path),
    };

    if let Ok(path) = resolve_claude_code_path(&config.claude_code.scope) {
        let mut value = read_json_file(&path);
        merge_mcp_entry(&mut value, &entry);
        let _ = write_json_file(&path, &value);
    }

    if config.claude_code.disable_builtin_tools {
        let tools: Vec<String> = DENIED_TOOLS.iter().map(|s| (*s).into()).collect();
        let _ = set_claude_code_denied_tools(tools);
    }
}

/// Unregister the MCP server from Claude Code config and restore built-in tools.
/// Called from `stop_vm` and the app exit handler.
pub fn unregister_mcp_server() {
    // Read our config to get server_name and scope
    let config = super::config::read_config_internal();

    // Remove MCP server entry from Claude Code config
    if let Ok(path) = resolve_claude_code_path(&config.claude_code.scope) {
        if path.exists() {
            let mut value = read_json_file(&path);
            remove_mcp_entry(&mut value, &config.claude_code.server_name);
            let _ = write_json_file(&path, &value);
        }
    }

    // Claude Desktop: remove disallowedTools (legacy cleanup)
    if let Some(path) = claude_desktop_config_path() {
        if path.exists() {
            let mut value = read_json_file(&path);
            if let Some(obj) = value.as_object_mut() {
                if obj.remove("disallowedTools").is_some() {
                    let _ = write_json_file(&path, &value);
                }
            }
        }
    }

    // Claude Code: remove our deny entries from settings.json
    if let Some(path) = claude_code_settings_path() {
        if path.exists() {
            let mut value = read_json_file(&path);
            if let Some(arr) = value
                .pointer_mut("/permissions/deny")
                .and_then(|v| v.as_array_mut())
            {
                let before_len = arr.len();
                arr.retain(|v| {
                    v.as_str()
                        .map(|s| !DENIED_TOOLS.contains(&s))
                        .unwrap_or(true)
                });
                if arr.len() != before_len {
                    let _ = write_json_file(&path, &value);
                }
            }
        }
    }
}

/// Generate a `claude mcp add` CLI command string.
#[tauri::command]
pub fn generate_claude_code_cli_command(entry: McpServerEntry) -> String {
    let mut parts = vec![
        "claude".into(),
        "mcp".into(),
        "add".into(),
        entry.server_name.clone(),
    ];

    parts.push(entry.command.clone());

    if !entry.args.is_empty() {
        // Add -- separator before args
        parts.push("--".into());
        parts.extend(entry.args.iter().cloned());
    }

    parts.join(" ")
}
