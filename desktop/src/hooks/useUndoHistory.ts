import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import type { UndoHistoryData } from "../lib/types";
import { useEffect } from "react";

interface UndoHistoryState {
  data: UndoHistoryData | null;
  loading: boolean;
  error: string | null;
  fetch: (undoDir: string) => Promise<void>;
  rollback: (count: number, force: boolean) => Promise<string>;
  clearHistory: (undoDir: string, vmRunning: boolean) => Promise<void>;
}

let requestIdCounter = 1000;

export const useUndoHistoryStore = create<UndoHistoryState>((set, get) => ({
  data: null,
  loading: false,
  error: null,

  fetch: async (undoDir: string) => {
    if (!undoDir) {
      set({ data: null, error: null, loading: false });
      return;
    }
    // Only show loading spinner on initial fetch, not polling refreshes.
    if (!get().data) {
      set({ loading: true });
    }
    try {
      const newData = await invoke<UndoHistoryData>("read_undo_history", {
        undoDir,
      });
      // Skip update if data hasn't changed to avoid unnecessary re-renders.
      const current = get();
      if (current.data && JSON.stringify(newData) === JSON.stringify(current.data)) {
        return;
      }
      set({ data: newData, error: null, loading: false });
    } catch (e) {
      set({ error: String(e), loading: false });
    }
  },

  rollback: async (count: number, force: boolean) => {
    const id = requestIdCounter++;
    const request = JSON.stringify({
      jsonrpc: "2.0",
      id,
      method: "tools/call",
      params: {
        name: "undo",
        arguments: { count, force },
      },
    });

    const response = await invoke<string>("send_mcp_request", {
      requestJson: request,
    });
    return response;
  },

  clearHistory: async (undoDir: string, vmRunning: boolean) => {
    if (vmRunning) {
      // Send MCP discard to the running sandbox — this properly resets
      // both in-memory state and on-disk undo directories.
      const id = requestIdCounter++;
      const request = JSON.stringify({
        jsonrpc: "2.0",
        id,
        method: "tools/call",
        params: {
          name: "discard_undo_history",
          arguments: {},
        },
      });
      await invoke<string>("send_mcp_request", { requestJson: request });
    } else {
      // VM not running — clean up on-disk files directly.
      await invoke("clear_undo_history", { undoDir });
    }
    set({ data: null, error: null });
  },
}));

/** Hook that polls undo history on a 500ms interval when sandbox is reachable.
 *  Polls when the VM is running (manual mode) or when connected via socket (MCP mode). */
export function useUndoHistoryPolling(
  undoDir: string,
  vmRunning: boolean,
  socketConnected = false,
) {
  const fetch = useUndoHistoryStore((s) => s.fetch);

  useEffect(() => {
    if (!undoDir || (!vmRunning && !socketConnected)) return;

    fetch(undoDir);
    const interval = setInterval(() => fetch(undoDir), 500);
    return () => clearInterval(interval);
  }, [undoDir, vmRunning, socketConnected, fetch]);
}
