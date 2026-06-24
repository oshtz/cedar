import type { DashboardSnapshot, RangeKey, ResourceRow, WorkerAuditPreferences, WorkerAuditPreference } from "./types";
import { compactNumber, formatBytes, money } from "./utils";

export type AuditFinding = {
  title: string;
  detail: string;
  tone: "good" | "warn" | "bad" | "neutral";
  section?: "overview" | "resources" | "workers" | "billing" | "settings";
  action?: string;
  evidence?: string[];
};

export type SnapshotChange = {
  title: string;
  detail: string;
  tone: "good" | "warn" | "neutral";
};

export function isOptionalScopeIssue(issue: string) {
  const normalized = issue.toLowerCase();
  return (
    normalized.startsWith("optional ") ||
    normalized.includes(" optional checks scoped") ||
    normalized.includes("write-gated") ||
    normalized.includes("cloudflare requires logs write") ||
    normalized.includes("cloudflare requires workers observability write")
  );
}

export function uniqueActionableIssues(issues: string[]) {
  const seen = new Set<string>();
  return issues.filter((issue) => {
    const key = issue.trim();
    if (!key || seen.has(key) || isOptionalScopeIssue(key)) return false;
    seen.add(key);
    return true;
  });
}

export function buildAuditFindings(snapshot: DashboardSnapshot, workerPreferences: WorkerAuditPreferences = {}): AuditFinding[] {
  if (!snapshot.resources.length) {
    return [
      {
        title: "No account inventory yet",
        detail: "Connect Cloudflare and sync to produce an audit snapshot.",
        tone: "neutral",
        section: "settings",
      },
    ];
  }

  const findings: AuditFinding[] = [];
  const actionableIssues = uniqueActionableIssues(snapshot.issues);
  const optionalIssues = snapshot.issues.filter(isOptionalScopeIssue);
  const workers = snapshot.resources.filter((resource) => resource.kind === "worker");
  const auditedWorkers = workers.filter((worker) => workerPreference(worker, workerPreferences) !== "ignore");
  const workersWithoutObservability = auditedWorkers.filter((worker) => !hasWorkerObservability(worker));
  const criticalWorkersWithoutObservability = workersWithoutObservability.filter((worker) => workerPreference(worker, workerPreferences) === "critical");
  const quietWorkers = auditedWorkers.filter((worker) => worker.status === "quiet");
  const attentionResources = snapshot.resources.filter((resource) => resource.status === "warning" || resource.status === "unknown");
  const failedAuditEvents = snapshot.audit.recent.filter(isFailedAuditEvent);
  const auditFailureCount = Math.max(snapshot.audit.failures, failedAuditEvents.length);
  const disabledLogpushJobs = snapshot.logpush.recent.filter((job) => !job.enabled);
  const collectorFailures = snapshot.collector.endpoints.filter((endpoint) => !endpoint.ok && !endpoint.optional);
  const logpushScopeBlocked = optionalIssues.some((issue) => issue.toLowerCase().includes("logpush"));
  const lowRateLimit = parseRateLimitRemaining(snapshot.collector.rateLimitRemaining);
  const costOverage = snapshot.metrics.costOverageUsd ?? 0;
  const coverageTone = snapshot.metrics.workerErrors > 0 || criticalWorkersWithoutObservability.length > 0 ? "warn" : "neutral";

  actionableIssues.slice(0, 3).forEach((issue) => {
    findings.push({
      title: "Sync issue",
      detail: issue,
      tone: issue.toLowerCase().includes("failed") ? "bad" : "warn",
      section: "settings",
      action: "Fix the token scope or Cloudflare API issue, then run the audit again.",
      evidence: [issue],
    });
  });

  if (snapshot.collector.apiErrors) {
    findings.push({
      title: "Collector errors",
      detail: `${compactNumber(snapshot.collector.apiErrors)} Cloudflare API calls failed during the last sync.`,
      tone: "bad",
      section: "settings",
      action: "Open Connection, check token scopes, then rerun the audit.",
      evidence: [
        ...collectorFailures.map(formatEndpoint),
        snapshot.collector.lastRayId ? `Last Ray ID: ${snapshot.collector.lastRayId}` : undefined,
      ].filter(Boolean) as string[],
    });
  }

  if (lowRateLimit != null && lowRateLimit < 100) {
    findings.push({
      title: "Cloudflare rate limit is low",
      detail: `${compactNumber(lowRateLimit)} API calls remain on the last observed rate-limit header.`,
      tone: "warn",
      section: "settings",
      action: "Wait before forcing another sync if Cloudflare starts returning 429s.",
      evidence: [`RateLimit-Remaining: ${lowRateLimit}`],
    });
  }

  if (snapshot.metrics.workerErrors > 0) {
    findings.push({
      title: "Worker errors",
      detail: `${compactNumber(snapshot.metrics.workerErrors)} Worker errors in the selected ${snapshot.range} range.`,
      tone: "warn",
      section: "workers",
      action: "Open Workers and inspect scripts with warning/quiet status first.",
      evidence: nameEvidence(auditedWorkers.filter((worker) => worker.status !== "healthy"), workerPreferences),
    });
  }

  if (workersWithoutObservability.length > 0) {
    findings.push({
      title: "Worker observability coverage",
      detail: `${compactNumber(workersWithoutObservability.length)} of ${compactNumber(auditedWorkers.length)} audited Workers have no logs, traces, destinations, or Logpush metadata.`,
      tone: coverageTone,
      section: "workers",
      action: "Prioritize logs/traces for production or traffic-bearing scripts.",
      evidence: nameEvidence(workersWithoutObservability, workerPreferences),
    });
  }

  if (snapshot.observability.gaps.length > 0) {
    findings.push({
      title: "Worker telemetry gaps",
      detail: `${compactNumber(snapshot.observability.gaps.length)} Workers Observability checks did not produce full coverage.`,
      tone: "warn",
      section: "workers",
      action: "Open Workers and verify Workers Observability access and telemetry configuration.",
      evidence: snapshot.observability.gaps.slice(0, 4),
    });
  }

  if (!logpushScopeBlocked && auditedWorkers.length > 0 && snapshot.logpush.workersTraceJobs === 0) {
    findings.push({
      title: "Worker trace Logpush coverage",
      detail: "No Worker trace Logpush jobs were found for this account.",
      tone: coverageTone,
      section: "workers",
      action: "Add Worker trace Logpush only when durable incident logs matter.",
      evidence: snapshot.logpush.recent.length ? snapshot.logpush.recent.map(formatLogpushJob).slice(0, 4) : ["No Worker trace jobs in the latest Logpush inventory."],
    });
  }

  if (!logpushScopeBlocked && snapshot.logpush.jobs > 0 && snapshot.logpush.auditJobs === 0) {
    findings.push({
      title: "Audit Logpush missing",
      detail: "Logpush exists, but no audit-log job was found.",
      tone: "warn",
      section: "workers",
      action: "Add an audit Logpush job if account changes need an external trail.",
      evidence: snapshot.logpush.recent.map(formatLogpushJob).slice(0, 4),
    });
  }

  if (snapshot.logpush.disabledJobs > 0) {
    findings.push({
      title: "Disabled Logpush jobs",
      detail: `${compactNumber(snapshot.logpush.disabledJobs)} Logpush jobs are disabled.`,
      tone: "warn",
      section: "workers",
      action: "Enable or delete disabled Logpush jobs so coverage is unambiguous.",
      evidence: disabledLogpushJobs.map(formatLogpushJob).slice(0, 4),
    });
  }

  if (auditFailureCount > 0) {
    findings.push({
      title: "Failed Cloudflare audit actions",
      detail: `${compactNumber(auditFailureCount)} failed audit-log events in the selected ${snapshot.range} range.`,
      tone: "warn",
      section: "settings",
      action: "Review failed account actions before treating the snapshot as clean.",
      evidence: failedAuditEvents.map(formatAuditEvent).slice(0, 4),
    });
  }

  if (attentionResources.length > 0) {
    findings.push({
      title: "Resources need attention",
      detail: `${compactNumber(attentionResources.length)} resources are warning or unknown: ${nameList(attentionResources)}.`,
      tone: "warn",
      section: "resources",
      action: "Open the relevant resource table and inspect warning or unknown rows.",
      evidence: nameEvidence(attentionResources, workerPreferences),
    });
  }

  if (quietWorkers.length > 0) {
    findings.push({
      title: "Quiet workers",
      detail: `${compactNumber(quietWorkers.length)} ${plural(quietWorkers.length, "Worker", "Workers")} had no request metrics in the selected ${snapshot.range} range.`,
      tone: "neutral",
      section: "workers",
      action: "Confirm quiet is expected for cron, queue, or low-traffic scripts.",
      evidence: nameEvidence(quietWorkers, workerPreferences),
    });
  }

  if (costOverage > 0) {
    findings.push({
      title: "Projected Workers overage",
      detail: `${money(costOverage)} over the Workers Paid base from current usage projection.`,
      tone: "warn",
      section: "billing",
      action: "Open Cost and check the allowance drivers before the month closes.",
      evidence: [`Projection: ${money(snapshot.metrics.costUsd ?? 0, snapshot.metrics.costCurrency)}`, `Overage: ${money(costOverage)}`],
    });
  }

  if (optionalIssues.length > 0) {
    findings.push({
      title: "Scoped coverage gaps",
      detail: `${compactNumber(optionalIssues.length)} optional checks were blocked by token scope, plan, or Cloudflare endpoint access.`,
      tone: "neutral",
      section: "settings",
      action: "Use a full audit token when Logpush or Workers Observability coverage matters.",
      evidence: optionalIssues.slice(0, 4),
    });
  }

  if (!findings.length) {
    findings.push({
      title: "No audit findings",
      detail: "Inventory, collector, observability, Logpush, and usage checks did not surface obvious action items.",
      tone: "good",
      section: "overview",
      action: "Copy the report or rerun after the next infrastructure change.",
    });
  }

  return findings.slice(0, 6);
}

