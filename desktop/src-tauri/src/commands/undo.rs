use std::fs;
use std::path::Path;

use codeagent_interceptor::history::{
    read_undo_history as read_interceptor_history, UndoHistoryData,
};

/// Read undo history directly from disk (no MCP needed).
///
/// The orchestrator creates per-working-directory subdirectories under the
/// base undo_dir (e.g. `{undo_dir}/0/`, `{undo_dir}/1/`). This function
/// scans all such subdirectories and merges the results.
#[tauri::command]
pub fn read_undo_history(undo_dir: String) -> Result<UndoHistoryData, String> {
    let base = Path::new(&undo_dir);

    if !base.exists() || !base.is_dir() {
        return Ok(UndoHistoryData {
            steps: Vec::new(),
            barriers: Vec::new(),
        });
    }

    let entries = fs::read_dir(base).map_err(|e| format!("Failed to read undo dir: {e}"))?;

    let mut all_steps = Vec::new();
    let mut all_barriers = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read dir entry: {e}"))?;
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }

        // Accept any subdirectory that contains a `steps/` dir (the real indicator
        // of an undo directory). This handles both old numeric names and new hash-based names.
        if !path.join("steps").is_dir() {
            continue;
        }

        let data = read_interceptor_history(&path)
            .map_err(|e| format!("Failed to read interceptor dir {}: {e}", path.display()))?;
        all_steps.extend(data.steps);
        all_barriers.extend(data.barriers);
    }

    // Sort by timestamp descending (newest first). Timestamp sorting is more
    // robust than step_id sorting because step IDs are monotonic within a session
    // but earlier sessions may have lower IDs interleaved on disk.
    all_steps.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    Ok(UndoHistoryData {
        steps: all_steps,
        barriers: all_barriers,
    })
}

/// Remove all undo history subdirectories under the given undo_dir.
#[tauri::command]
pub fn clear_undo_history(undo_dir: String) -> Result<(), String> {
    let base = Path::new(&undo_dir);

    if !base.exists() || !base.is_dir() {
        return Ok(());
    }

    let entries = fs::read_dir(base).map_err(|e| format!("Failed to read undo dir: {e}"))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read dir entry: {e}"))?;
        let path = entry.path();

        if path.is_dir() {
            fs::remove_dir_all(&path)
                .map_err(|e| format!("Failed to remove {}: {e}", path.display()))?;
        }
    }

    Ok(())
}
