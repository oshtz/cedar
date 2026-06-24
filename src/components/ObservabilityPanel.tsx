import { Activity, FileSearch, RadioTower, ShieldAlert, UploadCloud, Zap } from "lucide-react";
import type { AuditEvent, AuditSummary, CollectorTelemetry, LogpushJob, LogpushSummary, WorkerObservabilitySummary, WorkerTelemetryEvent, ZoneSummary } from "../types";
import { compactNumber } from "../utils";

type ObservabilityPanelProps = {
  zones: ZoneSummary;
  audit: AuditSummary;
  logpush: LogpushSummary;
  observability: WorkerObservabilitySummary;
  collector: CollectorTelemetry;
};

export function ObservabilityPanel({ zones, audit, logpush, observability, collector }: ObservabilityPanelProps) {
  const cacheHit = zones.cacheHitRatio == null ? "N/A" : `${Math.round(zones.cacheHitRatio * 100)}%`;
  const collectorP95 = collector.apiDurationP95Ms == null ? "N/A" : `${collector.apiDurationP95Ms.toFixed(0)} ms`;
  const collectorDetail = collector.apiErrors ? `${compactNumber(collector.apiErrors)} API errors` : "No blocking API errors";
  const latestEndpoints = collector.endpoints.slice(0, 3);
  const signals = buildRecentSignals(audit, logpush, observability);

  return (
    <section className="observability-panel observability-panel-compact" aria-label="Cloudflare observability coverage">
      <div className="panel-heading">
        <div>
          <h2>Observability</h2>
          <p>Coverage signals for audit, Logpush, Worker telemetry, zone traffic, and the Cedar collector.</p>
        </div>
        <span>{collector.apiCalls ? `${collector.apiCalls} API calls` : "Awaiting sync"}</span>
      </div>

      <div className="observability-grid">
        <MetricBlock icon={Activity} label="Zones" value={`${compactNumber(zones.activeZones)}/${compactNumber(zones.zones)}`} detail={`${compactNumber(zones.requests)} requests, ${cacheHit} cache hit`} />
        <MetricBlock icon={ShieldAlert} label="Audit" value={compactNumber(audit.events)} detail={`${compactNumber(audit.failures)} failed actions`} />
        <MetricBlock icon={UploadCloud} label="Logpush" value={`${compactNumber(logpush.enabledJobs)}/${compactNumber(logpush.jobs)}`} detail={`${compactNumber(logpush.workersTraceJobs)} Worker trace jobs`} />
        <MetricBlock icon={RadioTower} label="Worker logs" value={compactNumber((observability.logEvents ?? 0) + (observability.traces ?? 0))} detail={`${compactNumber(observability.fields)} fields`} />
        <MetricBlock icon={RadioTower} label="Worker config" value={compactNumber(observability.configuredWorkers)} detail={`${compactNumber(observability.fullSampleWorkers)} full-sample, ${compactNumber(observability.destinations)} destinations`} />
        <MetricBlock icon={Zap} label="Collector p95" value={collectorP95} detail={collectorDetail} />
      </div>

      {latestEndpoints.length > 0 && (
        <div className="observability-endpoints" aria-label="Recent collector endpoints">
          <span>
            <FileSearch size={14} />
            Recent collector calls
          </span>
          <div className="observability-chip-row">
            {latestEndpoints.map((endpoint) => (
              <em className={endpoint.ok ? "ok" : endpoint.optional ? "info" : "warn"} key={`${endpoint.method}-${endpoint.path}-${endpoint.durationMs}`}>
                {endpoint.method} {endpoint.path} - {endpoint.ok ? (endpoint.status ?? "OK") : endpoint.optional ? "scoped" : (endpoint.status ?? "ERR")}
              </em>
            ))}
          </div>
        </div>
      )}

      {signals.length > 0 && (
        <div className="observability-endpoints" aria-label="Recent observability signals">
          <span>
            <Activity size={14} />
            Recent signals
          </span>
          <div className="observability-chip-row">
            {signals.map((signal, index) => (
              <em className={signal.tone} key={`${signal.label}-${index}`}>
                {signal.label}
              </em>
            ))}
          </div>
        </div>
      )}
    </section>
  );
}

function MetricBlock({ icon: Icon, label, value, detail }: { icon: typeof Activity; label: string; value: string; detail: string }) {
  return (
    <div className="observability-metric">
      <Icon size={16} />
      <span>{label}</span>
      <strong>{value}</strong>
      <small>{detail}</small>
    </div>
  );
}

function buildRecentSignals(audit: AuditSummary, logpush: LogpushSummary, observability: WorkerObservabilitySummary) {
  const auditEvents = audit.recent.filter(isUsefulAuditEvent);
  const warnings = [
    ...observability.gaps.map((gap) => ({ tone: "warn", label: `Gap: ${gap}` })),
    ...auditEvents.filter((event) => isFailedResult(event.result)).map((event) => ({ tone: "warn", label: formatAuditEvent(event) })),
    ...logpush.recent.filter((job) => !job.enabled).map((job) => ({ tone: "warn", label: formatLogpushJob(job) })),
    ...observability.recentEvents.filter(isTelemetryWarning).map((event) => ({ tone: "warn", label: formatTelemetryEvent(event) })),
  ];

  if (warnings.length) return warnings.slice(0, 5);

  return [
    ...auditEvents.slice(0, 2).map((event) => ({ tone: "info", label: formatAuditEvent(event) })),
    ...logpush.recent.slice(0, 2).map((job) => ({ tone: job.enabled ? "ok" : "warn", label: formatLogpushJob(job) })),
    ...observability.recentEvents.slice(0, 2).map((event) => ({ tone: "info", label: formatTelemetryEvent(event) })),
  ].slice(0, 5);
}

function isUsefulAuditEvent(event: AuditEvent) {
  return isUsefulAuditValue(event.action) && isUsefulAuditValue(event.result);
}

function isUsefulAuditValue(value?: string) {
  const normalized = value?.trim().toLowerCase();
  return Boolean(normalized && normalized !== "unknown" && normalized !== "unknown action");
}

function isFailedResult(result: string) {
  const normalized = result.toLowerCase();
  return normalized.includes("fail") || normalized.includes("error") || normalized.includes("deny");
}

function isTelemetryWarning(event: WorkerTelemetryEvent) {
  return [event.level, event.message].filter(Boolean).some((value) => value?.toLowerCase().includes("error"));
}

function formatAuditEvent(event: AuditEvent) {
  return `Audit ${event.result}: ${[event.action, event.resource].filter(Boolean).join(" / ")}`;
}

function formatLogpushJob(job: LogpushJob) {
  return `${job.enabled ? "Logpush" : "Disabled Logpush"} ${job.dataset}: ${job.name}`;
}

function formatTelemetryEvent(event: WorkerTelemetryEvent) {
  return `Worker ${event.service}: ${event.message}`;
}
