import { type CSSProperties, type PointerEvent as ReactPointerEvent, type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AlertCircle, Check, ClipboardCopy, Info } from "lucide-react";
import {
  clearConnection,
  connectCloudflare,
  discoverAccounts,
  getCachedSnapshot,
  getConnection,
  openCloudflareTokenTemplate,
  syncCloudflare,
  type CloudflareTokenMode,
} from "./api";
import {
  buildAuditFindings,
  buildAuditReport,
  diffSnapshots,
  isOptionalScopeIssue,
  type AuditFinding,
  type SnapshotChange,
} from "./audit";
import { DetailDrawer } from "./components/DetailDrawer";
import { ObservabilityPanel } from "./components/ObservabilityPanel";
import { ResourceExplorer } from "./components/ResourceExplorer";
import { SetupPanel } from "./components/SetupPanel";
import { Sidebar, type NavSection } from "./components/Sidebar";
import { Topbar } from "./components/Topbar";
import { UsagePanels } from "./components/UsagePanels";
import { WindowChrome } from "./components/WindowChrome";
import { emptySnapshot } from "./emptyState";
import type { Account, ConnectionState, DashboardSnapshot, RangeKey, ResourceRow, WorkerAuditPreference, WorkerAuditPreferences } from "./types";
import { compactNumber, formatBytes, money } from "./utils";

const panelWidthBounds = {
  right: { min: 280, max: 460, fallback: 312, storageKey: "cedar-drawer-width" },
};
type ThemeMode = "light" | "dark";
type SectionStat = {
  label: string;
  value: string;
  detail: string;
  tone?: "good" | "warn" | "neutral";
};

function clamp(value: number, min: number, max: number) {
  return Math.min(Math.max(value, min), max);
}

function readPanelWidth(key: string, fallback: number, min: number, max: number) {
  try {
    const stored = window.localStorage.getItem(key);
    if (!stored) return fallback;
    return clamp(Number(stored), min, max);
  } catch {
    return fallback;
  }
}

function readThemeMode(): ThemeMode {
  try {
    return window.localStorage.getItem("cedar-theme") === "dark" ? "dark" : "light";
  } catch {
    return "light";
  }
}

function readWorkerAuditPreferences(): WorkerAuditPreferences {
  try {
    const parsed = JSON.parse(window.localStorage.getItem("cedar-worker-audit-preferences") ?? "{}") as Record<string, unknown>;
    return Object.fromEntries(Object.entries(parsed).filter((entry): entry is [string, WorkerAuditPreference] => entry[1] === "critical" || entry[1] === "ignore"));
  } catch {
    return {};
  }
}

function resourceKey(row: ResourceRow) {
  return `${row.kind}-${row.id}`;
}

const sectionCopy: Record<NavSection, { title: string; description: string }> = {
  overview: {
    title: "Audit",
    description: "Run a local Cloudflare account audit, review findings, and copy a handoff report.",
  },
  workers: {
    title: "Workers",
    description: "Worker scripts, binding drift, errors, Logpush, and telemetry coverage.",
  },
  resources: {
    title: "Resources",
    description: "Workers, Pages, D1, R2, and KV inventory in one table.",
  },
  billing: {
    title: "Cost",
    description: "Workers Paid projection and allowance drivers from current usage.",
  },
  settings: {
    title: "Connection",
    description: "Local token, scope mode, and selected account settings.",
  },
};

