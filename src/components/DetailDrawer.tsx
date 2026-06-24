import { Activity, Box, Cpu, GaugeCircle, Layers3, RadioTower, ShieldCheck, X } from "lucide-react";
import type { ResourceBinding, ResourceKind, ResourceRow, WorkerAuditPreference } from "../types";

type DetailDrawerProps = {
  resource?: ResourceRow;
  workerPreference?: WorkerAuditPreference;
  onWorkerPreferenceChange?: (preference: WorkerAuditPreference) => void;
  onClearSelection?: () => void;
};

const kindLabel: Record<ResourceKind, string> = {
  worker: "Worker",
  page: "Pages project",
  d1: "D1 database",
  r2: "R2 bucket",
  kv: "KV namespace",
};

function bindingLabel(binding: ResourceBinding) {
  const target = binding.resourceName ?? binding.resourceId;
  const type = binding.bindingType ? binding.bindingType.replace(/_/g, " ") : "binding";
  return target ? `${binding.name} -> ${target}` : `${binding.name} (${type})`;
}

export function DetailDrawer({ resource, workerPreference = "normal", onWorkerPreferenceChange, onClearSelection }: DetailDrawerProps) {
  if (!resource) {
    return (
      <aside className="detail-drawer">
        <div className="drawer-topline">
          <span>Resource details</span>
          <span className="drawer-status">Idle</span>
        </div>
        <div className="drawer-empty">
          <span>
            <Box size={28} />
          </span>
          <h2>No resource selected</h2>
          <p>Select a resource from inventory to view details, metrics, and bindings.</p>
          <div className="drawer-empty-grid" aria-label="Inspector preview">
            <PreviewTile icon={RadioTower} label="Traffic" value="Requests, errors" />
            <PreviewTile icon={Layers3} label="Bindings" value="KV, D1, R2" />
            <PreviewTile icon={ShieldCheck} label="Health" value="Status signals" />
          </div>
        </div>
      </aside>
    );
  }

  const updatedLabel = resource.updatedAt ? new Date(resource.updatedAt).toLocaleString([], { dateStyle: "medium", timeStyle: "short" }) : "Unknown";

  return (
    <aside className="detail-drawer">
      <div className="drawer-topline">
        <span>Resource details</span>
        <button className="drawer-clear" type="button" aria-label="Clear selected resource" onClick={onClearSelection}>
          <X size={14} />
        </button>
      </div>

      <div className="drawer-header">
        <span>Selected {kindLabel[resource.kind]}</span>
        <h2>{resource.name}</h2>
        <div className="drawer-meta">
          <span className={`status-pill ${resource.status}`}>{resource.status}</span>
          <small>{updatedLabel}</small>
        </div>
      </div>

      <div className="drawer-grid">
        {resource.kind === "worker" ? (
          <>
            <DetailMetric icon={Activity} label="Usage" value={resource.primaryMetric} />
            <DetailMetric icon={GaugeCircle} label="Status" value={resource.status} />
            <DetailMetric icon={Cpu} label="Runtime" value={resource.secondaryMetric} />
            <DetailMetric icon={RadioTower} label="Updated" value={updatedLabel} />
          </>
        ) : (
          <>
            <DetailMetric icon={Box} label="Type" value={resource.kind.toUpperCase()} />
            <DetailMetric icon={GaugeCircle} label="Status" value={resource.status} />
            <DetailMetric icon={Activity} label="Usage" value={resource.primaryMetric} />
            <DetailMetric icon={RadioTower} label="Updated" value={updatedLabel} />
          </>
        )}
      </div>

      <div className="drawer-section">
        <h3>Bindings</h3>
        <div className="binding-list">
          {resource.bindings?.length ? (
            resource.bindings.map((binding) => (
              <span key={`${binding.name}-${binding.resourceId ?? binding.resourceName ?? binding.bindingType ?? "binding"}`}>
                <Box size={14} />
                {bindingLabel(binding)}
              </span>
            ))
          ) : (
            <small>No bindings discovered from API metadata.</small>
          )}
        </div>
      </div>

      {resource.kind === "worker" && (
        <div className="drawer-section">
          <h3>Audit handling</h3>
          <div className="worker-preference-toggle" aria-label="Worker audit handling">
            {(["normal", "critical", "ignore"] as const).map((preference) => (
              <button
                className={workerPreference === preference ? "selected" : ""}
                key={preference}
                onClick={() => onWorkerPreferenceChange?.(preference)}
                type="button"
              >
                {preference}
              </button>
            ))}
          </div>
          <small>Critical escalates missing coverage. Ignore removes this Worker from coverage findings.</small>
        </div>
      )}

      <div className="drawer-section">
        <h3>Observability</h3>
        <p>{observabilityLabel(resource)}</p>
      </div>
    </aside>
  );
}

function observabilityLabel(resource: ResourceRow) {
  const details = resource.observability;
  if (!details) return resource.secondaryMetric;

  const parts = [];
  if (details.enabled) parts.push("Workers Observability enabled");
  if (details.logsEnabled || details.invocationLogs) parts.push("logs enabled");
  if (details.tracesEnabled) parts.push("traces enabled");
  if (typeof details.headSamplingRate === "number") parts.push(`${Math.round(details.headSamplingRate * 100)}% head sample`);
  if (details.logpush) parts.push("Logpush configured");
  parts.push(...details.destinations);
  return parts.length ? parts.join(", ") : resource.secondaryMetric;
}

function PreviewTile({ icon: Icon, label, value }: { icon: typeof Activity; label: string; value: string }) {
  return (
    <div className="drawer-preview-tile">
      <Icon size={15} />
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function DetailMetric({ icon: Icon, label, value }: { icon: typeof Activity; label: string; value: string }) {
  return (
    <div className="detail-metric">
      <Icon size={16} />
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}
