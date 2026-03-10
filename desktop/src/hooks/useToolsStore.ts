import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { type ToolsImageStatus, type ToolsBuildProgress } from "../lib/types";

interface ToolsState {
  imageStatus: ToolsImageStatus | null;
  building: boolean;
  buildStage: string;
  buildError: string | null;
  dockerAvailable: boolean | null;

  checkStatus: (path: string) => Promise<void>;
  buildImage: (packages: string[]) => Promise<string | null>;
  deleteImage: (path: string) => Promise<void>;
  checkDocker: () => Promise<void>;
}

export const useToolsStore = create<ToolsState>((set) => ({
  imageStatus: null,
  building: false,
  buildStage: "",
  buildError: null,
  dockerAvailable: null,

  checkStatus: async (path: string) => {
    try {
      const status = await invoke<ToolsImageStatus>("get_tools_image_status", {
        imagePath: path,
      });
      set({ imageStatus: status });
    } catch {
      set({ imageStatus: null });
    }
  },

  buildImage: async (packages: string[]) => {
    set({ building: true, buildStage: "starting", buildError: null });

    let unlisten: UnlistenFn | null = null;
    try {
      unlisten = await listen<ToolsBuildProgress>(
        "tools-build-progress",
        (event) => {
          set({
            buildStage: event.payload.stage,
          });
        }
      );

      const imagePath = await invoke<string>("build_tools_image", {
        packages,
      });
      set({ building: false, buildStage: "done" });
      return imagePath;
    } catch (e) {
      set({
        building: false,
        buildStage: "",
        buildError: String(e),
      });
      return null;
    } finally {
      if (unlisten) unlisten();
    }
  },

  deleteImage: async (path: string) => {
    try {
      await invoke("delete_tools_image", { imagePath: path });
      set({
        imageStatus: {
          exists: false,
          size_bytes: 0,
          created_at: "",
          packages: [],
        },
      });
    } catch (e) {
      set({ buildError: String(e) });
    }
  },

  checkDocker: async () => {
    try {
      const available = await invoke<boolean>("check_docker_available");
      set({ dockerAvailable: available });
    } catch {
      set({ dockerAvailable: false });
    }
  },
}));
