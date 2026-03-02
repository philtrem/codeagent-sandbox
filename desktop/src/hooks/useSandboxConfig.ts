import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { type SandboxConfig, defaultConfig } from "../lib/types";

interface ConfigState {
  config: SandboxConfig;
  loaded: boolean;
  saving: boolean;
  error: string | null;
  configPath: string | null;
  load: () => Promise<void>;
  update: (patch: Partial<SandboxConfig>) => void;
  updateSection: <K extends keyof SandboxConfig>(
    section: K,
    patch: Partial<SandboxConfig[K]>,
  ) => void;
}

let saveTimeout: ReturnType<typeof setTimeout> | null = null;

async function saveConfig(config: SandboxConfig) {
  await invoke("write_config", { config });
}

export const useSandboxConfig = create<ConfigState>((set, get) => ({
  config: defaultConfig(),
  loaded: false,
  saving: false,
  error: null,
  configPath: null,

  load: async () => {
    try {
      const [config, configPath] = await Promise.all([
        invoke<SandboxConfig>("read_config"),
        invoke<string>("get_config_path"),
      ]);
      set({ config, loaded: true, error: null, configPath });
    } catch (e) {
      set({ error: String(e), loaded: true });
    }
  },

  update: (patch) => {
    const newConfig = { ...get().config, ...patch };
    set({ config: newConfig });
    debouncedSave(newConfig, set);
  },

  updateSection: (section, patch) => {
    const current = get().config;
    const newConfig = {
      ...current,
      [section]: { ...current[section], ...patch },
    };
    set({ config: newConfig });
    debouncedSave(newConfig, set);
  },
}));

function debouncedSave(
  config: SandboxConfig,
  set: (state: Partial<ConfigState>) => void,
) {
  if (saveTimeout) clearTimeout(saveTimeout);
  saveTimeout = setTimeout(async () => {
    set({ saving: true });
    try {
      await saveConfig(config);
      set({ saving: false, error: null });
    } catch (e) {
      set({ saving: false, error: String(e) });
    }
  }, 500);
}
