use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use codeagent_common::StepId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepManifest {
    pub step_id: StepId,
    pub timestamp: String,
    pub command: Option<String>,
    pub entries: BTreeMap<String, ManifestEntry>,
    /// When true, preimage capture was incomplete (exceeded the single-step size
    /// limit). The step cannot be rolled back.
    #[serde(default)]
    pub unprotected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub existed_before: bool,
    pub path_hash: String,
    pub file_type: String,
}

impl StepManifest {
    pub fn new(step_id: StepId) -> Self {
        Self {
            step_id,
            timestamp: chrono::Utc::now().to_rfc3339(),
            command: None,
            entries: BTreeMap::new(),
            unprotected: false,
        }
    }

    pub fn add_entry(
        &mut self,
        relative_path: &str,
        path_hash: &str,
        existed_before: bool,
        file_type: &str,
    ) {
        self.entries.insert(
            relative_path.to_string(),
            ManifestEntry {
                existed_before,
                path_hash: path_hash.to_string(),
                file_type: file_type.to_string(),
            },
        );
    }

    /// Write manifest to the given directory as manifest.json.
    pub fn write_to(&self, dir: &Path) -> codeagent_common::Result<()> {
        let path = dir.join("manifest.json");
        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, json)?;
        Ok(())
    }

    /// Read manifest from a directory.
    pub fn read_from(dir: &Path) -> codeagent_common::Result<Self> {
        let path = dir.join("manifest.json");
        let json = fs::read_to_string(path)?;
        let manifest: Self = serde_json::from_str(&json)?;
        Ok(manifest)
    }

    /// Check if a path has already been recorded in this manifest.
    pub fn contains_path(&self, relative_path: &str) -> bool {
        self.entries.contains_key(relative_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn manifest_write_read_round_trip() {
        let dir = TempDir::new().unwrap();
        let mut manifest = StepManifest::new(42);
        manifest.command = Some("rm -rf node_modules".to_string());
        manifest.add_entry("src/main.rs", "abc123", true, "regular");
        manifest.add_entry("new_file.txt", "def456", false, "regular");

        manifest.write_to(dir.path()).unwrap();
        let loaded = StepManifest::read_from(dir.path()).unwrap();

        assert_eq!(loaded.step_id, 42);
        assert_eq!(loaded.command, Some("rm -rf node_modules".to_string()));
        assert_eq!(loaded.entries.len(), 2);
        assert!(loaded.entries["src/main.rs"].existed_before);
        assert!(!loaded.entries["new_file.txt"].existed_before);
    }

    #[test]
    fn manifest_contains_path() {
        let mut manifest = StepManifest::new(1);
        assert!(!manifest.contains_path("src/main.rs"));

        manifest.add_entry("src/main.rs", "hash", true, "regular");
        assert!(manifest.contains_path("src/main.rs"));
        assert!(!manifest.contains_path("src/lib.rs"));
    }

    #[test]
    fn manifest_new_has_no_entries() {
        let manifest = StepManifest::new(1);
        assert!(manifest.entries.is_empty());
        assert!(manifest.command.is_none());
        assert!(!manifest.unprotected);
    }

    #[test]
    fn manifest_unprotected_round_trip() {
        let dir = TempDir::new().unwrap();
        let mut manifest = StepManifest::new(1);
        manifest.unprotected = true;
        manifest.add_entry("file.txt", "hash", true, "regular");

        manifest.write_to(dir.path()).unwrap();
        let loaded = StepManifest::read_from(dir.path()).unwrap();

        assert!(loaded.unprotected);
    }

    #[test]
    fn manifest_without_unprotected_field_defaults_to_false() {
        let dir = TempDir::new().unwrap();
        // Simulate a manifest written by an older version without the unprotected field
        let json = r#"{
            "step_id": 1,
            "timestamp": "2024-01-01T00:00:00Z",
            "command": null,
            "entries": {}
        }"#;
        fs::write(dir.path().join("manifest.json"), json).unwrap();

        let loaded = StepManifest::read_from(dir.path()).unwrap();
        assert!(!loaded.unprotected);
    }
}
