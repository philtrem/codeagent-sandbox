import { useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  History,
  RotateCcw,
  Shield,
  AlertTriangle,
  FileText,
  Folder,
  X,
} from "lucide-react";
import { useSandboxConfig } from "../../hooks/useSandboxConfig";
import {
  useUndoHistoryStore,
  useUndoHistoryPolling,
} from "../../hooks/useUndoHistory";
import { useVmStore } from "../../hooks/useVmStatus";
import { useToastStore } from "../../hooks/useToastStore";
import type { UndoStepDetail, BarrierDetail } from "../../lib/types";

function StepTypeBadge({ step }: { step: UndoStepDetail }) {
  if (step.unprotected) {
    return (
      <span className="rounded bg-[var(--color-error)] px-1.5 py-0.5 text-[10px] font-medium text-white">
        unprotected
      </span>
    );
  }
  if (step.command) {
    return (
      <span className="rounded bg-[var(--color-accent)] px-1.5 py-0.5 text-[10px] font-medium text-white">
        command
      </span>
    );
  }
  return (
    <span className="rounded bg-[var(--color-bg-tertiary)] px-1.5 py-0.5 text-[10px] font-medium text-[var(--color-text-secondary)]">
      ambient
    </span>
  );
}

function formatTimestamp(timestamp: string): string {
  try {
    const date = new Date(timestamp);
    return date.toLocaleString(undefined, {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
  } catch {
    return timestamp;
  }
}

function StepCard({
  step,
  stepIndex,
  onRollback,
}: {
  step: UndoStepDetail;
  stepIndex: number;
  onRollback: (stepsToRollBack: number) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const rollbackCount = stepIndex + 1;

  return (
    <div className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-secondary)]">
      <div className="flex items-center gap-3 px-4 py-3">
        <button
          onClick={() => setExpanded(!expanded)}
          className="text-[var(--color-text-secondary)] hover:text-[var(--color-text)]"
        >
          {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        </button>

        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="text-sm font-medium">Step {step.step_id}</span>
            <StepTypeBadge step={step} />
            <span className="text-xs text-[var(--color-text-secondary)]">
              {formatTimestamp(step.timestamp)}
            </span>
          </div>
          {step.command && (
            <div className="mt-0.5 truncate text-xs text-[var(--color-text-secondary)] font-mono">
              {step.command}
            </div>
          )}
          <div className="mt-0.5 flex items-center gap-1 text-xs text-[var(--color-text-secondary)]">
            <FileText size={10} />
            {step.file_count} file{step.file_count !== 1 ? "s" : ""} affected
          </div>
        </div>

        {!step.unprotected && (
          <button
            onClick={() => onRollback(rollbackCount)}
            className="flex shrink-0 items-center gap-1 rounded border border-[var(--color-border)] px-2 py-1 text-xs hover:bg-[var(--color-bg-tertiary)]"
            title={`Roll back ${rollbackCount} step${rollbackCount > 1 ? "s" : ""}`}
          >
            <RotateCcw size={12} />
            Rollback
          </button>
        )}
      </div>

      {expanded && step.files.length > 0 && (
        <div className="border-t border-[var(--color-border)] px-4 py-2">
          <div className="max-h-40 space-y-1 overflow-auto">
            {step.files.map((file) => (
              <div
                key={file.path}
                className="flex items-center gap-2 text-xs text-[var(--color-text-secondary)]"
              >
                {file.file_type === "directory" ? (
                  <Folder size={10} />
                ) : (
                  <FileText size={10} />
                )}
                <span className="truncate font-mono">{file.path}</span>
                <span className="shrink-0">
                  {file.existed_before ? "(modified)" : "(created)"}
                </span>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

function BarrierIndicator({ barrier }: { barrier: BarrierDetail }) {
  const [expanded, setExpanded] = useState(false);

  return (
    <div className="flex items-center gap-2 px-2 py-1">
      <div className="h-px flex-1 bg-[var(--color-warning)]" />
      <button
        onClick={() => setExpanded(!expanded)}
        className="flex items-center gap-1 text-xs text-[var(--color-warning)]"
      >
        <Shield size={12} />
        Barrier (after step {barrier.after_step_id})
        {expanded ? <ChevronDown size={10} /> : <ChevronRight size={10} />}
      </button>
      <div className="h-px flex-1 bg-[var(--color-warning)]" />
      {expanded && barrier.affected_paths.length > 0 && (
        <div className="text-xs text-[var(--color-text-secondary)]">
          {barrier.affected_paths.join(", ")}
        </div>
      )}
    </div>
  );
}

function RollbackDialog({
  count,
  hasBarriers,
  onConfirm,
  onCancel,
}: {
  count: number;
  hasBarriers: boolean;
  onConfirm: (force: boolean) => void;
  onCancel: () => void;
}) {
  const [force, setForce] = useState(false);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div className="w-96 rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-secondary)] p-6 shadow-xl">
        <div className="mb-4 flex items-center justify-between">
          <h3 className="text-sm font-semibold">Confirm Rollback</h3>
          <button
            onClick={onCancel}
            className="text-[var(--color-text-secondary)] hover:text-[var(--color-text)]"
          >
            <X size={16} />
          </button>
        </div>

        <p className="mb-4 text-sm text-[var(--color-text-secondary)]">
          This will roll back the most recent{" "}
          <strong className="text-[var(--color-text)]">{count}</strong> step
          {count > 1 ? "s" : ""}. This action cannot be undone.
        </p>

        {hasBarriers && (
          <div className="mb-4 rounded border border-[var(--color-warning)] bg-[var(--color-bg)] p-3">
            <div className="mb-2 flex items-center gap-2 text-sm text-[var(--color-warning)]">
              <AlertTriangle size={14} />
              Barriers detected
            </div>
            <p className="mb-2 text-xs text-[var(--color-text-secondary)]">
              External modifications were detected between some steps. Force
              rollback will cross these barriers and may lose external changes.
            </p>
            <label className="flex items-center gap-2 text-xs">
              <input
                type="checkbox"
                checked={force}
                onChange={(e) => setForce(e.target.checked)}
                className="rounded"
              />
              Force rollback (cross barriers)
            </label>
          </div>
        )}

        <div className="flex justify-end gap-2">
          <button
            onClick={onCancel}
            className="rounded border border-[var(--color-border)] px-4 py-2 text-sm hover:bg-[var(--color-bg-tertiary)]"
          >
            Cancel
          </button>
          <button
            onClick={() => onConfirm(force)}
            disabled={hasBarriers && !force}
            className="rounded bg-[var(--color-error)] px-4 py-2 text-sm text-white hover:opacity-90 disabled:opacity-50"
          >
            Roll Back {count} Step{count > 1 ? "s" : ""}
          </button>
        </div>
      </div>
    </div>
  );
}

export default function UndoHistory() {
  const { config } = useSandboxConfig();
  const vmStatus = useVmStore((s) => s.status);
  const { data, loading, error } = useUndoHistoryStore();
  const rollback = useUndoHistoryStore((s) => s.rollback);
  const fetchHistory = useUndoHistoryStore((s) => s.fetch);
  const addToast = useToastStore((s) => s.addToast);

  const vmRunning = vmStatus.state === "running";
  const undoDir = config.sandbox.undo_dir;

  useUndoHistoryPolling(undoDir, vmRunning);

  const [pendingRollback, setPendingRollback] = useState<number | null>(null);

  const handleRollback = (count: number) => {
    setPendingRollback(count);
  };

  const confirmRollback = async (force: boolean) => {
    if (pendingRollback === null) return;
    const count = pendingRollback;
    setPendingRollback(null);

    try {
      const response = await rollback(count, force);
      const parsed = JSON.parse(response);
      if (parsed.error) {
        addToast("error", `Rollback failed: ${parsed.error.message || JSON.stringify(parsed.error)}`);
      } else {
        addToast("success", `Rolled back ${count} step${count > 1 ? "s" : ""}`);
        fetchHistory(undoDir);
      }
    } catch (e) {
      addToast("error", `Rollback failed: ${e}`);
    }
  };

  // Check if barriers exist in the range being rolled back
  const hasBarriersInRange = (count: number): boolean => {
    if (!data) return false;
    const stepsToRollBack = data.steps.slice(0, count);
    const stepIds = new Set(stepsToRollBack.map((s) => s.step_id));
    return data.barriers.some((b) => stepIds.has(b.after_step_id));
  };

  if (!vmRunning && !undoDir) {
    return (
      <div className="mx-auto max-w-2xl">
        <h1 className="mb-6 text-xl font-bold">Undo History</h1>
        <div className="flex flex-col items-center gap-4 rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-secondary)] py-12 text-center">
          <History size={48} className="text-[var(--color-text-secondary)]" />
          <p className="text-sm text-[var(--color-text-secondary)]">
            Start a session to view undo history
          </p>
          <p className="text-xs text-[var(--color-text-secondary)]">
            Configure an undo directory in the Settings tab, then start the VM
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-2xl">
      <h1 className="mb-6 text-xl font-bold">Undo History</h1>

      {loading && !data && (
        <div className="text-sm text-[var(--color-text-secondary)]">
          Loading undo history...
        </div>
      )}

      {error && (
        <div className="mb-4 rounded-lg border border-[var(--color-error)] bg-[var(--color-bg-secondary)] p-3 text-sm text-[var(--color-error)]">
          {error}
        </div>
      )}

      {data && data.steps.length === 0 && (
        <div className="flex flex-col items-center gap-4 rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-secondary)] py-12 text-center">
          <History size={48} className="text-[var(--color-text-secondary)]" />
          <p className="text-sm text-[var(--color-text-secondary)]">
            No undo steps recorded yet
          </p>
        </div>
      )}

      {data && data.steps.length > 0 && (
        <div className="space-y-2">
          {data.steps.map((step, index) => {
            // Find barriers that sit after this step (between this step and the next newer one)
            const barriersAfterStep = data.barriers.filter(
              (b) => b.after_step_id === step.step_id,
            );

            return (
              <div key={step.step_id}>
                {barriersAfterStep.map((barrier) => (
                  <BarrierIndicator
                    key={barrier.barrier_id}
                    barrier={barrier}
                  />
                ))}
                <StepCard
                  step={step}
                  stepIndex={index}
                  onRollback={handleRollback}
                />
              </div>
            );
          })}
        </div>
      )}

      {pendingRollback !== null && (
        <RollbackDialog
          count={pendingRollback}
          hasBarriers={hasBarriersInRange(pendingRollback)}
          onConfirm={confirmRollback}
          onCancel={() => setPendingRollback(null)}
        />
      )}
    </div>
  );
}
