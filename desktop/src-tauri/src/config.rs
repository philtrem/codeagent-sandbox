use serde::{Deserialize, Serialize};

/// Top-level configuration matching the `codeagent.toml` schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxConfig {
    pub sandbox: SandboxSection,
    pub vm: VmSection,
    pub undo: UndoSection,
    pub safeguards: SafeguardSection,
    pub symlinks: SymlinkSection,
    pub external_modifications: ExternalModificationsSection,
    pub gitignore: GitignoreSection,
    pub claude_code: ClaudeCodeSection,
    pub command_classifier: CommandClassifierSection,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            sandbox: SandboxSection::default(),
            vm: VmSection::default(),
            undo: UndoSection::default(),
            safeguards: SafeguardSection::default(),
            symlinks: SymlinkSection::default(),
            external_modifications: ExternalModificationsSection::default(),
            gitignore: GitignoreSection::default(),
            claude_code: ClaudeCodeSection::default(),
            command_classifier: CommandClassifierSection::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxSection {
    pub working_dirs: Vec<String>,
    pub undo_dir: String,
    pub vm_mode: String,
    pub protocol: String,
    pub log_level: String,
}

impl Default for SandboxSection {
    fn default() -> Self {
        Self {
            working_dirs: vec![],
            undo_dir: String::new(),
            vm_mode: "ephemeral".into(),
            protocol: "mcp".into(),
            log_level: "info".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VmSection {
    pub memory_mb: u32,
    pub cpus: u32,
    pub qemu_binary: String,
    pub kernel_path: String,
    pub initrd_path: String,
    pub rootfs_path: String,
    pub virtiofsd_binary: String,
    pub auto_start: bool,
    pub persist_vm: bool,
}

impl Default for VmSection {
    fn default() -> Self {
        Self {
            memory_mb: 2048,
            cpus: 2,
            qemu_binary: String::new(),
            kernel_path: String::new(),
            initrd_path: String::new(),
            rootfs_path: String::new(),
            virtiofsd_binary: String::new(),
            auto_start: false,
            persist_vm: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UndoSection {
    pub max_log_size_mb: u32,
    pub max_step_count: u32,
    pub max_single_step_size_mb: u32,
}

impl Default for UndoSection {
    fn default() -> Self {
        Self {
            max_log_size_mb: 500,
            max_step_count: 100,
            max_single_step_size_mb: 50,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SafeguardSection {
    pub enabled: bool,
    pub delete_threshold: u32,
    pub overwrite_file_size_kb: u32,
    pub rename_over_existing: bool,
    pub timeout_seconds: u32,
}

impl Default for SafeguardSection {
    fn default() -> Self {
        Self {
            enabled: true,
            delete_threshold: 10,
            overwrite_file_size_kb: 1024,
            rename_over_existing: true,
            timeout_seconds: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SymlinkSection {
    pub policy: String,
}

impl Default for SymlinkSection {
    fn default() -> Self {
        Self {
            policy: "ignore".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExternalModificationsSection {
    pub policy: String,
}

impl Default for ExternalModificationsSection {
    fn default() -> Self {
        Self {
            policy: "barrier".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GitignoreSection {
    pub enabled: bool,
}

impl Default for GitignoreSection {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeCodeSection {
    pub enabled: bool,
    pub server_name: String,
    pub scope: String,
    pub disable_builtin_tools: bool,
}

impl Default for ClaudeCodeSection {
    fn default() -> Self {
        Self {
            enabled: false,
            server_name: "codeagent-sandbox".into(),
            scope: "user".into(),
            disable_builtin_tools: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CommandClassifierSection {
    pub read_only_commands: Vec<String>,
    pub write_commands: Vec<String>,
    pub destructive_commands: Vec<String>,
    pub git_read_only_subcommands: Vec<String>,
    pub git_destructive_subcommands: Vec<String>,
    pub cargo_read_only_subcommands: Vec<String>,
    pub cargo_destructive_subcommands: Vec<String>,
    pub npm_read_only_subcommands: Vec<String>,
    pub npm_read_only_scripts: Vec<String>,
}

impl Default for CommandClassifierSection {
    fn default() -> Self {
        Self {
            read_only_commands: vec![
                "cd", "ls", "cat", "head", "tail", "less", "more", "wc", "file", "find",
                "grep", "egrep", "fgrep", "rg", "ag", "awk", "gawk", "which", "whereis",
                "type", "echo", "printf", "pwd", "env", "printenv", "whoami", "id",
                "hostname", "uname", "date", "cal", "uptime", "df", "du", "free", "top",
                "htop", "ps", "stat", "readlink", "realpath", "basename", "dirname",
                "test", "[", "true", "false", "diff", "cmp", "md5sum", "sha256sum",
                "sha1sum", "sha512sum", "xxd", "od", "strings", "tree", "bat", "jq",
                "yq", "sort", "uniq", "cut", "tr", "column", "comm", "join", "paste",
                "fold", "rev", "tac", "nl", "expand", "unexpand", "hexdump", "man",
                "help", "info",
            ].into_iter().map(String::from).collect(),
            write_commands: vec![
                "touch", "mkdir", "cp", "mv", "chmod", "chown", "chgrp", "curl", "wget",
                "tar", "unzip", "zip", "gzip", "gunzip", "bzip2", "bunzip2", "xz",
                "unxz", "make", "cmake", "patch", "ln",
            ].into_iter().map(String::from).collect(),
            destructive_commands: vec![
                "rm", "rmdir", "dd", "mkfs", "shred", "truncate",
            ].into_iter().map(String::from).collect(),
            git_read_only_subcommands: vec![
                "status", "log", "diff", "show", "branch", "tag", "remote", "rev-parse",
                "ls-files", "ls-tree", "describe", "shortlog", "blame", "bisect",
                "reflog", "stash list", "config", "help", "version",
            ].into_iter().map(String::from).collect(),
            git_destructive_subcommands: vec![
                "clean",
            ].into_iter().map(String::from).collect(),
            cargo_read_only_subcommands: vec![
                "check", "test", "clippy", "doc", "bench", "metadata", "tree", "version", "help",
            ].into_iter().map(String::from).collect(),
            cargo_destructive_subcommands: vec![
                "clean",
            ].into_iter().map(String::from).collect(),
            npm_read_only_subcommands: vec![
                "test", "list", "ls", "view", "info", "outdated", "help", "version",
            ].into_iter().map(String::from).collect(),
            npm_read_only_scripts: vec![
                "test", "lint", "check", "typecheck", "type-check", "validate",
            ].into_iter().map(String::from).collect(),
        }
    }
}
