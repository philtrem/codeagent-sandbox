use std::fs;
use std::path::Path;

use serde::Serialize;

use codeagent_common::{BarrierInfo, StepId};

use crate::manifest::StepManifest;
use crate::undo_interceptor::{read_step_barriers, synthesize_barrier_id};

/// A single entry in a step's manifest (file that was touched).
#[derive(Debug, Clone, Serialize)]
pub struct FileDetail {
    pub path: String,
    pub existed_before: bool,
    pub file_type: String,
}

/// An undo step as read from disk.
#[derive(Debug, Clone, Serialize)]
pub struct StepDetail {
    pub step_id: StepId,
    pub timestamp: String,
    pub command: Option<String>,
    pub file_count: usize,
    pub files: Vec<FileDetail>,
    pub unprotected: bool,
}

/// The full undo history data read from a single undo directory.
#[derive(Debug, Clone, Serialize)]
pub struct UndoHistoryData {
    pub steps: Vec<StepDetail>,
    pub barriers: Vec<BarrierInfo>,
}

/// Read undo history from an undo directory on disk.
///
/// Reads `steps/{id}/manifest.json` and `steps/{id}/barriers.json` files.
/// No `UndoInterceptor` instance needed — works purely from the filesystem.
///
/// Returns steps sorted by timestamp descending (newest first).
pub fn read_undo_history(undo_dir: &Path) -> codeagent_common::Result<UndoHistoryData> {
    let steps_dir = undo_dir.join("steps");
    let mut steps = Vec::new();
    let mut barriers = Vec::new();

    if !steps_dir.exists() || !steps_dir.is_dir() {
        return Ok(UndoHistoryData { steps, barriers });
    }

    let entries = fs::read_dir(&steps_dir)?;

    for entry in entries {
        let entry = entry?;
        let step_path = entry.path();

        if !step_path.is_dir() {
            continue;
        }

        let step_id: StepId = match entry.file_name().to_string_lossy().parse() {
            Ok(id) => id,
            Err(_) => continue,
        };

        // Read per-step barriers
        let barrier_entries = read_step_barriers(&step_path);
        for (index, be) in barrier_entries.into_iter().enumerate() {
            barriers.push(BarrierInfo {
                barrier_id: synthesize_barrier_id(step_id, index),
                after_step_id: step_id,
                timestamp: be.timestamp,
                affected_paths: be.affected_paths,
                reason: be.reason,
            });
        }

        // Read manifest
        let manifest = match StepManifest::read_from(&step_path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        // Skip empty steps (read-only commands)
        if manifest.entries.is_empty() {
            continue;
        }

        let files: Vec<FileDetail> = manifest
            .entries
            .iter()
            .map(|(path, entry)| FileDetail {
                path: path.clone(),
                existed_before: entry.existed_before,
                file_type: entry.file_type.clone(),
            })
            .collect();

        steps.push(StepDetail {
            step_id: manifest.step_id,
            timestamp: manifest.timestamp,
            command: manifest.command,
            file_count: files.len(),
            files,
            unprotected: manifest.unprotected,
        });
    }

    // Sort by timestamp descending (newest first)
    steps.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    Ok(UndoHistoryData { steps, barriers })
}
