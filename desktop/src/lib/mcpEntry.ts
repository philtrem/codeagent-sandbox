import type { SandboxConfig, McpServerEntry } from "./types";

/**
 * Build the MCP server entry for `.claude.json`.
 *
 * Working directories and undo directory are NOT included in the args —
 * the sandbox binary reads those from `codeagent.toml` at startup.
 */
export function buildMcpEntry(
  config: SandboxConfig,
  serverName: string,
  sandboxBinary: string,
  socketPath?: string,
  logFilePath?: string,
): McpServerEntry {
  const args: string[] = [];

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

export function generatePreviewJson(entry: McpServerEntry): string {
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
