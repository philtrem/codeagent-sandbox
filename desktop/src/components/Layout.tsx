import { useState } from "react";
import { Settings, Monitor, Plug } from "lucide-react";
import SandboxConfig from "./tabs/SandboxConfig";
import VmManager from "./tabs/VmManager";
import ClaudeIntegration from "./tabs/ClaudeIntegration";

const tabs = [
  { id: "config", label: "Configuration", icon: Settings },
  { id: "vm", label: "VM Manager", icon: Monitor },
  { id: "claude", label: "Claude Integration", icon: Plug },
] as const;

type TabId = (typeof tabs)[number]["id"];

export default function Layout() {
  const [activeTab, setActiveTab] = useState<TabId>("config");

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
      <main className="flex-1 overflow-y-auto p-6">
        {activeTab === "config" && <SandboxConfig />}
        {activeTab === "vm" && <VmManager />}
        {activeTab === "claude" && <ClaudeIntegration />}
      </main>
    </div>
  );
}