export function diffSnapshots(previous: DashboardSnapshot, next: DashboardSnapshot): SnapshotChange[] {
  if (!previous.generatedAt || previous.resources.length === 0) return [];
  if (previous.range !== next.range) return [];

  const changes: SnapshotChange[] = [];
  const previousRows = new Map(previous.resources.map((resource) => [resourceKey(resource), resource]));
  const nextRows = new Map(next.resources.map((resource) => [resourceKey(resource), resource]));
  const added = next.resources.filter((resource) => !previousRows.has(resourceKey(resource)));
  const removed = previous.resources.filter((resource) => !nextRows.has(resourceKey(resource)));
  const statusChanged = next.resources.filter((resource) => {
    const previousResource = previousRows.get(resourceKey(resource));
    return previousResource && previousResource.status !== resource.status;
  });
  const bindingsChanged = next.resources.filter((resource) => {
    const previousResource = previousRows.get(resourceKey(resource));
    return previousResource && bindingSignature(previousResource) !== bindingSignature(resource);
  });
  const newIssues = uniqueActionableIssues(next.issues).filter((issue) => !uniqueActionableIssues(previous.issues).includes(issue));

  if (added.length) {
    changes.push({
      title: "Resources added",
      detail: `${compactNumber(added.length)} new resources: ${nameList(added)}.`,
      tone: "good",
    });
  }

  if (removed.length) {
    changes.push({
      title: "Resources removed",
      detail: `${compactNumber(removed.length)} resources disappeared: ${nameList(removed)}.`,
      tone: "warn",
    });
  }

  if (statusChanged.length) {
    changes.push({
      title: "Status changed",
      detail: `${compactNumber(statusChanged.length)} resources changed health state: ${nameList(statusChanged)}.`,
      tone: "warn",
    });
  }

  if (bindingsChanged.length) {
    changes.push({
      title: "Bindings changed",
      detail: `${compactNumber(bindingsChanged.length)} Workers changed binding metadata: ${nameList(bindingsChanged)}.`,
      tone: "neutral",
    });
  }

  if (newIssues.length) {
    changes.push({
      title: "New sync findings",
      detail: newIssues.slice(0, 2).join(" / "),
      tone: "warn",
    });
  }

  const previousCost = previous.metrics.costUsd;
  const nextCost = next.metrics.costUsd;
  if (typeof previousCost === "number" && typeof nextCost === "number" && Math.abs(nextCost - previousCost) >= 0.01) {
    changes.push({
      title: "Cost projection moved",
      detail: `${money(previousCost, previous.metrics.costCurrency)} to ${money(nextCost, next.metrics.costCurrency)}.`,
      tone: nextCost > previousCost ? "warn" : "good",
    });
  }

  return changes.slice(0, 8);
}

