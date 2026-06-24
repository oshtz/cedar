import { Clock3, Database, RefreshCw, ShieldCheck } from "lucide-react";
import type { Account, RangeKey } from "../types";

type TopbarProps = {
  account?: Account;
  range: RangeKey;
  lastSync?: string;
  syncing: boolean;
  live: boolean;
  cached: boolean;
  expiresAt?: string;
  canSync: boolean;
  onRangeChange: (range: RangeKey) => void;
  onRefresh: () => void;
};

const ranges: RangeKey[] = ["24h", "7d", "30d"];

export function Topbar({
  account,
  range,
  lastSync,
  syncing,
  live,
  cached,
  expiresAt,
  canSync,
  onRangeChange,
  onRefresh,
}: TopbarProps) {
  const cacheExpired = Boolean(expiresAt && Date.now() > new Date(expiresAt).getTime());
  const syncLabel = !canSync
    ? "Setup required"
    : !live
      ? "Waiting for sync"
      : cached
        ? cacheExpired
          ? "Cached snapshot"
          : "Fresh cache"
        : "Live Cloudflare data";
  const SyncIcon = cached ? Database : ShieldCheck;
  const lastSyncLabel = lastSync ? new Date(lastSync).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) : "Never";

  return (
    <header className="topbar">
      <div className="topbar-main">
        <div className="command-title">
          <span>Audit target</span>
          <strong>{account?.name ?? "Local Cloudflare account"}</strong>
        </div>

        <div className={`sync-state ${cached ? "cached" : live ? "live" : "offline"}`}>
          <SyncIcon size={16} />
          <span>{syncLabel}</span>
        </div>
      </div>

      {canSync && (
        <div className="topbar-actions">
          <div className="last-sync">
            <Clock3 size={16} />
            <div>
              <span className="toolbar-label">Updated</span>
              <strong>{lastSyncLabel}</strong>
            </div>
          </div>

          <div className="segmented" aria-label="Time range">
            {ranges.map((item) => (
              <button
                aria-pressed={range === item}
                className={range === item ? "selected" : ""}
                disabled={syncing}
                key={item}
                onClick={() => onRangeChange(item)}
                type="button"
              >
                {item}
              </button>
            ))}
          </div>

          <button className="refresh-button" onClick={onRefresh} disabled={syncing} type="button">
            <RefreshCw size={16} className={syncing ? "spin" : ""} />
            <span>Run audit</span>
          </button>
        </div>
      )}
    </header>
  );
}
