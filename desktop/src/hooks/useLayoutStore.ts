import { create } from "zustand";

export type TabId = "config" | "vm" | "claude" | "history" | "terminal";

interface LayoutState {
  activeTab: TabId;
  setActiveTab: (tab: TabId) => void;
}

export const useLayoutStore = create<LayoutState>((set) => ({
  activeTab: "config",
  setActiveTab: (tab) => set({ activeTab: tab }),
}));