export function buildAuditReport(snapshot: DashboardSnapshot, findings: AuditFinding[], changes: SnapshotChange[], range: RangeKey) {
  const account = snapshot.account?.name ?? "Unknown account";
  const generatedAt = snapshot.generatedAt ? new Date(snapshot.generatedAt).toLocaleString() : "Not synced";
  const source = snapshot.live ? "Live Cloudflare API" : snapshot.cached ? "Local cache" : "Empty";
  const nextActions = findings.filter((finding) => finding.tone !== "good");
  const optionalCoverageGaps = snapshot.issues.filter(isOptionalScopeIssue).length;
  const actionGroups = groupNextActions(nextActions);
  const reportActions = formatNextActionsForReport(snapshot, actionGroups);
  const workerCount = snapshot.resources.filter((resource) => resource.kind === "worker").length;

  return [
    `# Cedar audit - ${account}`,
    "",
    `Generated: ${generatedAt}`,
    `Range: ${range}`,
    `Source: ${source}`,
    `Summary: ${formatActionSummary(actionGroups)}`,
    "",
    "## Inventory",
    `- Workers: ${compactNumber(snapshot.inventory.workers)}`,
    `- Pages: ${compactNumber(snapshot.inventory.pages)}`,
    `- D1: ${compactNumber(snapshot.inventory.d1)}`,
    `- R2: ${compactNumber(snapshot.inventory.r2)} (${formatBytes(snapshot.metrics.r2StorageBytes)})`,
    `- KV: ${compactNumber(snapshot.inventory.kv)} (${formatBytes(snapshot.metrics.kvStorageBytes)})`,
    `- Zones: ${compactNumber(snapshot.inventory.zones)}`,
    "",
    "## Coverage",
    `- Collector: ${compactNumber(snapshot.collector.apiCalls)} API calls, ${compactNumber(snapshot.collector.apiErrors)} errors`,
    `- Audit logs: ${compactNumber(snapshot.audit.events)} events, ${compactNumber(snapshot.audit.failures)} failures`,
    `- Logpush: ${compactNumber(snapshot.logpush.enabledJobs)}/${compactNumber(snapshot.logpush.jobs)} jobs enabled, ${compactNumber(snapshot.logpush.workersTraceJobs)} Worker trace jobs`,
    `- Workers Observability: ${compactNumber((snapshot.observability.logEvents ?? 0) + (snapshot.observability.traces ?? 0))} events/traces, ${compactNumber(snapshot.observability.fields)} fields, ${compactNumber(snapshot.observability.destinations)} destinations`,
    `- Worker observability config: ${compactNumber(snapshot.observability.configuredWorkers)}/${compactNumber(workerCount)} Workers configured, ${compactNumber(snapshot.observability.fullSampleWorkers)} full-sample, ${compactNumber(snapshot.observability.destinations)} destinations`,
    `- Token/scope: ${formatScopeStatus(snapshot, optionalCoverageGaps)}`,
    `- Scope gaps: ${compactNumber(optionalCoverageGaps)} optional checks blocked`,
    "",
    "## Findings",
    ...findings.flatMap(formatFindingForReport),
    "",
    "## Next actions",
    ...reportActions,
    "",
    "## Recent changes",
    ...(changes.length ? changes.map((change) => `- [${change.tone}] ${change.title}: ${change.detail}`) : ["- No tracked changes since the previous snapshot."]),
    "",
    "## Usage",
    `- Worker requests: ${compactNumber(snapshot.metrics.workerRequests)}`,
    `- Worker errors: ${compactNumber(snapshot.metrics.workerErrors)}`,
    `- D1 queries: ${compactNumber(snapshot.metrics.d1Queries)}`,
    `- R2 operations: ${compactNumber(snapshot.metrics.r2Operations)}`,
    `- KV operations: ${compactNumber(snapshot.metrics.kvOperations)}`,
    `- Workers cost projection: ${snapshot.metrics.costUsd == null ? "N/A" : money(snapshot.metrics.costUsd, snapshot.metrics.costCurrency)}`,
  ].join("\n");
}

