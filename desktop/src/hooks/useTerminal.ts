import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { type TerminalOutput } from "../lib/types";

export interface TerminalEntry {
  id: string;
  command: string;
  output: string;
  exitCode: number | null;
  status: "running" | "completed" | "timeout" | "error";
  timestamp: string;
}

interface TerminalState {
  entries: TerminalEntry[];
  commandHistory: string[];
  historyIndex: number;
  isRunning: boolean;

  execute: (command: string, timeout?: number) => Promise<void>;
  clear: () => void;
  navigateHistory: (direction: "up" | "down") => string;
}

let nextEntryId = 0;

export const useTerminalStore = create<TerminalState>((set, get) => ({
  entries: [],
  commandHistory: [],
  historyIndex: -1,
  isRunning: false,

  execute: async (command: string, timeout?: number) => {
    const id = `entry-${nextEntryId++}`;
    const entry: TerminalEntry = {
      id,
      command,
      output: "",
      exitCode: null,
      status: "running",
      timestamp: new Date().toISOString(),
    };

    set((state) => ({
      entries: [...state.entries, entry],
      commandHistory: [...state.commandHistory, command],
      historyIndex: -1,
      isRunning: true,
    }));

    try {
      const result = await invoke<TerminalOutput>("execute_terminal_command", {
        command,
        timeout: timeout ?? null,
      });

      set((state) => ({
        entries: state.entries.map((e) =>
          e.id === id
            ? {
                ...e,
                output: result.output,
                exitCode: result.exit_code,
                status: result.status as TerminalEntry["status"],
              }
            : e,
        ),
        isRunning: false,
      }));
    } catch (err) {
      set((state) => ({
        entries: state.entries.map((e) =>
          e.id === id
            ? { ...e, output: String(err), status: "error" as const }
            : e,
        ),
        isRunning: false,
      }));
    }
  },

  clear: () => set({ entries: [] }),

  navigateHistory: (direction: "up" | "down") => {
    const { commandHistory, historyIndex } = get();
    if (commandHistory.length === 0) return "";

    let newIndex: number;
    if (direction === "up") {
      newIndex =
        historyIndex === -1
          ? commandHistory.length - 1
          : Math.max(0, historyIndex - 1);
    } else {
      newIndex =
        historyIndex === -1 ? -1 : Math.min(commandHistory.length - 1, historyIndex + 1);
      if (newIndex === commandHistory.length - 1 && historyIndex === newIndex) {
        newIndex = -1;
      }
    }

    set({ historyIndex: newIndex });
    return newIndex === -1 ? "" : commandHistory[newIndex];
  },
}));
