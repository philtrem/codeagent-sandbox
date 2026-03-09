import { useEffect, useRef, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Copy,
  Check,
  RefreshCw,
  Terminal,
  ShieldOff,
  ShieldCheck,
  X,
  AlertTriangle,
} from "lucide-react";
import { useSandboxConfig } from "../../hooks/useSandboxConfig";
import { useToastStore } from "../../hooks/useToastStore";
import type { SandboxConfig, ClaudeConfigInfo, McpServerEntry } from "../../lib/types";

function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);

  const handleCopy = async () => {
    await navigator.clipboard.writeText(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <button
      onClick={handleCopy}
      className="flex items-center gap-1 rounded border border-[var(--color-border)] px-3 py-1.5 text-xs hover:bg-[var(--color-bg-tertiary)]"
    >
      {copied ? <Check size={12} /> : <Copy size={12} />}
      {copied ? "Copied" : "Copy"}
    </button>
  );
}

function buildMcpEntry(
  config: SandboxConfig,
  serverName: string,
  sandboxBinary: string,
  socketPath?: string,
  logFilePath?: string,
): McpServerEntry {
  const args: string[] = [];

  for (const dir of config.sandbox.working_dirs) {
    if (dir) {
      args.push("--working-dir", dir);
    }
  }
  if (config.sandbox.undo_dir) {
    args.push("--undo-dir", config.sandbox.undo_dir);
  }
  args.push("--protocol", "mcp");
  args.push("--memory-mb", String(config.vm.memory_mb));
  args.push("--cpus", String(config.vm.cpus));
  args.push("--server-name", serverName);
  if (socketPath) {
    args.push("--socket-path", socketPath);
  }
  if (logFilePath) {
    args.push("--log-file", logFilePath);
  }
  if (config.claude_code.disable_builtin_tools) {
    args.push("--disable-builtin-tools");
  }
  if (config.claude_code.auto_allow_write_tools) {
    args.push("--auto-allow-write-tools");
  }

  return {
    server_name: serverName,
    command: sandboxBinary,
    args,
  };
}

function KillProcessesDialog({
  count,
  onConfirm,
  onCancel,
}: {
  count: number;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div className="w-96 rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-secondary)] p-6 shadow-xl">
        <div className="mb-4 flex items-center justify-between">
          <h3 className="flex items-center gap-2 text-sm font-semibold">
            <AlertTriangle size={16} className="text-[var(--color-warning)]" />
            Running Sandbox Processes
          </h3>
          <button
            onClick={onCancel}
            className="text-[var(--color-text-secondary)] hover:text-[var(--color-text)]"
          >
            <X size={16} />
          </button>
        </div>

        <p className="mb-4 text-sm text-[var(--color-text-secondary)]">
          {count === 1
            ? "There is 1 sandbox process still running."
            : `There are ${count} sandbox processes still running.`}{" "}
          Disabling the MCP server will end {count === 1 ? "it" : "them"}.
        </p>

        <div className="flex justify-end gap-2">
          <button
            onClick={onCancel}
            className="rounded border border-[var(--color-border)] px-4 py-2 text-sm hover:bg-[var(--color-bg-tertiary)]"
          >
            Cancel
          </button>
          <button
            onClick={onConfirm}
            className="rounded bg-[var(--color-error)] px-4 py-2 text-sm text-white hover:opacity-90"
          >
            End {count === 1 ? "Process" : "Processes"}
          </button>
        </div>
      </div>
    </div>
  );
}

function generatePreviewJson(entry: McpServerEntry): string {
  const config = {
    mcpServers: {
      [entry.server_name]: {
        command: entry.command,
        args: entry.args,
      },
    },
  };
  return JSON.stringify(config, null, 2);
}

