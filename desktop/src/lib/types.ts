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
  };
}
