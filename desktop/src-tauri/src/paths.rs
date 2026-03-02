use std::path::PathBuf;

/// Returns the platform-specific config directory for Code Agent.
///
/// - Windows: `%APPDATA%\CodeAgent`
/// - macOS:   `~/Library/Application Support/CodeAgent`
/// - Linux:   `~/.config/codeagent`
pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("CodeAgent"))
}

/// Returns the full path to the `codeagent.toml` config file.
pub fn config_file_path() -> Option<PathBuf> {
    config_dir().map(|p| p.join("codeagent.toml"))
}

/// Returns the PID file path for the sandbox process.
pub fn pid_file_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|p| p.join("CodeAgent").join("vm.pid"))
}