export function App() {
  const snapshotRef = useRef<DashboardSnapshot>(emptySnapshot);
  const syncingRef = useRef(false);
  const [range, setRange] = useState<RangeKey>("24h");
  const [snapshot, setSnapshot] = useState<DashboardSnapshot>(emptySnapshot);
  const [connection, setConnection] = useState<ConnectionState | null>(null);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [syncing, setSyncing] = useState(false);
  const [error, setError] = useState<string>();
  const [activeSection, setActiveSection] = useState<NavSection>("overview");
  const [recentChanges, setRecentChanges] = useState<SnapshotChange[]>([]);
  const [reportCopied, setReportCopied] = useState(false);
  const [theme, setTheme] = useState<ThemeMode>(() => readThemeMode());
  const [workerAuditPreferences, setWorkerAuditPreferences] = useState<WorkerAuditPreferences>(() => readWorkerAuditPreferences());
  const [selectedResourceKey, setSelectedResourceKey] = useState<string>();
  const [drawerWidth, setDrawerWidth] = useState(() =>
    readPanelWidth(
      panelWidthBounds.right.storageKey,
      panelWidthBounds.right.fallback,
      panelWidthBounds.right.min,
      panelWidthBounds.right.max,
    ),
  );

  const account = snapshot.account ?? connection?.account;
  const issues = useMemo(() => snapshot.issues.filter((issue) => !isOptionalScopeIssue(issue)).slice(0, 3), [snapshot.issues]);
  const optionalIssues = useMemo(() => snapshot.issues.filter(isOptionalScopeIssue).slice(0, 3), [snapshot.issues]);
  const auditFindings = useMemo(() => buildAuditFindings(snapshot, workerAuditPreferences), [snapshot, workerAuditPreferences]);
  const canSync = Boolean(connection?.configured);
  const setupOnly = !canSync;
  const activeCopy = setupOnly
    ? {
        title: "Run a local Cloudflare audit",
        description: "Store a scoped token locally, then audit inventory, drift, coverage, and usage.",
      }
    : sectionCopy[activeSection];
  const totalResources = snapshot.resources.length;
  const healthyServices = snapshot.health.filter((item) => item.status === "ok").length;
  const healthLabel = snapshot.health.length ? `${healthyServices}/${snapshot.health.length} healthy` : "No health checks";
  const connectionLabel = canSync ? "Local keychain" : "Setup required";
  const workerResources = useMemo(() => snapshot.resources.filter((resource) => resource.kind === "worker"), [snapshot.resources]);
  const workerUsagePanels = useMemo(() => snapshot.usagePanels.filter((panel) => panel.id === "workers" || panel.id === "observability"), [snapshot.usagePanels]);
  const selectedResource = useMemo(
    () => snapshot.resources.find((resource) => resourceKey(resource) === selectedResourceKey),
    [selectedResourceKey, snapshot.resources],
  );
  const detailOpen = Boolean(selectedResource);
  const shellStyle = {
    "--drawer-width": `${drawerWidth}px`,
  } as CSSProperties & Record<"--drawer-width", string>;

  const beginDrawerResize = useCallback((event: ReactPointerEvent<HTMLButtonElement>) => {
    event.preventDefault();

    const bounds = panelWidthBounds.right;
    const handlePointerMove = (moveEvent: globalThis.PointerEvent) => {
      setDrawerWidth(clamp(window.innerWidth - moveEvent.clientX, bounds.min, bounds.max));
    };
    const handlePointerUp = () => {
      window.removeEventListener("pointermove", handlePointerMove);
      window.removeEventListener("pointerup", handlePointerUp);
      window.removeEventListener("pointercancel", handlePointerUp);
      document.body.classList.remove("is-resizing-panels");
    };

    document.body.classList.add("is-resizing-panels");
    window.addEventListener("pointermove", handlePointerMove);
    window.addEventListener("pointerup", handlePointerUp);
    window.addEventListener("pointercancel", handlePointerUp);
  }, []);

  const toggleTheme = useCallback(() => {
    setTheme((current) => (current === "light" ? "dark" : "light"));
  }, []);

  useEffect(() => {
    try {
      window.localStorage.setItem(panelWidthBounds.right.storageKey, String(drawerWidth));
    } catch {
      // Local storage can be unavailable in restricted browser contexts.
    }
  }, [drawerWidth]);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    try {
      window.localStorage.setItem("cedar-theme", theme);
    } catch {
      // Local storage can be unavailable in restricted browser contexts.
    }
  }, [theme]);

  useEffect(() => {
    try {
      window.localStorage.setItem("cedar-worker-audit-preferences", JSON.stringify(workerAuditPreferences));
    } catch {
      // Local storage can be unavailable in restricted browser contexts.
    }
  }, [workerAuditPreferences]);

  useEffect(() => {
    if (!reportCopied) return;
    const timer = window.setTimeout(() => setReportCopied(false), 1600);
    return () => window.clearTimeout(timer);
  }, [reportCopied]);

  useEffect(() => {
    if (!selectedResourceKey) return;
    if (!snapshot.resources.some((resource) => resourceKey(resource) === selectedResourceKey)) {
      setSelectedResourceKey(undefined);
    }
  }, [selectedResourceKey, snapshot.resources]);

  const commitSnapshot = useCallback((nextSnapshot: DashboardSnapshot) => {
    const previousSnapshot = snapshotRef.current;

    setRecentChanges(diffSnapshots(previousSnapshot, nextSnapshot));
    snapshotRef.current = nextSnapshot;
    setSnapshot(nextSnapshot);
  }, []);

  const refresh = useCallback(
    async (nextRange = range, forceRefresh = false) => {
      if (!connection?.configured) {
        setActiveSection("settings");
        return;
      }

      if (syncingRef.current) return;
      syncingRef.current = true;
      setSyncing(true);
      setError(undefined);
      try {
        const nextSnapshot = await syncCloudflare(nextRange, forceRefresh);
        commitSnapshot(nextSnapshot);
      } catch (err) {
        setError(String(err));
      } finally {
        syncingRef.current = false;
        setSyncing(false);
      }
    },
    [commitSnapshot, connection?.configured, range],
  );

  useEffect(() => {
    let cancelled = false;

    async function boot() {
      try {
        const state = await getConnection();
        if (cancelled) return;
        setConnection(state);
        if (state.configured) {
          const cached = await getCachedSnapshot("24h");
          if (!cancelled && cached) commitSnapshot(cached);
          const nextSnapshot = await syncCloudflare("24h");
          if (!cancelled) commitSnapshot(nextSnapshot);
        }
      } catch (err) {
        if (!cancelled) setError(String(err));
      }
    }

    void boot();
    return () => {
      cancelled = true;
    };
  }, [commitSnapshot]);

  async function handleRangeChange(nextRange: RangeKey) {
    setRange(nextRange);
    await refresh(nextRange);
  }

  async function handleDiscover(token: string) {
    setSyncing(true);
    setError(undefined);
    try {
      setAccounts(await discoverAccounts(token));
    } catch (err) {
      setError(String(err));
    } finally {
      setSyncing(false);
    }
  }

  async function handleConnect(token: string, accountId?: string) {
    setSyncing(true);
    setError(undefined);
    try {
      const result = await connectCloudflare(token, accountId);
      setAccounts(result.accounts);
      if (result.connection) setConnection(result.connection);
      if (result.snapshot) commitSnapshot(result.snapshot);
    } catch (err) {
      setError(String(err));
    } finally {
      setSyncing(false);
    }
  }

  async function handleCreateToken(mode: CloudflareTokenMode) {
    setError(undefined);
    try {
      await openCloudflareTokenTemplate(account?.id, mode);
    } catch (err) {
      setError(String(err));
    }
  }

  async function handleClear() {
    setSyncing(true);
    setError(undefined);
    try {
      await clearConnection();
      const state = await getConnection();
      setConnection(state);
      commitSnapshot(emptySnapshot);
      setAccounts([]);
      setSelectedResourceKey(undefined);
      setActiveSection("settings");
    } catch (err) {
      setError(String(err));
    } finally {
      setSyncing(false);
    }
  }

  async function handleCopyAuditReport() {
    try {
      await navigator.clipboard.writeText(buildAuditReport(snapshot, auditFindings, recentChanges, range));
      setReportCopied(true);
    } catch {
      setError("Could not copy the Cedar audit report.");
    }
  }

  function handleAuditFindingAction(section?: NavSection) {
    if (!section) return;
    setActiveSection(section);
    setSelectedResourceKey(undefined);
  }

  function handleWorkerPreferenceChange(preference: WorkerAuditPreference) {
    if (!selectedResource || selectedResource.kind !== "worker") return;
    const key = resourceKey(selectedResource);
    setWorkerAuditPreferences((current) => {
      const next = { ...current };
      if (preference === "normal") delete next[key];
      else next[key] = preference;
      return next;
    });
  }

  const renderResourceExplorer = (rows: ResourceRow[], title: string, description: string) => (
    <ResourceExplorer
      rows={rows}
      selectedResourceKey={selectedResourceKey}
      title={title}
      description={description}
      onSelectResource={(resource) => setSelectedResourceKey(resourceKey(resource))}
    />
  );

  const workerBindingCount = workerResources.reduce((total, resource) => total + (resource.bindings?.length ?? 0), 0);
  const storageBytes = snapshot.metrics.r2StorageBytes + snapshot.metrics.kvStorageBytes;
  const billingSource = snapshot.metrics.costSource?.replace(/-/g, " ") ?? "estimate";
  const tokenScopeLabel = !canSync
    ? "Not connected"
    : snapshot.collector.apiErrors > 0
      ? "Collector errors"
      : issues.length > 0
        ? `${issues.length} blocking issues`
        : optionalIssues.length > 0
          ? `${optionalIssues.length} optional gaps`
          : "Required checks passed";

  function renderWorkspaceSection() {
    if (activeSection === "workers") {
      return (
        <>
          <SectionStatGrid
            stats={[
              { label: "Scripts", value: compactNumber(workerResources.length), detail: "Workers discovered" },
              {
                label: "Requests",
                value: compactNumber(snapshot.metrics.workerRequests),
                detail: `${compactNumber(snapshot.metrics.workerErrors)} errors`,
                tone: snapshot.metrics.workerErrors > 0 ? "warn" : "good",
              },
              { label: "Bindings", value: compactNumber(workerBindingCount), detail: "Linked resources" },
              { label: "Telemetry", value: compactNumber((snapshot.metrics.workerLogEvents ?? 0) + (snapshot.metrics.workerTraceEvents ?? 0)), detail: `${compactNumber(snapshot.observability.fields)} fields` },
            ]}
          />
          <div className="content-grid section-content-grid">
            {renderResourceExplorer(workerResources, "Worker scripts", "Runtime inventory, bindings, and Worker observability metadata.")}
            <UsagePanels panels={workerUsagePanels} health={snapshot.health.filter((item) => item.id === "observability" || item.id === "collector")} />
          </div>
        </>
      );
    }

    if (activeSection === "resources") {
      return (
        <>
          <SectionStatGrid
            stats={[
              { label: "Total", value: compactNumber(totalResources), detail: "Cloudflare resources" },
              { label: "Workers", value: compactNumber(snapshot.inventory.workers), detail: "Scripts" },
              { label: "Pages", value: compactNumber(snapshot.inventory.pages), detail: "Projects" },
              { label: "Storage + D1", value: compactNumber(snapshot.inventory.r2 + snapshot.inventory.kv + snapshot.inventory.d1), detail: "Buckets, namespaces, databases" },
            ]}
          />
          <div className="section-wide-resource">
            {renderResourceExplorer(snapshot.resources, "Infrastructure", "Workers, Pages, D1, R2, and KV resources with binding metadata when available.")}
          </div>
        </>
      );
    }

    if (activeSection === "billing") {
      return (
        <>
          <SectionStatGrid
            stats={[
              { label: "Projected monthly", value: snapshot.metrics.costUsd == null ? "N/A" : money(snapshot.metrics.costUsd, snapshot.metrics.costCurrency), detail: billingSource },
              { label: "Base", value: snapshot.metrics.costBaseUsd == null ? "$0.00" : money(snapshot.metrics.costBaseUsd), detail: "Workers Paid plan" },
              { label: "Overage", value: snapshot.metrics.costOverageUsd == null ? "$0.00" : money(snapshot.metrics.costOverageUsd), detail: "Current range projected" },
              { label: "Storage", value: formatBytes(storageBytes), detail: "R2 + KV stored" },
            ]}
          />
          <SectionPanel title="Allowance drivers" meta={range}>
            <MetricList
              rows={[
                ["Worker requests", compactNumber(snapshot.metrics.workerRequests)],
                ["Worker CPU", `${compactNumber(Math.round(snapshot.metrics.workerCpuTimeMs ?? 0))} ms`],
                ["D1 rows read", compactNumber(snapshot.metrics.d1RowsRead ?? 0)],
                ["R2 class A", compactNumber(snapshot.metrics.r2ClassAOperations ?? 0)],
                ["KV reads", compactNumber(snapshot.metrics.kvReadOperations ?? 0)],
              ]}
            />
          </SectionPanel>
        </>
      );
    }

    if (activeSection === "settings") {
      return (
        <div className="settings-grid">
          <SectionPanel title="Local connection" meta={connection?.storage ?? "none"}>
            <MetricList
              rows={[
                ["Account", account?.name ?? "Not connected"],
                ["Token", connection?.tokenPresent ? "Stored in keychain" : "Not stored"],
                ["Token/scope", tokenScopeLabel],
                ["Snapshots", snapshot.cached ? "Cached" : snapshot.live ? "Live" : "Empty"],
                ["Resources", compactNumber(totalResources)],
              ]}
            />
          </SectionPanel>
          <SectionPanel title="Coverage modes" meta="Token setup">
            <MetricList
              rows={[
                ["Read-only", "Inventory, analytics, audit logs"],
                ["Full", "Logpush + Workers telemetry"],
                ["Account Logs row", "Required for account Logpush"],
                ["Zone Logs row", "Required for zone Logpush"],
              ]}
            />
          </SectionPanel>
        </div>
      );
    }

    return (
      <>
        <AuditPanel
          findings={auditFindings}
          changes={recentChanges}
          copied={reportCopied}
          onCopyReport={handleCopyAuditReport}
          onOpenSection={handleAuditFindingAction}
        />
        <ObservabilityPanel
          zones={snapshot.zones}
          audit={snapshot.audit}
          logpush={snapshot.logpush}
          observability={snapshot.observability}
          collector={snapshot.collector}
        />
        <div className="content-grid">
          {renderResourceExplorer(snapshot.resources, "Infrastructure", "Workers, Pages, D1, R2, and KV resources with binding metadata when available.")}
          <UsagePanels panels={snapshot.usagePanels} health={snapshot.health} />
        </div>
      </>
    );
  }

  return (
    <div
      className="app-shell"
      data-detail-open={detailOpen ? "true" : "false"}
      data-section={activeSection}
      data-setup-only={setupOnly ? "true" : "false"}
      data-theme={theme}
      style={shellStyle}
    >
      <WindowChrome accountName={account?.name} connected={canSync} syncing={syncing} live={snapshot.live} />

      <Sidebar
        activeSection={activeSection}
        connected={canSync}
        accountName={account?.name}
        inventory={snapshot.inventory}
        theme={theme}
        onToggleTheme={toggleTheme}
        onSectionChange={(section) => {
          setActiveSection(section);
          setSelectedResourceKey(undefined);
        }}
      />

      <main className="workspace">
        <Topbar
          account={account}
          range={range}
          lastSync={snapshot.generatedAt}
          syncing={syncing}
          live={snapshot.live}
          cached={snapshot.cached}
          expiresAt={snapshot.expiresAt}
          canSync={canSync}
          onRangeChange={handleRangeChange}
          onRefresh={() => refresh(range, true)}
        />

        <div className="workspace-scroll">
          <div className="section-title">
            <div className="section-heading">
              <h1>{activeCopy.title}</h1>
              {activeCopy.description && <p>{activeCopy.description}</p>}
              {!setupOnly && (
                <div className="section-meta" aria-label="Current audit status">
                  <span>{connectionLabel}</span>
                  <span>{totalResources} resources</span>
                  <span>{snapshot.inventory.zones} zones</span>
                  <span>{healthLabel}</span>
                </div>
              )}
            </div>
          </div>

          <SetupPanel
            visible={setupOnly || activeSection === "settings"}
            loading={syncing}
            accounts={accounts}
            error={error}
            onCreateToken={handleCreateToken}
            onDiscover={handleDiscover}
            onConnect={handleConnect}
            onClear={handleClear}
          />

          {(error || issues.length > 0) && (
            <div className="issue-strip">
              <AlertCircle size={16} />
              <div>
                {error && <strong>{error}</strong>}
                {issues.map((issue) => (
                  <span key={issue}>{issue}</span>
                ))}
              </div>
            </div>
          )}

          {optionalIssues.length > 0 && (
            <div className="issue-strip scope-strip">
              <Info size={16} />
              <div>
                <strong>Coverage detail</strong>
                {optionalIssues.map((issue) => (
                  <span key={issue}>{issue}</span>
                ))}
              </div>
            </div>
          )}

          {!setupOnly && renderWorkspaceSection()}
        </div>
      </main>

      {selectedResource && (
        <button
          className="rail-resizer rail-resizer-right"
          type="button"
          aria-hidden="true"
          tabIndex={-1}
          onPointerDown={beginDrawerResize}
        />
      )}

      {selectedResource && (
        <DetailDrawer
          resource={selectedResource}
          workerPreference={selectedResource.kind === "worker" ? workerAuditPreferences[resourceKey(selectedResource)] ?? "normal" : undefined}
          onWorkerPreferenceChange={handleWorkerPreferenceChange}
          onClearSelection={() => setSelectedResourceKey(undefined)}
        />
      )}
    </div>
  );
}

