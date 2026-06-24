import { describe, expect, it } from "vitest";
import { getCloudflareTokenTemplateUrl } from "./api";

function permissionKeys(url: string) {
  const value = new URL(url).searchParams.get("permissionGroupKeys");
  expect(value).toBeTruthy();
  return JSON.parse(value ?? "[]") as Array<{ key: string; type: string }>;
}

describe("Cloudflare token template URLs", () => {
  it("opens the full-observability user token form for first-run setup across all accounts and zones", () => {
    const url = new URL(getCloudflareTokenTemplateUrl());

    expect(url.origin).toBe("https://dash.cloudflare.com");
    expect(url.pathname).toBe("/profile/api-tokens");
    expect(url.searchParams.get("accountId")).toBe("*");
    expect(url.searchParams.get("zoneId")).toBe("all");
    expect(permissionKeys(url.toString())).toEqual(
      expect.arrayContaining([
        { key: "logs", type: "edit" },
        { key: "workers_observability", type: "edit" },
        { key: "analytics", type: "read" },
      ]),
    );
  });

  it("opens a read-only token form without write-gated observability scopes", () => {
    const url = new URL(getCloudflareTokenTemplateUrl(undefined, "read-only"));
    const permissions = permissionKeys(url.toString());

    expect(url.searchParams.get("name")).toBe("Cedar read-only collector");
    expect(permissions).toEqual(
      expect.arrayContaining([
        { key: "account_settings", type: "read" },
        { key: "account_analytics", type: "read" },
        { key: "analytics", type: "read" },
      ]),
    );
    expect(permissions).not.toEqual(
      expect.arrayContaining([
        { key: "logs", type: "edit" },
        { key: "workers_observability", type: "edit" },
      ]),
    );
  });

  it("opens the account token form once an account is connected", () => {
    const url = new URL(getCloudflareTokenTemplateUrl("account-123"));

    expect(url.origin).toBe("https://dash.cloudflare.com");
    expect(url.pathname).toBe("/");
    expect(url.searchParams.get("to")).toBe("/:account/api-tokens");
    expect(url.searchParams.has("accountId")).toBe(false);
    expect(url.searchParams.has("zoneId")).toBe(false);
  });
});