type ReportActionGroups = {
  fixNow: AuditFinding[];
  checks: AuditFinding[];
  optional: AuditFinding[];
};

function groupNextActions(findings: AuditFinding[]): ReportActionGroups {
  return {
    fixNow: findings.filter((finding) => finding.tone === "bad" || finding.tone === "warn"),
    checks: findings.filter((finding) => finding.tone === "neutral" && !isOptionalHardening(finding)),
    optional: findings.filter((finding) => finding.tone === "neutral" && isOptionalHardening(finding)),
  };
}

function formatNextActionsForReport(snapshot: DashboardSnapshot, groups: ReportActionGroups) {
  if (!groups.fixNow.length && !groups.checks.length && !groups.optional.length) return ["- No immediate action items from this snapshot."];

  const rows = [
    ["Fix now", groups.fixNow],
    ["Check", groups.checks],
    ["Optional hardening", groups.optional],
  ] as const;

  return rows.flatMap(([title, findings]) => findings.length ? [`### ${title}`, ...findings.map((finding) => formatActionForReport(snapshot, finding)), ""] : []).slice(0, -1);
}

function formatActionSummary(groups: ReportActionGroups) {
  return [
    groups.fixNow.length ? `${groups.fixNow.length} urgent ${plural(groups.fixNow.length, "item", "items")}` : "No urgent issues",
    `${groups.checks.length} ${plural(groups.checks.length, "check", "checks")}`,
    `${groups.optional.length} optional hardening ${plural(groups.optional.length, "item", "items")}`,
  ].join(". ") + ".";
}