function AuditPanel({
  findings,
  changes,
  copied,
  onCopyReport,
  onOpenSection,
}: {
  findings: AuditFinding[];
  changes: SnapshotChange[];
  copied: boolean;
  onCopyReport: () => void;
  onOpenSection: (section?: NavSection) => void;
}) {
  return (
    <section className="panel audit-panel" aria-label="Cloudflare account audit">
      <div className="panel-heading audit-heading">
        <div>
          <h2>Action queue</h2>
          <p>Findings from the latest local snapshot, recent drift, and a paste-ready report.</p>
        </div>
        <button className="secondary-button audit-copy-button" type="button" onClick={onCopyReport}>
          {copied ? <Check size={15} /> : <ClipboardCopy size={15} />}
          <span>{copied ? "Copied" : "Copy report"}</span>
        </button>
      </div>

      <div className="audit-grid">
        <div className="audit-column">
          <h3>Findings</h3>
          {findings.map((finding) => (
            <article className={`audit-card ${finding.tone}`} key={`${finding.title}-${finding.detail}`}>
              <strong>{finding.title}</strong>
              <small>{finding.detail}</small>
              {finding.evidence?.length ? (
                <ul className="audit-evidence">
                  {finding.evidence.slice(0, 2).map((item) => (
                    <li key={item}>{item}</li>
                  ))}
                </ul>
              ) : null}
              {finding.action && <small className="audit-next">Action: {finding.action}</small>}
              {finding.section && finding.section !== "overview" && (
                <button className="audit-card-action" type="button" onClick={() => onOpenSection(finding.section)}>
                  Open {sectionCopy[finding.section].title}
                </button>
              )}
            </article>
          ))}
        </div>

        <div className="audit-column">
          <h3>Recent changes</h3>
          {changes.length ? (
            changes.map((change) => (
              <article className={`audit-card ${change.tone}`} key={`${change.title}-${change.detail}`}>
                <strong>{change.title}</strong>
                <small>{change.detail}</small>
              </article>
            ))
          ) : (
            <article className="audit-card neutral">
              <strong>No tracked changes</strong>
              <small>Nothing changed since the previous local snapshot.</small>
            </article>
          )}
        </div>
      </div>
    </section>
  );
}

function SectionStatGrid({ stats }: { stats: SectionStat[] }) {
  return (
    <div className="section-stat-grid">
      {stats.map((stat) => (
        <article className={`section-stat-card ${stat.tone ?? "neutral"}`} key={stat.label}>
          <span>{stat.label}</span>
          <strong>{stat.value}</strong>
          <small>{stat.detail}</small>
        </article>
      ))}
    </div>
  );
}

function SectionPanel({ title, meta, children }: { title: string; meta: string; children: ReactNode }) {
  return (
    <article className="panel section-panel">
      <div className="panel-heading compact">
        <h2>{title}</h2>
        <span>{meta}</span>
      </div>
      {children}
    </article>
  );
}

function MetricList({ rows }: { rows: Array<[string, string]> }) {
  return (
    <div className="section-metric-list">
      {rows.map(([label, value]) => (
        <div className="section-metric-row" key={label}>
          <span>{label}</span>
          <strong>{value}</strong>
        </div>
      ))}
    </div>
  );
}
