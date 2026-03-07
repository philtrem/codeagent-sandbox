import { useEffect, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Copy,
  Check,
  RefreshCw,
  Terminal,
  ShieldOff,
  ShieldCheck,
} from "lucide-react";
import { useSandboxConfig } from "../../hooks/useSandboxConfig";
import { useToastStore } from "../../hooks/useToastStore";
import { useVmStore } from "../../hooks/useVmStatus";
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

  return {
    server_name: serverName,
    command: sandboxBinary,
    args,
  };
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
  const vmStatus = useVmStore((s) => s.status);
  const [info, setInfo] = useState<ClaudeConfigInfo | null>(null);
  const [sandboxBinary, setSandboxBinary] = useState("sandbox");
  const [cliCommand, setCliCommand] = useState("");

  const isVmRunning = vmStatus.state === "running";

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
  }, [detect]);

  const entry = buildMcpEntry(
    config,
    config.claude_code.server_name,
    sandboxBinary,
  );
  const preview = generatePreviewJson(entry);

  useEffect(() => {
    invoke<string>("generate_claude_code_cli_command", { entry }).then(
      setCliCommand,
    );
  }, [entry.server_name, entry.command, entry.args.join(",")]);

  const CODE_DENIED_TOOLS = ["Read", "Edit", "Write", "Glob", "Grep", "Bash"];

  // MCP registration is handled by the backend on VM start/stop.
  // The toggle saves the preference; immediate effect only if the VM is running.
  const handleToggle = async (enabled: boolean) => {
    updateSection("claude_code", { enabled });
    if (!isVmRunning) {
      addToast(
        "info",
        enabled
          ? "MCP server will be registered when the sandbox starts."
          : "MCP server preference disabled.",
      );
      return;
    }
    try {
      if (enabled) {
        await invoke("write_claude_code_config", {
          entry,
          scope: config.claude_code.scope,
        });
        if (config.claude_code.disable_builtin_tools) {
          await invoke("set_claude_code_denied_tools", { tools: CODE_DENIED_TOOLS });
        }
        // Set allowed tools (read always; write if toggled)
        const allowTools = [
          "read_file", "list_directory", "glob", "grep",
          "get_undo_history", "get_session_status", "get_working_directory",
          ...(config.claude_code.auto_allow_write_tools
            ? ["Bash", "write_file", "edit_file", "undo"]
            : []),
        ];
        await invoke("set_claude_code_allowed_tools", {
          serverName: config.claude_code.server_name,
          tools: allowTools,
        });
        addToast("warning", "Restart Claude Code for changes to take effect.");
      } else {
        await invoke("remove_claude_code_config", {
          serverName: config.claude_code.server_name,
          scope: config.claude_code.scope,
        });
        await invoke("remove_claude_code_denied_tools", { tools: CODE_DENIED_TOOLS });
        await invoke("remove_claude_code_allowed_tools", {
          serverName: config.claude_code.server_name,
        });
        addToast("warning", "Restart Claude Code for changes to take effect.");
      }
      detect();
    } catch (e) {
      addToast("error", `Failed to update Claude Code config: ${e}`);
    }
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
          The MCP server is automatically registered when the sandbox starts
          and removed when it stops.
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
              onClick={async () => {
                const next = !config.claude_code.auto_allow_write_tools;
                updateSection("claude_code", { auto_allow_write_tools: next });
                if (isVmRunning && config.claude_code.enabled) {
                  try {
                    // Remove existing allow entries and re-add with correct set
                    await invoke("remove_claude_code_allowed_tools", {
                      serverName: config.claude_code.server_name,
                    });
                    const tools = [
                      "read_file", "list_directory", "glob", "grep",
                      "get_undo_history", "get_session_status", "get_working_directory",
                      ...(next ? ["Bash", "write_file", "edit_file", "undo"] : []),
                    ];
                    await invoke("set_claude_code_allowed_tools", {
                      serverName: config.claude_code.server_name,
                      tools,
                    });
                  } catch (e) {
                    addToast("error", `Failed to update allowed tools: ${e}`);
                  }
                }
              }}
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
    </div>
  );
}
