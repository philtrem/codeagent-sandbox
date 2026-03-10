import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useSandboxConfig } from "./useSandboxConfig";
import { useToastStore } from "./useToastStore";
import { buildMcpEntry } from "../lib/mcpEntry";

/**
 * Globally-mounted hook that syncs the MCP server entry to `.claude.json`
 * whenever enabled and config values change.
 *
 * Mounted in App.tsx so it runs regardless of which tab is active.
 */
export function useClaudeConfigSync() {
  const { config } = useSandboxConfig();
  const addToast = useToastStore((s) => s.addToast);
  const [sandboxBinary, setSandboxBinary] = useState("sandbox");
  const [socketPath, setSocketPath] = useState<string | undefined>();
  const [logFilePath, setLogFilePath] = useState<string | undefined>();
  const prevScope = useRef(config.claude_code.scope);
  const prevServerName = useRef(config.claude_code.server_name);

  useEffect(() => {
    invoke<string>("resolve_sandbox_binary")
      .then(setSandboxBinary)
      .catch(() => {});
    invoke<string>("get_socket_path")
      .then(setSocketPath)
      .catch(() => {});
    invoke<string>("get_log_file_path")
      .then(setLogFilePath)
      .catch(() => {});
  }, []);

  const entry = buildMcpEntry(
    config,
    config.claude_code.server_name,
    sandboxBinary,
    socketPath,
    logFilePath,
  );

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
      .catch((e: unknown) =>
        addToast("error", `Failed to write Claude config: ${e}`),
      );
  }, [config.claude_code.enabled, config.claude_code.scope, entry.server_name, entryKey]);
}
