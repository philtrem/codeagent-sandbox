use std::fs;
use std::path::PathBuf;

const DENIED_TOOLS: &[&str] = &["Read", "Edit", "Write", "Glob", "Grep", "Bash"];

const READ_TOOLS: &[&str] = &[
    "read_file",
    "glob",
    "grep",
    "get_undo_history",
    "get_session_status",
    "get_working_directory",
];

const WRITE_TOOLS: &[&str] = &["Bash", "write_file", "edit_file", "undo"];

fn settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("settings.json"))
}

fn read_json(path: &std::path::Path) -> serde_json::Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

fn write_json(path: &std::path::Path, value: &serde_json::Value) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(contents) = serde_json::to_string_pretty(value) {
        let _ = fs::write(path, contents);
    }
}

fn ensure_array<'a>(
    value: &'a mut serde_json::Value,
    pointer_parts: &[&str],
) -> &'a mut Vec<serde_json::Value> {
    let mut current = value.as_object_mut().unwrap();
    for &part in &pointer_parts[..pointer_parts.len() - 1] {
        current = current
            .entry(part)
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .unwrap();
    }
    let last = pointer_parts[pointer_parts.len() - 1];
    current
        .entry(last)
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .unwrap()
}

// ---------------------------------------------------------------------------
// Denied tools (permissions.deny)
// ---------------------------------------------------------------------------

/// Add Claude Code's built-in tools to the `permissions.deny` list in
/// `~/.claude/settings.json` so all file/command operations go through
/// the sandbox's MCP tools instead.
pub fn deny_builtin_tools() {
    let Some(path) = settings_path() else { return };
    let mut value = read_json(&path);

    let deny = ensure_array(&mut value, &["permissions", "deny"]);
    for tool in DENIED_TOOLS {
        if !deny.iter().any(|v| v.as_str() == Some(tool)) {
            deny.push(serde_json::Value::String((*tool).into()));
        }
    }

    write_json(&path, &value);
}

/// Remove the sandbox's denied-tool entries from `~/.claude/settings.json`,
/// restoring Claude Code's built-in tools.
pub fn restore_builtin_tools() {
    let Some(path) = settings_path() else { return };
    if !path.exists() {
        return;
    }

    let mut value = read_json(&path);
    let Some(arr) = value
        .pointer_mut("/permissions/deny")
        .and_then(|v| v.as_array_mut())
    else {
        return;
    };

    let before_len = arr.len();
    arr.retain(|v| {
        v.as_str()
            .map(|s| !DENIED_TOOLS.contains(&s))
            .unwrap_or(true)
    });

    if arr.len() != before_len {
        write_json(&path, &value);
    }
}

// ---------------------------------------------------------------------------
// Allowed tools (permissions.allow)
// ---------------------------------------------------------------------------

/// Add `MCP(<server_name>:<tool>)` entries for read tools (always) and
/// optionally write tools to `permissions.allow` in `~/.claude/settings.json`.
pub fn set_allowed_tools(server_name: &str, include_write_tools: bool) {
    let Some(path) = settings_path() else { return };
    let mut value = read_json(&path);

    let allow = ensure_array(&mut value, &["permissions", "allow"]);
    let mut tools: Vec<&str> = READ_TOOLS.to_vec();
    if include_write_tools {
        tools.extend_from_slice(WRITE_TOOLS);
    }

    for tool in tools {
        let entry = format!("MCP({server_name}:{tool})");
        if !allow.iter().any(|v| v.as_str() == Some(&entry)) {
            allow.push(serde_json::Value::String(entry));
        }
    }

    write_json(&path, &value);
}

/// Remove all `MCP(<server_name>:*)` entries from `permissions.allow`.
pub fn remove_allowed_tools(server_name: &str) {
    let Some(path) = settings_path() else { return };
    if !path.exists() {
        return;
    }

    let mut value = read_json(&path);
    let Some(arr) = value
        .pointer_mut("/permissions/allow")
        .and_then(|v| v.as_array_mut())
    else {
        return;
    };

    let prefix = format!("MCP({server_name}:");
    let before_len = arr.len();
    arr.retain(|v| {
        v.as_str()
            .map(|s| !s.starts_with(&prefix))
            .unwrap_or(true)
    });

    if arr.len() != before_len {
        write_json(&path, &value);
    }
}

// ---------------------------------------------------------------------------
// Batched operations (single read-modify-write to minimize file watches)
// ---------------------------------------------------------------------------

/// Apply startup settings in a single write: deny built-in tools and add
/// allowed MCP tool entries. This avoids triggering Claude Code's file watcher
/// multiple times.
pub fn apply_startup_settings(
    server_name: &str,
    deny_builtins: bool,
    include_write_tools: bool,
) {
    let Some(path) = settings_path() else { return };
    let mut value = read_json(&path);

    if deny_builtins {
        let deny = ensure_array(&mut value, &["permissions", "deny"]);
        for tool in DENIED_TOOLS {
            if !deny.iter().any(|v| v.as_str() == Some(tool)) {
                deny.push(serde_json::Value::String((*tool).into()));
            }
        }
    }

    let allow = ensure_array(&mut value, &["permissions", "allow"]);
    let mut tools: Vec<&str> = READ_TOOLS.to_vec();
    if include_write_tools {
        tools.extend_from_slice(WRITE_TOOLS);
    }
    for tool in tools {
        let entry = format!("MCP({server_name}:{tool})");
        if !allow.iter().any(|v| v.as_str() == Some(&entry)) {
            allow.push(serde_json::Value::String(entry));
        }
    }

    write_json(&path, &value);
}

/// Apply shutdown settings in a single write: restore denied tools and remove
/// allowed MCP tool entries. This avoids triggering Claude Code's file watcher
/// multiple times.
pub fn apply_shutdown_settings(server_name: &str, restore_builtins: bool) {
    let Some(path) = settings_path() else { return };
    if !path.exists() {
        return;
    }

    let mut value = read_json(&path);
    let mut changed = false;

    if restore_builtins {
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
    }

    if let Some(arr) = value
        .pointer_mut("/permissions/allow")
        .and_then(|v| v.as_array_mut())
    {
        let prefix = format!("MCP({server_name}:");
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
        write_json(&path, &value);
    }
}
