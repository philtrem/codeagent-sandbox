import { useEffect, useState } from "react";
import {
  Play,
  Square,
  RotateCcw,
  FolderOpen,
  ChevronDown,
  ChevronRight,
  Info,
  AlertTriangle,
  Bug,
  Link2,
  Link2Off,
} from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { useVmStore, useVmPolling } from "../../hooks/useVmStatus";
import { useSandboxConfig } from "../../hooks/useSandboxConfig";
import { invoke } from "@tauri-apps/api/core";
import { useLayoutStore } from "../../hooks/useLayoutStore";
import { useDebugConsoleStore } from "../../hooks/useDebugConsole";

function StatusDot({ state }: { state: string }) {
  const colorMap: Record<string, string> = {
    stopped: "bg-gray-500",
    starting: "bg-yellow-500 animate-pulse",
    running: "bg-green-500",
    error: "bg-red-500",
  };
  return (
    <span
      className={`inline-block h-3 w-3 rounded-full ${colorMap[state] ?? "bg-gray-500"}`}
    />
  );
}

function CollapsibleSection({
  title,
  defaultOpen = false,
  children,
}: {
  title: string;
  defaultOpen?: boolean;
  children: React.ReactNode;
}) {
  const [isOpen, setIsOpen] = useState(defaultOpen);
  return (
    <div className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-secondary)]">
      <button
        onClick={() => setIsOpen(!isOpen)}
        className="flex w-full items-center gap-2 px-4 py-3 text-left text-sm font-semibold"
      >
        {isOpen ? <ChevronDown size={16} /> : <ChevronRight size={16} />}
        {title}
      </button>
      {isOpen && (
        <div className="space-y-3 border-t border-[var(--color-border)] px-4 py-3">
          {children}
        </div>
      )}
    </div>
  );
}

function Slider({
  label,
  value,
  onChange,
  min,
  max,
  step,
  suffix,
  warning,
}: {
  label: string;
  value: number;
  onChange: (v: number) => void;
  min: number;
  max: number;
  step: number;
  suffix?: string;
  warning?: boolean;
}) {
  return (
    <div>
      <div className="mb-1 flex items-center justify-between">
        <label className="text-xs text-[var(--color-text-secondary)]">
          {label}
        </label>
        <span className={`text-xs font-medium ${warning ? "text-amber-400" : ""}`}>
          {value}
          {suffix ? ` ${suffix}` : ""}
        </span>
      </div>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
        className={`w-full ${warning ? "accent-amber-500" : "accent-[var(--color-accent)]"}`}
      />
    </div>
  );
}

function FilePicker({
  label,
  value,
  onChange,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
}) {
  const pick = async () => {
    const selected = await open({ multiple: false });
    if (selected) onChange(selected as string);
  };

  return (
    <div>
      <label className="mb-1 block text-xs text-[var(--color-text-secondary)]">
        {label}
      </label>
      <div className="flex gap-2">
        <input
          type="text"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder="Auto-detect"
          className="min-w-0 flex-1 rounded border border-[var(--color-border)] bg-[var(--color-bg)] px-3 py-1.5 text-sm"
        />
        <button
          onClick={pick}
          className="flex items-center gap-1 rounded border border-[var(--color-border)] px-3 py-1.5 text-sm hover:bg-[var(--color-bg-tertiary)]"
        >
          <FolderOpen size={14} />
        </button>
      </div>
    </div>
  );
}