function formatActionForReport(snapshot: DashboardSnapshot, finding: AuditFinding) {
  const link = cloudflareActionUrl(snapshot, finding);
  return `- ${finding.section ? `Open ${sectionLabel(finding.section)}: ` : ""}${finding.action ?? finding.title}${link ? ` ([Open Cloudflare](${link}))` : ""}`;
}

function formatScopeStatus(snapshot: DashboardSnapshot, optionalCoverageGaps: number) {
  const blockingIssues = uniqueActionableIssues(snapshot.issues).length;
  if (snapshot.collector.apiErrors > 0) return "Collector errors need review";
  if (blockingIssues > 0) return `${compactNumber(blockingIssues)} blocking scope or API issue${blockingIssues === 1 ? "" : "s"}`;
  if (optionalCoverageGaps > 0) return `${compactNumber(optionalCoverageGaps)} optional checks blocked`;
  return "Required audit checks passed";
}

function cloudflareActionUrl(snapshot: DashboardSnapshot, finding: AuditFinding) {
  const accountId = snapshot.account?.id;
  if (!accountId) return undefined;

  const text = `${finding.title} ${finding.action ?? ""}`.toLowerCase();
  if (text.includes("logpush")) return `https://dash.cloudflare.com/${accountId}/analytics/logpush`;
  if (finding.section === "workers") return `https://dash.cloudflare.com/${accountId}/workers/services`;
  if (finding.section === "billing") return `https://dash.cloudflare.com/${accountId}/billing`;
  if (finding.section === "settings") return "https://dash.cloudflare.com/profile/api-tokens";
  if (finding.section === "resources") return `https://dash.cloudflare.com/${accountId}`;
  return undefined;
}

function plural(count: number, singular: string, pluralValue: string) {
  return count === 1 ? singular : pluralValue;
}

