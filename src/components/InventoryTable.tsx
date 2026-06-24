import { Archive, Cloud, Database, FileCode2, FolderKanban, HardDrive, KeySquare, Search, X } from "lucide-react";
import { useMemo, useState } from "react";
import type { ResourceBinding, ResourceKind, ResourceRow } from "../types";

const kindIcon: Record<ResourceKind, typeof Cloud> = {
  worker: FileCode2,
  page: FolderKanban,
  d1: Database,
  r2: HardDrive,
  kv: KeySquare,
};

type InventoryTableProps = {
  rows: ResourceRow[];
  selectedResourceKey?: string;
  title?: string;
  description?: string;
  onSelectResource?: (resource: ResourceRow) => void;
};

function resourceKey(row: ResourceRow) {
  return `${row.kind}-${row.id}`;
}

function bindingSummary(bindings?: ResourceBinding[]) {
  if (!bindings?.length) return undefined;
  return bindings
    .slice(0, 3)
    .map((binding) => binding.resourceName ? `${binding.name} -> ${binding.resourceName}` : binding.name)
    .join(", ");
}

export function InventoryTable({ rows, selectedResourceKey, title = "Inventory", description, onSelectResource }: InventoryTableProps) {
  const [query, setQuery] = useState("");
  const [statusFilter, setStatusFilter] = useState<"all" | "healthy" | "attention">("all");
  const visibleRows = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return rows.filter((row) => {
      const matchesQuery = !needle || [row.name, row.kind, row.primaryMetric, row.secondaryMetric].some((value) => value.toLowerCase().includes(needle));
      const matchesStatus =
        statusFilter === "all" ||
        (statusFilter === "healthy" && row.status === "healthy") ||
        (statusFilter === "attention" && row.status !== "healthy");
      return matchesQuery && matchesStatus;
    });
  }, [query, rows, statusFilter]);
  const hasQuery = query.trim().length > 0;
  const countLabel = hasQuery ? `${visibleRows.length}/${rows.length} resources` : `${visibleRows.length} resources`;
  const emptyTitle = hasQuery ? "No matching resources" : "No resources yet";
  const emptyDetail = hasQuery ? "Try a different name, type, metric, or binding." : "Connect Cloudflare to discover your resources.";

  return (
    <section className="panel inventory-panel">
      <div className="panel-heading">
        <div>
          <h2>{title}</h2>
          <p>{description ?? "Workers, Pages, D1, R2, and KV resources discovered through Cloudflare APIs."}</p>
        </div>
        <span>{countLabel}</span>
      </div>

      <div className="table-toolbar">
        <div className="table-tabs" aria-label="Resource status filter">
          <button className={statusFilter === "all" ? "selected" : ""} type="button" onClick={() => setStatusFilter("all")}>
            All
          </button>
          <button className={statusFilter === "healthy" ? "selected" : ""} type="button" onClick={() => setStatusFilter("healthy")}>
            Healthy
          </button>
          <button className={statusFilter === "attention" ? "selected" : ""} type="button" onClick={() => setStatusFilter("attention")}>
            Attention
          </button>
        </div>
        <label className="resource-search">
          <span className="sr-only">Search resources</span>
          <Search size={15} />
          <input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search resources..." />
          {hasQuery && (
            <button className="search-clear" type="button" aria-label="Clear resource search" onClick={() => setQuery("")}>
              <X size={14} />
            </button>
          )}
        </label>
      </div>

      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              <th>Resource</th>
              <th>Type</th>
              <th>Status</th>
              <th>Usage</th>
              <th>Updated</th>
            </tr>
          </thead>
          <tbody>
            {visibleRows.length === 0 && (
              <tr>
                <td className="empty-cell" colSpan={5}>
                  <span className="empty-icon">
                    <Archive size={22} />
                  </span>
                  <strong>{emptyTitle}</strong>
                  <small>{emptyDetail}</small>
                  {hasQuery && (
                    <button className="secondary-button empty-action" type="button" onClick={() => setQuery("")}>
                      Clear search
                    </button>
                  )}
                </td>
              </tr>
            )}
            {visibleRows.map((row) => {
              const Icon = kindIcon[row.kind];
              const key = resourceKey(row);
              const selected = key === selectedResourceKey;
              return (
                <tr
                  aria-selected={selected}
                  className={selected ? "resource-row is-selected" : "resource-row"}
                  key={key}
                  onClick={() => onSelectResource?.(row)}
                  onKeyDown={(event) => {
                    if (event.key !== "Enter" && event.key !== " ") return;
                    event.preventDefault();
                    onSelectResource?.(row);
                  }}
                  tabIndex={0}
                >
                  <td>
                    <div className="resource-cell">
                      <span className="resource-icon">
                        <Icon size={16} />
                      </span>
                      <div>
                        <strong>{row.name}</strong>
                        <small>{bindingSummary(row.bindings) || row.secondaryMetric}</small>
                      </div>
                    </div>
                  </td>
                  <td className="kind-cell">{row.kind.toUpperCase()}</td>
                  <td>
                    <span className={`status-pill ${row.status}`}>{row.status}</span>
                  </td>
                  <td>
                    <strong className="table-metric">{row.primaryMetric}</strong>
                    <small>{row.secondaryMetric}</small>
                  </td>
                  <td>{row.updatedAt ? new Date(row.updatedAt).toLocaleDateString() : "Unknown"}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </section>
  );
}
