import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Account, ConnectResult, ConnectionState, DashboardSnapshot, RangeKey } from "./types";

export type CloudflareTokenMode = "read-only" | "full-observability";

const cloudflareReadOnlyTokenPermissions = [
  { key: "account_settings", type: "read" },
  { key: "account_analytics", type: "read" },
  { key: "workers_scripts", type: "read" },
  { key: "page", type: "read" },
  { key: "zone", type: "read" },
  { key: "analytics", type: "read" },
  { key: "d1", type: "read" },
  { key: "audit_logs", type: "read" },
  { key: "workers_r2", type: "read" },
  { key: "workers_kv_storage", type: "read" },
] satisfies Array<{ key: string; type: "read" | "edit" }>;

const cloudflareFullObservabilityPermissions = [
  ...cloudflareReadOnlyTokenPermissions,
  { key: "logs", type: "edit" },
  { key: "workers_observability", type: "edit" },
] satisfies Array<{ key: string; type: "read" | "edit" }>;

const browserOnlyConnection: ConnectionState = {
  configured: false,
  tokenPresent: false,
  storage: "none",
};

export function isDesktopRuntime() {
  return "__TAURI_INTERNALS__" in window;
}

export function getCloudflareTokenTemplateUrl(accountId?: string, mode: CloudflareTokenMode = "full-observability") {
  const permissions = mode === "read-only" ? cloudflareReadOnlyTokenPermissions : cloudflareFullObservabilityPermissions;
  const permissionGroupKeys = JSON.stringify(permissions);
  const tokenName = mode === "read-only" ? "Cedar read-only collector" : "Cedar full observability collector";

  if (accountId) {
    const params = new URLSearchParams({
      to: "/:account/api-tokens",
      permissionGroupKeys,
      name: tokenName,
    });

    return `https://dash.cloudflare.com/?${params.toString()}`;
  }

  const params = new URLSearchParams({
    permissionGroupKeys,
    accountId: "*",
    zoneId: "all",
    name: tokenName,
  });

  return `https://dash.cloudflare.com/profile/api-tokens?${params.toString()}`;
}

export async function openCloudflareTokenTemplate(accountId?: string, mode: CloudflareTokenMode = "full-observability"): Promise<void> {
  const tokenTemplateUrl = getCloudflareTokenTemplateUrl(accountId, mode);

  if (isDesktopRuntime()) {
    await openUrl(tokenTemplateUrl);
    return;
  }

  const opened = window.open(tokenTemplateUrl, "_blank", "noopener,noreferrer");
  if (!opened) {
    window.location.assign(tokenTemplateUrl);
  }
}

export async function getConnection(): Promise<ConnectionState> {
  if (!isDesktopRuntime()) return browserOnlyConnection;
  return invoke<ConnectionState>("get_connection");
}

export async function discoverAccounts(token: string): Promise<Account[]> {
  if (!isDesktopRuntime()) {
    throw new Error("Live Cloudflare access requires the Cedar desktop app.");
  }

  return invoke<Account[]>("discover_accounts", { token });
}

export async function connectCloudflare(token: string, accountId?: string): Promise<ConnectResult> {
  if (!isDesktopRuntime()) {
    throw new Error("Live Cloudflare access requires the Cedar desktop app.");
  }

  return invoke<ConnectResult>("connect_cloudflare", { token, accountId: accountId || null });
}

export async function getCachedSnapshot(range: RangeKey): Promise<DashboardSnapshot | null> {
  if (!isDesktopRuntime()) {
    throw new Error("Live Cloudflare access requires the Cedar desktop app.");
  }

  return invoke<DashboardSnapshot | null>("get_cached_snapshot", { range });
}

export async function syncCloudflare(range: RangeKey, forceRefresh = false): Promise<DashboardSnapshot> {
  if (!isDesktopRuntime()) {
    throw new Error("Live Cloudflare access requires the Cedar desktop app.");
  }

  return invoke<DashboardSnapshot>("sync_cloudflare", { range, forceRefresh });
}

export async function clearConnection(): Promise<void> {
  if (!isDesktopRuntime()) return;
  return invoke<void>("clear_connection");
}
