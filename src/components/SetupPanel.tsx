import { useState } from "react";
import { Cloud, ExternalLink, Eye, EyeOff, KeyRound, LockKeyhole, Search, ShieldCheck } from "lucide-react";
import type { CloudflareTokenMode } from "../api";
import type { Account } from "../types";

type SetupPanelProps = {
  visible: boolean;
  loading: boolean;
  accounts: Account[];
  error?: string;
  onCreateToken: (mode: CloudflareTokenMode) => Promise<void>;
  onDiscover: (token: string) => Promise<void>;
  onConnect: (token: string, accountId?: string) => Promise<void>;
  onClear: () => Promise<void>;
};

export function SetupPanel({
  visible,
  loading,
  accounts,
  error,
  onCreateToken,
  onDiscover,
  onConnect,
  onClear,
}: SetupPanelProps) {
  const [token, setToken] = useState("");
  const [selectedAccount, setSelectedAccount] = useState("");
  const [tokenVisible, setTokenVisible] = useState(false);
  const [tokenMode, setTokenMode] = useState<CloudflareTokenMode>("full-observability");
  const fullObservability = tokenMode === "full-observability";

  if (!visible) return null;

  return (
    <section className="setup-panel">
      <div className="setup-copy">
        <div className="setup-icon">
          <Cloud size={19} />
        </div>
        <div>
          <span className="setup-kicker">Setup</span>
          <h2>Run a local audit</h2>
        </div>
      </div>

      <div className="setup-form">
        <div className="setup-form-header">
          <div>
            <strong>Cloudflare audit connection</strong>
          </div>
          <ShieldCheck size={18} />
        </div>

        <div className="connection-spec">
          <span>
            <LockKeyhole size={14} />
            OS keychain
          </span>
          <span>Account + all zones</span>
          <span>Local audit snapshots</span>
        </div>

        <div className="scope-modes" aria-label="Cloudflare token mode">
          <button
            className={tokenMode === "read-only" ? "selected" : ""}
            type="button"
            aria-pressed={tokenMode === "read-only"}
            onClick={() => setTokenMode("read-only")}
          >
            <strong>Minimal read-only</strong>
            <span>Inventory, zones, analytics, audit logs, D1, R2, and KV without edit scopes.</span>
          </button>
          <button
            className={fullObservability ? "selected" : ""}
            type="button"
            aria-pressed={fullObservability}
            onClick={() => setTokenMode("full-observability")}
          >
            <strong>Full observability</strong>
            <span>Add Logs Edit and Workers Observability Edit for Logpush jobs and Worker telemetry.</span>
          </button>
        </div>

        <details className="scope-details">
          <summary>Required scopes</summary>
          <div className="scope-checklist">
            {fullObservability ? (
              <>
                <span>Account row: Account Analytics Read, Logs Edit, Workers Observability Edit</span>
                <span>All zones row: Zone Read, Zone Analytics Read, Logs Edit</span>
              </>
            ) : (
              <>
                <span>Account row: Account Settings Read, Account Analytics Read, Audit Logs Read</span>
                <span>All zones row: Zone Read, Zone Analytics Read</span>
              </>
            )}
          </div>
        </details>

        <label>
          <span>Cloudflare API token</span>
          <div className="input-shell">
            <input
              type={tokenVisible ? "text" : "password"}
              value={token}
              onChange={(event) => setToken(event.target.value)}
              placeholder="Enter API token"
              autoComplete="off"
            />
            <button
              className="input-action"
              type="button"
              aria-label={tokenVisible ? "Hide API token" : "Show API token"}
              disabled={!token}
              onClick={() => setTokenVisible((visible) => !visible)}
            >
              {tokenVisible ? <EyeOff size={15} /> : <Eye size={15} />}
            </button>
          </div>
        </label>

        <div className="setup-actions">
          <button
            className="secondary-button"
            disabled={loading}
            onClick={() => void onCreateToken(tokenMode)}
            title={fullObservability ? "Open Cloudflare with inventory, analytics, Logpush, and Workers Observability scopes prefilled" : "Open Cloudflare with read-only inventory and analytics scopes prefilled"}
            type="button"
          >
            <ExternalLink size={15} />
            <span>{fullObservability ? "Create full token" : "Create read-only token"}</span>
          </button>
          <button
            className="primary-button"
            disabled={!token || loading}
            onClick={() => onConnect(token, selectedAccount || undefined)}
            type="button"
          >
            <KeyRound size={15} />
            <span>Connect Cloudflare</span>
          </button>
          <button className="secondary-button" disabled={!token || loading} onClick={() => onDiscover(token)} type="button">
            <Search size={15} />
            <span>Discover accounts</span>
          </button>
          <button className="text-button" disabled={loading} onClick={onClear} type="button">
            Clear
          </button>
        </div>

        {accounts.length > 0 && (
          <label>
            <span>Account</span>
            <select value={selectedAccount} onChange={(event) => setSelectedAccount(event.target.value)}>
              <option value="">Use first account</option>
              {accounts.map((account) => (
                <option value={account.id} key={account.id}>
                  {account.name}
                </option>
              ))}
            </select>
          </label>
        )}

        {error && (
          <div className="setup-error" role="alert">
            {error}
          </div>
        )}
      </div>
    </section>
  );
}
