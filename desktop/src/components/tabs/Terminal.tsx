import { useEffect, useRef, useState } from "react";
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
} from "lucide-react";
import { useTerminalStore, type TerminalEntry } from "../../hooks/useTerminal";
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
  const outputRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (outputRef.current) {
      outputRef.current.scrollTop = outputRef.current.scrollHeight;
    }
  }, [entries]);

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = input.trim();
    if (!trimmed || isRunning) return;
    setInput("");
    execute(trimmed);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowUp") {
      e.preventDefault();
      const cmd = useTerminalStore.getState().navigateHistory("up");
      setInput(cmd);
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      const cmd = useTerminalStore.getState().navigateHistory("down");
      setInput(cmd);
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

      {/* Output area */}
      <div
        ref={outputRef}
        className="flex-1 overflow-y-auto bg-[#1a1a2e] p-3 font-mono text-sm text-gray-200"
        onClick={() => inputRef.current?.focus()}
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

      {/* Input */}
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
  );
}

function DebugConsolePanel() {
  const { lines, filter, autoScroll, visible, setFilter, toggleAutoScroll, clear } =
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
    <div className="flex flex-col border-t-2 border-[var(--color-border)]" style={{ height: "220px" }}>
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

      {/* Log lines */}
      <div
        ref={logRef}
        className="flex-1 overflow-y-auto bg-[#1a1a2e] px-3 py-1 font-mono text-xs"
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
  const { visible, toggleVisible } = useDebugConsoleStore();

  return (
    <div className="flex h-full flex-col">
      {/* Top bar with debug toggle */}
      <div className="flex items-center justify-end px-3 py-1">
        <button
          onClick={toggleVisible}
          className="flex items-center gap-1 rounded px-2 py-1 text-xs text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-tertiary)] hover:text-[var(--color-text)]"
        >
          Debug {visible ? <ChevronDown size={12} /> : <ChevronUp size={12} />}
        </button>
      </div>

      {/* Terminal panel takes remaining space */}
      <TerminalPanel />

      {/* Debug console (collapsible) */}
      <DebugConsolePanel />
    </div>
  );
}
