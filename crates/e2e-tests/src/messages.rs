use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a unique request_id for this test run.
fn next_request_id() -> String {
    REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed).to_string()
}

/// Create a `session.start` request.
/// Returns `(message, request_id)`.
pub fn session_start(working_dirs: &[&str], vm_mode: &str) -> (Value, String) {
    let id = next_request_id();
    let dirs: Vec<Value> = working_dirs.iter().map(|p| json!({ "path": p })).collect();
    let msg = json!({
        "type": "session.start",
        "request_id": &id,
        "payload": {
            "working_directories": dirs,
            "vm_mode": vm_mode,
            "network_policy": "disabled"
        }
    });
    (msg, id)
}

/// Create a `session.stop` request.
pub fn session_stop() -> (Value, String) {
    let id = next_request_id();
    let msg = json!({
        "type": "session.stop",
        "request_id": &id
    });
    (msg, id)
}

/// Create a `session.reset` request.
pub fn session_reset() -> (Value, String) {
    let id = next_request_id();
    let msg = json!({
        "type": "session.reset",
        "request_id": &id
    });
    (msg, id)
}

/// Create a `session.status` request.
pub fn session_status() -> (Value, String) {
    let id = next_request_id();
    let msg = json!({
        "type": "session.status",
        "request_id": &id
    });
    (msg, id)
}

/// Create an `agent.execute` request.
pub fn agent_execute(command: &str) -> (Value, String) {
    let id = next_request_id();
    let msg = json!({
        "type": "agent.execute",
        "request_id": &id,
        "payload": {
            "command": command
        }
    });
    (msg, id)
}

/// Create an `undo.rollback` request.
pub fn undo_rollback(count: u32) -> (Value, String) {
    let id = next_request_id();
    let msg = json!({
        "type": "undo.rollback",
        "request_id": &id,
        "payload": {
            "count": count,
            "force": false
        }
    });
    (msg, id)
}

/// Create an `undo.rollback` request with `force: true`.
pub fn undo_rollback_force(count: u32) -> (Value, String) {
    let id = next_request_id();
    let msg = json!({
        "type": "undo.rollback",
        "request_id": &id,
        "payload": {
            "count": count,
            "force": true
        }
    });
    (msg, id)
}

/// Create an `undo.history` request.
pub fn undo_history() -> (Value, String) {
    let id = next_request_id();
    let msg = json!({
        "type": "undo.history",
        "request_id": &id
    });
    (msg, id)
}

/// Create a `safeguard.configure` request.
pub fn safeguard_configure(
    delete_threshold: Option<u64>,
    overwrite_size_threshold: Option<u64>,
    rename_over_existing: bool,
) -> (Value, String) {
    let id = next_request_id();
    let mut payload = json!({
        "rename_over_existing": rename_over_existing,
    });
    if let Some(threshold) = delete_threshold {
        payload["delete_threshold"] = json!(threshold);
    }
    if let Some(threshold) = overwrite_size_threshold {
        payload["overwrite_file_size_threshold"] = json!(threshold);
    }
    let msg = json!({
        "type": "safeguard.configure",
        "request_id": &id,
        "payload": payload
    });
    (msg, id)
}

/// Create a `safeguard.confirm` request.
pub fn safeguard_confirm(safeguard_id: &str, action: &str) -> (Value, String) {
    let id = next_request_id();
    let msg = json!({
        "type": "safeguard.confirm",
        "request_id": &id,
        "payload": {
            "safeguard_id": safeguard_id,
            "action": action
        }
    });
    (msg, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_start_builds_valid_json() {
        let (msg, id) = session_start(&["/tmp/work"], "ephemeral");
        assert_eq!(msg["type"], "session.start");
        assert_eq!(msg["request_id"], id);
        assert_eq!(msg["payload"]["working_directories"][0]["path"], "/tmp/work");
        assert_eq!(msg["payload"]["vm_mode"], "ephemeral");
        assert_eq!(msg["payload"]["network_policy"], "disabled");
    }

    #[test]
    fn agent_execute_builds_valid_json() {
        let (msg, id) = agent_execute("echo hello");
        assert_eq!(msg["type"], "agent.execute");
        assert_eq!(msg["request_id"], id);
        assert_eq!(msg["payload"]["command"], "echo hello");
    }

    #[test]
    fn undo_rollback_builds_valid_json() {
        let (msg, id) = undo_rollback(3);
        assert_eq!(msg["type"], "undo.rollback");
        assert_eq!(msg["request_id"], id);
        assert_eq!(msg["payload"]["count"], 3);
        assert_eq!(msg["payload"]["force"], false);
    }

    #[test]
    fn undo_rollback_force_builds_valid_json() {
        let (msg, _id) = undo_rollback_force(2);
        assert_eq!(msg["payload"]["force"], true);
    }

    #[test]
    fn safeguard_configure_with_all_fields() {
        let (msg, id) = safeguard_configure(Some(10), Some(1024), true);
        assert_eq!(msg["type"], "safeguard.configure");
        assert_eq!(msg["request_id"], id);
        assert_eq!(msg["payload"]["delete_threshold"], 10);
        assert_eq!(msg["payload"]["overwrite_file_size_threshold"], 1024);
        assert_eq!(msg["payload"]["rename_over_existing"], true);
    }

    #[test]
    fn safeguard_configure_without_optional_fields() {
        let (msg, _id) = safeguard_configure(None, None, false);
        assert!(msg["payload"].get("delete_threshold").is_none());
        assert!(msg["payload"].get("overwrite_file_size_threshold").is_none());
        assert_eq!(msg["payload"]["rename_over_existing"], false);
    }

    #[test]
    fn request_ids_are_unique() {
        let (_, id1) = session_stop();
        let (_, id2) = session_stop();
        assert_ne!(id1, id2);
    }
}
