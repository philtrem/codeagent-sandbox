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

/// Resolve a binary name to its full path via the system PATH.
#[tauri::command]
pub fn resolve_binary(name: String) -> Result<Option<String>, String> {
    match which::which(&name) {
        Ok(path) => Ok(Some(path.to_string_lossy().into_owned())),
        Err(which::Error::CannotFindBinaryPath) => Ok(None),
        Err(e) => Err(format!("Failed to resolve binary '{name}': {e}")),
    }
}
