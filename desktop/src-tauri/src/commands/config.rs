use crate::config::SandboxConfig;
use crate::paths;
use std::fs;

/// Read the config, returning defaults on any failure. For internal use.
pub fn read_config_internal() -> SandboxConfig {
    let Some(path) = paths::config_file_path() else {
        return SandboxConfig::default();
    };
    if !path.exists() {
        return SandboxConfig::default();
    }
    fs::read_to_string(&path)
        .ok()
        .and_then(|contents| toml::from_str(&contents).ok())
        .unwrap_or_default()
}

/// Read the sandbox configuration from `codeagent.toml`.
/// Returns defaults if the file does not exist.
#[tauri::command]
pub fn read_config() -> Result<SandboxConfig, String> {
    let path = paths::config_file_path().ok_or("Could not determine config directory")?;

    if !path.exists() {
        return Ok(SandboxConfig::default());
    }

    let contents = fs::read_to_string(&path).map_err(|e| format!("Failed to read config: {e}"))?;
    let config: SandboxConfig =
        toml::from_str(&contents).map_err(|e| format!("Failed to parse config: {e}"))?;
    Ok(config)
}

/// Write the sandbox configuration to `codeagent.toml`.
/// Creates the config directory if it does not exist.
#[tauri::command]
pub fn write_config(config: SandboxConfig) -> Result<(), String> {
    let path = paths::config_file_path().ok_or("Could not determine config directory")?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create config directory: {e}"))?;
    }

    let contents =
        toml::to_string_pretty(&config).map_err(|e| format!("Failed to serialize config: {e}"))?;
    fs::write(&path, contents).map_err(|e| format!("Failed to write config: {e}"))?;
    Ok(())
}

/// Return the platform-specific path to the `codeagent.toml` config file.
#[tauri::command]
pub fn get_config_path() -> Result<String, String> {
    let path = paths::config_file_path().ok_or("Could not determine config directory")?;
    Ok(path.to_string_lossy().into_owned())
}
