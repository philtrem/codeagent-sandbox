import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { type VmStatus, type SandboxConfig } from "../lib/types";
import { useEffect } from "react";

export type SandboxMode = "vm" | "host_only" | null;

interface VmState {
  status: VmStatus;
  sandboxMode: SandboxMode;
  polling: boolean;
  start: (config: SandboxConfig) => Promise<void>;
  stop: () => Promise<void>;
  poll: () => Promise<void>;
  connectSocket: () => Promise<void>;
  disconnectSocket: () => Promise<void>;
}

const STOPPED_STATUS: VmStatus = {
  state: "stopped",
  pid: null,
  error: null,
  socket_connected: false,
};

/** Query the sandbox process for its session status via MCP. */
async function querySandboxMode(): Promise<SandboxMode> {
  try {
    const request = JSON.stringify({
      jsonrpc: "2.0",
      id: "status-probe",
      method: "tools/call",
      params: { name: "get_session_status", arguments: {} },
    });
    const response = await invoke<string>("send_mcp_request", {
      requestJson: request,
    });
    const parsed = JSON.parse(response);
    const content = parsed?.result?.content;
    if (Array.isArray(content) && content.length > 0) {
      const status = JSON.parse(content[0].text);
      return status.vm_status === "running" ? "vm" : "host_only";
    }
    return "host_only";
  } catch {
    return null;
  }
}

export const useVmStore = create<VmState>((set) => ({
  status: { ...STOPPED_STATUS },
  sandboxMode: null,
  polling: false,

  start: async (config: SandboxConfig) => {
    set({
      status: { state: "starting", pid: null, error: null, socket_connected: false },
      sandboxMode: null,
    });
    try {
      const status = await invoke<VmStatus>("start_vm", { config });
      set({ status });
      // Give the sandbox a moment to initialize, then query its mode
      setTimeout(async () => {
        const mode = await querySandboxMode();
        set({ sandboxMode: mode });
      }, 1000);
    } catch (e) {
      set({
        status: { state: "error", pid: null, error: String(e), socket_connected: false },
        sandboxMode: null,
      });
    }
  },

  stop: async () => {
    try {
      const status = await invoke<VmStatus>("stop_vm");
      set({ status, sandboxMode: null });
    } catch (e) {
      set({
        status: { state: "error", pid: null, error: String(e), socket_connected: false },
        sandboxMode: null,
      });
    }
  },

  poll: async () => {
    try {
      const status = await invoke<VmStatus>("get_vm_status");
      set({ status });
    } catch (_) {
      // Silently ignore polling errors
    }
  },

  connectSocket: async () => {
    try {
      const status = await invoke<VmStatus>("connect_to_sandbox");
      set({ status });
      // Query sandbox mode once connected
      const mode = await querySandboxMode();
      set({ sandboxMode: mode });
    } catch (_) {
      // Socket not available yet — ignore
    }
  },

  disconnectSocket: async () => {
    try {
      await invoke("disconnect_from_sandbox");
      set({ status: { ...STOPPED_STATUS }, sandboxMode: null });
    } catch (_) {
      // Ignore
    }
  },
}));

/** Hook that polls VM status on a 2-second interval.
 *  In MCP mode (mcpEnabled=true), also attempts socket connection if not connected. */
export function useVmPolling(mcpEnabled = false) {
  const poll = useVmStore((s) => s.poll);
  const connectSocket = useVmStore((s) => s.connectSocket);
  const socketConnected = useVmStore((s) => s.status.socket_connected);

  useEffect(() => {
    poll();
    const interval = setInterval(() => {
      poll();
      // In MCP mode, attempt socket connection if not yet connected
      if (mcpEnabled && !socketConnected) {
        connectSocket();
      }
    }, 2000);
    return () => clearInterval(interval);
  }, [poll, connectSocket, mcpEnabled, socketConnected]);
}
