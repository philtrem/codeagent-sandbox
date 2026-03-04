import { Settings, Monitor, Plug, History, SquareTerminal } from "lucide-react";
import { useLayoutStore, type TabId } from "../hooks/useLayoutStore";
import SandboxConfig from "./tabs/SandboxConfig";
import VmManager from "./tabs/VmManager";
import ClaudeIntegration from "./tabs/ClaudeIntegration";
import UndoHistory from "./tabs/UndoHistory";
import Terminal from "./tabs/Terminal";

const tabs: { id: TabId; label: string; icon: typeof Settings }[] = [
  { id: "config", label: "Configuration", icon: Settings },
  { id: "vm", label: "VM Manager", icon: Monitor },
  { id: "claude", label: "Claude Integration", icon: Plug },
  { id: "history", label: "Undo History", icon: History },
  { id: "terminal", label: "Terminal", icon: SquareTerminal },
];

export default function Layout() {
  const activeTab = useLayoutStore((s) => s.activeTab);
  const setActiveTab = useLayoutStore((s) => s.setActiveTab);

  return (
    <div className="flex h-full">
      {/* Sidebar */}
      <nav className="flex w-14 flex-col items-center gap-1 border-r border-[var(--color-border)] bg-[var(--color-bg-secondary)] pt-4">
        {tabs.map((tab) => {
          const Icon = tab.icon;
          const active = activeTab === tab.id;
          return (
            <button
              key={tab.id}
              onClick={() => setActiveTab(tab.id)}
              title={tab.label}
              className={`flex h-10 w-10 items-center justify-center rounded-lg transition-colors ${
                active
                  ? "bg-[var(--color-accent)] text-white"
                  : "text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-tertiary)] hover:text-[var(--color-text)]"
              }`}
            >
              <Icon size={20} />
            </button>
          );
        })}
      </nav>

      {/* Content */}
      <main className={`flex-1 ${activeTab === "terminal" ? "flex flex-col overflow-hidden" : "overflow-y-auto p-6"}`}>
        {activeTab === "config" && <SandboxConfig />}
        {activeTab === "vm" && <VmManager />}
        {activeTab === "claude" && <ClaudeIntegration />}
        {activeTab === "history" && <UndoHistory />}
        {activeTab === "terminal" && <Terminal />}
      </main>
    </div>
  );
}