export default function ClaudeIntegration() {
  const { config, updateSection } = useSandboxConfig();
  const addToast = useToastStore((s) => s.addToast);
  const [info, setInfo] = useState<ClaudeConfigInfo | null>(null);
  const [sandboxBinary, setSandboxBinary] = useState("sandbox");
  const [socketPath, setSocketPath] = useState<string | undefined>();
  const [logFilePath, setLogFilePath] = useState<string | undefined>();
  const [cliCommand, setCliCommand] = useState("");
  const [killConfirm, setKillConfirm] = useState<number | null>(null);
  const prevScope = useRef(config.claude_code.scope);
  const prevServerName = useRef(config.claude_code.server_name);

  const detect = useCallback(async () => {
    try {
      const result = await invoke<ClaudeConfigInfo>(
        "detect_claude_code_config",
        { scope: config.claude_code.scope },
      );
      setInfo(result);
    } catch (_) {
      // Ignore detection errors
    }
  }, [config.claude_code.scope]);

  useEffect(() => {
    detect();
    invoke<string>("resolve_sandbox_binary")
      .then(setSandboxBinary)
      .catch(() => {});
    invoke<string>("get_socket_path")
      .then(setSocketPath)
      .catch(() => {});
    invoke<string>("get_log_file_path")
      .then(setLogFilePath)
      .catch(() => {});
  }, [detect]);

  const entry = buildMcpEntry(
    config,
    config.claude_code.server_name,
    sandboxBinary,
    socketPath,
    logFilePath,
  );
  const preview = generatePreviewJson(entry);

  useEffect(() => {
    invoke<string>("generate_claude_code_cli_command", { entry }).then(
      setCliCommand,
    );
  }, [entry.server_name, entry.command, entry.args.join(",")]);

  // Sync MCP entry to Claude config whenever enabled and entry content changes.
  const entryKey = entry.args.join(",");
  useEffect(() => {
    if (!config.claude_code.enabled) return;

    const scope = config.claude_code.scope;
    const serverName = config.claude_code.server_name;

    // If scope or server name changed, remove the old entry first.
    if (prevScope.current !== scope) {
      invoke("remove_claude_code_config", {
        serverName: prevServerName.current,
        scope: prevScope.current,
      }).catch(() => {});
    } else if (prevServerName.current !== serverName) {
      invoke("remove_claude_code_config", {
        serverName: prevServerName.current,
        scope,
      }).catch(() => {});
    }
    prevScope.current = scope;
    prevServerName.current = serverName;

    invoke("write_claude_code_config", { entry, scope })
      .then(() => detect())
      .catch((e) => addToast("error", `Failed to write Claude config: ${e}`));
  }, [config.claude_code.enabled, config.claude_code.scope, entry.server_name, entryKey]);

  const disableMcp = async (killPids?: number[]) => {
    try {
      if (killPids && killPids.length > 0) {
        await invoke("cleanup_claude_settings", {
          serverName: config.claude_code.server_name,
        });
        await invoke("kill_sandbox_processes", { pids: killPids });
      }
      await invoke("remove_claude_code_config", {
        serverName: config.claude_code.server_name,
        scope: config.claude_code.scope,
      });
      detect();
    } catch (e) {
      addToast("error", `Failed to disable MCP server: ${e}`);
    }
    updateSection("claude_code", { enabled: false });
  };

  const handleToggle = async (enabled: boolean) => {
    if (!enabled) {
      const pids = await invoke<number[]>("find_sandbox_processes");
      if (pids.length > 0) {
        setKillConfirm(pids.length);
        return;
      }
      await disableMcp();
      return;
    }
    updateSection("claude_code", { enabled });
  };

  const confirmKill = async () => {
    const pids = await invoke<number[]>("find_sandbox_processes");
    setKillConfirm(null);
    await disableMcp(pids);
  };

  return (
    <div className="mx-auto max-w-4xl">
      <h1 className="mb-6 text-xl font-bold">Claude Integration</h1>

      <div className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-secondary)] p-4">
        <h2 className="mb-3 text-sm font-semibold">Claude Code</h2>

        {info && (
          <div className="mb-3 space-y-1 text-xs text-[var(--color-text-secondary)]">
            <div>Config: {info.path}</div>
            <div>Status: {info.exists ? "Found" : "Not found"}</div>
            {info.mcp_servers.length > 0 && (
              <div className="flex flex-wrap gap-1">
                Servers:{" "}
                {info.mcp_servers.map((s) => (
                  <span
                    key={s}
                    className="rounded bg-[var(--color-bg-tertiary)] px-1.5 py-0.5"
                  >
                    {s}
                  </span>
                ))}
              </div>
            )}
          </div>
        )}

        <div className="mb-3 flex items-center gap-3">
          <label className="flex items-center gap-2 text-sm">
            <button
              role="switch"
              aria-checked={config.claude_code.enabled}
              onClick={() => handleToggle(!config.claude_code.enabled)}
              className={`relative h-5 w-9 rounded-full transition-colors ${
                config.claude_code.enabled
                  ? "bg-[var(--color-accent)]"
                  : "bg-[var(--color-bg-tertiary)]"
              }`}
            >
              <span
                className={`absolute top-0.5 left-0.5 h-4 w-4 rounded-full bg-white transition-transform ${
                  config.claude_code.enabled ? "translate-x-4" : ""
                }`}
              />
            </button>
            Enable MCP server
          </label>
          <button
            onClick={detect}
            className="ml-auto text-[var(--color-text-secondary)] hover:text-[var(--color-text)]"
            title="Refresh"
          >
            <RefreshCw size={14} />
          </button>
        </div>
        <p className="mb-3 text-xs text-[var(--color-text-secondary)]">
          When enabled, Claude Code spawns the sandbox process. The desktop app
          connects via a side-channel socket for terminal, debug, and rollback.
        </p>

        <div className="space-y-3">
          <div>
            <label className="mb-1 block text-xs text-[var(--color-text-secondary)]">
              Server Name
            </label>
            <input
              type="text"
              value={config.claude_code.server_name}
              onChange={(e) =>
                updateSection("claude_code", { server_name: e.target.value })
              }
              className="w-full rounded border border-[var(--color-border)] bg-[var(--color-bg)] px-3 py-1.5 text-sm"
            />
          </div>

          <div>
            <label className="mb-1 block text-xs text-[var(--color-text-secondary)]">
              Scope
            </label>
            <select
              value={config.claude_code.scope}
              onChange={(e) =>
                updateSection("claude_code", { scope: e.target.value })
              }
              className="rounded border border-[var(--color-border)] bg-[var(--color-bg)] px-3 py-1.5 text-sm"
            >
              <option value="user">User (global)</option>
              <option value="project">Project (.mcp.json)</option>
            </select>
          </div>
        </div>

        <div className="mt-3">
          <label className="flex items-center gap-2 text-sm">
            <button
              role="switch"
              aria-checked={config.claude_code.disable_builtin_tools}
              onClick={() =>
                updateSection("claude_code", {
                  disable_builtin_tools: !config.claude_code.disable_builtin_tools,
                })
              }
              className={`relative h-5 w-9 rounded-full transition-colors ${
                config.claude_code.disable_builtin_tools
                  ? "bg-[var(--color-accent)]"
                  : "bg-[var(--color-bg-tertiary)]"
              }`}
            >
              <span
                className={`absolute top-0.5 left-0.5 h-4 w-4 rounded-full bg-white transition-transform ${
                  config.claude_code.disable_builtin_tools ? "translate-x-4" : ""
                }`}
              />
            </button>
            <span className="flex items-center gap-1">
              <ShieldOff size={14} />
              Disable built-in tools
            </span>
          </label>
          <p className="mt-1 text-xs text-[var(--color-text-secondary)]">
            Prevents Claude from using its own filesystem tools, ensuring all operations go through the sandbox.
          </p>
        </div>

        <div className="mt-3">
          <label className="flex items-center gap-2 text-sm">
            <button
              role="switch"
              aria-checked={config.claude_code.auto_allow_write_tools}
              onClick={() =>
                updateSection("claude_code", {
                  auto_allow_write_tools: !config.claude_code.auto_allow_write_tools,
                })
              }
              className={`relative h-5 w-9 rounded-full transition-colors ${
                config.claude_code.auto_allow_write_tools
                  ? "bg-[var(--color-accent)]"
                  : "bg-[var(--color-bg-tertiary)]"
              }`}
            >
              <span
                className={`absolute top-0.5 left-0.5 h-4 w-4 rounded-full bg-white transition-transform ${
                  config.claude_code.auto_allow_write_tools ? "translate-x-4" : ""
                }`}
              />
            </button>
            <span className="flex items-center gap-1">
              <ShieldCheck size={14} />
              Auto-allow write tools
            </span>
          </label>
          <p className="mt-1 text-xs text-[var(--color-text-secondary)]">
            Skip confirmation prompts for write and execute operations.
          </p>
        </div>

        <div className="mt-3">
          <div className="mb-1 flex items-center justify-between">
            <span className="text-xs text-[var(--color-text-secondary)]">
              Config Preview
            </span>
            <CopyButton text={preview} />
          </div>
          <pre className="max-h-40 overflow-auto rounded bg-[var(--color-bg)] p-3 text-xs">
            {preview}
          </pre>
        </div>

        {cliCommand && (
          <div className="mt-3">
            <div className="mb-1 flex items-center justify-between">
              <span className="flex items-center gap-1 text-xs text-[var(--color-text-secondary)]">
                <Terminal size={12} /> CLI Command
              </span>
              <CopyButton text={cliCommand} />
            </div>
            <pre className="overflow-auto rounded bg-[var(--color-bg)] p-3 text-xs">
              {cliCommand}
            </pre>
          </div>
        )}
      </div>

      {killConfirm !== null && (
        <KillProcessesDialog
          count={killConfirm}
          onConfirm={confirmKill}
          onCancel={() => setKillConfirm(null)}
        />
      )}
    </div>
  );
}
