import { useEffect, useState } from "react";
import {
  Package,
  HardDrive,
  Trash2,
  AlertCircle,
  Loader2,
  Plus,
  X,
  Check,
} from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { useToolsStore } from "../../hooks/useToolsStore";
import { useSandboxConfig } from "../../hooks/useSandboxConfig";
import { TOOL_CATEGORIES } from "../../lib/types";

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}

function formatDate(iso: string): string {
  if (!iso) return "";
  try {
    return new Date(iso).toLocaleString();
  } catch {
    return iso;
  }
}

export default function GuestToolsSection() {
  const { config, updateSection } = useSandboxConfig();
  const {
    imageStatus,
    building,
    buildStage,
    buildError,
    dockerAvailable,
    checkStatus,
    buildImage,
    deleteImage,
    checkDocker,
  } = useToolsStore();

  const [additionalInput, setAdditionalInput] = useState("");
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);

  const selectedPackages = config.tools.selected_packages;
  const additionalPackages = config.tools.additional_packages;
  const imagePath = config.tools.image_path;

  useEffect(() => {
    checkDocker();
  }, [checkDocker]);

  useEffect(() => {
    if (imagePath) {
      checkStatus(imagePath);
    } else {
      // Get default path and check it
      invoke<string>("get_default_tools_image_path").then((defaultPath) => {
        checkStatus(defaultPath);
      });
    }
  }, [imagePath, checkStatus]);

  const togglePackage = (pkg: string) => {
    const updated = selectedPackages.includes(pkg)
      ? selectedPackages.filter((p) => p !== pkg)
      : [...selectedPackages, pkg];
    updateSection("tools", { selected_packages: updated });
  };

  const addAdditionalPackage = () => {
    const trimmed = additionalInput.trim();
    if (trimmed && !additionalPackages.includes(trimmed)) {
      updateSection("tools", {
        additional_packages: [...additionalPackages, trimmed],
      });
      setAdditionalInput("");
    }
  };

  const removeAdditionalPackage = (index: number) => {
    updateSection("tools", {
      additional_packages: additionalPackages.filter((_, i) => i !== index),
    });
  };

  const handleBuild = async () => {
    const allPackages = [...selectedPackages, ...additionalPackages];
    if (allPackages.length === 0) return;

    const result = await buildImage(allPackages);
    if (result) {
      updateSection("tools", { image_path: result });
      checkStatus(result);
    }
  };

  const handleDelete = async () => {
    const pathToDelete = imagePath || (await invoke<string>("get_default_tools_image_path"));
    await deleteImage(pathToDelete);
    setShowDeleteConfirm(false);
  };

  const allPackages = [...selectedPackages, ...additionalPackages];

  return (
    <div className="space-y-4">
      {/* Status Card */}
      <div className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg)] p-3">
        <div className="flex items-center gap-2 text-sm font-medium">
          <HardDrive size={14} />
          Tools Image Status
        </div>
        {imageStatus?.exists ? (
          <div className="mt-2 space-y-1 text-xs text-[var(--color-text-secondary)]">
            <div>
              Path: <code className="text-[var(--color-text)]">{imagePath || "default"}</code>
            </div>
            <div>Size: {formatBytes(imageStatus.size_bytes)}</div>
            {imageStatus.created_at && (
              <div>Built: {formatDate(imageStatus.created_at)}</div>
            )}
          </div>
        ) : (
          <p className="mt-2 text-xs text-[var(--color-text-secondary)]">
            No tools image built. Select packages below and click Build.
          </p>
        )}
      </div>

      {/* Curated Tools */}
      <div>
        <label className="mb-2 flex items-center gap-1.5 text-xs font-medium text-[var(--color-text-secondary)]">
          <Package size={12} />
          Curated Tools
        </label>
        <div className="space-y-3">
          {Object.entries(TOOL_CATEGORIES).map(([category, packages]) => (
            <div key={category}>
              <div className="mb-1 text-xs font-medium text-[var(--color-text-secondary)]">
                {category}
              </div>
              <div className="flex flex-wrap gap-1.5">
                {packages.map((pkg) => {
                  const isSelected = selectedPackages.includes(pkg);
                  return (
                    <button
                      key={pkg}
                      onClick={() => togglePackage(pkg)}
                      className={`inline-flex items-center gap-1 rounded-md border px-2 py-0.5 text-xs transition-colors ${
                        isSelected
                          ? "border-[var(--color-accent)] bg-[var(--color-accent)] bg-opacity-10 text-[var(--color-accent)]"
                          : "border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-[var(--color-accent)]"
                      }`}
                    >
                      {isSelected && <Check size={10} />}
                      {pkg}
                    </button>
                  );
                })}
              </div>
            </div>
          ))}
        </div>
      </div>

      {/* Additional Packages */}
      <div>
        <label className="mb-1.5 block text-xs font-medium text-[var(--color-text-secondary)]">
          Additional Packages (Alpine package names)
        </label>
        <div className="mb-2 flex flex-wrap gap-1.5">
          {additionalPackages.map((pkg, index) => (
            <span
              key={`${pkg}-${index}`}
              className="inline-flex items-center gap-1 rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2 py-0.5 text-xs"
            >
              <code>{pkg}</code>
              <button
                onClick={() => removeAdditionalPackage(index)}
                className="ml-0.5 text-[var(--color-text-secondary)] hover:text-[var(--color-error)]"
              >
                <X size={10} />
              </button>
            </span>
          ))}
          {additionalPackages.length === 0 && (
            <span className="text-xs italic text-[var(--color-text-secondary)]">
              No additional packages
            </span>
          )}
        </div>
        <div className="flex gap-1.5">
          <input
            type="text"
            value={additionalInput}
            onChange={(e) => setAdditionalInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                addAdditionalPackage();
              }
            }}
            placeholder="e.g. openssh-client"
            className="w-48 rounded border border-[var(--color-border)] bg-[var(--color-bg)] px-2 py-1 text-xs"
          />
          <button
            onClick={addAdditionalPackage}
            disabled={!additionalInput.trim()}
            className="flex items-center gap-1 rounded border border-[var(--color-border)] px-2 py-1 text-xs hover:bg-[var(--color-bg-tertiary)] disabled:opacity-40"
          >
            <Plus size={10} /> Add
          </button>
        </div>
      </div>

      {/* Build / Delete Actions */}
      <div className="flex items-center gap-2">
        <button
          onClick={handleBuild}
          disabled={building || allPackages.length === 0 || dockerAvailable === false}
          className="flex items-center gap-1.5 rounded bg-[var(--color-accent)] px-3 py-1.5 text-sm text-white hover:opacity-90 disabled:opacity-40"
        >
          {building ? (
            <>
              <Loader2 size={14} className="animate-spin" />
              {buildStage === "pulling"
                ? "Pulling..."
                : buildStage === "installing"
                  ? "Installing..."
                  : buildStage === "creating"
                    ? "Creating image..."
                    : "Building..."}
            </>
          ) : (
            <>
              <Package size={14} />
              Build Tools Image
            </>
          )}
        </button>

        {imageStatus?.exists && !building && (
          <>
            {showDeleteConfirm ? (
              <div className="flex items-center gap-1.5 text-xs">
                <span className="text-[var(--color-text-secondary)]">
                  Delete image?
                </span>
                <button
                  onClick={handleDelete}
                  className="rounded border border-[var(--color-error)] px-2 py-0.5 text-[var(--color-error)] hover:bg-[var(--color-error)] hover:text-white"
                >
                  Yes
                </button>
                <button
                  onClick={() => setShowDeleteConfirm(false)}
                  className="rounded border border-[var(--color-border)] px-2 py-0.5 hover:bg-[var(--color-bg-tertiary)]"
                >
                  No
                </button>
              </div>
            ) : (
              <button
                onClick={() => setShowDeleteConfirm(true)}
                className="flex items-center gap-1 rounded border border-[var(--color-border)] px-2 py-1.5 text-xs text-[var(--color-text-secondary)] hover:border-[var(--color-error)] hover:text-[var(--color-error)]"
              >
                <Trash2 size={12} /> Delete
              </button>
            )}
          </>
        )}
      </div>

      {/* Error display */}
      {buildError && (
        <div className="flex items-start gap-1.5 rounded border border-[var(--color-error)] bg-[var(--color-error)] bg-opacity-5 p-2 text-xs text-[var(--color-error)]">
          <AlertCircle size={12} className="mt-0.5 shrink-0" />
          <span>{buildError}</span>
        </div>
      )}

      {/* Docker note */}
      {dockerAvailable === false && (
        <p className="text-xs text-[var(--color-warning)]">
          Docker is required to build tools images. Install Docker from{" "}
          <a
            href="https://docs.docker.com/get-docker/"
            className="underline"
            target="_blank"
            rel="noreferrer"
          >
            docs.docker.com
          </a>
        </p>
      )}
      {dockerAvailable !== false && (
        <p className="text-xs text-[var(--color-text-secondary)]">
          Requires Docker. Packages are installed from Alpine Linux repositories.
        </p>
      )}
    </div>
  );
}
