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

fn resolve_claude_code_path(scope: &str) -> Result<PathBuf, String> {
    match scope {
        "user" => claude_code_user_config_path()
            .ok_or_else(|| "Could not determine home directory".into()),
        "project" => Ok(claude_code_project_config_path()),
        _ => Err(format!("Invalid scope: {scope}")),
    }
}

fn write_json_file(path: &std::path::Path, value: &serde_json::Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {e}"))?;
    }
    let contents =
        serde_json::to_string_pretty(value).map_err(|e| format!("Failed to serialize JSON: {e}"))?;
    fs::write(path, contents).map_err(|e| format!("Failed to write file: {e}"))?;
    Ok(())
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

// --- Tauri commands ---

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

/// Clean up Claude Code settings that the sandbox process would have restored
/// on exit. Call this before killing sandbox processes to ensure
/// `~/.claude/settings.json` is left in a clean state.
#[tauri::command]
pub fn cleanup_claude_settings(server_name: String) -> Result<(), String> {
    let Some(settings_path) = dirs::home_dir().map(|h| h.join(".claude").join("settings.json"))
    else {
        return Ok(());
    };
    if !settings_path.exists() {
        return Ok(());
    }

    let contents = fs::read_to_string(&settings_path)
        .map_err(|e| format!("Failed to read settings: {e}"))?;
    let mut value: serde_json::Value =
        serde_json::from_str(&contents).unwrap_or_else(|_| serde_json::json!({}));
    let mut changed = false;

    // Remove denied built-in tools (mirrors sandbox's apply_shutdown_settings)
    const DENIED_TOOLS: &[&str] = &["Read", "Edit", "Write", "Glob", "Grep", "Bash"];
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
            changed = true;
        }
    }

    // Remove MCP allowed-tool entries for this server name
    let prefix = format!("MCP({server_name}:");
    if let Some(arr) = value
        .pointer_mut("/permissions/allow")
        .and_then(|v| v.as_array_mut())
    {
        let before_len = arr.len();
        arr.retain(|v| {
            v.as_str()
                .map(|s| !s.starts_with(&prefix))
                .unwrap_or(true)
        });
        if arr.len() != before_len {
            changed = true;
        }
    }

    if changed {
        let contents = serde_json::to_string_pretty(&value)
            .map_err(|e| format!("Failed to serialize settings: {e}"))?;
        fs::write(&settings_path, contents)
            .map_err(|e| format!("Failed to write settings: {e}"))?;
    }

    Ok(())
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
