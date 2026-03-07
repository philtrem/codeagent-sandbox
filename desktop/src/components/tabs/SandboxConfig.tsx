import { useEffect, useState, useCallback } from "react";
import {
  ChevronDown,
  ChevronRight,
  FolderOpen,
  Save,
  AlertCircle,
  Plus,
  X,
  RotateCcw,
} from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useSandboxConfig } from "../../hooks/useSandboxConfig";
import { defaultCommandClassifier } from "../../lib/types";

function Section({
  title,
  defaultOpen = true,
  children,
}: {
  title: string;
  defaultOpen?: boolean;
  children: React.ReactNode;
}) {
  const [isOpen, setIsOpen] = useState(defaultOpen);
  return (
    <div className="mb-4 rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-secondary)]">
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

function DirPicker({
  label,
  value,
  onChange,
  autoCreate,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  autoCreate?: boolean;
}) {
  const [invalid, setInvalid] = useState(false);

  useEffect(() => {
    if (!value) {
      setInvalid(false);
      return;
    }
    invoke<boolean>("validate_directory", { path: value }).then(async (valid) => {
      if (!valid && autoCreate) {
        try {
          await invoke("ensure_directory", { path: value });
          setInvalid(false);
          return;
        } catch {
          // Creation failed — fall through to show error
        }
      }
      setInvalid(!valid);
    });
  }, [value, autoCreate]);

  const pickDir = async () => {
    const selected = await open({ directory: true, multiple: false });
    if (selected) onChange(selected as string);
  };

  return (
    <div>
      <label className="mb-1 block text-xs text-[var(--color-text-secondary)]">
        {label}
      </label>
      <div className="flex gap-2">
        <div className="relative min-w-0 flex-1">
          <input
            type="text"
            value={value}
            onChange={(e) => onChange(e.target.value)}
            className={`w-full rounded border px-3 py-1.5 text-sm bg-[var(--color-bg)] ${
              invalid
                ? "border-[var(--color-error)]"
                : "border-[var(--color-border)]"
            }`}
            placeholder="Select a directory..."
          />
          {invalid && (
            <AlertCircle
              size={14}
              className="absolute top-1/2 right-2 -translate-y-1/2 text-[var(--color-error)]"
            />
          )}
        </div>
        <button
          onClick={pickDir}
          className="flex items-center gap-1 rounded border border-[var(--color-border)] px-3 py-1.5 text-sm hover:bg-[var(--color-bg-tertiary)]"
        >
          <FolderOpen size={14} />
          Browse
        </button>
      </div>
      {invalid && (
        <p className="mt-1 text-xs text-[var(--color-error)]">
          Directory does not exist
        </p>
      )}
    </div>
  );
}

function NumberInput({
  label,
  value,
  onChange,
  min = 0,
  max,
  suffix,
}: {
  label: string;
  value: number;
  onChange: (v: number) => void;
  min?: number;
  max?: number;
  suffix?: string;
}) {
  const outOfRange =
    (min !== undefined && value < min) || (max !== undefined && value > max);

  return (
    <div>
      <label className="mb-1 block text-xs text-[var(--color-text-secondary)]">
        {label}
      </label>
      <div className="flex items-center gap-2">
        <input
          type="number"
          value={value}
          onChange={(e) => onChange(Number(e.target.value))}
          min={min}
          max={max}
          className={`w-32 rounded border px-3 py-1.5 text-sm bg-[var(--color-bg)] ${
            outOfRange
              ? "border-[var(--color-warning)]"
              : "border-[var(--color-border)]"
          }`}
        />
        {suffix && (
          <span className="text-xs text-[var(--color-text-secondary)]">
            {suffix}
          </span>
        )}
        {outOfRange && (
          <span className="text-xs text-[var(--color-warning)]">
            Range: {min ?? "..."}–{max ?? "..."}
          </span>
        )}
      </div>
    </div>
  );
}

function Toggle({
  label,
  checked,
  onChange,
}: {
  label: string;
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <label className="flex items-center gap-3 text-sm">
      <button
        role="switch"
        aria-checked={checked}
        onClick={() => onChange(!checked)}
        className={`relative h-5 w-9 rounded-full transition-colors ${
          checked ? "bg-[var(--color-accent)]" : "bg-[var(--color-bg-tertiary)]"
        }`}
      >
        <span
          className={`absolute top-0.5 left-0.5 h-4 w-4 rounded-full bg-white transition-transform ${
            checked ? "translate-x-4" : ""
          }`}
        />
      </button>
      {label}
    </label>
  );
}

function Select({
  label,
  value,
  onChange,
  options,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  options: { value: string; label: string }[];
}) {
  return (
    <div>
      <label className="mb-1 block text-xs text-[var(--color-text-secondary)]">
        {label}
      </label>
      <select
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="rounded border border-[var(--color-border)] bg-[var(--color-bg)] px-3 py-1.5 text-sm"
      >
        {options.map((opt) => (
          <option key={opt.value} value={opt.value}>
            {opt.label}
          </option>
        ))}
      </select>
    </div>
  );
}

function CommandListEditor({
  label,
  items,
  onChange,
}: {
  label: string;
  items: string[];
  onChange: (items: string[]) => void;
}) {
  const [inputValue, setInputValue] = useState("");

  const addItem = () => {
    const trimmed = inputValue.trim();
    if (trimmed && !items.includes(trimmed)) {
      onChange([...items, trimmed]);
      setInputValue("");
    }
  };

  const removeItem = (index: number) => {
    onChange(items.filter((_, i) => i !== index));
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      addItem();
    }
  };

  return (
    <div>
      <label className="mb-1.5 block text-xs font-medium text-[var(--color-text-secondary)]">
        {label}
      </label>
      <div className="mb-2 flex flex-wrap gap-1.5">
        {items.map((item, index) => (
          <span
            key={`${item}-${index}`}
            className="inline-flex items-center gap-1 rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2 py-0.5 text-xs"
          >
            <code>{item}</code>
            <button
              onClick={() => removeItem(index)}
              className="ml-0.5 text-[var(--color-text-secondary)] hover:text-[var(--color-error)]"
              title={`Remove ${item}`}
            >
              <X size={10} />
            </button>
          </span>
        ))}
        {items.length === 0 && (
          <span className="text-xs italic text-[var(--color-text-secondary)]">
            No commands configured
          </span>
        )}
      </div>
      <div className="flex gap-1.5">
        <input
          type="text"
          value={inputValue}
          onChange={(e) => setInputValue(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder="Add command..."
          className="w-40 rounded border border-[var(--color-border)] bg-[var(--color-bg)] px-2 py-1 text-xs"
        />
        <button
          onClick={addItem}
          disabled={!inputValue.trim()}
          className="flex items-center gap-1 rounded border border-[var(--color-border)] px-2 py-1 text-xs hover:bg-[var(--color-bg-tertiary)] disabled:opacity-40"
        >
          <Plus size={10} /> Add
        </button>
      </div>
    </div>
  );
}

function WorkingDirSection({
  workingDirs,
  undoDir,
  vmMode,
  onWorkingDirsChange,
  onUndoDirChange,
  onVmModeChange,
}: {
  workingDirs: string[];
  undoDir: string;
  vmMode: string;
  onWorkingDirsChange: (dirs: string[]) => void;
  onUndoDirChange: (v: string) => void;
  onVmModeChange: (v: string) => void;
}) {
  const [overlapError, setOverlapError] = useState<string | null>(null);

  const checkOverlap = useCallback(
    async (dirs: string[], undo: string) => {
      const nonEmpty = dirs.filter((d) => d.length > 0);
      if (nonEmpty.length === 0 || !undo) {
        setOverlapError(null);
        return;
      }
      const result = await invoke<string | null>("validate_paths_overlap", {
        workingDirs: nonEmpty,
        undoDir: undo,
      });
      setOverlapError(result);
    },
    []
  );

  useEffect(() => {
    checkOverlap(workingDirs, undoDir);
  }, [workingDirs, undoDir, checkOverlap]);

  const updateDir = (index: number, value: string) => {
    const updated = [...workingDirs];
    updated[index] = value;
    onWorkingDirsChange(updated);
  };

  const addDir = () => {
    onWorkingDirsChange([...workingDirs, ""]);
  };

  const removeDir = (index: number) => {
    if (workingDirs.length <= 1) return;
    const updated = workingDirs.filter((_, i) => i !== index);
    onWorkingDirsChange(updated);
  };

  return (
    <Section title="Working Directory">
      {workingDirs.map((dir, index) => (
        <div key={index} className="flex items-end gap-2">
          <div className="min-w-0 flex-1">
            <DirPicker
              label={
                workingDirs.length === 1
                  ? "Working Directory"
                  : `Working Directory ${index + 1}`
              }
              value={dir}
              onChange={(v) => updateDir(index, v)}
            />
          </div>
          {workingDirs.length > 1 && (
            <button
              onClick={() => removeDir(index)}
              className="mb-0.5 rounded border border-[var(--color-border)] p-1.5 text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-tertiary)] hover:text-[var(--color-error)]"
              title="Remove directory"
            >
              <X size={14} />
            </button>
          )}
        </div>
      ))}
      <button
        onClick={addDir}
        className="flex items-center gap-1 text-xs text-[var(--color-accent)] hover:underline"
      >
        <Plus size={12} /> Add working directory
      </button>

      <DirPicker
        label="Undo Directory"
        value={undoDir}
        onChange={onUndoDirChange}
        autoCreate
      />
      {overlapError && (
        <p className="flex items-center gap-1 text-xs text-[var(--color-error)]">
          <AlertCircle size={12} /> {overlapError}
        </p>
      )}

      <Select
        label="VM Mode"
        value={vmMode}
        onChange={onVmModeChange}
        options={[
          { value: "ephemeral", label: "Ephemeral" },
          { value: "persistent", label: "Persistent" },
        ]}
      />
    </Section>
  );
}

export default function SandboxConfig() {
  const { config, loaded, saving, error, configPath, load, updateSection } =
    useSandboxConfig();

  useEffect(() => {
    load();
  }, [load]);

  if (!loaded) {
    return (
      <div className="text-[var(--color-text-secondary)]">
        Loading configuration...
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-2xl">
      <div className="mb-6 flex items-center justify-between">
        <div>
          <h1 className="text-xl font-bold">Sandbox Configuration</h1>
          {configPath && (
            <p className="mt-1 text-xs text-[var(--color-text-secondary)]">
              {configPath}
            </p>
          )}
        </div>
        <div className="flex items-center gap-2 text-xs">
          {saving && (
            <span className="flex items-center gap-1 text-[var(--color-text-secondary)]">
              <Save size={12} /> Saving...
            </span>
          )}
          {error && (
            <span className="flex items-center gap-1 text-[var(--color-error)]">
              <AlertCircle size={12} /> {error}
            </span>
          )}
        </div>
      </div>

      <WorkingDirSection
        workingDirs={config.sandbox.working_dirs}
        undoDir={config.sandbox.undo_dir}
        vmMode={config.sandbox.vm_mode}
        onWorkingDirsChange={(dirs) =>
          updateSection("sandbox", { working_dirs: dirs })
        }
        onUndoDirChange={(v) => updateSection("sandbox", { undo_dir: v })}
        onVmModeChange={(v) => updateSection("sandbox", { vm_mode: v })}
      />

      <Section title="Resource Limits">
        <Toggle
          label="Enable Resource Limits"
          checked={config.undo.enabled}
          onChange={(v) => updateSection("undo", { enabled: v })}
        />
        {config.undo.enabled && (
          <>
            <NumberInput
              label="Max Undo Log Size (0 = unlimited)"
              value={config.undo.max_log_size_mb}
              onChange={(v) => updateSection("undo", { max_log_size_mb: v })}
              suffix="MB"
            />
            <NumberInput
              label="Max Step Count (0 = unlimited)"
              value={config.undo.max_step_count}
              onChange={(v) => updateSection("undo", { max_step_count: v })}
            />
            <NumberInput
              label="Max Single Step Size (0 = unlimited)"
              value={config.undo.max_single_step_size_mb}
              onChange={(v) =>
                updateSection("undo", { max_single_step_size_mb: v })
              }
              suffix="MB"
            />
          </>
        )}
      </Section>

      <Section title="Safeguards">
        <Toggle
          label="Enable Safeguards"
          checked={config.safeguards.enabled}
          onChange={(v) =>
            updateSection("safeguards", { enabled: v })
          }
        />
        {config.safeguards.enabled && (
          <>
            <NumberInput
              label="Delete Threshold (0 = disabled)"
              value={config.safeguards.delete_threshold}
              onChange={(v) =>
                updateSection("safeguards", { delete_threshold: v })
              }
              suffix="files"
            />
            <NumberInput
              label="Overwrite File Size Threshold (0 = disabled)"
              value={config.safeguards.overwrite_file_size_kb}
              onChange={(v) =>
                updateSection("safeguards", { overwrite_file_size_kb: v })
              }
              suffix="KB"
            />
            <Toggle
              label="Rename Over Existing"
              checked={config.safeguards.rename_over_existing}
              onChange={(v) =>
                updateSection("safeguards", { rename_over_existing: v })
              }
            />
            <NumberInput
              label="Safeguard Timeout"
              value={config.safeguards.timeout_seconds}
              onChange={(v) =>
                updateSection("safeguards", { timeout_seconds: v })
              }
              suffix="seconds"
            />
          </>
        )}
      </Section>

      <Section title="Advanced" defaultOpen={false}>
        <Select
          label="Symlink Policy"
          value={config.symlinks.policy}
          onChange={(v) => updateSection("symlinks", { policy: v })}
          options={[
            { value: "ignore", label: "Ignore" },
            { value: "read_only", label: "Read Only" },
            { value: "read_write", label: "Read Write" },
          ]}
        />
        <Select
          label="External Modification Policy"
          value={config.external_modifications.policy}
          onChange={(v) =>
            updateSection("external_modifications", { policy: v })
          }
          options={[
            { value: "barrier", label: "Barrier" },
            { value: "warn", label: "Warn" },
          ]}
        />
        <Toggle
          label="Gitignore Filtering"
          checked={config.gitignore.enabled}
          onChange={(v) => updateSection("gitignore", { enabled: v })}
        />
        <Select
          label="Log Level"
          value={config.sandbox.log_level}
          onChange={(v) => updateSection("sandbox", { log_level: v })}
          options={[
            { value: "error", label: "Error" },
            { value: "warn", label: "Warn" },
            { value: "info", label: "Info" },
            { value: "debug", label: "Debug" },
            { value: "trace", label: "Trace" },
          ]}
        />
      </Section>

      <Section title="Command Classification" defaultOpen={false}>
        <p className="text-xs text-[var(--color-text-secondary)]">
          Configure which shell commands are classified as read-only, write, or
          destructive. Sanitization rules (fork bombs, sudo, raw device access)
          are hardcoded and cannot be changed here.
        </p>
        <div className="flex justify-end">
          <button
            onClick={() =>
              updateSection("command_classifier", defaultCommandClassifier())
            }
            className="flex items-center gap-1 rounded border border-[var(--color-border)] px-2 py-1 text-xs hover:bg-[var(--color-bg-tertiary)]"
          >
            <RotateCcw size={10} /> Reset to Defaults
          </button>
        </div>

        <CommandListEditor
          label="Read-Only Commands"
          items={config.command_classifier.read_only_commands}
          onChange={(items) =>
            updateSection("command_classifier", { read_only_commands: items })
          }
        />
        <CommandListEditor
          label="Write Commands"
          items={config.command_classifier.write_commands}
          onChange={(items) =>
            updateSection("command_classifier", { write_commands: items })
          }
        />
        <CommandListEditor
          label="Destructive Commands"
          items={config.command_classifier.destructive_commands}
          onChange={(items) =>
            updateSection("command_classifier", {
              destructive_commands: items,
            })
          }
        />

        <Section title="Git Subcommands" defaultOpen={false}>
          <CommandListEditor
            label="Read-Only"
            items={config.command_classifier.git_read_only_subcommands}
            onChange={(items) =>
              updateSection("command_classifier", {
                git_read_only_subcommands: items,
              })
            }
          />
          <CommandListEditor
            label="Destructive"
            items={config.command_classifier.git_destructive_subcommands}
            onChange={(items) =>
              updateSection("command_classifier", {
                git_destructive_subcommands: items,
              })
            }
          />
        </Section>

        <Section title="Cargo Subcommands" defaultOpen={false}>
          <CommandListEditor
            label="Read-Only"
            items={config.command_classifier.cargo_read_only_subcommands}
            onChange={(items) =>
              updateSection("command_classifier", {
                cargo_read_only_subcommands: items,
              })
            }
          />
          <CommandListEditor
            label="Destructive"
            items={config.command_classifier.cargo_destructive_subcommands}
            onChange={(items) =>
              updateSection("command_classifier", {
                cargo_destructive_subcommands: items,
              })
            }
          />
        </Section>

        <Section title="NPM Subcommands & Scripts" defaultOpen={false}>
          <CommandListEditor
            label="Read-Only Subcommands"
            items={config.command_classifier.npm_read_only_subcommands}
            onChange={(items) =>
              updateSection("command_classifier", {
                npm_read_only_subcommands: items,
              })
            }
          />
          <CommandListEditor
            label="Read-Only Scripts (for npm run)"
            items={config.command_classifier.npm_read_only_scripts}
            onChange={(items) =>
              updateSection("command_classifier", {
                npm_read_only_scripts: items,
              })
            }
          />
        </Section>
      </Section>
    </div>
  );
}
