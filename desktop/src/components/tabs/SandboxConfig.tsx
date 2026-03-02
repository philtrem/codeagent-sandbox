import { useEffect, useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  FolderOpen,
  Save,
  AlertCircle,
} from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useSandboxConfig } from "../../hooks/useSandboxConfig";

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
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
}) {
  const [invalid, setInvalid] = useState(false);

  useEffect(() => {
    if (!value) {
      setInvalid(false);
      return;
    }
    invoke<boolean>("validate_directory", { path: value }).then((valid) => {
      setInvalid(!valid);
    });
  }, [value]);

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

      <Section title="Working Directory">
        <DirPicker
          label="Working Directory"
          value={config.sandbox.working_dir}
          onChange={(v) => updateSection("sandbox", { working_dir: v })}
        />
        <DirPicker
          label="Undo Directory"
          value={config.sandbox.undo_dir}
          onChange={(v) => updateSection("sandbox", { undo_dir: v })}
        />
        <Select
          label="VM Mode"
          value={config.sandbox.vm_mode}
          onChange={(v) => updateSection("sandbox", { vm_mode: v })}
          options={[
            { value: "ephemeral", label: "Ephemeral" },
            { value: "persistent", label: "Persistent" },
          ]}
        />
      </Section>

      <Section title="Resource Limits">
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
      </Section>

      <Section title="Safeguards">
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
    </div>
  );
}
