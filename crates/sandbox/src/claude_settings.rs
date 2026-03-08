use std::fs;
use std::path::PathBuf;

const DENIED_TOOLS: &[&str] = &["Read", "Edit", "Write", "Glob", "Grep", "Bash"];

fn settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("settings.json"))
}

fn read_settings(path: &std::path::Path) -> serde_json::Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

fn write_settings(path: &std::path::Path, value: &serde_json::Value) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(contents) = serde_json::to_string_pretty(value) {
        let _ = fs::write(path, contents);
    }
}

/// Add Claude Code's built-in tools to the `permissions.deny` list in
/// `~/.claude/settings.json` so all file/command operations go through
/// the sandbox's MCP tools instead.
pub fn deny_builtin_tools() {
    let Some(path) = settings_path() else { return };
    let mut value = read_settings(&path);

    let deny = value
        .as_object_mut()
        .unwrap()
        .entry("permissions")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .unwrap()
        .entry("deny")
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .unwrap();

    for tool in DENIED_TOOLS {
        if !deny.iter().any(|v| v.as_str() == Some(tool)) {
            deny.push(serde_json::Value::String((*tool).into()));
        }
    }

    write_settings(&path, &value);
}

/// Remove the sandbox's denied-tool entries from `~/.claude/settings.json`,
/// restoring Claude Code's built-in tools.
pub fn restore_builtin_tools() {
    let Some(path) = settings_path() else { return };
    if !path.exists() {
        return;
    }

    let mut value = read_settings(&path);

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
        write_settings(&path, &value);
    }
}
