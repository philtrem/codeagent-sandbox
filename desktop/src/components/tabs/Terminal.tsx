import { useCallback, useEffect, useRef, useState } from "react";
import {
  Trash2,
  ChevronDown,
  ChevronUp,
  ArrowDownToLine,
  Search,
  Loader2,
  Check,
  X,
  Clock,
  Bug,
} from "lucide-react";
import { useTerminalStore, getSuggestions, type TerminalEntry } from "../../hooks/useTerminal";
import {
  useDebugConsoleStore,
  startDebugConsoleListener,
  stopDebugConsoleListener,
} from "../../hooks/useDebugConsole";
import { useVmStore } from "../../hooks/useVmStatus";

function ExitCodeBadge({ entry }: { entry: TerminalEntry }) {
  if (entry.status === "running") {
    return (
      <span className="inline-flex items-center gap-1 text-xs text-yellow-400">
        <Loader2 size={12} className="animate-spin" /> running...
      </span>
    );
  }
  if (entry.status === "error") {
    return (
      <span className="inline-flex items-center gap-1 text-xs text-red-400">
        <X size={12} /> error
      </span>
    );
  }
  if (entry.status === "timeout") {
    return (
      <span className="inline-flex items-center gap-1 text-xs text-yellow-400">
        <Clock size={12} /> timeout
      </span>
    );
  }
  if (entry.exitCode === 0) {
    return (
      <span className="inline-flex items-center gap-1 text-xs text-green-400">
        <Check size={12} /> exit 0
      </span>
    );
  }
  return (
    <span className="inline-flex items-center gap-1 text-xs text-red-400">
      <X size={12} /> exit {entry.exitCode}
    </span>
  );
}