export default function VmManager() {
  const { status, sandboxMode, start, stop, connectSocket, disconnectSocket } =
    useVmStore();
  const { config, loaded, updateSection } = useSandboxConfig();
  const [platform, setPlatform] = useState<string>("");
  const [cpuCount, setCpuCount] = useState<number>(16);
  const [totalMemoryMb, setTotalMemoryMb] = useState<number>(16384);

  const mcpEnabled = config.claude_code.enabled;

  useVmPolling(mcpEnabled);

  useEffect(() => {
    invoke<string>("get_platform").then(setPlatform);
    invoke<number>("get_cpu_count").then(setCpuCount);
    invoke<number>("get_total_memory_mb").then(setTotalMemoryMb);
  }, []);

  const handleStart = () => start(config);
  const handleStop = () => stop();
  const handleRestart = async () => {
    await stop();
    // Brief delay to let the process fully exit
    await new Promise((r) => setTimeout(r, 500));
    await start(config);
  };

  const isRunning = status.state === "running";
  const isStopped = status.state === "stopped";
  const socketConnected = status.socket_connected;

  return (
    <div className="mx-auto max-w-2xl space-y-4">
      <h1 className="text-xl font-bold">VM Manager</h1>

      {/* Status Panel */}
      <div className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-secondary)] p-4">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3">
            <StatusDot
              state={
                mcpEnabled
                  ? socketConnected
                    ? "running"
                    : "stopped"
                  : status.state
              }
            />
            <div>
              <div className="text-sm font-semibold capitalize">
                {mcpEnabled
                  ? socketConnected
                    ? "Connected"
                    : "Waiting for sandbox\u2026"
                  : status.state}
              </div>
              {!mcpEnabled && status.pid && (
                <div className="text-xs text-[var(--color-text-secondary)]">
                  PID: {status.pid}
                </div>
              )}
              {status.error && (
                <div className="text-xs text-[var(--color-error)]">
                  {status.error}
                </div>
              )}
            </div>
          </div>

          {isRunning && !mcpEnabled && (
            <div className="text-xs text-[var(--color-text-secondary)]">
              {sandboxMode === "vm" ? (
                <>
                  {config.vm.memory_mb} MB &middot; {config.vm.cpus} CPU
                  {config.vm.cpus !== 1 ? "s" : ""} &middot;{" "}
                  {platform === "windows" ? "9P" : "virtiofs"}
                </>
              ) : sandboxMode === "host_only" ? (
                "Host-only mode"
              ) : (
                "Connecting\u2026"
              )}
            </div>
          )}
        </div>
      </div>

      {/* MCP mode info banner */}
      {mcpEnabled && (
        <div className="flex gap-2 rounded-lg border border-blue-500/30 bg-blue-500/10 p-3 text-xs text-[var(--color-text-secondary)]">
          <Info size={14} className="mt-0.5 shrink-0 text-blue-400" />
          <div>
            <span className="font-medium text-[var(--color-text)]">
              Managed by Claude Code
            </span>{" "}
            — The sandbox process is started and stopped by Claude Code. Use the
            Claude Integration tab to configure the MCP server. Terminal and
            debug tools are available once connected.
          </div>
        </div>
      )}

      {/* Host-only mode info banner */}
      {!mcpEnabled && isRunning && sandboxMode === "host_only" && (
        <div className="flex gap-2 rounded-lg border border-blue-500/30 bg-blue-500/10 p-3 text-xs text-[var(--color-text-secondary)]">
          <Info size={14} className="mt-0.5 shrink-0 text-blue-400" />
          <div>
            <span className="font-medium text-[var(--color-text)]">
              Host-only mode
            </span>{" "}
            — Filesystem tools (read, write, edit, undo, etc.) are available.
            Command execution requires a VM. Configure a QEMU binary and guest
            images below to enable full VM mode.
          </div>
        </div>
      )}

      {/* Controls */}
      <div className="flex gap-2">
        {mcpEnabled ? (
          <>
            <button
              onClick={() =>
                socketConnected ? disconnectSocket() : connectSocket()
              }
              className="flex items-center gap-2 rounded border border-[var(--color-border)] px-4 py-2 text-sm font-medium transition-colors hover:bg-[var(--color-bg-tertiary)]"
            >
              {socketConnected ? (
                <>
                  <Link2Off size={14} /> Disconnect
                </>
              ) : (
                <>
                  <Link2 size={14} /> Connect
                </>
              )}
            </button>
            <button
              onClick={() => {
                useDebugConsoleStore.getState().showPanel();
                useLayoutStore.getState().setActiveTab("terminal");
              }}
              disabled={!socketConnected}
              className="flex items-center gap-2 rounded border border-[var(--color-border)] px-4 py-2 text-sm font-medium transition-colors hover:bg-[var(--color-bg-tertiary)] disabled:opacity-40"
            >
              <Bug size={14} /> Debug
            </button>
          </>
        ) : (
          <>
            <button
              onClick={handleStart}
              disabled={!isStopped || !loaded}
              className="flex items-center gap-2 rounded bg-[var(--color-accent)] px-4 py-2 text-sm font-medium text-white transition-colors hover:bg-[var(--color-accent-hover)] disabled:opacity-40"
            >
              <Play size={14} /> Start
            </button>
            <button
              onClick={handleStop}
              disabled={!isRunning}
              className="flex items-center gap-2 rounded border border-[var(--color-border)] px-4 py-2 text-sm font-medium transition-colors hover:bg-[var(--color-bg-tertiary)] disabled:opacity-40"
            >
              <Square size={14} /> Stop
            </button>
            <button
              onClick={handleRestart}
              disabled={!isRunning}
              className="flex items-center gap-2 rounded border border-[var(--color-border)] px-4 py-2 text-sm font-medium transition-colors hover:bg-[var(--color-bg-tertiary)] disabled:opacity-40"
            >
              <RotateCcw size={14} /> Restart
            </button>
            <button
              onClick={() => {
                useDebugConsoleStore.getState().showPanel();
                useLayoutStore.getState().setActiveTab("terminal");
              }}
              disabled={!isRunning}
              className="flex items-center gap-2 rounded border border-[var(--color-border)] px-4 py-2 text-sm font-medium transition-colors hover:bg-[var(--color-bg-tertiary)] disabled:opacity-40"
            >
              <Bug size={14} /> Debug
            </button>

            <div className="ml-auto flex items-center gap-4">
              <label className="flex items-center gap-2 text-xs">
                <input
                  type="checkbox"
                  checked={config.vm.auto_start}
                  onChange={(e) =>
                    updateSection("vm", { auto_start: e.target.checked })
                  }
                  className="accent-[var(--color-accent)]"
                />
                Auto-start
              </label>
              <label className="flex items-center gap-2 text-xs">
                <input
                  type="checkbox"
                  checked={config.vm.persist_vm}
                  onChange={(e) =>
                    updateSection("vm", { persist_vm: e.target.checked })
                  }
                  className="accent-[var(--color-accent)]"
                />
                Keep alive on close
              </label>
            </div>
          </>
        )}
      </div>

      {/* VM Settings */}
      <CollapsibleSection title="VM Settings" defaultOpen={true}>
        <div>
          <Slider
            label="Memory"
            value={config.vm.memory_mb}
            onChange={(v) => updateSection("vm", { memory_mb: v })}
            min={256}
            max={totalMemoryMb}
            step={256}
            suffix="MB"
            warning={config.vm.memory_mb > totalMemoryMb / 2}
          />
          {config.vm.memory_mb > totalMemoryMb / 2 && (
            <div className="mt-1.5 flex items-start gap-1.5 text-xs text-amber-400">
              <AlertTriangle size={12} className="mt-0.5 shrink-0" />
              <span>
                Exceeds 50% of system RAM ({Math.round(totalMemoryMb / 1024)} GB).
                This may cause memory pressure, leading to paging and degraded system performance.
              </span>
            </div>
          )}
        </div>
        <Slider
          label="CPUs"
          value={config.vm.cpus}
          onChange={(v) => updateSection("vm", { cpus: v })}
          min={1}
          max={cpuCount}
          step={1}
        />
        <FilePicker
          label="QEMU Binary"
          value={config.vm.qemu_binary}
          onChange={(v) => updateSection("vm", { qemu_binary: v })}
        />
        <FilePicker
          label="Kernel Image"
          value={config.vm.kernel_path}
          onChange={(v) => updateSection("vm", { kernel_path: v })}
        />
        <FilePicker
          label="Initrd Image"
          value={config.vm.initrd_path}
          onChange={(v) => updateSection("vm", { initrd_path: v })}
        />
        <FilePicker
          label="Rootfs Image (optional)"
          value={config.vm.rootfs_path}
          onChange={(v) => updateSection("vm", { rootfs_path: v })}
        />
        {platform !== "windows" && (
          <FilePicker
            label="virtiofsd Binary"
            value={config.vm.virtiofsd_binary}
            onChange={(v) => updateSection("vm", { virtiofsd_binary: v })}
          />
        )}
      </CollapsibleSection>
    </div>
  );
}
