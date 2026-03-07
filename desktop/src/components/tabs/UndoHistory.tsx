import { useState, useMemo } from "react";
import {
  ChevronDown,
  ChevronRight,
  History,
  RotateCcw,
  Shield,
  AlertTriangle,
  FileText,
  Folder,
  Trash2,
  X,
  Clock,
} from "lucide-react";
import { useSandboxConfig } from "../../hooks/useSandboxConfig";
import {
  useUndoHistoryStore,
  useUndoHistoryPolling,
} from "../../hooks/useUndoHistory";
import { useVmStore } from "../../hooks/useVmStatus";
import { useToastStore } from "../../hooks/useToastStore";
import type { UndoStepDetail, BarrierDetail, UndoHistoryData } from "../../lib/types";

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
  const rollbackCount = stepIndex;
  const hasFiles = step.files.length > 0;

  return (
    <div className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-secondary)]">
      <div className="flex items-center gap-3 px-4 py-3">
        {hasFiles ? (
          <button
            onClick={() => setExpanded(!expanded)}
            className="text-[var(--color-text-secondary)] hover:text-[var(--color-text)]"
          >
            {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
          </button>
        ) : (
          <span className="w-[14px]" />
        )}

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
            onClick={() => onRollback(stepIndex === 0 ? 1 : rollbackCount)}
            className="flex shrink-0 items-center gap-1 rounded border border-[var(--color-border)] px-2 py-1 text-xs hover:bg-[var(--color-bg-tertiary)]"
            title={stepIndex === 0
              ? "Undo this step, restoring files to the state before it"
              : `Undo the ${rollbackCount} most recent step${rollbackCount > 1 ? "s" : ""}, restoring files to the state after this step`
            }
          >
            <RotateCcw size={12} />
            {stepIndex === 0 ? "Undo this step" : "Undo to here"}
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
  const [expanded, setExpanded] = useState(true);

  const label =
    barrier.reason === "session_start"
      ? "Session start — files may have changed between sessions"
      : "External change — files modified outside the sandbox";

  return (
    <div className="px-2 py-1">
      <div className="flex items-center gap-2">
        <div className="h-px flex-1 bg-[var(--color-warning)]" />
        <button
          onClick={() => setExpanded(!expanded)}
          className="flex items-center gap-1 text-xs text-[var(--color-warning)]"
        >
          <Shield size={12} />
          {label}
          {expanded ? <ChevronDown size={10} /> : <ChevronRight size={10} />}
        </button>
        <div className="h-px flex-1 bg-[var(--color-warning)]" />
      </div>
      {expanded && barrier.affected_paths.length > 0 && (
        <div className="mt-1.5 rounded border border-[var(--color-border)] bg-[var(--color-bg-secondary)] px-3 py-2">
          <div className="space-y-0.5">
            {[...barrier.affected_paths].reverse().map((path) => (
              <div key={path} className="truncate text-xs font-mono text-[var(--color-text-secondary)]">
                {path}
              </div>
            ))}
          </div>
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

function ClearHistoryDialog({
  onConfirm,
  onCancel,
}: {
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div className="w-96 rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-secondary)] p-6 shadow-xl">
        <div className="mb-4 flex items-center justify-between">
          <h3 className="text-sm font-semibold">Clear Undo History</h3>
          <button
            onClick={onCancel}
            className="text-[var(--color-text-secondary)] hover:text-[var(--color-text)]"
          >
            <X size={16} />
          </button>
        </div>

        <p className="mb-4 text-sm text-[var(--color-text-secondary)]">
          This will permanently remove all undo history. You will not be able to
          roll back any previous changes. This action cannot be undone.
        </p>

        <div className="flex justify-end gap-2">
          <button
            onClick={onCancel}
            className="rounded border border-[var(--color-border)] px-4 py-2 text-sm hover:bg-[var(--color-bg-tertiary)]"
          >
            Cancel
          </button>
          <button
            onClick={onConfirm}
            className="rounded bg-[var(--color-error)] px-4 py-2 text-sm text-white hover:opacity-90"
          >
            Clear All History
          </button>
        </div>
      </div>
    </div>
  );
}

/** Renders steps using pre-computed original indices for correct rollback counts. */
function StepList({
  steps,
  startIndex = 0,
  barriers,
  onRollback,
}: {
  steps: UndoStepDetail[];
  startIndex?: number;
  barriers: BarrierDetail[];
  onRollback: (stepsToRollBack: number) => void;
}) {
  return (
    <div className="space-y-2">
      {steps.map((step, index) => {
        const barriersAfterStep = barriers.filter(
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
              stepIndex={startIndex + index}
              onRollback={onRollback}
            />
          </div>
        );
      })}
    </div>
  );
}

function SessionGroupedSteps({
  data,
  onRollback,
}: {
  data: UndoHistoryData;
  onRollback: (stepsToRollBack: number) => void;
}) {
  const [showPrevious, setShowPrevious] = useState(false);

  // Find the session boundary: the session_start barrier with the highest
  // after_step_id marks where the current session started. Steps above it
  // are current session. Watcher barriers (external_modification) are ignored
  // for session boundary detection.
  const sessionBoundary = useMemo(() => {
    const sessionBarriers = data.barriers.filter(
      (b) => b.reason === "session_start",
    );
    if (sessionBarriers.length === 0) return null;
    return sessionBarriers.reduce((max, b) =>
      b.after_step_id > max.after_step_id ? b : max,
    );
  }, [data.barriers]);

  const { currentSteps, previousSteps } = useMemo(() => {
    if (!sessionBoundary) {
      return { currentSteps: data.steps, previousSteps: [] as UndoStepDetail[] };
    }
    const splitIndex = data.steps.findIndex(
      (s) => s.step_id <= sessionBoundary.after_step_id,
    );
    if (splitIndex <= 0) {
      return { currentSteps: data.steps, previousSteps: [] as UndoStepDetail[] };
    }
    return {
      currentSteps: data.steps.slice(0, splitIndex),
      previousSteps: data.steps.slice(splitIndex),
    };
  }, [data.steps, sessionBoundary]);

  const hasPreviousSteps = previousSteps.length > 0;

  // Barriers whose after_step_id doesn't match any existing step (e.g., pre-step
  // host modifications stored with after_step_id = 0).
  const allStepIds = new Set(data.steps.map((s) => s.step_id));
  const orphanBarriers = data.barriers.filter((b) => !allStepIds.has(b.after_step_id));

  return (
    <div className="space-y-2">
      {currentSteps.length > 0 && (
        <StepList
          steps={currentSteps}
          barriers={data.barriers}
          onRollback={onRollback}
        />
      )}

      {orphanBarriers.map((barrier) => (
        <BarrierIndicator key={barrier.barrier_id} barrier={barrier} />
      ))}

      {currentSteps.length === 0 && orphanBarriers.length === 0 && (
        <div className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-secondary)] px-4 py-6 text-center text-sm text-[var(--color-text-secondary)]">
          No steps in the current session
        </div>
      )}

      {hasPreviousSteps && (
        <div className="pt-2">
          <button
            onClick={() => setShowPrevious(!showPrevious)}
            className="flex w-full items-center gap-2 rounded-lg border border-dashed border-[var(--color-border)] px-4 py-2.5 text-xs text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-secondary)]"
          >
            {showPrevious ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
            <Clock size={12} />
            {showPrevious ? "Hide" : "Show"} {previousSteps.length} step{previousSteps.length !== 1 ? "s" : ""} from previous sessions
          </button>

          {showPrevious && (
            <div className="mt-2 space-y-2 border-l-2 border-dashed border-[var(--color-border)] pl-3">
              <StepList
                steps={previousSteps}
                startIndex={currentSteps.length}
                barriers={data.barriers}
                onRollback={onRollback}
              />
            </div>
          )}
        </div>
      )}

    </div>
  );
}

export default function UndoHistory() {
  const { config } = useSandboxConfig();
  const vmStatus = useVmStore((s) => s.status);
  const { data, loading, error } = useUndoHistoryStore();
  const rollback = useUndoHistoryStore((s) => s.rollback);
  const clearHistory = useUndoHistoryStore((s) => s.clearHistory);
  const fetchHistory = useUndoHistoryStore((s) => s.fetch);
  const addToast = useToastStore((s) => s.addToast);

  const vmRunning = vmStatus.state === "running";
  const undoDir = config.sandbox.undo_dir;

  useUndoHistoryPolling(undoDir, vmRunning);

  const [pendingRollback, setPendingRollback] = useState<number | null>(null);
  const [showClearConfirm, setShowClearConfirm] = useState(false);

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

  const confirmClear = async () => {
    setShowClearConfirm(false);
    try {
      await clearHistory(undoDir, vmRunning);
      addToast("success", "Undo history cleared");
    } catch (e) {
      addToast("error", `Failed to clear history: ${e}`);
    }
  };

  // Check if any barriers exist in the range being rolled back.
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
            Undo history will appear here
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-2xl">
      <div className="mb-6 flex items-center justify-between">
        <h1 className="text-xl font-bold">Undo History</h1>
        {data && data.steps.length > 0 && (
          <button
            onClick={() => setShowClearConfirm(true)}
            className="flex items-center gap-1.5 rounded border border-[var(--color-border)] px-3 py-1.5 text-xs hover:bg-[var(--color-bg-tertiary)]"
          >
            <Trash2 size={12} />
            Clear History
          </button>
        )}
      </div>

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

      {!loading && !error && (!data || (data.steps.length === 0 && data.barriers.length === 0)) && (
        <div className="flex flex-col items-center gap-4 rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-secondary)] py-12 text-center">
          <History size={48} className="text-[var(--color-text-secondary)]" />
          <p className="text-sm text-[var(--color-text-secondary)]">
            Undo history will appear here
          </p>
        </div>
      )}

      {data && (data.steps.length > 0 || data.barriers.length > 0) && (
        <SessionGroupedSteps
          data={data}
          onRollback={handleRollback}
        />
      )}

      {pendingRollback !== null && (
        <RollbackDialog
          count={pendingRollback}
          hasBarriers={hasBarriersInRange(pendingRollback)}
          onConfirm={confirmRollback}
          onCancel={() => setPendingRollback(null)}
        />
      )}

      {showClearConfirm && (
        <ClearHistoryDialog
          onConfirm={confirmClear}
          onCancel={() => setShowClearConfirm(false)}
        />
      )}
    </div>
  );
}
