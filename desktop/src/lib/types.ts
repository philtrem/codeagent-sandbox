/** Mirrors the Rust SandboxConfig struct. */
export interface SandboxConfig {
  sandbox: SandboxSection;
  vm: VmSection;
  undo: UndoSection;
  safeguards: SafeguardSection;
  symlinks: SymlinkSection;
  external_modifications: ExternalModificationsSection;
  gitignore: GitignoreSection;
  claude_code: ClaudeCodeSection;
  command_classifier: CommandClassifierSection;
}

export interface SandboxSection {
  working_dirs: string[];
  undo_dir: string;
  vm_mode: string;
  protocol: string;
  log_level: string;
}

export interface VmSection {
  memory_mb: number;
  cpus: number;
  qemu_binary: string;
  kernel_path: string;
  initrd_path: string;
  rootfs_path: string;
  virtiofsd_binary: string;
  auto_start: boolean;
  persist_vm: boolean;
}

export interface UndoSection {
  max_log_size_mb: number;
  max_step_count: number;
  max_single_step_size_mb: number;
}

export interface SafeguardSection {
  enabled: boolean;
  delete_threshold: number;
  overwrite_file_size_kb: number;
  rename_over_existing: boolean;
  timeout_seconds: number;
}

export interface SymlinkSection {
  policy: string;
}

export interface ExternalModificationsSection {
  policy: string;
}

export interface GitignoreSection {
  enabled: boolean;
}

export interface ClaudeCodeSection {
  enabled: boolean;
  server_name: string;
  scope: string;
  disable_builtin_tools: boolean;
}

export interface CommandClassifierSection {
  read_only_commands: string[];
  write_commands: string[];
  destructive_commands: string[];
  git_read_only_subcommands: string[];
  git_destructive_subcommands: string[];
  cargo_read_only_subcommands: string[];
  cargo_destructive_subcommands: string[];
  npm_read_only_subcommands: string[];
  npm_read_only_scripts: string[];
}

export function defaultCommandClassifier(): CommandClassifierSection {
  return {
    read_only_commands: [
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
    ],
    write_commands: [
      "touch", "mkdir", "cp", "mv", "chmod", "chown", "chgrp", "curl", "wget",
      "tar", "unzip", "zip", "gzip", "gunzip", "bzip2", "bunzip2", "xz",
      "unxz", "make", "cmake", "patch", "ln",
    ],
    destructive_commands: [
      "rm", "rmdir", "dd", "mkfs", "shred", "truncate",
    ],
    git_read_only_subcommands: [
      "status", "log", "diff", "show", "branch", "tag", "remote", "rev-parse",
      "ls-files", "ls-tree", "describe", "shortlog", "blame", "bisect",
      "reflog", "stash list", "config", "help", "version",
    ],
    git_destructive_subcommands: ["clean"],
    cargo_read_only_subcommands: [
      "check", "test", "clippy", "doc", "bench", "metadata", "tree", "version", "help",
    ],
    cargo_destructive_subcommands: ["clean"],
    npm_read_only_subcommands: [
      "test", "list", "ls", "view", "info", "outdated", "help", "version",
    ],
    npm_read_only_scripts: [
      "test", "lint", "check", "typecheck", "type-check", "validate",
    ],
  };
}

export interface VmStatus {
  state: "stopped" | "starting" | "running" | "error";
  pid: number | null;
  error: string | null;
}

export interface ClaudeConfigInfo {
  path: string;
  exists: boolean;
  mcp_servers: string[];
}

export interface McpServerEntry {
  server_name: string;
  command: string;
  args: string[];
}

export interface ManifestEntryDetail {
  path: string;
  existed_before: boolean;
  file_type: string;
}

export interface UndoStepDetail {
  step_id: number;
  timestamp: string;
  command: string | null;
  file_count: number;
  files: ManifestEntryDetail[];
  unprotected: boolean;
}

export interface BarrierDetail {
  barrier_id: number;
  after_step_id: number;
  timestamp: string;
  affected_paths: string[];
}

export interface UndoHistoryData {
  steps: UndoStepDetail[];
  barriers: BarrierDetail[];
}

export interface TerminalOutput {
  exit_code: number | null;
  output: string;
  status: "completed" | "timeout" | "error";
}

export interface DebugLogLine {
  index: number;
  timestamp: string;
  line: string;
}

/** Default config matching Rust defaults. */
export function defaultConfig(): SandboxConfig {
  return {
    sandbox: {
      working_dirs: [""],
      undo_dir: "",
      vm_mode: "ephemeral",
      protocol: "mcp",
      log_level: "info",
    },
    vm: {
      memory_mb: 2048,
      cpus: 2,
      qemu_binary: "",
      kernel_path: "",
      initrd_path: "",
      rootfs_path: "",
      virtiofsd_binary: "",
      auto_start: false,
      persist_vm: false,
    },
    undo: {
      max_log_size_mb: 500,
      max_step_count: 100,
      max_single_step_size_mb: 50,
    },
    safeguards: {
      enabled: true,
      delete_threshold: 10,
      overwrite_file_size_kb: 1024,
      rename_over_existing: true,
      timeout_seconds: 30,
    },
    symlinks: { policy: "ignore" },
    external_modifications: { policy: "barrier" },
    gitignore: { enabled: true },
    claude_code: {
      enabled: false,
      server_name: "codeagent-sandbox",
      scope: "user",
      disable_builtin_tools: true,
    },
    command_classifier: defaultCommandClassifier(),
  };
}
