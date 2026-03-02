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
}

let requestIdCounter = 1000;

export const useUndoHistoryStore = create<UndoHistoryState>((set) => ({
  data: null,
  loading: false,
  error: null,

  fetch: async (undoDir: string) => {
    if (!undoDir) {
      set({ data: null, error: null, loading: false });
      return;
    }
    set({ loading: true });
    try {
      const data = await invoke<UndoHistoryData>("read_undo_history", {
        undoDir,
      });
      set({ data, error: null, loading: false });
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
}));

/** Hook that polls undo history on a 5-second interval when undoDir is set. */
export function useUndoHistoryPolling(undoDir: string, vmRunning: boolean) {
  const fetch = useUndoHistoryStore((s) => s.fetch);

  useEffect(() => {
    if (!undoDir || !vmRunning) return;

    fetch(undoDir);
    const interval = setInterval(() => fetch(undoDir), 5000);
    return () => clearInterval(interval);
  }, [undoDir, vmRunning, fetch]);
}