function TerminalPanel() {
  const { entries, isRunning, execute, clear, cwd } = useTerminalStore();
  const vmState = useVmStore((s) => s.status.state);
  const sandboxMode = useVmStore((s) => s.sandboxMode);
  const [input, setInput] = useState("");
  const [suggestions, setSuggestions] = useState<string[]>([]);
  const [selectedSuggestionIndex, setSelectedSuggestionIndex] = useState(-1);
  const outputRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const prevIsRunning = useRef(isRunning);

  useEffect(() => {
    if (outputRef.current) {
      outputRef.current.scrollTop = outputRef.current.scrollHeight;
    }
  }, [entries]);

  // Refocus input when a command finishes running
  useEffect(() => {
    if (prevIsRunning.current && !isRunning) {
      inputRef.current?.focus();
    }
    prevIsRunning.current = isRunning;
  }, [isRunning]);

  // Derive suggestions from input
  useEffect(() => {
    if (!input) {
      setSuggestions([]);
      setSelectedSuggestionIndex(-1);
      return;
    }
    const { commandHistory } = useTerminalStore.getState();
    const matches = getSuggestions(input, commandHistory, 6);
    setSuggestions(matches);
    setSelectedSuggestionIndex(-1);
  }, [input]);

  const dismissSuggestions = useCallback(() => {
    setSuggestions([]);
    setSelectedSuggestionIndex(-1);
  }, []);

  const acceptSuggestion = useCallback((suggestion: string) => {
    setInput(suggestion);
    dismissSuggestions();
    inputRef.current?.focus();
  }, [dismissSuggestions]);

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = input.trim();
    if (!trimmed || isRunning) return;
    setInput("");
    dismissSuggestions();
    execute(trimmed);
    inputRef.current?.focus();
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Tab") {
      e.preventDefault();
      if (suggestions.length === 0) return;
      if (selectedSuggestionIndex === -1) {
        // First Tab press: select and accept the top suggestion
        acceptSuggestion(suggestions[0]);
      } else {
        // Subsequent Tab presses: cycle to next suggestion
        const nextIndex = (selectedSuggestionIndex + 1) % suggestions.length;
        setSelectedSuggestionIndex(nextIndex);
        setInput(suggestions[nextIndex]);
      }
      return;
    }
    if (e.key === "Escape") {
      dismissSuggestions();
      return;
    }
    if (e.key === "ArrowUp") {
      if (suggestions.length > 0) {
        e.preventDefault();
        setSelectedSuggestionIndex((prev) => Math.max(0, prev - 1));
        return;
      }
      e.preventDefault();
      const cmd = useTerminalStore.getState().navigateHistory("up");
      setInput(cmd);
    } else if (e.key === "ArrowDown") {
      if (suggestions.length > 0) {
        e.preventDefault();
        setSelectedSuggestionIndex((prev) =>
          Math.min(suggestions.length - 1, prev + 1),
        );
        return;
      }
      e.preventDefault();
      const cmd = useTerminalStore.getState().navigateHistory("down");
      setInput(cmd);
    } else if (e.key === "Enter" && suggestions.length > 0 && selectedSuggestionIndex >= 0) {
      // If a suggestion is highlighted via arrow keys, accept it on Enter instead of submitting
      e.preventDefault();
      acceptSuggestion(suggestions[selectedSuggestionIndex]);
    }
  };

  const isVmRunning = vmState === "running" && sandboxMode === "vm";

  return (
    <div className="flex flex-1 flex-col overflow-hidden">
      {/* Header */}
      <div className="flex items-center justify-between border-b border-[var(--color-border)] px-3 py-2">
        <span className="text-sm font-semibold">Terminal</span>
        <button
          onClick={clear}
          title="Clear terminal"
          className="rounded p-1 text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-tertiary)] hover:text-[var(--color-text)]"
        >
          <Trash2 size={14} />
        </button>
      </div>

      {/* Output area — select-text overrides global user-select: none */}
      <div
        ref={outputRef}
        className="flex-1 select-text overflow-y-auto bg-[#1a1a2e] p-3 font-mono text-sm text-gray-200"
      >
        {!isVmRunning && vmState !== "running" && (
          <div className="mb-2 text-gray-500">
            VM is not running. Start the VM from the VM Manager tab to use the
            terminal.
          </div>
        )}
        {vmState === "running" && sandboxMode !== "vm" && (
          <div className="mb-2 text-gray-500">
            Running in host-only mode. Command execution requires a VM with QEMU.
          </div>
        )}
        {entries.map((entry) => (
          <div key={entry.id} className="mb-3">
            <div className="text-green-400">
              <span className="text-gray-500">$ </span>{entry.command}
            </div>
            {entry.output && (
              <pre className="mt-1 whitespace-pre-wrap break-all text-gray-300">
                {entry.output}
              </pre>
            )}
            <ExitCodeBadge entry={entry} />
          </div>
        ))}
      </div>

      {/* Input with autocomplete */}
      <div className="relative">
        {/* Suggestions dropdown (above input) */}
        {suggestions.length > 0 && (
          <div className="absolute bottom-full left-0 right-0 z-10 max-h-48 overflow-y-auto border border-[var(--color-border)] bg-[#1e1e3a] shadow-lg">
            {suggestions.map((suggestion, index) => (
              <button
                key={suggestion}
                type="button"
                onClick={() => acceptSuggestion(suggestion)}
                onMouseEnter={() => setSelectedSuggestionIndex(index)}
                className={`block w-full px-3 py-1 text-left font-mono text-sm ${
                  index === selectedSuggestionIndex
                    ? "bg-[var(--color-accent)] text-white"
                    : "text-gray-300 hover:bg-[#2a2a4a]"
                }`}
              >
                {suggestion}
              </button>
            ))}
          </div>
        )}
        <form
          onSubmit={handleSubmit}
          className="flex items-center border-t border-[var(--color-border)] bg-[#1a1a2e] px-3 py-2"
        >
          <span className="mr-2 shrink-0 font-mono text-sm text-gray-500">{cwd}</span>
          <span className="mr-2 font-mono text-sm text-green-400">$</span>
          <input
            ref={inputRef}
            type="text"
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            disabled={!isVmRunning || isRunning}
            placeholder={
              isRunning
                ? "Running..."
                : isVmRunning
                  ? "Type a command..."
                  : "VM not available"
            }
            className="flex-1 bg-transparent font-mono text-sm text-gray-200 outline-none placeholder:text-gray-600 disabled:opacity-50"
            autoFocus
          />
        </form>
      </div>
    </div>
  );
}

function ResizeHandle() {
  const setPanelHeight = useDebugConsoleStore((s) => s.setPanelHeight);
  const containerRef = useRef<HTMLDivElement>(null);

  const handleMouseDown = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault();
      const startY = e.clientY;
      const startHeight = useDebugConsoleStore.getState().panelHeight;

      const handleMouseMove = (moveEvent: MouseEvent) => {
        // Dragging up = increasing panel height (startY - currentY)
        const delta = startY - moveEvent.clientY;
        setPanelHeight(startHeight + delta);
      };

      const handleMouseUp = () => {
        document.removeEventListener("mousemove", handleMouseMove);
        document.removeEventListener("mouseup", handleMouseUp);
        document.body.style.cursor = "";
        document.body.style.userSelect = "";
      };

      document.addEventListener("mousemove", handleMouseMove);
      document.addEventListener("mouseup", handleMouseUp);
      document.body.style.cursor = "row-resize";
      document.body.style.userSelect = "none";
    },
    [setPanelHeight],
  );

  return (
    <div
      ref={containerRef}
      onMouseDown={handleMouseDown}
      className="group flex h-2 cursor-row-resize items-center justify-center border-y border-[var(--color-border)] bg-[var(--color-bg-secondary)] hover:bg-[var(--color-bg-tertiary)]"
    >
      <div className="h-0.5 w-8 rounded-full bg-gray-600 group-hover:bg-gray-400" />
    </div>
  );
}

