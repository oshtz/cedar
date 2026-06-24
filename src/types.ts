export type RangeKey = "24h" | "7d" | "30d";

export type Account = {
  id: string;
  name: string;
};

export type ConnectionState = {
  configured: boolean;
  account?: Account;
  tokenPresent: boolean;
  storage: "os-keychain" | "none";
};

export type InventorySummary = {
  workers: number;
  pages: number;
  d1: number;
  r2: number;
  kv: number;
  zones: number;
};

export type MetricSummary = {
  workerRequests: number;
  workerErrors: number;
  workerCpuTimeMs?: number;
  d1Queries: number;
  d1LatencyP90Ms?: number;
  d1RowsRead?: number;
  d1RowsWritten?: number;
  d1StorageBytes?: number;
  r2Operations: number;
  r2StorageBytes: number;
  r2ClassAOperations?: number;
  r2ClassBOperations?: number;
  kvOperations: number;
  kvStorageBytes: number;
  kvReadOperations?: number;
  kvWriteOperations?: number;
  kvDeleteOperations?: number;
  kvListOperations?: number;
  costUsd?: number | null;
  costCurrency?: string | null;
  costSource?: "billing" | "paid-plan-projection" | "analytics-estimate";
  costBaseUsd?: number | null;
  costOverageUsd?: number | null;
  billingRows?: number | null;
  zoneRequests?: number;
  zoneSecurityEvents?: number;
  zoneCacheHitRatio?: number | null;
  auditEvents?: number;
  auditFailures?: number;
  logpushJobs?: number;
  logpushEnabledJobs?: number;
  workerLogEvents?: number;
  workerTraceEvents?: number;
  workerObservabilityFields?: number;
  workerObservabilityDestinations?: number;
  collectorApiCalls?: number;
  collectorApiErrors?: number;
  collectorApiP95Ms?: number | null;
};

export type ResourceKind = "worker" | "page" | "d1" | "r2" | "kv";

export type ResourceBinding = {
  name: string;
  bindingType?: string;
  resourceKind?: ResourceKind;
  resourceId?: string;
  resourceName?: string;
};

export type ResourceObservability = {
  enabled?: boolean;
  logsEnabled?: boolean;
  tracesEnabled?: boolean;
  invocationLogs?: boolean;
  headSamplingRate?: number;
  logpush?: boolean;
  destinations: string[];
};

export type ResourceRow = {
  id: string;
  name: string;
  kind: ResourceKind;
  status: "healthy" | "warning" | "quiet" | "unknown";
  primaryMetric: string;
  secondaryMetric: string;
  updatedAt?: string;
  bindings?: ResourceBinding[];
  observability?: ResourceObservability;
};

export type WorkerAuditPreference = "normal" | "critical" | "ignore";
export type WorkerAuditPreferences = Record<string, WorkerAuditPreference>;

export type UsagePanel = {
  id: string;
  title: string;
  value: string;
  detail: string;
  tone: "neutral" | "good" | "warn" | "bad";
  points: number[];
};

export type ServiceHealth = {
  id: string;
  service: string;
  status: "ok" | "warn" | "unknown";
  label: string;
  detail: string;
};

export type ZoneHostMetric = {
  host: string;
  requests: number;
};

export type ZoneSummary = {
  zones: number;
  activeZones: number;
  requests: number;
  securityEvents: number;
  cacheHitRatio?: number | null;
  topHosts: ZoneHostMetric[];
};

export type AuditEvent = {
  action: string;
  actor: string;
  interface: string;
  method: string;
  result: string;
  timestamp?: string;
  resource?: string;
};

export type AuditSummary = {
  events: number;
  failures: number;
  apiEvents: number;
  dashboardEvents: number;
  recent: AuditEvent[];
};

export type LogpushJob = {
  id: string;
  name: string;
  dataset: string;
  enabled: boolean;
  destination: string;
  kind?: string;
};

export type LogpushSummary = {
  jobs: number;
  enabledJobs: number;
  workersTraceJobs: number;
  auditJobs: number;
  disabledJobs: number;
  recent: LogpushJob[];
};

export type WorkerTelemetryEvent = {
  service: string;
  message: string;
  timestamp?: string;
  level?: string;
};

export type WorkerObservabilitySummary = {
  logEvents: number;
  errorEvents: number;
  traces: number;
  fields: number;
  destinations: number;
  configuredWorkers: number;
  fullSampleWorkers: number;
  liveTailAvailable: boolean;
  recentEvents: WorkerTelemetryEvent[];
  gaps: string[];
};

export type CollectorEndpoint = {
  method: string;
  path: string;
  status?: number;
  durationMs: number;
  ok: boolean;
  optional?: boolean;
  rayId?: string;
  error?: string;
};

export type CollectorTelemetry = {
  apiCalls: number;
  apiErrors: number;
  apiDurationP95Ms?: number | null;
  rateLimitRemaining?: string;
  lastRayId?: string;
  endpoints: CollectorEndpoint[];
};

export type DashboardSnapshot = {
  generatedAt: string;
  range: RangeKey;
  live: boolean;
  cached: boolean;
  expiresAt?: string;
  account?: Account;
  inventory: InventorySummary;
  metrics: MetricSummary;
  resources: ResourceRow[];
  usagePanels: UsagePanel[];
  health: ServiceHealth[];
  issues: string[];
  zones: ZoneSummary;
  audit: AuditSummary;
  logpush: LogpushSummary;
  observability: WorkerObservabilitySummary;
  collector: CollectorTelemetry;
};

export type ConnectResult = {
  accounts: Account[];
  connection?: ConnectionState;
  snapshot?: DashboardSnapshot;
};
