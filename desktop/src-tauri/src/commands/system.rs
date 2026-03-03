/// Return the current platform: "windows", "macos", or "linux".
#[tauri::command]
pub fn get_platform() -> String {
    if cfg!(target_os = "windows") {
        "windows".into()
    } else if cfg!(target_os = "macos") {
        "macos".into()
    } else {
        "linux".into()
    }
}

/// Check whether the given path is an existing directory.
#[tauri::command]
pub fn validate_directory(path: String) -> bool {
    std::path::Path::new(&path).is_dir()
}

/// Check whether the undo directory overlaps with any working directory.
/// Returns `None` if no overlap, or an error message if they overlap.
#[tauri::command]
pub fn validate_paths_overlap(working_dirs: Vec<String>, undo_dir: String) -> Option<String> {
    let undo_path = std::path::Path::new(&undo_dir);
    let canonical_undo = match std::fs::canonicalize(undo_path) {
        Ok(p) => p,
        Err(_) => return None,
    };

    for dir in &working_dirs {
        if dir.is_empty() {
            continue;
        }
        let working_path = std::path::Path::new(dir);
        let canonical_working = match std::fs::canonicalize(working_path) {
            Ok(p) => p,
            Err(_) => continue,
        };

        if canonical_undo.starts_with(&canonical_working) {
            return Some(format!(
                "Undo directory is inside working directory \"{}\"",
                dir
            ));
        }
        if canonical_working.starts_with(&canonical_undo) {
            return Some(format!(
                "Working directory \"{}\" is inside undo directory",
                dir
            ));
        }
    }

    None
}

/// Resolve a binary name to its full path via the system PATH.
#[tauri::command]
pub fn resolve_binary(name: String) -> Result<Option<String>, String> {
    match which::which(&name) {
        Ok(path) => Ok(Some(path.to_string_lossy().into_owned())),
        Err(which::Error::CannotFindBinaryPath) => Ok(None),
        Err(e) => Err(format!("Failed to resolve binary '{name}': {e}")),
    }
}
