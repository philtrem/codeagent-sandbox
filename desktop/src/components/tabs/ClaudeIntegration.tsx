import { useEffect, useRef, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Copy,
  Check,
  RefreshCw,
  Terminal,
  ShieldOff,
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

function ClaudeDesktopPanel({
  onRestart,
}: {
  onRestart: () => void;
}) {
  const { config, updateSection } = useSandboxConfig();
  const addToast = useToastStore((s) => s.addToast);
  const [info, setInfo] = useState<ClaudeConfigInfo | null>(null);
  const [sandboxBinary, setSandboxBinary] = useState("sandbox");

  const detect = useCallback(async () => {
    try {
      const result = await invoke<ClaudeConfigInfo>(
        "detect_claude_desktop_config",
      );
      setInfo(result);
    } catch (_) {
      // Ignore detection errors
    }
  }, []);

  useEffect(() => {
    detect();
    invoke<string>("resolve_sandbox_binary")
      .then(setSandboxBinary)
      .catch(() => {});
  }, [detect]);

  const entry = buildMcpEntry(
    config,
    config.claude_desktop.server_name,
    sandboxBinary,
  );
  const preview = generatePreviewJson(entry);

  const DESKTOP_DISALLOWED_TOOLS = ["text_editor", "bash"];

  // Auto-sync config file when settings change and registration is enabled
  const prevPreviewRef = useRef(preview);
  useEffect(() => {
    if (!config.claude_desktop.enabled) return;
    if (sandboxBinary === "sandbox") return;
    if (prevPreviewRef.current === preview) return;
    prevPreviewRef.current = preview;
    invoke("write_claude_desktop_config", { entry }).catch(() => {});
  }, [preview, config.claude_desktop.enabled, sandboxBinary, entry]);

  // Sync tool restrictions when the toggle changes
  const prevDisableRef = useRef(config.claude_desktop.disable_builtin_tools);
  useEffect(() => {
    if (!config.claude_desktop.enabled) return;
    if (prevDisableRef.current === config.claude_desktop.disable_builtin_tools) return;
    prevDisableRef.current = config.claude_desktop.disable_builtin_tools;
    if (config.claude_desktop.disable_builtin_tools) {
      invoke("set_claude_desktop_disallowed_tools", { tools: DESKTOP_DISALLOWED_TOOLS }).catch(() => {});
    } else {
      invoke("remove_claude_desktop_disallowed_tools").catch(() => {});
    }
  }, [config.claude_desktop.disable_builtin_tools, config.claude_desktop.enabled]);

  const handleToggle = async (enabled: boolean) => {
    updateSection("claude_desktop", { enabled });
    try {
      if (enabled) {
        await invoke("write_claude_desktop_config", { entry });
        if (config.claude_desktop.disable_builtin_tools) {
          await invoke("set_claude_desktop_disallowed_tools", { tools: DESKTOP_DISALLOWED_TOOLS });
        }
      } else {
        await invoke("remove_claude_desktop_config", {
          serverName: config.claude_desktop.server_name,
        });
        await invoke("remove_claude_desktop_disallowed_tools");
      }
      onRestart();
      detect();
    } catch (e) {
      addToast("error", `Failed to update Claude Desktop config: ${e}`);
    }
  };

  return (
    <div className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-secondary)] p-4">
      <h2 className="mb-3 text-sm font-semibold">Claude Desktop</h2>

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
            aria-checked={config.claude_desktop.enabled}
            onClick={() => handleToggle(!config.claude_desktop.enabled)}
            className={`relative h-5 w-9 rounded-full transition-colors ${
              config.claude_desktop.enabled
                ? "bg-[var(--color-accent)]"
                : "bg-[var(--color-bg-tertiary)]"
            }`}
          >
            <span
              className={`absolute top-0.5 left-0.5 h-4 w-4 rounded-full bg-white transition-transform ${
                config.claude_desktop.enabled ? "translate-x-4" : ""
              }`}
            />
          </button>
          Register as MCP server
        </label>
        <button
          onClick={detect}
          className="ml-auto text-[var(--color-text-secondary)] hover:text-[var(--color-text)]"
          title="Refresh"
        >
          <RefreshCw size={14} />
        </button>
      </div>

      <div>
        <label className="mb-1 block text-xs text-[var(--color-text-secondary)]">
          Server Name
        </label>
        <input
          type="text"
          value={config.claude_desktop.server_name}
          onChange={(e) =>
            updateSection("claude_desktop", { server_name: e.target.value })
          }
          className="w-full rounded border border-[var(--color-border)] bg-[var(--color-bg)] px-3 py-1.5 text-sm"
        />
      </div>

      <div className="mt-3">
        <label className="flex items-center gap-2 text-sm">
          <button
            role="switch"
            aria-checked={config.claude_desktop.disable_builtin_tools}
            onClick={() =>
              updateSection("claude_desktop", {
                disable_builtin_tools: !config.claude_desktop.disable_builtin_tools,
              })
            }
            className={`relative h-5 w-9 rounded-full transition-colors ${
              config.claude_desktop.disable_builtin_tools
                ? "bg-[var(--color-accent)]"
                : "bg-[var(--color-bg-tertiary)]"
            }`}
          >
            <span
              className={`absolute top-0.5 left-0.5 h-4 w-4 rounded-full bg-white transition-transform ${
                config.claude_desktop.disable_builtin_tools ? "translate-x-4" : ""
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
    </div>
  );
}

function ClaudeCodePanel({
  onRestart,
}: {
  onRestart: () => void;
}) {
  const { config, updateSection } = useSandboxConfig();
  const addToast = useToastStore((s) => s.addToast);
  const [info, setInfo] = useState<ClaudeConfigInfo | null>(null);
  const [sandboxBinary, setSandboxBinary] = useState("sandbox");
  const [cliCommand, setCliCommand] = useState("");

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

  // Auto-sync config file when settings change and registration is enabled
  const prevPreviewRef = useRef(preview);
  useEffect(() => {
    if (!config.claude_code.enabled) return;
    if (sandboxBinary === "sandbox") return;
    if (prevPreviewRef.current === preview) return;
    prevPreviewRef.current = preview;
    invoke("write_claude_code_config", {
      entry,
      scope: config.claude_code.scope,
    }).catch(() => {});
  }, [preview, config.claude_code.enabled, sandboxBinary, entry, config.claude_code.scope]);

  // Sync tool restrictions when the toggle changes
  const prevDisableRef = useRef(config.claude_code.disable_builtin_tools);
  useEffect(() => {
    if (!config.claude_code.enabled) return;
    if (prevDisableRef.current === config.claude_code.disable_builtin_tools) return;
    prevDisableRef.current = config.claude_code.disable_builtin_tools;
    if (config.claude_code.disable_builtin_tools) {
      invoke("set_claude_code_denied_tools", { tools: CODE_DENIED_TOOLS }).catch(() => {});
    } else {
      invoke("remove_claude_code_denied_tools", { tools: CODE_DENIED_TOOLS }).catch(() => {});
    }
  }, [config.claude_code.disable_builtin_tools, config.claude_code.enabled]);

  const handleToggle = async (enabled: boolean) => {
    updateSection("claude_code", { enabled });
    try {
      if (enabled) {
        await invoke("write_claude_code_config", {
          entry,
          scope: config.claude_code.scope,
        });
        if (config.claude_code.disable_builtin_tools) {
          await invoke("set_claude_code_denied_tools", { tools: CODE_DENIED_TOOLS });
        }
      } else {
        await invoke("remove_claude_code_config", {
          serverName: config.claude_code.server_name,
          scope: config.claude_code.scope,
        });
        await invoke("remove_claude_code_denied_tools", { tools: CODE_DENIED_TOOLS });
      }
      onRestart();
      detect();
    } catch (e) {
      addToast("error", `Failed to update Claude Code config: ${e}`);
    }
  };

  return (
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
          Register as MCP server
        </label>
        <button
          onClick={detect}
          className="ml-auto text-[var(--color-text-secondary)] hover:text-[var(--color-text)]"
          title="Refresh"
        >
          <RefreshCw size={14} />
        </button>
      </div>

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
  );
}

export default function ClaudeIntegration() {
  const addToast = useToastStore((s) => s.addToast);

  const handleRestart = () => {
    addToast("warning", "Restart Claude Desktop / Claude Code for changes to take effect.");
  };

  return (
    <div className="mx-auto max-w-4xl">
      <h1 className="mb-6 text-xl font-bold">Claude Integration</h1>

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <ClaudeDesktopPanel onRestart={handleRestart} />
        <ClaudeCodePanel onRestart={handleRestart} />
      </div>
    </div>
  );
}