function DebugConsolePanel() {
  const { lines, filter, autoScroll, visible, panelHeight, setFilter, toggleAutoScroll, clear } =
    useDebugConsoleStore();
  const logRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    startDebugConsoleListener();
    return () => stopDebugConsoleListener();
  }, []);

  useEffect(() => {
    if (autoScroll && logRef.current) {
      logRef.current.scrollTop = logRef.current.scrollHeight;
    }
  }, [lines, autoScroll]);

  const filteredLines = filter
    ? lines.filter((l) => l.line.toLowerCase().includes(filter.toLowerCase()))
    : lines;

  const lineColor = (line: string) => {
    if (line.includes("[qemu serial]")) return "text-cyan-400";
    if (line.includes("[qemu stderr]")) return "text-yellow-400";
    return "text-gray-400";
  };

  if (!visible) return null;

  return (
    <div className="flex flex-col border-t-2 border-[var(--color-border)]" style={{ height: `${panelHeight}px` }}>
      {/* Debug header */}
      <div className="flex items-center gap-2 border-b border-[var(--color-border)] px-3 py-1.5">
        <span className="text-xs font-semibold text-[var(--color-text-secondary)]">
          Debug Console
        </span>
        <div className="ml-auto flex items-center gap-1">
          <div className="relative">
            <Search
              size={12}
              className="absolute left-2 top-1/2 -translate-y-1/2 text-gray-500"
            />
            <input
              type="text"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              placeholder="Filter..."
              className="w-36 rounded border border-[var(--color-border)] bg-[var(--color-bg)] py-0.5 pl-6 pr-2 text-xs"
            />
          </div>
          <button
            onClick={toggleAutoScroll}
            title={autoScroll ? "Auto-scroll on" : "Auto-scroll off"}
            className={`rounded p-1 text-xs ${
              autoScroll
                ? "text-[var(--color-accent)]"
                : "text-[var(--color-text-secondary)]"
            } hover:bg-[var(--color-bg-tertiary)]`}
          >
            <ArrowDownToLine size={12} />
          </button>
          <button
            onClick={clear}
            title="Clear debug log"
            className="rounded p-1 text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-tertiary)] hover:text-[var(--color-text)]"
          >
            <Trash2 size={12} />
          </button>
        </div>
      </div>

      {/* Log lines — select-text overrides global user-select: none */}
      <div
        ref={logRef}
        className="flex-1 select-text overflow-y-auto bg-[#1a1a2e] px-3 py-1 font-mono text-xs"
      >
        {filteredLines.length === 0 ? (
          <div className="py-2 text-gray-600">No debug output yet.</div>
        ) : (
          filteredLines.map((l) => (
            <div key={l.index} className={`leading-5 ${lineColor(l.line)}`}>
              <span className="mr-2 text-gray-600">
                {new Date(l.timestamp).toLocaleTimeString()}
              </span>
              {l.line}
            </div>
          ))
        )}
      </div>
    </div>
  );
}

export default function Terminal() {
  const visible = useDebugConsoleStore((s) => s.visible);
  const toggleVisible = useDebugConsoleStore((s) => s.toggleVisible);

  return (
    <div className="flex h-full flex-col">
      {/* Top bar with debug toggle */}
      <div className="flex items-center justify-end px-3 py-1">
        <button
          onClick={toggleVisible}
          title={visible ? "Hide debug console" : "Show debug console"}
          className="flex h-8 w-8 items-center justify-center rounded text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-tertiary)] hover:text-[var(--color-text)]"
        >
          <Bug size={16} />
          {visible ? <ChevronDown size={10} className="-ml-0.5" /> : <ChevronUp size={10} className="-ml-0.5" />}
        </button>
      </div>

      {/* Terminal panel takes remaining space */}
      <TerminalPanel />

      {/* Resize handle (only when debug console is visible) */}
      {visible && <ResizeHandle />}

      {/* Debug console (collapsible) */}
      <DebugConsolePanel />
    </div>
  );
}
