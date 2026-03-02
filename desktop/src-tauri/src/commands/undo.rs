use serde::Serialize;
use std::fs;
use std::path::Path;

/// A single entry in a step's manifest (file that was touched).
#[derive(Debug, Clone, Serialize)]
pub struct ManifestEntryDetail {
    pub path: String,
    pub existed_before: bool,
    pub file_type: String,
}

/// An undo step as read from disk.
#[derive(Debug, Clone, Serialize)]
pub struct UndoStepDetail {
    pub step_id: u64,
    pub timestamp: String,
    pub command: Option<String>,
    pub file_count: usize,
    pub files: Vec<ManifestEntryDetail>,
    pub unprotected: bool,
}

/// A barrier as read from disk.
#[derive(Debug, Clone, Serialize)]
pub struct BarrierDetail {
    pub barrier_id: u64,
    pub after_step_id: u64,
    pub timestamp: String,
    pub affected_paths: Vec<String>,
}

/// The full undo history data returned to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct UndoHistoryData {
    pub steps: Vec<UndoStepDetail>,
    pub barriers: Vec<BarrierDetail>,
}

/// Read undo history directly from disk (no MCP needed).
#[tauri::command]
pub fn read_undo_history(undo_dir: String) -> Result<UndoHistoryData, String> {
    let base = Path::new(&undo_dir);
    let steps_dir = base.join("steps");

    let mut steps = Vec::new();

    if steps_dir.exists() && steps_dir.is_dir() {
        let entries = fs::read_dir(&steps_dir).map_err(|e| format!("Failed to read steps dir: {e}"))?;

        for entry in entries {
            let entry = entry.map_err(|e| format!("Failed to read dir entry: {e}"))?;
            let step_path = entry.path();

            if !step_path.is_dir() {
                continue;
            }

            let manifest_path = step_path.join("manifest.json");
            if !manifest_path.exists() {
                continue;
            }

            let json = fs::read_to_string(&manifest_path)
                .map_err(|e| format!("Failed to read manifest: {e}"))?;

            let manifest: serde_json::Value =
                serde_json::from_str(&json).map_err(|e| format!("Invalid manifest JSON: {e}"))?;

            let step_id = manifest["step_id"].as_u64().unwrap_or(0);
            let timestamp = manifest["timestamp"]
                .as_str()
                .unwrap_or("")
                .to_string();
            let command = manifest["command"].as_str().map(|s| s.to_string());
            let unprotected = manifest["unprotected"].as_bool().unwrap_or(false);

            let mut files = Vec::new();
            if let Some(entries) = manifest["entries"].as_object() {
                for (path, entry) in entries {
                    files.push(ManifestEntryDetail {
                        path: path.clone(),
                        existed_before: entry["existed_before"].as_bool().unwrap_or(false),
                        file_type: entry["file_type"]
                            .as_str()
                            .unwrap_or("unknown")
                            .to_string(),
                    });
                }
            }

            let file_count = files.len();
            steps.push(UndoStepDetail {
                step_id,
                timestamp,
                command,
                file_count,
                files,
                unprotected,
            });
        }
    }

    // Sort by step_id descending (newest first)
    steps.sort_by(|a, b| b.step_id.cmp(&a.step_id));

    // Read barriers
    let mut barriers = Vec::new();
    let barriers_path = base.join("barriers.json");
    if barriers_path.exists() {
        let json = fs::read_to_string(&barriers_path)
            .map_err(|e| format!("Failed to read barriers: {e}"))?;

        if let Ok(barrier_list) = serde_json::from_str::<Vec<serde_json::Value>>(&json) {
            for b in barrier_list {
                let affected_paths = b["affected_paths"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();

                barriers.push(BarrierDetail {
                    barrier_id: b["barrier_id"].as_u64().unwrap_or(0),
                    after_step_id: b["after_step_id"].as_u64().unwrap_or(0),
                    timestamp: b["timestamp"].as_str().unwrap_or("").to_string(),
                    affected_paths,
                });
            }
        }
    }

    Ok(UndoHistoryData { steps, barriers })
}