function isOptionalHardening(finding: AuditFinding) {
  const text = `${finding.title} ${finding.action ?? ""}`.toLowerCase();
  return text.includes("coverage") || text.includes("logpush") || text.includes("observability") || text.includes("scope");
}

function hasWorkerObservability(worker: ResourceRow) {
  const observability = worker.observability;
  if (!observability) return false;
  return Boolean(
    observability.enabled ||
      observability.logsEnabled ||
      observability.tracesEnabled ||
      observability.invocationLogs ||
      observability.logpush ||
      observability.destinations.length,
  );
}

function workerPreference(worker: ResourceRow, preferences: WorkerAuditPreferences): WorkerAuditPreference {
  return preferences[resourceKey(worker)] ?? "normal";
}

function parseRateLimitRemaining(value?: string) {
  if (!value) return undefined;
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) ? parsed : undefined;
}

function isFailedAuditEvent(event: { result: string }) {
  const result = event.result.trim().toLowerCase();
  return result.includes("fail") || result.includes("error") || result.includes("deny");
}

function formatAuditEvent(event: { action: string; actor: string; result: string; resource?: string; timestamp?: string }) {
  return [event.action, event.result, event.resource, event.actor, event.timestamp].map(cleanAuditPart).filter(Boolean).join(" / ") || "Audit event";
}

function cleanAuditPart(value?: string) {
  const trimmed = value?.trim();
  if (!trimmed) return undefined;
  const normalized = trimmed.toLowerCase();
  return normalized === "unknown" || normalized === "unknown action" || normalized === "unknown actor" ? undefined : trimmed;
}

function formatLogpushJob(job: { name: string; dataset: string; enabled: boolean; destination: string; kind?: string }) {
  return `${job.enabled ? "enabled" : "disabled"} ${job.kind ?? "logpush"} ${job.dataset}: ${job.name} -> ${job.destination}`;
}

function formatEndpoint(endpoint: { method: string; path: string; status?: number; durationMs: number; error?: string; rayId?: string }) {
  return [
    `${endpoint.method} ${endpoint.path}`,
    endpoint.status == null ? undefined : String(endpoint.status),
    `${Math.round(endpoint.durationMs)} ms`,
    endpoint.rayId ? `ray ${endpoint.rayId}` : undefined,
    endpoint.error,
  ]
    .filter(Boolean)
    .join(" / ");
}

function nameEvidence(rows: ResourceRow[], preferences: WorkerAuditPreferences = {}) {
  return rows.slice(0, 4).map((resource) => {
    const preference = resource.kind === "worker" ? workerPreference(resource, preferences) : "normal";
    const suffix = preference === "critical" ? ", critical" : "";
    return `${resource.name} (${resource.kind}, ${resource.status}${suffix})`;
  });
}

function formatFindingForReport(finding: AuditFinding) {
  const lines = [`- [${finding.tone}] ${finding.title}: ${finding.detail}`];
  if (finding.action) lines.push(`  - Action: ${finding.action}`);
  finding.evidence?.slice(0, 4).forEach((item) => lines.push(`  - Evidence: ${item}`));
  return lines;
}

function resourceKey(row: ResourceRow) {
  return `${row.kind}-${row.id}`;
}

function nameList(rows: ResourceRow[]) {
  const names = rows.slice(0, 3).map((resource) => resource.name);
  return rows.length > names.length ? `${names.join(", ")} +${rows.length - names.length}` : names.join(", ");
}

function bindingSignature(row: ResourceRow) {
  return (row.bindings ?? [])
    .map((binding) => [binding.name, binding.bindingType, binding.resourceKind, binding.resourceId, binding.resourceName].filter(Boolean).join(":"))
    .sort()
    .join("|");
}

function sectionLabel(section: NonNullable<AuditFinding["section"]>) {
  const labels: Record<NonNullable<AuditFinding["section"]>, string> = {
    overview: "Audit",
    resources: "Resources",
    workers: "Workers",
    billing: "Cost",
    settings: "Connection",
  };

  return labels[section];
}
