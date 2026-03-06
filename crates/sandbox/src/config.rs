//! Sandbox configuration loading from TOML files.
//!
//! The sandbox binary reads its config from:
//! 1. The path given by `--config-file` CLI arg, if present.
//! 2. The platform default: `{config_dir}/CodeAgent/codeagent.toml`.
//! 3. Built-in defaults (if the file is missing or unparseable).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::command_classifier::CommandClassifierConfig;

/// Top-level sandbox TOML config.
///
/// Additional sections can be added here as the sandbox gains more
/// configurable behaviour. For now only command classification is included.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxTomlConfig {
    pub command_classifier: CommandClassifierConfig,
    pub file_watcher: FileWatcherConfig,
}

/// Configuration for the filesystem watcher, loaded from TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FileWatcherConfig {
    /// Whether the watcher is enabled (default: true).
    pub enabled: bool,
    /// Debounce duration in milliseconds (default: 2000).
    pub debounce_ms: u64,
    /// TTL for recent backend write records in milliseconds (default: 5000).
    pub recent_write_ttl_ms: u64,
    /// Additional path substring patterns to exclude from watching.
    pub exclude_patterns: Vec<String>,
    /// Whether to respect `.gitignore` rules when filtering external modifications (default: true).
    pub use_gitignore: bool,
}

impl Default for FileWatcherConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            debounce_ms: 2000,
            recent_write_ttl_ms: 5000,
            exclude_patterns: vec![],
            use_gitignore: true,
        }
    }
}

/// Return the platform-default config directory for CodeAgent.
///
/// Uses the same path convention as the desktop app (`desktop/src-tauri/src/paths.rs`):
/// `{dirs::config_dir()}/CodeAgent/`.
pub fn default_config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("CodeAgent"))
}

/// Return the platform-default config file path.
pub fn default_config_file_path() -> Option<PathBuf> {
    default_config_dir().map(|d| d.join("codeagent.toml"))
}

/// Load configuration from a TOML file.
///
/// - If `explicit_path` is `Some`, that file is read.
/// - Otherwise, the platform default path is tried.
/// - If the file doesn't exist or can't be parsed, built-in defaults are returned.
pub fn load_config(explicit_path: Option<&Path>) -> SandboxTomlConfig {
    let path = explicit_path
        .map(PathBuf::from)
        .or_else(default_config_file_path);

    let Some(path) = path else {
        return SandboxTomlConfig::default();
    };

    let contents = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return SandboxTomlConfig::default(),
    };

    toml::from_str(&contents).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonexistent_file_returns_defaults() {
        let config = load_config(Some(Path::new("/nonexistent/path/config.toml")));
        let defaults = SandboxTomlConfig::default();
        assert_eq!(
            config.command_classifier.read_only_commands.len(),
            defaults.command_classifier.read_only_commands.len()
        );
    }

    #[test]
    fn empty_file_returns_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.toml");
        std::fs::write(&path, "").unwrap();

        let config = load_config(Some(&path));
        let defaults = SandboxTomlConfig::default();
        assert_eq!(
            config.command_classifier.read_only_commands.len(),
            defaults.command_classifier.read_only_commands.len()
        );
    }

    #[test]
    fn partial_config_merges_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("partial.toml");
        std::fs::write(
            &path,
            r#"
[command_classifier]
read_only_commands = ["ls", "cat", "mytool"]
"#,
        )
        .unwrap();

        let config = load_config(Some(&path));
        // Read-only list should be the overridden one
        assert_eq!(config.command_classifier.read_only_commands.len(), 3);
        assert!(config.command_classifier.read_only_commands.contains(&"mytool".to_string()));

        // Other lists should still have defaults
        let defaults = CommandClassifierConfig::default();
        assert_eq!(
            config.command_classifier.write_commands.len(),
            defaults.write_commands.len()
        );
        assert_eq!(
            config.command_classifier.destructive_commands.len(),
            defaults.destructive_commands.len()
        );
    }

    #[test]
    fn full_roundtrip_serialize_deserialize() {
        let original = SandboxTomlConfig::default();
        let serialized = toml::to_string(&original).unwrap();
        let deserialized: SandboxTomlConfig = toml::from_str(&serialized).unwrap();

        assert_eq!(
            original.command_classifier.read_only_commands,
            deserialized.command_classifier.read_only_commands
        );
        assert_eq!(
            original.command_classifier.write_commands,
            deserialized.command_classifier.write_commands
        );
        assert_eq!(
            original.command_classifier.destructive_commands,
            deserialized.command_classifier.destructive_commands
        );
    }

    #[test]
    fn malformed_toml_returns_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "this is not valid toml {{{{").unwrap();

        let config = load_config(Some(&path));
        let defaults = SandboxTomlConfig::default();
        assert_eq!(
            config.command_classifier.read_only_commands.len(),
            defaults.command_classifier.read_only_commands.len()
        );
    }
}
