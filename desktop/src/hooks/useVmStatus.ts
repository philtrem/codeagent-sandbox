import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { type VmStatus, type SandboxConfig } from "../lib/types";
import { useEffect } from "react";

interface VmState {
  status: VmStatus;
  polling: boolean;
  start: (config: SandboxConfig) => Promise<void>;
  stop: () => Promise<void>;
  poll: () => Promise<void>;
}

export const useVmStore = create<VmState>((set) => ({
  status: { state: "stopped", pid: null, error: null },
  polling: false,

  start: async (config: SandboxConfig) => {
    set({
      status: { state: "starting", pid: null, error: null },
    });
    try {
      const status = await invoke<VmStatus>("start_vm", { config });
      set({ status });
    } catch (e) {
      set({
        status: { state: "error", pid: null, error: String(e) },
      });
    }
  },

  stop: async () => {
    try {
      const status = await invoke<VmStatus>("stop_vm");
      set({ status });
    } catch (e) {
      set({
        status: { state: "error", pid: null, error: String(e) },
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
}));

/** Hook that polls VM status on a 2-second interval. */
export function useVmPolling() {
  const poll = useVmStore((s) => s.poll);

  useEffect(() => {
    poll();
    const interval = setInterval(poll, 2000);
    return () => clearInterval(interval);
  }, [poll]);
}
