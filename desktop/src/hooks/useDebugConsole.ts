import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { type DebugLogLine } from "../lib/types";

interface DebugConsoleState {
  lines: DebugLogLine[];
  filter: string;
  autoScroll: boolean;
  visible: boolean;
  panelHeight: number;

  setFilter: (f: string) => void;
  toggleAutoScroll: () => void;
  toggleVisible: () => void;
  showPanel: () => void;
  setPanelHeight: (h: number) => void;
  clear: () => void;
  addLine: (line: DebugLogLine) => void;
  loadExisting: () => Promise<void>;
}

export const useDebugConsoleStore = create<DebugConsoleState>((set) => ({
  lines: [],
  filter: "",
  autoScroll: true,
  visible: false,
  panelHeight: 220,

  setFilter: (f) => set({ filter: f }),
  toggleAutoScroll: () => set((s) => ({ autoScroll: !s.autoScroll })),
  toggleVisible: () => set((s) => ({ visible: !s.visible })),
  showPanel: () => set({ visible: true }),
  setPanelHeight: (h) => set({ panelHeight: Math.max(100, Math.min(500, h)) }),

  clear: () => {
    set({ lines: [] });
    invoke("clear_debug_log").catch(() => {});
  },

  addLine: (line) =>
    set((state) => ({
      lines: [...state.lines, line],
    })),

  loadExisting: async () => {
    try {
      const lines = await invoke<DebugLogLine[]>("get_debug_log", {
        sinceIndex: 0,
      });
      set({ lines });
    } catch {
      // Ignore errors when VM is not running
    }
  },
}));

let unlistenFn: UnlistenFn | null = null;

export async function startDebugConsoleListener() {
  if (unlistenFn) return;

  await useDebugConsoleStore.getState().loadExisting();

  unlistenFn = await listen<DebugLogLine>("vm-debug-log", (event) => {
    useDebugConsoleStore.getState().addLine(event.payload);
  });
}

export function stopDebugConsoleListener() {
  if (unlistenFn) {
    unlistenFn();
    unlistenFn = null;
  }
}
