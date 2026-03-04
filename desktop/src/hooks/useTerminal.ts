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
  cwd: string;

  execute: (command: string, timeout?: number) => Promise<void>;
  clear: () => void;
  navigateHistory: (direction: "up" | "down") => string;
}

let nextEntryId = 0;

/** Check whether the command is a bare `cd` (possibly with a path argument). */
function isCdCommand(command: string): boolean {
  const trimmed = command.trim();
  return trimmed === "cd" || trimmed.startsWith("cd ");
}

/**
 * Wrap a command so it runs inside the tracked working directory.
 *
 * For `cd` commands we append `&& pwd` so we can read back the new
 * absolute path from the output.  For everything else we just prepend
 * `cd <cwd> && `.
 */
function wrapCommand(command: string, cwd: string): { wrapped: string; expectsCwd: boolean } {
  if (isCdCommand(command)) {
    return {
      wrapped: `cd ${shellQuote(cwd)} && ${command} && pwd`,
      expectsCwd: true,
    };
  }
  return {
    wrapped: `cd ${shellQuote(cwd)} && ${command}`,
    expectsCwd: false,
  };
}

/** Minimal POSIX shell quoting (single-quote the path). */
function shellQuote(path: string): string {
  return `'${path.replace(/'/g, "'\\''")}'`;
}

export const useTerminalStore = create<TerminalState>((set, get) => ({
  entries: [],
  commandHistory: [],
  historyIndex: -1,
  isRunning: false,
  cwd: "/mnt/working",

  execute: async (command: string, timeout?: number) => {
    const id = `entry-${nextEntryId++}`;
    const { cwd } = get();

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

    const { wrapped, expectsCwd } = wrapCommand(command, cwd);

    try {
      const result = await invoke<TerminalOutput>("execute_terminal_command", {
        command: wrapped,
        timeout: timeout ?? null,
      });

      let output = result.output;
      let newCwd = cwd;

      // For cd commands, the last line of output is the new working directory
      if (expectsCwd && result.status === "completed" && result.exit_code === 0) {
        const lines = output.trimEnd().split("\n");
        if (lines.length > 0) {
          const lastLine = lines[lines.length - 1].trim();
          // pwd output is always an absolute path
          if (lastLine.startsWith("/")) {
            newCwd = lastLine;
            // Remove the pwd output line from displayed output
            lines.pop();
            output = lines.join("\n");
          }
        }
      }

      set((state) => ({
        entries: state.entries.map((e) =>
          e.id === id
            ? {
                ...e,
                output,
                exitCode: result.exit_code,
                status: result.status as TerminalEntry["status"],
              }
            : e,
        ),
        isRunning: false,
        cwd: newCwd,
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
