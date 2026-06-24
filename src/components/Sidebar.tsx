import {
  Cloud,
  Database,
  Gauge,
  KeyRound,
  Moon,
  ReceiptText,
  Settings,
  SunMedium,
} from "lucide-react";
import appIcon from "../assets/cedar-app-icon.png";
import type { InventorySummary } from "../types";

export type NavSection = "overview" | "resources" | "workers" | "billing" | "settings";

const items = [
  { id: "overview", label: "Audit", icon: Gauge },
  { id: "resources", label: "Resources", icon: Database },
  { id: "workers", label: "Workers", icon: Cloud },
  { id: "billing", label: "Cost", icon: ReceiptText },
  { id: "settings", label: "Connection", icon: Settings },
] as const;

type SidebarProps = {
  activeSection: NavSection;
  connected: boolean;
  accountName?: string;
  inventory?: InventorySummary;
  theme: "light" | "dark";
  onSectionChange: (section: NavSection) => void;
  onToggleTheme: () => void;
};

export function Sidebar({
  activeSection,
  connected,
  accountName,
  inventory,
  theme,
  onSectionChange,
  onToggleTheme,
}: SidebarProps) {
  const getCount = (id: NavSection) => {
    if (!inventory) return undefined;
    if (id === "overview") return inventory.workers + inventory.pages + inventory.d1 + inventory.r2 + inventory.kv;
    if (id === "resources") return inventory.workers + inventory.pages + inventory.d1 + inventory.r2 + inventory.kv;
    if (id === "workers") return inventory.workers;
    return undefined;
  };
  const ThemeIcon = theme === "light" ? SunMedium : Moon;
  const showNav = connected;

  return (
    <aside className="sidebar" aria-label="Primary navigation">
      <div className="brand">
        <div className="brand-mark">
          <img src={appIcon} alt="" aria-hidden="true" />
        </div>
        <div className="brand-copy">
          <strong>Cedar</strong>
        </div>
      </div>

      {showNav && (
        <nav className="nav-list">
          <span className="nav-label">Audit</span>
          {items.map((item) => {
            const count = getCount(item.id);
            return (
              <button
                aria-label={count != null ? `${item.label}, ${count}` : item.label}
                className={`nav-item ${activeSection === item.id ? "is-active" : ""}`}
                key={item.id}
                onClick={() => onSectionChange(item.id)}
                type="button"
              >
                <item.icon size={17} />
                <span>{item.label}</span>
                {count != null && <em>{count}</em>}
              </button>
            );
          })}
        </nav>
      )}

      <div className={`connection-card ${connected ? "connected" : "disconnected"}`}>
        <div className="connection-icon">
          <KeyRound size={18} />
        </div>
        <div>
          <strong>{connected ? accountName ?? "Connected" : "Not connected"}</strong>
          <span>{connected ? "Local keychain" : "Setup required"}</span>
        </div>
      </div>

      <div className="secret-note">
        <button
          aria-label={theme === "light" ? "Switch to dark mode" : "Switch to light mode"}
          aria-pressed={theme === "dark"}
          className="theme-toggle"
          onClick={onToggleTheme}
          type="button"
        >
          <ThemeIcon size={14} />
          {theme === "light" ? "Light" : "Dark"}
        </button>
        <span>v0.1.0</span>
      </div>
    </aside>
  );
}
