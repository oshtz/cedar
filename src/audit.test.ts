import { describe, expect, it } from "vitest";
import { buildAuditFindings, buildAuditReport, diffSnapshots } from "./audit";
import { emptySnapshot } from "./emptyState";
import type { DashboardSnapshot, ResourceRow } from "./types";

const worker: ResourceRow = {
  id: "worker-1",
  name: "api-worker",
  kind: "worker",
  status: "healthy",
  primaryMetric: "100 requests",
  secondaryMetric: "1 binding",
  bindings: [{ name: "DB", resourceKind: "d1", resourceName: "main-db" }],
};

const database: ResourceRow = {
  id: "db-1",
  name: "main-db",
  kind: "d1",
  status: "healthy",
  primaryMetric: "10 queries",
  secondaryMetric: "D1 database",
};

function snapshot(resources: ResourceRow[]): DashboardSnapshot {
  return {
    ...emptySnapshot,
    generatedAt: "2026-06-24T10:00:00.000Z",
    live: true,
    inventory: { workers: resources.filter((resource) => resource.kind === "worker").length, pages: 0, d1: resources.filter((resource) => resource.kind === "d1").length, r2: 0, kv: 0, zones: 1 },
    resources,
    logpush: { ...emptySnapshot.logpush },
    collector: { ...emptySnapshot.collector },
  };
}

describe("audit helpers", () => {
  it("surfaces dev-utility audit findings from existing snapshot data", () => {
    const findings = buildAuditFindings({
      ...snapshot([worker, database]),
      metrics: { ...emptySnapshot.metrics, workerErrors: 3 },
      issues: ["inventory collector failed"],
    });

    expect(findings.map((finding) => finding.title)).toEqual(expect.arrayContaining(["Sync issue", "Worker errors", "Worker observability coverage"]));
    expect(findings.find((finding) => finding.title === "Worker observability coverage")?.tone).toBe("warn");
  });

  it("treats quiet workers and missing observability as coverage guidance without errors", () => {
    const quietWorker = { ...worker, id: "worker-quiet", name: "queue-worker", status: "quiet" as const, primaryMetric: "0 requests" };
    const findings = buildAuditFindings(snapshot([quietWorker]));

    expect(findings.find((finding) => finding.title === "Worker observability coverage")?.tone).toBe("neutral");
    expect(findings.find((finding) => finding.title === "Worker trace Logpush coverage")?.tone).toBe("neutral");
    expect(findings.find((finding) => finding.title === "Quiet workers")?.tone).toBe("neutral");
    expect(findings.find((finding) => finding.title === "Quiet workers")?.detail).toContain("1 Worker had no request metrics");
    expect(findings.some((finding) => finding.title === "Resources need attention")).toBe(false);
  });

  it("lets worker audit handling escalate or ignore coverage findings", () => {
    const criticalFindings = buildAuditFindings(snapshot([worker]), { "worker-worker-1": "critical" });
    const ignoredFindings = buildAuditFindings(snapshot([worker]), { "worker-worker-1": "ignore" });

    expect(criticalFindings.find((finding) => finding.title === "Worker observability coverage")?.tone).toBe("warn");
    expect(criticalFindings.find((finding) => finding.title === "Worker observability coverage")?.evidence?.[0]).toContain("critical");
    expect(ignoredFindings.some((finding) => finding.title.includes("coverage"))).toBe(false);
    expect(ignoredFindings.some((finding) => finding.title.includes("Logpush"))).toBe(false);
  });

  it("attaches evidence to coverage and collector findings", () => {
    const findings = buildAuditFindings({
      ...snapshot([worker, database]),
      audit: {
        ...emptySnapshot.audit,
        failures: 1,
        recent: [{ action: "workers.script.update", actor: "dev@example.com", interface: "api", method: "POST", result: "failure", resource: "api-worker" }],
      },
      collector: {
        ...emptySnapshot.collector,
        apiErrors: 1,
        endpoints: [{ method: "GET", path: "/accounts/abc/workers/scripts", status: 403, durationMs: 42, ok: false }],
      },
      logpush: {
        ...emptySnapshot.logpush,
        jobs: 1,
        recent: [{ id: "job-1", name: "old-job", dataset: "workers_trace_events", enabled: false, destination: "r2://logs", kind: "account" }],
        disabledJobs: 1,
      },
      observability: { ...emptySnapshot.observability, gaps: ["workers telemetry query returned no events"] },
    });

    expect(findings.find((finding) => finding.title === "Collector errors")?.evidence?.[0]).toContain("/accounts/abc/workers/scripts");
    expect(findings.find((finding) => finding.title === "Worker telemetry gaps")?.evidence).toContain("workers telemetry query returned no events");
    expect(findings.find((finding) => finding.title === "Disabled Logpush jobs")?.evidence?.[0]).toContain("old-job");
  });

  it("ignores placeholder audit rows when finding failed audit actions", () => {
    const findings = buildAuditFindings({
      ...snapshot([worker, database]),
      audit: {
        ...emptySnapshot.audit,
        recent: [
          {
            action: "unknown action",
            actor: "unknown actor",
            interface: "api",
            method: "GET",
            result: "unknown",
            resource: "observability.telemetry.query",
          },
        ],
      },
    });

    expect(findings.some((finding) => finding.title === "Failed Cloudflare audit actions")).toBe(false);
  });

  it("reports added resources and binding drift between local snapshots", () => {
    const changes = diffSnapshots(snapshot([{ ...worker, bindings: [] }]), snapshot([worker, database]));

    expect(changes.map((change) => change.title)).toEqual(expect.arrayContaining(["Resources added", "Bindings changed"]));
  });

  it("builds a paste-ready markdown audit report", () => {
    const current = { ...snapshot([worker, database]), account: { id: "account-123", name: "Test account" } };
    const findings = buildAuditFindings(current);
    const report = buildAuditReport(current, findings, [], "24h");

    expect(report).toContain("# Cedar audit");
    expect(report).toContain("Summary: No urgent issues.");
    expect(report).toContain("## Coverage");
    expect(report).toContain("- Collector: 0 API calls, 0 errors");
    expect(report).toContain("- Worker observability config:");
    expect(report).toContain("- Token/scope: Required audit checks passed");
    expect(report).toContain("## Findings");
    expect(report).toContain("Action:");
    expect(report).toContain("Evidence:");
    expect(report).toContain("## Next actions");
    expect(report).toContain("### Optional hardening");
    expect(report).toContain("https://dash.cloudflare.com/account-123/workers/services");
    expect(report).not.toContain("### Fix now");
    expect(report).toContain("## Recent changes");
    expect(report).toContain("- Workers: 1");
  });
});
