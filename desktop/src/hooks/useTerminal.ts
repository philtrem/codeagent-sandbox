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

/** Common Linux/shell commands for autocomplete suggestions (busybox sh compatible). */
const COMMON_COMMANDS = [
  "ls", "cd", "cat", "echo", "mkdir", "rmdir", "rm", "cp", "mv", "touch",
  "pwd", "find", "grep", "head", "tail", "wc", "sort", "uniq", "chmod",
  "chown", "ps", "kill", "df", "du", "tar", "gzip", "gunzip", "curl",
  "wget", "which", "whoami", "env", "export", "source", "history", "clear",
  "sed", "awk", "cut", "tr", "tee", "xargs", "diff", "file", "stat",
  "ln", "readlink", "mount", "umount", "uname", "date", "sleep", "true",
  "false", "test", "sh", "bash",
];

/**
 * Return autocomplete suggestions for the given input prefix.
 *
 * History matches appear first (most recent first, deduplicated),
 * followed by common commands. Results are capped at `limit`.
 */
export function getSuggestions(prefix: string, commandHistory: string[], limit = 8): string[] {
  if (!prefix) return [];

  const lower = prefix.toLowerCase();
  const seen = new Set<string>();
  const results: string[] = [];

  // History matches — iterate newest-first so recent commands rank higher.
  for (let i = commandHistory.length - 1; i >= 0; i--) {
    const cmd = commandHistory[i];
    if (cmd.toLowerCase().startsWith(lower) && cmd !== prefix && !seen.has(cmd)) {
      seen.add(cmd);
      results.push(cmd);
      if (results.length >= limit) return results;
    }
  }

  // Common command matches.
  for (const cmd of COMMON_COMMANDS) {
    if (cmd.startsWith(lower) && cmd !== prefix && !seen.has(cmd)) {
      seen.add(cmd);
      results.push(cmd);
      if (results.length >= limit) return results;
    }
  }

  return results;
}

/** Commands that take directory arguments (filter to directories only). */
const DIRECTORY_COMMANDS = new Set(["cd", "mkdir", "rmdir", "ls", "pushd"]);

/**
 * Parse the input to extract the argument being typed (the last whitespace-delimited token
 * after the command name). Returns null if no argument is being typed yet.
 */
function parseArgumentPrefix(input: string): { command: string; argPrefix: string } | null {
  const spaceIndex = input.indexOf(" ");
  if (spaceIndex === -1) return null;

  const command = input.slice(0, spaceIndex);
  const rest = input.slice(spaceIndex + 1);

  // Find the last "word" being typed (handle multiple args — only complete the last one)
  const lastSpaceIndex = rest.lastIndexOf(" ");
  const argPrefix = lastSpaceIndex === -1 ? rest : rest.slice(lastSpaceIndex + 1);

  return { command, argPrefix };
}

/** Directory listing cache to avoid repeated VM calls for the same directory. */
const pathCache = new Map<string, { entries: string[]; timestamp: number }>();
const PATH_CACHE_TTL_MS = 5000;

/**
 * Fetch path completions from the VM using bash `compgen`.
 *
 * Calls `execute_terminal_command` directly (bypassing the terminal store)
 * so the command doesn't appear in terminal output.
 */
export async function getPathSuggestions(
  input: string,
  cwd: string,
  limit = 6,
): Promise<string[]> {
  const parsed = parseArgumentPrefix(input);
  if (!parsed || parsed.argPrefix.length === 0) return [];

  const { command, argPrefix } = parsed;
  const directoriesOnly = DIRECTORY_COMMANDS.has(command);
  const compgenFlag = directoriesOnly ? "-d" : "-f";

  const cacheKey = `${cwd}:${compgenFlag}:${argPrefix}`;
  const cached = pathCache.get(cacheKey);
  if (cached && Date.now() - cached.timestamp < PATH_CACHE_TTL_MS) {
    return cached.entries.slice(0, limit);
  }

  try {
    const shellCmd = `cd ${shellQuote(cwd)} && compgen ${compgenFlag} -- ${shellQuote(argPrefix)}`;
    const result = await invoke<TerminalOutput>("execute_terminal_command", {
      command: shellCmd,
      timeout: 3,
    });

    // compgen returns exit code 1 when there are no matches
    if (result.status !== "completed") return [];
    if (result.exit_code !== 0 && result.exit_code !== 1) {
      console.warn("[autocomplete] unexpected exit code:", result.exit_code, result.output);
      return [];
    }

    const entries = result.output
      .trim()
      .split("\n")
      .filter((line) => line.length > 0);

    pathCache.set(cacheKey, { entries, timestamp: Date.now() });
    return entries.slice(0, limit);
  } catch (err) {
    console.warn("[autocomplete] path suggestion failed:", err);
    return [];
  }
}

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

/** Root working directory inside the guest VM. */
const ROOT_CWD = "/mnt/working";

export const useTerminalStore = create<TerminalState>((set, get) => ({
  entries: [],
  commandHistory: [],
  historyIndex: -1,
  isRunning: false,
  cwd: ROOT_CWD,

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

      // Detect when the cwd no longer exists (e.g. deleted by rollback).
      // The wrapped command starts with `cd '<cwd>' && ...`, so when the
      // directory is gone bash reports "cd: <path>: No such file or directory".
      // We check for this specific pattern to avoid false positives from the
      // user's own command (e.g. `cat nonexistent.txt`).
      if (
        result.exit_code !== 0 &&
        cwd !== ROOT_CWD &&
        output.includes(`cd: ${cwd}`)
      ) {
        newCwd = ROOT_CWD;
        output += `\n(directory no longer exists — returned to ${ROOT_CWD})`;
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
