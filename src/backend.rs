use chrono::{DateTime, Duration, Utc};
use reqwest::{Client, header::HeaderMap};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

const CF_API_BASE: &str = "https://api.cloudflare.com/client/v4";
const KEYRING_SERVICE: &str = "cedar";
const KEYRING_USER: &str = "cloudflare-api-token";
const CF_REST_PAGE_SIZE: usize = 100;
const CF_REST_MAX_PAGES: usize = 100;
const ZONE_GRAPHQL_BATCH_SIZE: usize = 10;

pub(crate) type AppResult<T> = Result<T, AppError>;

#[derive(Debug)]
pub(crate) enum AppError {
    Message(String),
    Http(reqwest::Error),
    Database(rusqlite::Error),
    Keyring(keyring::Error),
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(message) => formatter.write_str(message),
            Self::Http(error) => write!(formatter, "Cloudflare API request failed: {error}"),
            Self::Database(error) => write!(formatter, "Local database failed: {error}"),
            Self::Keyring(error) => write!(formatter, "Local secret storage failed: {error}"),
            Self::Io(error) => write!(formatter, "File system failed: {error}"),
            Self::Json(error) => write!(formatter, "JSON failed: {error}"),
        }
    }
}

impl std::error::Error for AppError {}

impl From<reqwest::Error> for AppError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error)
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

impl From<keyring::Error> for AppError {
    fn from(error: keyring::Error) -> Self {
        Self::Keyring(error)
    }
}

impl From<std::io::Error> for AppError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for AppError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

pub(crate) struct Backend {
    db: Mutex<Connection>,
    client: Client,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Account {
    pub(crate) id: String,
    pub(crate) name: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectionState {
    pub(crate) configured: bool,
    pub(crate) account: Option<Account>,
    pub(crate) token_present: bool,
    pub(crate) storage: &'static str,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct InventorySummary {
    pub(crate) workers: usize,
    pub(crate) pages: usize,
    pub(crate) d1: usize,
    pub(crate) r2: usize,
    pub(crate) kv: usize,
    pub(crate) zones: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct MetricSummary {
    pub(crate) worker_requests: u64,
    pub(crate) worker_errors: u64,
    pub(crate) worker_cpu_time_ms: Option<f64>,
    pub(crate) d1_queries: u64,
    pub(crate) d1_latency_p90_ms: Option<f64>,
    pub(crate) d1_rows_read: u64,
    pub(crate) d1_rows_written: u64,
    pub(crate) d1_storage_bytes: u64,
    pub(crate) r2_operations: u64,
    pub(crate) r2_storage_bytes: u64,
    pub(crate) r2_class_a_operations: u64,
    pub(crate) r2_class_b_operations: u64,
    pub(crate) kv_operations: u64,
    pub(crate) kv_storage_bytes: u64,
    pub(crate) kv_read_operations: u64,
    pub(crate) kv_write_operations: u64,
    pub(crate) kv_delete_operations: u64,
    pub(crate) kv_list_operations: u64,
    pub(crate) cost_usd: Option<f64>,
    pub(crate) cost_currency: Option<String>,
    pub(crate) cost_source: Option<String>,
    pub(crate) cost_base_usd: Option<f64>,
    pub(crate) cost_overage_usd: Option<f64>,
    pub(crate) billing_rows: Option<usize>,
    pub(crate) zone_requests: u64,
    pub(crate) zone_security_events: u64,
    pub(crate) zone_cache_hit_ratio: Option<f64>,
    pub(crate) audit_events: u64,
    pub(crate) audit_failures: u64,
    pub(crate) logpush_jobs: u64,
    pub(crate) logpush_enabled_jobs: u64,
    pub(crate) worker_log_events: u64,
    pub(crate) worker_trace_events: u64,
    pub(crate) worker_observability_fields: u64,
    pub(crate) worker_observability_destinations: u64,
    pub(crate) collector_api_calls: u64,
    pub(crate) collector_api_errors: u64,
    pub(crate) collector_api_p95_ms: Option<f64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ZoneSummary {
    pub(crate) zones: usize,
    pub(crate) active_zones: usize,
    pub(crate) requests: u64,
    pub(crate) security_events: u64,
    pub(crate) cache_hit_ratio: Option<f64>,
    pub(crate) top_hosts: Vec<ZoneHostMetric>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ZoneHostMetric {
    pub(crate) host: String,
    pub(crate) requests: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct AuditSummary {
    pub(crate) events: usize,
    pub(crate) failures: usize,
    pub(crate) api_events: usize,
    pub(crate) dashboard_events: usize,
    pub(crate) recent: Vec<AuditEvent>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct AuditEvent {
    pub(crate) action: String,
    pub(crate) actor: String,
    pub(crate) interface: String,
    pub(crate) method: String,
    pub(crate) result: String,
    pub(crate) timestamp: Option<String>,
    pub(crate) resource: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct LogpushSummary {
    pub(crate) jobs: usize,
    pub(crate) enabled_jobs: usize,
    pub(crate) workers_trace_jobs: usize,
    pub(crate) audit_jobs: usize,
    pub(crate) disabled_jobs: usize,
    pub(crate) recent: Vec<LogpushJob>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct LogpushJob {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) dataset: String,
    pub(crate) enabled: bool,
    pub(crate) destination: String,
    pub(crate) kind: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct WorkerObservabilitySummary {
    pub(crate) log_events: u64,
    pub(crate) error_events: u64,
    pub(crate) traces: u64,
    pub(crate) fields: usize,
    pub(crate) destinations: usize,
    pub(crate) configured_workers: usize,
    pub(crate) full_sample_workers: usize,
    pub(crate) live_tail_available: bool,
    pub(crate) recent_events: Vec<WorkerTelemetryEvent>,
    pub(crate) gaps: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct WorkerTelemetryEvent {
    pub(crate) service: String,
    pub(crate) message: String,
    pub(crate) timestamp: Option<String>,
    pub(crate) level: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct CollectorTelemetry {
    pub(crate) api_calls: u64,
    pub(crate) api_errors: u64,
    pub(crate) api_duration_p95_ms: Option<f64>,
    pub(crate) rate_limit_remaining: Option<String>,
    pub(crate) last_ray_id: Option<String>,
    pub(crate) endpoints: Vec<CollectorEndpoint>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct CollectorEndpoint {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) status: Option<u16>,
    pub(crate) duration_ms: f64,
    pub(crate) ok: bool,
    #[serde(default)]
    pub(crate) optional: bool,
    pub(crate) ray_id: Option<String>,
    pub(crate) error: Option<String>,
}

#[derive(Default)]
struct CollectorTelemetryBuilder {
    endpoints: Vec<CollectorEndpoint>,
    rate_limit_remaining: Option<String>,
    last_ray_id: Option<String>,
}

impl CollectorTelemetryBuilder {
    #[allow(clippy::too_many_arguments)]
    fn record(
        &mut self,
        method: &str,
        path: &str,
        status: Option<u16>,
        duration_ms: f64,
        ok: bool,
        headers: Option<&HeaderMap>,
        error: Option<String>,
    ) {
        let ray_id = headers.and_then(|items| header_value(items, "cf-ray"));
        if let Some(value) = headers.and_then(|items| header_value(items, "x-ratelimit-remaining"))
        {
            self.rate_limit_remaining = Some(value);
        }
        if let Some(value) = &ray_id {
            self.last_ray_id = Some(value.clone());
        }
        let optional = !ok && is_optional_collector_endpoint(path, status, error.as_deref());

        self.endpoints.push(CollectorEndpoint {
            method: method.into(),
            path: path.into(),
            status,
            duration_ms,
            ok,
            optional,
            ray_id,
            error,
        });
    }

    fn finish(self) -> CollectorTelemetry {
        let mut durations = self
            .endpoints
            .iter()
            .map(|endpoint| endpoint.duration_ms)
            .filter(|duration| duration.is_finite())
            .collect::<Vec<_>>();
        durations
            .sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
        let p95 = if durations.is_empty() {
            None
        } else {
            let index = ((durations.len() as f64 - 1.0) * 0.95).round() as usize;
            durations.get(index).copied()
        };
        let api_calls = self.endpoints.len() as u64;
        let api_errors = self
            .endpoints
            .iter()
            .filter(|endpoint| !endpoint.ok && !endpoint.optional)
            .count() as u64;

        CollectorTelemetry {
            api_calls,
            api_errors,
            api_duration_p95_ms: p95,
            rate_limit_remaining: self.rate_limit_remaining,
            last_ray_id: self.last_ray_id,
            endpoints: self
                .endpoints
                .into_iter()
                .rev()
                .take(120)
                .collect::<Vec<_>>(),
        }
    }
}

fn is_optional_collector_endpoint(path: &str, status: Option<u16>, error: Option<&str>) -> bool {
    let scoped_status = matches!(status, Some(401 | 403));
    if scoped_status && (path.contains("/logpush/jobs") || path.contains("/workers/observability/"))
    {
        return true;
    }

    path == "/graphql"
        && error
            .map(|message| {
                is_graphql_retention_error(message)
                    || message.to_lowercase().contains("firewalleventsadaptive")
            })
            .unwrap_or(false)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResourceRow {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) status: String,
    pub(crate) primary_metric: String,
    pub(crate) secondary_metric: String,
    pub(crate) updated_at: Option<String>,
    pub(crate) bindings: Option<Vec<ResourceBinding>>,
    pub(crate) observability: Option<ResourceObservability>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResourceBinding {
    pub(crate) name: String,
    pub(crate) binding_type: Option<String>,
    pub(crate) resource_kind: Option<String>,
    pub(crate) resource_id: Option<String>,
    pub(crate) resource_name: Option<String>,
}

impl<'de> Deserialize<'de> for ResourceBinding {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct ResourceBindingDetail {
            name: String,
            binding_type: Option<String>,
            resource_kind: Option<String>,
            resource_id: Option<String>,
            resource_name: Option<String>,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum ResourceBindingCompat {
            Name(String),
            Detail(ResourceBindingDetail),
        }

        Ok(match ResourceBindingCompat::deserialize(deserializer)? {
            ResourceBindingCompat::Name(name) => ResourceBinding {
                name,
                binding_type: None,
                resource_kind: None,
                resource_id: None,
                resource_name: None,
            },
            ResourceBindingCompat::Detail(detail) => ResourceBinding {
                name: detail.name,
                binding_type: detail.binding_type,
                resource_kind: detail.resource_kind,
                resource_id: detail.resource_id,
                resource_name: detail.resource_name,
            },
        })
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ResourceObservability {
    pub(crate) enabled: Option<bool>,
    pub(crate) logs_enabled: Option<bool>,
    pub(crate) traces_enabled: Option<bool>,
    pub(crate) invocation_logs: Option<bool>,
    pub(crate) head_sampling_rate: Option<f64>,
    pub(crate) logpush: Option<bool>,
    pub(crate) destinations: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsagePanel {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) value: String,
    pub(crate) detail: String,
    pub(crate) tone: String,
    pub(crate) points: Vec<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ServiceHealth {
    pub(crate) id: String,
    pub(crate) service: String,
    pub(crate) status: String,
    pub(crate) label: String,
    pub(crate) detail: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DashboardSnapshot {
    pub(crate) generated_at: String,
    pub(crate) range: String,
    pub(crate) live: bool,
    #[serde(default)]
    pub(crate) cached: bool,
    #[serde(default)]
    pub(crate) expires_at: Option<String>,
    pub(crate) account: Option<Account>,
    pub(crate) inventory: InventorySummary,
    pub(crate) metrics: MetricSummary,
    pub(crate) resources: Vec<ResourceRow>,
    pub(crate) usage_panels: Vec<UsagePanel>,
    pub(crate) health: Vec<ServiceHealth>,
    pub(crate) issues: Vec<String>,
    #[serde(default)]
    pub(crate) zones: ZoneSummary,
    #[serde(default)]
    pub(crate) audit: AuditSummary,
    #[serde(default)]
    pub(crate) logpush: LogpushSummary,
    #[serde(default)]
    pub(crate) observability: WorkerObservabilitySummary,
    #[serde(default)]
    pub(crate) collector: CollectorTelemetry,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectResult {
    pub(crate) accounts: Vec<Account>,
    pub(crate) connection: Option<ConnectionState>,
    pub(crate) snapshot: Option<DashboardSnapshot>,
}

#[derive(Clone, Debug)]
struct ZoneInfo {
    id: String,
    name: String,
    status: String,
}

#[derive(Default)]
struct WorkerMetric {
    requests: u64,
    errors: u64,
    cpu_p50: Option<f64>,
    cpu_p99: Option<f64>,
}

struct MetricsResult {
    summary: MetricSummary,
    workers: HashMap<String, WorkerMetric>,
    points: HashMap<String, Vec<(String, u64)>>,
    issues: Vec<String>,
}

struct CostProjection {
    total: f64,
    base: f64,
    overage: f64,
}

impl Backend {
    pub(crate) fn new() -> AppResult<Self> {
        Ok(Self {
            db: Mutex::new(open_database()?),
            client: Client::builder()
                .user_agent(concat!("cedar/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(AppError::Http)?,
        })
    }

    pub(crate) fn new_visual_qa() -> AppResult<Self> {
        let db = Connection::open_in_memory()?;
        initialize_database(&db)?;
        Ok(Self {
            db: Mutex::new(db),
            client: Client::builder()
                .user_agent(concat!("cedar/", env!("CARGO_PKG_VERSION"), " visual-qa"))
                .build()
                .map_err(AppError::Http)?,
        })
    }

    pub(crate) fn get_connection(&self) -> AppResult<ConnectionState> {
        let token_present = read_token().is_ok();
        let account = read_account(self)?;

        Ok(ConnectionState {
            configured: token_present && account.is_some(),
            account,
            token_present,
            storage: "os-keychain",
        })
    }

    pub(crate) async fn discover_accounts(&self, token: &str) -> AppResult<Vec<Account>> {
        let mut collector = CollectorTelemetryBuilder::default();
        fetch_accounts(&self.client, token, &mut collector).await
    }

    pub(crate) async fn connect_cloudflare(
        &self,
        token: &str,
        account_id: Option<&str>,
    ) -> AppResult<ConnectResult> {
        if token.trim().is_empty() {
            return Err(AppError::Message(
                "Cloudflare API token is required.".into(),
            ));
        }

        let mut collector = CollectorTelemetryBuilder::default();
        let accounts = fetch_accounts(&self.client, token.trim(), &mut collector).await?;
        if accounts.is_empty() {
            return Err(AppError::Message(
                "The token is valid, but no Cloudflare accounts were returned.".into(),
            ));
        }

        let account = match account_id {
            Some(id) if !id.is_empty() => accounts
                .iter()
                .find(|candidate| candidate.id == id)
                .cloned()
                .ok_or_else(|| {
                    AppError::Message("Selected account was not returned by Cloudflare.".into())
                })?,
            _ => accounts[0].clone(),
        };

        write_token(token.trim())?;
        write_account(self, &account)?;

        let snapshot = collect_snapshot(self, token.trim(), &account, "24h").await?;

        Ok(ConnectResult {
            accounts,
            connection: Some(ConnectionState {
                configured: true,
                account: Some(account),
                token_present: true,
                storage: "os-keychain",
            }),
            snapshot: Some(snapshot),
        })
    }

    pub(crate) fn get_cached_snapshot(&self, range: &str) -> AppResult<Option<DashboardSnapshot>> {
        let Some(account) = read_account(self)? else {
            return Ok(None);
        };
        read_latest_snapshot(self, &account, normalize_range(range))
    }

    pub(crate) async fn sync_cloudflare(
        &self,
        range: &str,
        force_refresh: bool,
    ) -> AppResult<DashboardSnapshot> {
        let token = read_token()?;
        let account = read_account(self)?.ok_or_else(|| {
            AppError::Message("Connect a Cloudflare account before syncing.".into())
        })?;
        sync_snapshot(self, &token, &account, range, force_refresh).await
    }

    pub(crate) fn clear_connection(&self) -> AppResult<()> {
        let _ = keyring_entry().delete_credential();
        let db = self
            .db
            .lock()
            .map_err(|_| AppError::Message("Local database lock failed.".into()))?;
        db.execute(
            "DELETE FROM config WHERE key IN ('account_id', 'account_name')",
            [],
        )?;
        Ok(())
    }

    pub(crate) fn preference(&self, key: &str) -> AppResult<Option<String>> {
        let db = self
            .db
            .lock()
            .map_err(|_| AppError::Message("Local database lock failed.".into()))?;
        get_config(&db, key)
    }

    pub(crate) fn set_preference(&self, key: &str, value: &str) -> AppResult<()> {
        let db = self
            .db
            .lock()
            .map_err(|_| AppError::Message("Local database lock failed.".into()))?;
        set_config(&db, key, value)
    }
}

async fn sync_snapshot(
    state: &Backend,
    token: &str,
    account: &Account,
    range: &str,
    force_refresh: bool,
) -> AppResult<DashboardSnapshot> {
    let normalized_range = normalize_range(range);

    if !force_refresh && let Some(snapshot) = read_fresh_snapshot(state, account, normalized_range)?
    {
        return Ok(snapshot);
    }

    collect_snapshot(state, token, account, normalized_range).await
}

async fn collect_snapshot(
    state: &Backend,
    token: &str,
    account: &Account,
    normalized_range: &str,
) -> AppResult<DashboardSnapshot> {
    let mut collector_builder = CollectorTelemetryBuilder::default();
    let mut issues = Vec::new();
    let inventory = collect_inventory(
        &state.client,
        token,
        account,
        &mut issues,
        &mut collector_builder,
    )
    .await;
    let mut metrics = collect_metrics(
        &state.client,
        token,
        &account.id,
        normalized_range,
        &mut collector_builder,
    )
    .await;
    issues.extend(metrics.issues);
    let zones = collect_zone_analytics(
        &state.client,
        token,
        account,
        &inventory.zones,
        normalized_range,
        &mut issues,
        &mut collector_builder,
    )
    .await;
    let audit = collect_audit_logs(
        &state.client,
        token,
        account,
        normalized_range,
        &mut issues,
        &mut collector_builder,
    )
    .await;
    let logpush = collect_logpush(
        &state.client,
        token,
        account,
        &inventory.zones,
        &mut issues,
        &mut collector_builder,
    )
    .await;
    let observability = collect_workers_observability(
        &state.client,
        token,
        account,
        &inventory.resources,
        &logpush,
        normalized_range,
        &mut issues,
        &mut collector_builder,
    )
    .await;
    metrics.summary.zone_requests = zones.requests;
    metrics.summary.zone_security_events = zones.security_events;
    metrics.summary.zone_cache_hit_ratio = zones.cache_hit_ratio;
    metrics.summary.audit_events = audit.events as u64;
    metrics.summary.audit_failures = audit.failures as u64;
    metrics.summary.logpush_jobs = logpush.jobs as u64;
    metrics.summary.logpush_enabled_jobs = logpush.enabled_jobs as u64;
    metrics.summary.worker_log_events = observability.log_events;
    metrics.summary.worker_trace_events = observability.traces;
    metrics.summary.worker_observability_fields = observability.fields as u64;
    metrics.summary.worker_observability_destinations = observability.destinations as u64;
    let collector = collector_builder.finish();
    metrics.summary.collector_api_calls = collector.api_calls;
    metrics.summary.collector_api_errors = collector.api_errors;
    metrics.summary.collector_api_p95_ms = collector.api_duration_p95_ms;

    let resources = merge_resources(&inventory.resources, &metrics.workers);
    let summary = inventory.summary.clone();

    let generated_at = Utc::now();
    let snapshot = DashboardSnapshot {
        generated_at: generated_at.to_rfc3339(),
        range: normalized_range.to_string(),
        live: true,
        cached: false,
        expires_at: Some(cache_expires_at(generated_at, normalized_range)),
        account: Some(account.clone()),
        inventory: summary,
        usage_panels: usage_panels(&metrics.summary, &metrics.points),
        health: health_panels(&issues),
        resources,
        metrics: metrics.summary,
        issues,
        zones,
        audit,
        logpush,
        observability,
        collector,
    };

    save_snapshot(state, account, &snapshot)?;
    Ok(snapshot)
}

struct InventoryResult {
    summary: InventorySummary,
    resources: Vec<ResourceRow>,
    zones: Vec<ZoneInfo>,
}

async fn collect_inventory(
    client: &Client,
    token: &str,
    account: &Account,
    issues: &mut Vec<String>,
    collector: &mut CollectorTelemetryBuilder,
) -> InventoryResult {
    let workers = match cf_get_paged_result_items(
        client,
        token,
        &format!("/accounts/{}/workers/scripts", account.id),
        collector,
    )
    .await
    {
        Ok(items) => {
            let value = result_items_value(items);
            hydrate_worker_bindings(
                client,
                token,
                account,
                parse_workers(&value),
                issues,
                collector,
            )
            .await
        }
        Err(error) => {
            issues.push(format!("Workers inventory failed: {error}"));
            Vec::new()
        }
    };

    let pages = match cf_get_result_items(
        client,
        token,
        &format!("/accounts/{}/pages/projects", account.id),
        collector,
    )
    .await
    {
        Ok(items) => parse_pages(&result_items_value(items)),
        Err(error) => {
            issues.push(format!("Pages inventory failed: {error}"));
            Vec::new()
        }
    };

    let d1 = match cf_get_paged_result_items(
        client,
        token,
        &format!("/accounts/{}/d1/database", account.id),
        collector,
    )
    .await
    {
        Ok(items) => parse_d1(&result_items_value(items)),
        Err(error) => {
            issues.push(format!("D1 inventory failed: {error}"));
            Vec::new()
        }
    };

    let r2 = match cf_get_paged_result_items(
        client,
        token,
        &format!("/accounts/{}/r2/buckets", account.id),
        collector,
    )
    .await
    {
        Ok(items) => parse_r2(&result_items_value(items)),
        Err(error) => {
            issues.push(format!("R2 inventory failed: {error}"));
            Vec::new()
        }
    };

    let kv = match cf_get_paged_result_items(
        client,
        token,
        &format!("/accounts/{}/storage/kv/namespaces", account.id),
        collector,
    )
    .await
    {
        Ok(items) => parse_kv(&result_items_value(items)),
        Err(error) => {
            issues.push(format!("KV inventory failed: {error}"));
            Vec::new()
        }
    };

    let zones = match cf_get_paged_result_items(
        client,
        token,
        &format!("/zones?account.id={}", account.id),
        collector,
    )
    .await
    {
        Ok(items) => parse_zones(&result_items_value(items)),
        Err(error) => {
            issues.push(format!("Zone inventory failed: {error}"));
            Vec::new()
        }
    };

    let mut resources = Vec::new();
    resources.extend(workers.clone());
    resources.extend(pages.clone());
    resources.extend(d1.clone());
    resources.extend(r2.clone());
    resources.extend(kv.clone());

    InventoryResult {
        summary: InventorySummary {
            workers: workers.len(),
            pages: pages.len(),
            d1: d1.len(),
            r2: r2.len(),
            kv: kv.len(),
            zones: zones.len(),
        },
        resources,
        zones,
    }
}

async fn collect_metrics(
    client: &Client,
    token: &str,
    account_id: &str,
    range: &str,
    collector: &mut CollectorTelemetryBuilder,
) -> MetricsResult {
    let mut result = MetricsResult {
        summary: MetricSummary::default(),
        workers: HashMap::new(),
        points: HashMap::new(),
        issues: Vec::new(),
    };

    let window = metric_window(range);

    let worker_variables = json!({
        "accountTag": account_id,
        "datetimeStart": window.start_time,
        "datetimeEnd": window.end_time
    });

    match graphql(
        client,
        token,
        workers_query(),
        worker_variables.clone(),
        collector,
    )
    .await
    {
        Ok(value) => parse_workers_metrics(&value, &mut result),
        Err(cpu_error) => match graphql(
            client,
            token,
            workers_query_without_cpu(),
            worker_variables,
            collector,
        )
        .await
        {
            Ok(value) => {
                parse_workers_metrics(&value, &mut result);
                result.issues.push(format!(
                    "Workers CPU time unavailable for cost projection: {cpu_error}"
                ));
            }
            Err(error) => result
                .issues
                .push(format!("Workers metrics failed: {error}")),
        },
    }

    match graphql(
        client,
        token,
        d1_query(),
        json!({
            "accountTag": account_id,
            "start": window.start_date,
            "end": window.end_date
        }),
        collector,
    )
    .await
    {
        Ok(value) => parse_d1_metrics(&value, &mut result),
        Err(error) => result.issues.push(format!("D1 metrics failed: {error}")),
    }

    match graphql(
        client,
        token,
        r2_query(),
        json!({
            "accountTag": account_id,
            "startDate": window.start_time,
            "endDate": window.end_time
        }),
        collector,
    )
    .await
    {
        Ok(value) => parse_r2_metrics(&value, &mut result),
        Err(error) => result.issues.push(format!("R2 metrics failed: {error}")),
    }

    match graphql(
        client,
        token,
        kv_query(),
        json!({
            "accountTag": account_id,
            "start": window.start_date,
            "end": window.end_date
        }),
        collector,
    )
    .await
    {
        Ok(value) => parse_kv_metrics(&value, &mut result),
        Err(error) => result.issues.push(format!("KV metrics failed: {error}")),
    }

    apply_cost(&mut result, &window);
    result
}

async fn collect_zone_analytics(
    client: &Client,
    token: &str,
    _account: &Account,
    zones: &[ZoneInfo],
    range: &str,
    issues: &mut Vec<String>,
    collector: &mut CollectorTelemetryBuilder,
) -> ZoneSummary {
    let mut summary = ZoneSummary {
        zones: zones.len(),
        active_zones: zones.iter().filter(|zone| zone.status == "active").count(),
        ..ZoneSummary::default()
    };
    if zones.is_empty() {
        return summary;
    }

    let window = metric_window(range);
    let mut first_traffic_error: Option<String> = None;
    let mut first_security_error: Option<String> = None;
    let mut traffic_path_access_denials = 0usize;
    let mut security_path_access_denials = 0usize;
    let mut traffic_successful_zones = 0usize;
    let mut security_successful_zones = 0usize;
    let mut host_totals: HashMap<String, u64> = HashMap::new();
    let queried_zones = zones.len();

    for batch in zone_batches(zones) {
        let batch_label = zone_batch_label(&batch);
        let mut traffic_batch_succeeded = false;
        let mut traffic_batch_failed = false;
        let mut traffic_batch_path_access_denied = false;

        match zone_graphql_slices_with_retention_retry(
            client,
            token,
            zone_traffic_query(),
            &batch,
            &window,
            collector,
        )
        .await
        {
            Ok(values) => {
                for value in values {
                    traffic_batch_succeeded = true;
                    parse_zone_traffic(&value, &mut summary, &mut host_totals);
                }
            }
            Err(error) => {
                let error = error.to_string();
                traffic_batch_failed = true;
                if is_graphql_path_access_error(&error) {
                    traffic_batch_path_access_denied = true;
                }
                if first_traffic_error.is_none() {
                    first_traffic_error = Some(format!("{batch_label}: GraphQL {error}"));
                }
            }
        }

        if traffic_batch_succeeded {
            traffic_successful_zones += batch.len();
        } else if traffic_batch_failed && traffic_batch_path_access_denied {
            traffic_path_access_denials += batch.len();
        }

        let mut security_batch_succeeded = false;
        let mut security_batch_failed = false;
        let mut security_batch_path_access_denied = false;

        match zone_graphql_slices_with_retention_retry(
            client,
            token,
            zone_security_query(),
            &batch,
            &window,
            collector,
        )
        .await
        {
            Ok(values) => {
                for value in values {
                    security_batch_succeeded = true;
                    parse_zone_security(&value, &mut summary);
                }
            }
            Err(error) => {
                let aggregate_error = error.to_string();
                match zone_graphql_slices_with_retention_retry(
                    client,
                    token,
                    zone_security_events_query(),
                    &batch,
                    &window,
                    collector,
                )
                .await
                {
                    Ok(values) => {
                        for value in values {
                            security_batch_succeeded = true;
                            parse_zone_security_events(&value, &mut summary);
                        }
                    }
                    Err(fallback_error) => {
                        let fallback_error = fallback_error.to_string();
                        security_batch_failed = true;
                        if is_graphql_path_access_error(&aggregate_error)
                            && is_graphql_path_access_error(&fallback_error)
                        {
                            security_batch_path_access_denied = true;
                        }
                        if first_security_error.is_none() {
                            first_security_error = Some(format!(
                                "{batch_label}: GraphQL aggregate {aggregate_error}; raw Security Events fallback {fallback_error}"
                            ));
                        }
                    }
                }
            }
        }

        if security_batch_succeeded {
            security_successful_zones += batch.len();
        } else if security_batch_failed && security_batch_path_access_denied {
            security_path_access_denials += batch.len();
        }
    }

    if let Some(error) = first_traffic_error {
        if traffic_path_access_denials == queried_zones {
            issues.push(format!(
                "Zone analytics unauthorized for all queried zones: create the token with Zone Analytics Read on all zone resources; first failure: {error}"
            ));
        } else if traffic_path_access_denials > 0 {
            issues.push(format!(
                "Zone analytics partial: {traffic_successful_zones}/{queried_zones} zones returned traffic data; add Zone Analytics Read for the remaining zone resources; first failure: {error}"
            ));
        } else {
            issues.push(format!("Zone analytics failed: {error}"));
        }
    }
    if let Some(error) = first_security_error {
        if security_path_access_denials == queried_zones {
            issues.push(format!(
                "Optional zone Security Events GraphQL unavailable for all queried zones; Cloudflare denied firewallEventsAdaptiveGroups and firewallEventsAdaptive for this token, resource scope, or plan. Traffic-level Security Analytics continues through httpRequestsAdaptiveGroups; first failure: {error}"
            ));
        } else if security_path_access_denials > 0 {
            issues.push(format!(
                "Optional zone Security Events GraphQL partial: {security_successful_zones}/{queried_zones} zones returned Security Events; first failure: {error}"
            ));
        } else {
            issues.push(format!(
                "Optional zone Security Events GraphQL failed: {error}"
            ));
        }
    }
    let mut top_hosts = host_totals
        .into_iter()
        .map(|(host, requests)| ZoneHostMetric { host, requests })
        .collect::<Vec<_>>();
    top_hosts.sort_by(|left, right| {
        right
            .requests
            .cmp(&left.requests)
            .then_with(|| left.host.cmp(&right.host))
    });
    top_hosts.truncate(8);
    summary.top_hosts = top_hosts;
    summary
}

async fn collect_audit_logs(
    client: &Client,
    token: &str,
    account: &Account,
    range: &str,
    issues: &mut Vec<String>,
    collector: &mut CollectorTelemetryBuilder,
) -> AuditSummary {
    let window = metric_window(range);
    let path = format!(
        "/accounts/{}/logs/audit?since={}&before={}",
        account.id,
        percent_encode_query_value(&window.start_time),
        percent_encode_query_value(&window.end_time)
    );

    match cf_get_paged_result_items(client, token, &path, collector).await {
        Ok(items) => parse_audit_logs(&result_items_value(items)),
        Err(error) => {
            issues.push(format!(
                "Audit Logs failed or needs account audit-log scope: {error}"
            ));
            AuditSummary::default()
        }
    }
}

async fn collect_logpush(
    client: &Client,
    token: &str,
    account: &Account,
    zones: &[ZoneInfo],
    issues: &mut Vec<String>,
    collector: &mut CollectorTelemetryBuilder,
) -> LogpushSummary {
    let mut jobs = Vec::new();
    let mut zone_error: Option<(String, String)> = None;
    let mut zone_failures = 0usize;
    let mut zone_successes = 0usize;

    if let Ok(items) = cf_get_paged_result_items(
        client,
        token,
        &format!("/accounts/{}/logpush/jobs", account.id),
        collector,
    )
    .await
    {
        jobs.extend(parse_logpush_jobs(&result_items_value(items), "account"));
    }

    for zone in zones {
        match cf_get_paged_result_items(
            client,
            token,
            &format!("/zones/{}/logpush/jobs", zone.id),
            collector,
        )
        .await
        {
            Ok(value) => {
                zone_successes += 1;
                jobs.extend(parse_logpush_jobs(&result_items_value(value), "zone"));
            }
            Err(error) => {
                zone_failures += 1;
                if zone_error.is_none() {
                    zone_error = Some((zone.name.clone(), error.to_string()));
                }
            }
        }
    }

    if zone_successes == 0 {
        if let Some((zone_name, error)) = zone_error {
            issues.push(format!(
                "Optional zone Logpush inventory unavailable: add zone-scoped Logs Write for {zone_name} and any monitored zones: {error}"
            ));
        }
    } else if let Some((zone_name, error)) = zone_error {
        issues.push(format!(
            "Optional zone Logpush inventory partial: {zone_failures} zone(s) failed; first failure was {zone_name}: {error}"
        ));
    }

    summarize_logpush(jobs)
}

#[allow(clippy::too_many_arguments)]
async fn collect_workers_observability(
    client: &Client,
    token: &str,
    account: &Account,
    resources: &[ResourceRow],
    logpush: &LogpushSummary,
    range: &str,
    issues: &mut Vec<String>,
    collector: &mut CollectorTelemetryBuilder,
) -> WorkerObservabilitySummary {
    let mut summary = summarize_worker_observability_config(resources, logpush);
    let window = metric_window(range);

    match cf_get(
        client,
        token,
        &format!(
            "/accounts/{}/workers/observability/destinations",
            account.id
        ),
        collector,
    )
    .await
    {
        Ok(value) => {
            let destinations = result_array(&value).len();
            summary.destinations = destinations.max(summary.destinations);
        }
        Err(error) => summary.gaps.push(format!(
            "Optional Workers Observability destination check unavailable; add Workers Observability Read or Write to inspect export destinations: {error}"
        )),
    }

    let keys_body = json!({
        "timeframe": {
            "from": window.start_ms,
            "to": window.end_ms
        },
        "datasets": ["cloudflare-workers"],
        "filters": [],
        "limit": 200
    });
    match cf_post(
        client,
        token,
        &format!(
            "/accounts/{}/workers/observability/telemetry/keys",
            account.id
        ),
        keys_body,
        collector,
    )
    .await
    {
        Ok(value) => {
            summary.fields = parse_observability_key_count(&value);
            summary.live_tail_available = true;
        }
        Err(error) => summary.gaps.push(format!(
            "Optional Workers Observability key discovery unavailable; Cloudflare may require Workers Observability Write for telemetry keys: {error}"
        )),
    }

    let body = json!({
        "queryId": "cedar-workers-observability",
        "view": "events",
        "timeframe": {
            "from": window.start_ms,
            "to": window.end_ms
        },
        "dry": true,
        "limit": 50,
        "parameters": {
            "datasets": ["cloudflare-workers"],
            "filters": [],
            "calculations": [],
            "groupBys": []
        }
    });
    match cf_post(
        client,
        token,
        &format!(
            "/accounts/{}/workers/observability/telemetry/query",
            account.id
        ),
        body,
        collector,
    )
    .await
    {
        Ok(value) => parse_worker_telemetry(&value, &mut summary),
        Err(error) => summary.gaps.push(format!(
            "Optional Workers Observability telemetry query unavailable: Cloudflare requires Workers Observability Write for ad hoc telemetry queries: {error}"
        )),
    }

    if !summary.gaps.is_empty() {
        issues.push(format!(
            "Workers Observability optional checks scoped: {}",
            summary
                .gaps
                .iter()
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }

    summary
}

fn apply_cost(result: &mut MetricsResult, window: &MetricWindow) {
    apply_estimated_cost(&mut result.summary, window.range.as_str());
}

fn apply_estimated_cost(summary: &mut MetricSummary, range: &str) {
    let projection = estimate_workers_paid_plan_cost(summary, range);
    summary.cost_usd = Some(projection.total);
    summary.cost_currency = Some("USD".into());
    summary.cost_source = Some("paid-plan-projection".into());
    summary.cost_base_usd = Some(projection.base);
    summary.cost_overage_usd = Some(projection.overage);
}

struct MetricWindow {
    range: String,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    start_time: String,
    end_time: String,
    start_ms: i64,
    end_ms: i64,
    start_date: String,
    end_date: String,
}

#[derive(Clone, Debug)]
struct MetricTimeSlice {
    start_time: String,
    end_time: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CloudflareRetentionLimit {
    duration: Duration,
    label: String,
}

fn metric_window(range: &str) -> MetricWindow {
    let end: DateTime<Utc> = Utc::now();
    let start = match range {
        "30d" => end - Duration::days(30),
        "7d" => end - Duration::days(7),
        _ => end - Duration::hours(24),
    };
    let start_time = start.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let end_time = end.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let start_ms = start.timestamp_millis();
    let end_ms = end.timestamp_millis();
    let start_date = start.date_naive().to_string();
    let end_date = end.date_naive().to_string();

    MetricWindow {
        range: range.to_string(),
        start,
        end,
        start_time,
        end_time,
        start_ms,
        end_ms,
        start_date,
        end_date,
    }
}

fn metric_time_slices_between(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    max_duration: Duration,
) -> Vec<MetricTimeSlice> {
    if start >= end || max_duration <= Duration::zero() {
        return Vec::new();
    }

    let mut slices = Vec::new();
    let mut cursor = start;
    while cursor < end {
        let candidate_end = cursor + max_duration;
        let slice_end = if candidate_end < end {
            candidate_end
        } else {
            end
        };
        slices.push(MetricTimeSlice {
            start_time: cursor.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            end_time: slice_end.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        });
        cursor = slice_end;
    }
    slices
}

fn zone_batches(zones: &[ZoneInfo]) -> Vec<Vec<&ZoneInfo>> {
    zones
        .iter()
        .collect::<Vec<_>>()
        .chunks(ZONE_GRAPHQL_BATCH_SIZE)
        .map(|chunk| chunk.to_vec())
        .collect()
}

fn zone_batch_variables(batch: &[&ZoneInfo], slice: &MetricTimeSlice) -> Value {
    let zone_tags = batch.iter().map(|zone| zone.id.clone()).collect::<Vec<_>>();

    json!({
        "zoneTags": zone_tags,
        "datetimeStart": slice.start_time,
        "datetimeEnd": slice.end_time,
    })
}

fn zone_batch_label(batch: &[&ZoneInfo]) -> String {
    let names = batch
        .iter()
        .take(3)
        .map(|zone| zone.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    if names.is_empty() {
        "zones".into()
    } else if batch.len() > 3 {
        format!("{names}, +{} zones", batch.len() - 3)
    } else {
        names
    }
}

async fn zone_graphql_slices_with_retention_retry(
    client: &Client,
    token: &str,
    query: &'static str,
    batch: &[&ZoneInfo],
    window: &MetricWindow,
    collector: &mut CollectorTelemetryBuilder,
) -> AppResult<Vec<Value>> {
    let mut start = window.start;

    for _ in 0..3 {
        let slices = metric_time_slices_between(start, window.end, Duration::days(1));
        match run_zone_graphql_slices(client, token, query, batch, &slices, collector).await {
            Ok(values) => return Ok(values),
            Err(error) => {
                let error_text = error.to_string();
                let Some(limit) = parse_cloudflare_retention_limit(&error_text) else {
                    return Err(error);
                };
                let next_start = retention_limited_start(window, &limit);
                if next_start <= start || next_start >= window.end {
                    return Err(error);
                }
                start = next_start;
            }
        }
    }

    Err(AppError::Message(
        "Cloudflare GraphQL retention retry limit reached".into(),
    ))
}

async fn run_zone_graphql_slices(
    client: &Client,
    token: &str,
    query: &'static str,
    batch: &[&ZoneInfo],
    slices: &[MetricTimeSlice],
    collector: &mut CollectorTelemetryBuilder,
) -> AppResult<Vec<Value>> {
    let mut values = Vec::with_capacity(slices.len());
    for slice in slices {
        values.push(
            graphql(
                client,
                token,
                query,
                zone_batch_variables(batch, slice),
                collector,
            )
            .await?,
        );
    }
    Ok(values)
}

fn retention_limited_start(
    window: &MetricWindow,
    limit: &CloudflareRetentionLimit,
) -> DateTime<Utc> {
    let guard = if limit.duration > Duration::minutes(5) {
        Duration::minutes(5)
    } else {
        Duration::zero()
    };
    let limited_start = window.end - limit.duration + guard;
    if limited_start > window.start {
        limited_start
    } else {
        window.start
    }
}

fn parse_cloudflare_retention_limit(error: &str) -> Option<CloudflareRetentionLimit> {
    let needle = "cannot request data older than ";
    let lower = error.to_lowercase();
    let index = lower.find(needle)?;
    let mut chars = error[index + needle.len()..].chars().peekable();
    let mut total_seconds = 0i64;
    let mut number = String::new();
    let mut label = String::new();

    while let Some(character) = chars.peek().copied() {
        if character.is_ascii_digit() {
            number.push(character);
            label.push(character);
            chars.next();
            continue;
        }

        if number.is_empty() {
            break;
        }

        let seconds_per_unit = match character.to_ascii_lowercase() {
            'w' => 7 * 24 * 60 * 60,
            'd' => 24 * 60 * 60,
            'h' => 60 * 60,
            'm' => 60,
            's' => 1,
            _ => break,
        };
        total_seconds += number.parse::<i64>().ok()? * seconds_per_unit;
        number.clear();
        label.push(character);
        chars.next();
    }

    if total_seconds <= 0 || !number.is_empty() {
        return None;
    }

    Some(CloudflareRetentionLimit {
        duration: Duration::seconds(total_seconds),
        label,
    })
}

fn is_graphql_retention_error(error: &str) -> bool {
    parse_cloudflare_retention_limit(error).is_some()
}

async fn fetch_accounts(
    client: &Client,
    token: &str,
    collector: &mut CollectorTelemetryBuilder,
) -> AppResult<Vec<Account>> {
    let result = cf_get_paged_result_items(client, token, "/accounts", collector).await?;

    let accounts = result
        .iter()
        .filter_map(|item| {
            Some(Account {
                id: string_field(item, "id")?,
                name: string_field(item, "name").unwrap_or_else(|| "Unnamed account".into()),
            })
        })
        .collect();

    Ok(accounts)
}

async fn cf_get_paged_result_items(
    client: &Client,
    token: &str,
    path: &str,
    collector: &mut CollectorTelemetryBuilder,
) -> AppResult<Vec<Value>> {
    cf_get_paged_result_items_with_page_size(client, token, path, CF_REST_PAGE_SIZE, collector)
        .await
}

async fn cf_get_paged_result_items_with_page_size(
    client: &Client,
    token: &str,
    path: &str,
    page_size: usize,
    collector: &mut CollectorTelemetryBuilder,
) -> AppResult<Vec<Value>> {
    let mut items = Vec::new();
    let mut previous_page_items: Option<Vec<Value>> = None;
    let mut page = 1usize;

    loop {
        if page > CF_REST_MAX_PAGES {
            return Err(AppError::Message(format!(
                "Cloudflare pagination exceeded {CF_REST_MAX_PAGES} pages for {path}."
            )));
        }

        let page_path = paged_path(path, page, page_size);
        let value = cf_get(client, token, &page_path, collector).await?;
        let page_items = result_array(&value)
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();

        if page > 1 && previous_page_items.as_ref() == Some(&page_items) {
            break;
        }

        let page_count = page_items.len();
        items.extend(page_items.clone());

        let has_next_page = match result_total_pages(&value) {
            Some(total_pages) => page < total_pages,
            None => page_count == page_size,
        };
        if !has_next_page || page_count == 0 {
            break;
        }

        previous_page_items = Some(page_items);
        page += 1;
    }

    Ok(items)
}

async fn cf_get_result_items(
    client: &Client,
    token: &str,
    path: &str,
    collector: &mut CollectorTelemetryBuilder,
) -> AppResult<Vec<Value>> {
    let value = cf_get(client, token, path, collector).await?;
    Ok(result_array(&value).into_iter().cloned().collect())
}

fn paged_path(path: &str, page: usize, per_page: usize) -> String {
    let separator = if path.contains('?') { "&" } else { "?" };
    format!("{path}{separator}page={page}&per_page={per_page}")
}

fn result_total_pages(value: &Value) -> Option<usize> {
    value
        .pointer("/result_info/total_pages")
        .and_then(Value::as_u64)
        .and_then(|pages| usize::try_from(pages).ok())
        .filter(|pages| *pages > 0)
}

async fn cf_get(
    client: &Client,
    token: &str,
    path: &str,
    collector: &mut CollectorTelemetryBuilder,
) -> AppResult<Value> {
    let started = Instant::now();
    let response = match client
        .get(format!("{CF_API_BASE}{path}"))
        .bearer_auth(token)
        .header("Accept", "application/json")
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            collector.record(
                "GET",
                path,
                None,
                elapsed_ms(started),
                false,
                None,
                Some(error.to_string()),
            );
            return Err(AppError::Http(error));
        }
    };

    let status = response.status();
    let headers = response.headers().clone();
    let value: Value = match response.json().await {
        Ok(value) => value,
        Err(error) => {
            collector.record(
                "GET",
                path,
                Some(status.as_u16()),
                elapsed_ms(started),
                false,
                Some(&headers),
                Some(error.to_string()),
            );
            return Err(AppError::Http(error));
        }
    };
    if !status.is_success() || value.get("success").and_then(Value::as_bool) == Some(false) {
        let message = cf_error_message(status.as_u16(), &value);
        collector.record(
            "GET",
            path,
            Some(status.as_u16()),
            elapsed_ms(started),
            false,
            Some(&headers),
            Some(message.clone()),
        );
        return Err(AppError::Message(message));
    }
    collector.record(
        "GET",
        path,
        Some(status.as_u16()),
        elapsed_ms(started),
        true,
        Some(&headers),
        None,
    );
    Ok(value)
}

async fn cf_post(
    client: &Client,
    token: &str,
    path: &str,
    body: Value,
    collector: &mut CollectorTelemetryBuilder,
) -> AppResult<Value> {
    let started = Instant::now();
    let response = match client
        .post(format!("{CF_API_BASE}{path}"))
        .bearer_auth(token)
        .header("Accept", "application/json")
        .json(&body)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            collector.record(
                "POST",
                path,
                None,
                elapsed_ms(started),
                false,
                None,
                Some(error.to_string()),
            );
            return Err(AppError::Http(error));
        }
    };

    let status = response.status();
    let headers = response.headers().clone();
    let body_text = match response.text().await {
        Ok(body_text) => body_text,
        Err(error) => {
            collector.record(
                "POST",
                path,
                Some(status.as_u16()),
                elapsed_ms(started),
                false,
                Some(&headers),
                Some(error.to_string()),
            );
            return Err(AppError::Http(error));
        }
    };
    let value: Value = match serde_json::from_str(&body_text) {
        Ok(value) => value,
        Err(error) => {
            let message = format!(
                "HTTP {} returned a non-JSON response: {}",
                status.as_u16(),
                truncate_response_body(&body_text)
            );
            collector.record(
                "POST",
                path,
                Some(status.as_u16()),
                elapsed_ms(started),
                false,
                Some(&headers),
                Some(format!("{message}; {error}")),
            );
            return Err(AppError::Message(message));
        }
    };
    if !status.is_success() || value.get("success").and_then(Value::as_bool) == Some(false) {
        let message = cf_error_message(status.as_u16(), &value);
        collector.record(
            "POST",
            path,
            Some(status.as_u16()),
            elapsed_ms(started),
            false,
            Some(&headers),
            Some(message.clone()),
        );
        return Err(AppError::Message(message));
    }
    collector.record(
        "POST",
        path,
        Some(status.as_u16()),
        elapsed_ms(started),
        true,
        Some(&headers),
        None,
    );
    Ok(value)
}

async fn graphql(
    client: &Client,
    token: &str,
    query: &'static str,
    variables: Value,
    collector: &mut CollectorTelemetryBuilder,
) -> AppResult<Value> {
    let started = Instant::now();
    let response = match client
        .post(format!("{CF_API_BASE}/graphql"))
        .bearer_auth(token)
        .header("Accept", "application/json")
        .json(&json!({ "query": query, "variables": variables }))
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            collector.record(
                "POST",
                "/graphql",
                None,
                elapsed_ms(started),
                false,
                None,
                Some(error.to_string()),
            );
            return Err(AppError::Http(error));
        }
    };

    let status = response.status();
    let headers = response.headers().clone();
    let value: Value = match response.json().await {
        Ok(value) => value,
        Err(error) => {
            collector.record(
                "POST",
                "/graphql",
                Some(status.as_u16()),
                elapsed_ms(started),
                false,
                Some(&headers),
                Some(error.to_string()),
            );
            return Err(AppError::Http(error));
        }
    };

    if !status.is_success() {
        let message = cf_error_message(status.as_u16(), &value);
        collector.record(
            "POST",
            "/graphql",
            Some(status.as_u16()),
            elapsed_ms(started),
            false,
            Some(&headers),
            Some(message.clone()),
        );
        return Err(AppError::Message(message));
    }

    if let Some(errors) = value.get("errors").and_then(Value::as_array) {
        let message = errors
            .iter()
            .filter_map(graphql_error_detail)
            .collect::<Vec<_>>()
            .join("; ");
        if !message.is_empty() {
            collector.record(
                "POST",
                "/graphql",
                Some(status.as_u16()),
                elapsed_ms(started),
                false,
                Some(&headers),
                Some(message.clone()),
            );
            return Err(AppError::Message(message));
        }
    }

    collector.record(
        "POST",
        "/graphql",
        Some(status.as_u16()),
        elapsed_ms(started),
        true,
        Some(&headers),
        None,
    );
    Ok(value)
}

fn graphql_error_detail(item: &Value) -> Option<String> {
    let message = item.get("message").and_then(Value::as_str)?;
    let path = item
        .get("path")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.as_str()
                        .map(ToOwned::to_owned)
                        .or_else(|| item.as_u64().map(|number| number.to_string()))
                        .or_else(|| item.as_i64().map(|number| number.to_string()))
                })
                .collect::<Vec<_>>()
                .join(".")
        })
        .filter(|path| !path.is_empty());

    Some(match path {
        Some(path) => format!("{path}: {message}"),
        None => message.to_string(),
    })
}

fn cf_error_message(status: u16, value: &Value) -> String {
    let errors = value
        .get("errors")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.get("message")
                        .and_then(Value::as_str)
                        .or_else(|| item.as_str())
                })
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_default();

    if errors.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {errors}")
    }
}

fn truncate_response_body(value: &str) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return "empty body".into();
    }

    let preview = normalized.chars().take(180).collect::<String>();
    if normalized.chars().count() > 180 {
        format!("{preview}...")
    } else {
        preview
    }
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

fn header_value(headers: &HeaderMap, key: &str) -> Option<String> {
    headers
        .get(key)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
}

fn parse_workers(value: &Value) -> Vec<ResourceRow> {
    result_array(value)
        .iter()
        .filter_map(|item| {
            let name = string_field(item, "id").or_else(|| string_field(item, "name"))?;
            let updated_at = string_field(item, "modified_on")
                .or_else(|| string_field(item, "created_on"))
                .or_else(|| string_field(item, "updated_on"));
            let bindings = parse_worker_bindings(item);
            Some(ResourceRow {
                id: name.clone(),
                name,
                kind: "worker".into(),
                status: "unknown".into(),
                primary_metric: "No metric sync yet".into(),
                secondary_metric: if bindings.is_empty() {
                    "No bindings discovered".into()
                } else {
                    format!("{} bindings", bindings.len())
                },
                updated_at,
                bindings: Some(bindings),
                observability: parse_worker_observability(item),
            })
        })
        .collect()
}

async fn hydrate_worker_bindings(
    client: &Client,
    token: &str,
    account: &Account,
    workers: Vec<ResourceRow>,
    issues: &mut Vec<String>,
    collector: &mut CollectorTelemetryBuilder,
) -> Vec<ResourceRow> {
    let mut hydrated = Vec::with_capacity(workers.len());
    let mut failures = Vec::new();

    for mut worker in workers {
        let encoded_name = percent_encode_path_segment(&worker.name);
        match cf_get(
            client,
            token,
            &format!(
                "/accounts/{}/workers/scripts/{encoded_name}/settings",
                account.id
            ),
            collector,
        )
        .await
        {
            Ok(value) => {
                let settings = value.get("result").unwrap_or(&value);
                let bindings = parse_worker_bindings(settings);
                worker.secondary_metric = if bindings.is_empty() {
                    "No bindings discovered".into()
                } else {
                    format!("{} bindings", bindings.len())
                };
                worker.bindings = Some(bindings);
                worker.observability =
                    parse_worker_observability(settings).or(worker.observability);
            }
            Err(error) => failures.push(format!("{} ({error})", worker.name)),
        }

        hydrated.push(worker);
    }

    if !failures.is_empty() {
        let failure_count = failures.len();
        let preview = failures.into_iter().take(3).collect::<Vec<_>>().join(", ");
        issues.push(format!(
            "Worker binding metadata failed for {} script(s): {preview}",
            failure_count
        ));
    }

    hydrated
}

fn parse_worker_observability(item: &Value) -> Option<ResourceObservability> {
    let observability = item
        .get("observability")
        .or_else(|| item.get("observability_config"))
        .or_else(|| item.get("observabilityConfig"))
        .unwrap_or(item);
    let logs = observability
        .get("logs")
        .or_else(|| observability.get("worker_logs"))
        .or_else(|| observability.get("workers_logs"))
        .unwrap_or(&Value::Null);
    let traces = observability
        .get("traces")
        .or_else(|| observability.get("trace_events"))
        .or_else(|| observability.get("tracing"))
        .unwrap_or(&Value::Null);
    let enabled =
        bool_field(observability, "enabled").or_else(|| bool_field(item, "observability_enabled"));
    let logs_enabled = bool_field(logs, "enabled")
        .or_else(|| bool_field(observability, "logs_enabled"))
        .or_else(|| bool_field(observability, "logsEnabled"));
    let traces_enabled = bool_field(traces, "enabled")
        .or_else(|| bool_field(observability, "traces_enabled"))
        .or_else(|| bool_field(observability, "tracesEnabled"));
    let invocation_logs = bool_field(observability, "invocation_logs")
        .or_else(|| bool_field(observability, "invocationLogs"))
        .or_else(|| bool_field(logs, "invocation_logs"));
    let head_sampling_rate = first_f64_field(
        observability,
        &[
            "head_sampling_rate",
            "headSamplingRate",
            "sample_rate",
            "sampling_rate",
        ],
    );
    let logpush = bool_field(item, "logpush")
        .or_else(|| bool_field(observability, "logpush"))
        .or_else(|| bool_field(logs, "logpush"));

    if enabled.is_none()
        && logs_enabled.is_none()
        && traces_enabled.is_none()
        && invocation_logs.is_none()
        && head_sampling_rate.is_none()
        && logpush.is_none()
    {
        return None;
    }

    Some(ResourceObservability {
        enabled,
        logs_enabled,
        traces_enabled,
        invocation_logs,
        head_sampling_rate,
        logpush,
        destinations: Vec::new(),
    })
}

fn parse_worker_bindings(item: &Value) -> Vec<ResourceBinding> {
    item.get("bindings")
        .and_then(Value::as_array)
        .map(|bindings| {
            bindings
                .iter()
                .filter_map(parse_worker_binding)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn parse_worker_binding(binding: &Value) -> Option<ResourceBinding> {
    let name = string_field(binding, "name")?;
    let binding_type = string_field(binding, "type");
    let resource_kind = binding_type
        .as_deref()
        .and_then(binding_resource_kind)
        .map(str::to_string);
    let resource_id = binding_resource_id(binding, resource_kind.as_deref());
    let resource_name = binding_resource_name(binding, resource_kind.as_deref());

    Some(ResourceBinding {
        name,
        binding_type,
        resource_kind,
        resource_id,
        resource_name,
    })
}

fn binding_resource_kind(binding_type: &str) -> Option<&'static str> {
    let normalized = normalized_action(binding_type);

    if normalized.contains("kv") && normalized.contains("namespace") {
        Some("kv")
    } else if normalized.contains("d1") {
        Some("d1")
    } else if normalized.contains("r2") {
        Some("r2")
    } else if normalized.contains("service") || normalized.contains("worker") {
        Some("worker")
    } else {
        None
    }
}

fn binding_resource_id(binding: &Value, resource_kind: Option<&str>) -> Option<String> {
    match resource_kind {
        Some("kv") => first_string_field(binding, &["namespace_id", "id"]),
        Some("d1") => first_string_field(binding, &["database_id", "id"]),
        Some("r2") => first_string_field(binding, &["bucket_name", "bucket"]),
        Some("worker") => first_string_field(binding, &["service", "script_name", "script"]),
        _ => first_string_field(
            binding,
            &["resource_id", "namespace_id", "database_id", "id"],
        ),
    }
}

fn binding_resource_name(binding: &Value, resource_kind: Option<&str>) -> Option<String> {
    match resource_kind {
        Some("kv") => first_string_field(binding, &["namespace_name", "namespace", "title"]),
        Some("d1") => first_string_field(binding, &["database_name", "database"]),
        Some("r2") => first_string_field(binding, &["bucket_name", "bucket"]),
        Some("worker") => first_string_field(binding, &["service", "script_name", "script"]),
        _ => first_string_field(
            binding,
            &[
                "resource_name",
                "namespace_name",
                "database_name",
                "bucket_name",
            ],
        ),
    }
}

fn parse_pages(value: &Value) -> Vec<ResourceRow> {
    result_array(value)
        .iter()
        .filter_map(|item| {
            let id = string_field(item, "id").or_else(|| string_field(item, "name"))?;
            let name = string_field(item, "name").unwrap_or_else(|| id.clone());
            let branch =
                string_field(item, "production_branch").unwrap_or_else(|| "production".into());
            Some(ResourceRow {
                id,
                name,
                kind: "page".into(),
                status: "unknown".into(),
                primary_metric: "Project discovered".into(),
                secondary_metric: branch,
                updated_at: string_field(item, "created_on"),
                bindings: None,
                observability: None,
            })
        })
        .collect()
}

fn parse_d1(value: &Value) -> Vec<ResourceRow> {
    result_array(value)
        .iter()
        .filter_map(|item| {
            let id = string_field(item, "uuid").or_else(|| string_field(item, "id"))?;
            let name = string_field(item, "name").unwrap_or_else(|| id.clone());
            Some(ResourceRow {
                id,
                name,
                kind: "d1".into(),
                status: "unknown".into(),
                primary_metric: "Database discovered".into(),
                secondary_metric: string_field(item, "version")
                    .unwrap_or_else(|| "D1 database".into()),
                updated_at: string_field(item, "created_at"),
                bindings: None,
                observability: None,
            })
        })
        .collect()
}

fn parse_r2(value: &Value) -> Vec<ResourceRow> {
    result_array(value)
        .iter()
        .filter_map(|item| {
            let name = string_field(item, "name")?;
            Some(ResourceRow {
                id: name.clone(),
                name,
                kind: "r2".into(),
                status: "unknown".into(),
                primary_metric: "Bucket discovered".into(),
                secondary_metric: string_field(item, "location")
                    .unwrap_or_else(|| "R2 bucket".into()),
                updated_at: string_field(item, "creation_date")
                    .or_else(|| string_field(item, "created_at")),
                bindings: None,
                observability: None,
            })
        })
        .collect()
}

fn parse_kv(value: &Value) -> Vec<ResourceRow> {
    result_array(value)
        .iter()
        .filter_map(|item| {
            let id = string_field(item, "id")?;
            let name = string_field(item, "title")
                .or_else(|| string_field(item, "name"))
                .unwrap_or_else(|| id.clone());
            Some(ResourceRow {
                id,
                name,
                kind: "kv".into(),
                status: "unknown".into(),
                primary_metric: "Namespace discovered".into(),
                secondary_metric: "Workers KV".into(),
                updated_at: None,
                bindings: None,
                observability: None,
            })
        })
        .collect()
}

fn parse_zones(value: &Value) -> Vec<ZoneInfo> {
    result_array(value)
        .iter()
        .filter_map(|item| {
            Some(ZoneInfo {
                id: string_field(item, "id")?,
                name: string_field(item, "name").unwrap_or_else(|| "unnamed-zone".into()),
                status: string_field(item, "status").unwrap_or_else(|| "unknown".into()),
            })
        })
        .collect()
}

fn merge_resources(
    resources: &[ResourceRow],
    workers: &HashMap<String, WorkerMetric>,
) -> Vec<ResourceRow> {
    resources
        .iter()
        .map(|resource| {
            let mut item = resource.clone();
            if item.kind == "worker" {
                if let Some(metric) = workers.get(&item.name) {
                    item.status = if metric.errors > 0 {
                        "warning".into()
                    } else {
                        "healthy".into()
                    };
                    item.primary_metric = compact(metric.requests);
                    item.secondary_metric = format!("{} errors", compact(metric.errors));
                } else {
                    item.status = "quiet".into();
                    item.primary_metric = "0 requests".into();
                }
            } else if item.status == "unknown" {
                item.status = "healthy".into();
            }
            item
        })
        .collect()
}

fn parse_zone_traffic(
    value: &Value,
    summary: &mut ZoneSummary,
    host_totals: &mut HashMap<String, u64>,
) {
    let mut cached = summary
        .cache_hit_ratio
        .map(|ratio| ratio * summary.requests as f64);

    for row in zone_dataset(value, "httpRequestsAdaptiveGroups") {
        let sum = row.get("sum").unwrap_or(&Value::Null);
        let requests = adaptive_request_count(row);
        summary.requests += requests;
        if sum.get("cachedRequests").is_some() {
            *cached.get_or_insert(0.0) += u64_field(sum, "cachedRequests") as f64;
        }

        if let Some(host) = row.get("dimensions").and_then(|dimensions| {
            first_string_field(
                dimensions,
                &["clientRequestHTTPHost", "clientRequestHost", "host"],
            )
        }) {
            *host_totals.entry(host).or_default() += requests;
        }
    }

    if summary.requests > 0
        && let Some(cached) = cached
    {
        summary.cache_hit_ratio = Some(cached / summary.requests as f64);
    }
}

fn parse_zone_security(value: &Value, summary: &mut ZoneSummary) {
    for row in zone_dataset(value, "firewallEventsAdaptiveGroups") {
        summary.security_events += row
            .get("count")
            .and_then(|count| {
                count
                    .as_u64()
                    .or_else(|| count.as_f64().map(|number| number as u64))
            })
            .unwrap_or_else(|| {
                row.get("sum")
                    .map(|sum| u64_field(sum, "requests"))
                    .unwrap_or_default()
            });
    }
}

fn parse_zone_security_events(value: &Value, summary: &mut ZoneSummary) {
    summary.security_events += zone_dataset(value, "firewallEventsAdaptive").len() as u64;
}

fn adaptive_request_count(row: &Value) -> u64 {
    row.get("count")
        .and_then(|count| {
            count
                .as_u64()
                .or_else(|| count.as_f64().map(|number| number as u64))
        })
        .unwrap_or_else(|| {
            row.get("sum")
                .map(|sum| u64_field(sum, "visits"))
                .unwrap_or_default()
        })
}

fn parse_audit_logs(value: &Value) -> AuditSummary {
    let mut summary = AuditSummary::default();
    let events = result_array(value);
    summary.events = events.len();

    for item in events {
        let action = first_string_field(item, &["action", "action_type", "actionType", "event"])
            .unwrap_or_else(|| "unknown action".into());
        let actor = item
            .get("actor")
            .and_then(|actor| first_string_field(actor, &["email", "name", "id", "type"]))
            .or_else(|| first_string_field(item, &["actor_email", "actorEmail", "actor"]))
            .unwrap_or_else(|| "unknown actor".into());
        let interface =
            first_string_field(item, &["interface", "source", "actor_type", "actorType"])
                .unwrap_or_else(|| "unknown".into());
        let method = first_string_field(item, &["method", "resource_type", "resourceType"])
            .unwrap_or_else(|| "unknown".into());
        let result = first_string_field(item, &["result", "status", "outcome"])
            .unwrap_or_else(|| "unknown".into());
        let timestamp = first_string_field(item, &["when", "timestamp", "created_on", "createdAt"]);
        let resource = item
            .get("resource")
            .and_then(|resource| first_string_field(resource, &["name", "id", "type"]))
            .or_else(|| {
                first_string_field(
                    item,
                    &["resource_id", "resourceId", "resource_name", "resourceName"],
                )
            });

        let normalized_interface = normalized_action(&interface);
        if normalized_interface.contains("api") {
            summary.api_events += 1;
        }
        if normalized_interface.contains("dash") || normalized_interface.contains("ui") {
            summary.dashboard_events += 1;
        }
        let normalized_result = normalized_action(&result);
        if normalized_result.contains("fail")
            || normalized_result.contains("error")
            || normalized_result.contains("deny")
        {
            summary.failures += 1;
        }

        if summary.recent.len() < 10 {
            summary.recent.push(AuditEvent {
                action,
                actor,
                interface,
                method,
                result,
                timestamp,
                resource,
            });
        }
    }

    summary
}

fn parse_logpush_jobs(value: &Value, kind: &str) -> Vec<LogpushJob> {
    result_array(value)
        .iter()
        .filter_map(|item| {
            let id = field_as_string(item, "id").or_else(|| field_as_string(item, "job_id"))?;
            let dataset = first_string_field(item, &["dataset", "logpull_options", "name"])
                .unwrap_or_else(|| "unknown".into());
            let name = first_string_field(item, &["name", "job_name"])
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| format!("{kind} {dataset}"));
            let destination = first_string_field(
                item,
                &["destination_conf", "destinationConf", "destination"],
            )
            .map(|value| sanitize_destination(&value))
            .unwrap_or_else(|| "destination hidden".into());

            Some(LogpushJob {
                id,
                name,
                dataset,
                enabled: bool_field(item, "enabled").unwrap_or(false),
                destination,
                kind: Some(kind.into()),
            })
        })
        .collect()
}

fn summarize_logpush(jobs: Vec<LogpushJob>) -> LogpushSummary {
    let enabled_jobs = jobs.iter().filter(|job| job.enabled).count();
    let workers_trace_jobs = jobs
        .iter()
        .filter(|job| normalized_action(&job.dataset).contains("workerstrace"))
        .count();
    let audit_jobs = jobs
        .iter()
        .filter(|job| normalized_action(&job.dataset).contains("audit"))
        .count();
    let disabled_jobs = jobs.len().saturating_sub(enabled_jobs);

    LogpushSummary {
        jobs: jobs.len(),
        enabled_jobs,
        workers_trace_jobs,
        audit_jobs,
        disabled_jobs,
        recent: jobs.into_iter().take(10).collect(),
    }
}

fn summarize_worker_observability_config(
    resources: &[ResourceRow],
    logpush: &LogpushSummary,
) -> WorkerObservabilitySummary {
    let mut summary = WorkerObservabilitySummary {
        destinations: logpush.workers_trace_jobs,
        ..WorkerObservabilitySummary::default()
    };

    for worker in resources
        .iter()
        .filter(|resource| resource.kind == "worker")
    {
        let Some(observability) = &worker.observability else {
            continue;
        };

        if observability.enabled == Some(true)
            || observability.logs_enabled == Some(true)
            || observability.traces_enabled == Some(true)
            || observability.invocation_logs == Some(true)
        {
            summary.configured_workers += 1;
        }
        if observability.head_sampling_rate.unwrap_or_default() >= 1.0 {
            summary.full_sample_workers += 1;
        }
        summary.destinations += observability.destinations.len();
    }

    summary
}

fn parse_observability_key_count(value: &Value) -> usize {
    let direct = result_array(value).len();
    if direct > 0 {
        return direct;
    }

    ["keys", "items", "fields", "result"]
        .iter()
        .find_map(|key| value.get(key).and_then(Value::as_array).map(Vec::len))
        .unwrap_or_default()
}

fn parse_worker_telemetry(value: &Value, summary: &mut WorkerObservabilitySummary) {
    let rows = telemetry_rows(value);
    for item in rows {
        let metadata = item.get("$metadata");
        let workers = item.get("$workers");
        let message = first_string_field(
            item,
            &["message", "event", "outcome", "exception", "scriptName"],
        )
        .or_else(|| {
            metadata.and_then(|metadata| {
                first_string_field(
                    metadata,
                    &["message", "error", "spanName", "eventType", "outcome"],
                )
            })
        })
        .or_else(|| {
            workers.and_then(|workers| {
                first_string_field(workers, &["message", "eventType", "outcome", "scriptName"])
            })
        })
        .unwrap_or_else(|| "telemetry event".into());
        let service = first_string_field(item, &["scriptName", "script", "service", "worker"])
            .or_else(|| {
                metadata.and_then(|metadata| {
                    first_string_field(metadata, &["service", "scriptName", "cloudService"])
                })
            })
            .or_else(|| {
                workers.and_then(|workers| first_string_field(workers, &["scriptName", "service"]))
            })
            .unwrap_or_else(|| "worker".into());
        let level =
            first_string_field(item, &["level", "severity", "outcome", "status"]).or_else(|| {
                metadata.and_then(|metadata| {
                    first_string_field(metadata, &["level", "severity", "outcome", "error"])
                })
            });
        let timestamp = first_string_field(item, &["timestamp", "datetime", "eventTimestamp"])
            .or_else(|| field_as_string(item, "timestamp"))
            .or_else(|| metadata.and_then(|metadata| field_as_string(metadata, "timestamp")));
        let normalized_level = level.as_deref().map(normalized_action).unwrap_or_default();
        let normalized_item = normalized_action(&format!(
            "{message} {}",
            field_as_string(item, "type").unwrap_or_default()
        ));
        let metadata_error = metadata
            .and_then(|metadata| first_string_field(metadata, &["error", "exception"]))
            .map(|error| normalized_action(&error))
            .unwrap_or_default();

        summary.log_events += 1;
        if normalized_level.contains("error")
            || normalized_item.contains("error")
            || normalized_item.contains("exception")
            || !metadata_error.is_empty()
        {
            summary.error_events += 1;
        }
        if normalized_item.contains("trace")
            || item.get("traceId").is_some()
            || item.get("spanId").is_some()
            || metadata
                .and_then(|metadata| metadata.get("traceId"))
                .is_some()
            || metadata
                .and_then(|metadata| metadata.get("spanId"))
                .is_some()
        {
            summary.traces += 1;
        }
        if summary.recent_events.len() < 10 {
            summary.recent_events.push(WorkerTelemetryEvent {
                service,
                message,
                timestamp,
                level,
            });
        }
    }
}

fn parse_workers_metrics(value: &Value, result: &mut MetricsResult) {
    for row in account_dataset(value, "workersInvocationsAdaptive") {
        let sum = row.get("sum").unwrap_or(&Value::Null);
        let requests = u64_field(sum, "requests");
        let errors = u64_field(sum, "errors");
        result.summary.worker_requests += requests;
        result.summary.worker_errors += errors;
        if let Some(cpu_time_us) = f64_field(sum, "cpuTimeUs") {
            *result.summary.worker_cpu_time_ms.get_or_insert(0.0) += cpu_time_us / 1000.0;
        }

        let script = row
            .get("dimensions")
            .and_then(|dimensions| string_field(dimensions, "scriptName"))
            .unwrap_or_else(|| "unknown-worker".into());
        let quantiles = row.get("quantiles").unwrap_or(&Value::Null);
        let metric = result.workers.entry(script).or_default();
        metric.requests += requests;
        metric.errors += errors;
        metric.cpu_p50 = metric
            .cpu_p50
            .or_else(|| f64_field(quantiles, "cpuTimeP50"));
        metric.cpu_p99 = metric
            .cpu_p99
            .or_else(|| f64_field(quantiles, "cpuTimeP99"));
        push_metric_point(&mut result.points, "workers", row, requests);
    }
}

fn parse_d1_metrics(value: &Value, result: &mut MetricsResult) {
    let mut p90_values = Vec::new();
    for row in account_dataset(value, "d1AnalyticsAdaptiveGroups") {
        let sum = row.get("sum").unwrap_or(&Value::Null);
        let queries = u64_field(sum, "readQueries") + u64_field(sum, "writeQueries");
        result.summary.d1_queries += queries;
        result.summary.d1_rows_read += u64_field(sum, "rowsRead");
        result.summary.d1_rows_written += u64_field(sum, "rowsWritten");

        if let Some(p90) = row
            .get("quantiles")
            .and_then(|quantiles| f64_field(quantiles, "queryBatchTimeMsP90"))
        {
            p90_values.push(p90);
        }
        push_metric_point(&mut result.points, "d1", row, queries);
    }

    if !p90_values.is_empty() {
        result.summary.d1_latency_p90_ms =
            Some(p90_values.iter().sum::<f64>() / p90_values.len() as f64);
    }

    for row in account_dataset(value, "d1StorageAdaptiveGroups") {
        let max = row.get("max").unwrap_or(&Value::Null);
        result.summary.d1_storage_bytes = result
            .summary
            .d1_storage_bytes
            .max(u64_field(max, "databaseSizeBytes"));
    }
}

fn parse_r2_metrics(value: &Value, result: &mut MetricsResult) {
    for row in account_dataset(value, "r2OperationsAdaptiveGroups") {
        let requests = row
            .get("sum")
            .map(|sum| u64_field(sum, "requests"))
            .unwrap_or_default();
        result.summary.r2_operations += requests;
        let action = row
            .get("dimensions")
            .and_then(|dimensions| string_field(dimensions, "actionType"))
            .unwrap_or_default();
        match r2_operation_class(&action) {
            Some("class-a") => result.summary.r2_class_a_operations += requests,
            Some("class-b") => result.summary.r2_class_b_operations += requests,
            _ => {}
        }
        push_metric_point(&mut result.points, "r2", row, requests);
    }

    for row in account_dataset(value, "r2StorageAdaptiveGroups") {
        let max = row.get("max").unwrap_or(&Value::Null);
        let payload = u64_field(max, "payloadSize");
        let metadata = u64_field(max, "metadataSize");
        result.summary.r2_storage_bytes = result.summary.r2_storage_bytes.max(payload + metadata);
    }
}

fn parse_kv_metrics(value: &Value, result: &mut MetricsResult) {
    for row in account_dataset(value, "kvOperationsAdaptiveGroups") {
        let requests = row
            .get("sum")
            .map(|sum| u64_field(sum, "requests"))
            .unwrap_or_default();
        result.summary.kv_operations += requests;
        let action = row
            .get("dimensions")
            .and_then(|dimensions| string_field(dimensions, "actionType"))
            .unwrap_or_default();
        match kv_operation_class(&action) {
            Some("read") => result.summary.kv_read_operations += requests,
            Some("write") => result.summary.kv_write_operations += requests,
            Some("delete") => result.summary.kv_delete_operations += requests,
            Some("list") => result.summary.kv_list_operations += requests,
            _ => {}
        }
        push_metric_point(&mut result.points, "kv", row, requests);
    }

    for row in account_dataset(value, "kvStorageAdaptiveGroups") {
        let max = row.get("max").unwrap_or(&Value::Null);
        result.summary.kv_storage_bytes = result
            .summary
            .kv_storage_bytes
            .max(u64_field(max, "byteCount"));
    }
}

fn r2_operation_class(action: &str) -> Option<&'static str> {
    let normalized = normalized_action(action);
    if normalized.is_empty() {
        return None;
    }

    if normalized.contains("delete") || normalized.contains("abortmultipart") {
        return Some("free");
    }

    if normalized.contains("head") || normalized.contains("get") || normalized.contains("usage") {
        return Some("class-b");
    }

    if normalized.contains("put")
        || normalized.contains("list")
        || normalized.contains("copy")
        || normalized.contains("upload")
        || normalized.contains("multipart")
        || normalized.contains("lifecycle")
    {
        return Some("class-a");
    }

    None
}

fn kv_operation_class(action: &str) -> Option<&'static str> {
    let normalized = normalized_action(action);
    if normalized.is_empty() {
        return None;
    }

    if normalized.contains("delete") || normalized.contains("remove") {
        Some("delete")
    } else if normalized.contains("list") {
        Some("list")
    } else if normalized.contains("write")
        || normalized.contains("put")
        || normalized.contains("set")
    {
        Some("write")
    } else if normalized.contains("read") || normalized.contains("get") {
        Some("read")
    } else {
        None
    }
}

fn normalized_action(action: &str) -> String {
    action
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn account_dataset<'a>(value: &'a Value, dataset: &str) -> Vec<&'a Value> {
    value
        .pointer("/data/viewer/accounts/0")
        .and_then(|account| account.get(dataset))
        .and_then(Value::as_array)
        .map(|items| items.iter().collect())
        .unwrap_or_default()
}

fn zone_dataset<'a>(value: &'a Value, dataset: &str) -> Vec<&'a Value> {
    value
        .pointer("/data/viewer/zones")
        .and_then(Value::as_array)
        .map(|zones| {
            zones
                .iter()
                .filter_map(|zone| zone.get(dataset).and_then(Value::as_array))
                .flat_map(|items| items.iter())
                .collect()
        })
        .unwrap_or_default()
}

fn telemetry_rows(value: &Value) -> Vec<&Value> {
    let direct = result_array(value);
    if !direct.is_empty() {
        return direct;
    }

    for pointer in [
        "/result/events/events",
        "/result/traces/traces",
        "/result/requests/requests",
        "/result/agents/agents",
        "/result/events",
        "/result/logs",
        "/result/traces",
        "/result/data",
        "/result/rows",
        "/events",
        "/logs",
        "/traces",
        "/data",
        "/rows",
    ] {
        if let Some(items) = value.pointer(pointer).and_then(Value::as_array) {
            return items.iter().collect();
        }
    }

    Vec::new()
}

fn push_metric_point(
    points: &mut HashMap<String, Vec<(String, u64)>>,
    key: &str,
    row: &Value,
    value: u64,
) {
    if value == 0 {
        return;
    }
    let Some(bucket) = row.get("dimensions").and_then(|dimensions| {
        first_string_field(
            dimensions,
            &["datetime", "date", "datetimeHour", "eventDatetime"],
        )
    }) else {
        return;
    };

    points.entry(key.into()).or_default().push((bucket, value));
}

fn workers_query() -> &'static str {
    r#"
    query GetWorkersAnalytics($accountTag: string!, $datetimeStart: string, $datetimeEnd: string) {
      viewer {
        accounts(filter: { accountTag: $accountTag }) {
          workersInvocationsAdaptive(
            limit: 10000
            filter: { datetime_geq: $datetimeStart, datetime_leq: $datetimeEnd }
          ) {
            sum { subrequests requests errors cpuTimeUs }
            quantiles { cpuTimeP50 cpuTimeP99 }
            dimensions { datetime scriptName status }
          }
        }
      }
    }
    "#
}

fn workers_query_without_cpu() -> &'static str {
    r#"
    query GetWorkersAnalytics($accountTag: string!, $datetimeStart: string, $datetimeEnd: string) {
      viewer {
        accounts(filter: { accountTag: $accountTag }) {
          workersInvocationsAdaptive(
            limit: 10000
            filter: { datetime_geq: $datetimeStart, datetime_leq: $datetimeEnd }
          ) {
            sum { subrequests requests errors }
            quantiles { cpuTimeP50 cpuTimeP99 }
            dimensions { datetime scriptName status }
          }
        }
      }
    }
    "#
}

fn d1_query() -> &'static str {
    r#"
    query D1AccountAnalytics($accountTag: string!, $start: Date, $end: Date) {
      viewer {
        accounts(filter: { accountTag: $accountTag }) {
          d1AnalyticsAdaptiveGroups(
            limit: 10000
            filter: { date_geq: $start, date_leq: $end }
          ) {
            sum { readQueries writeQueries rowsRead rowsWritten }
            quantiles { queryBatchTimeMsP90 }
            dimensions { date }
          }
          d1StorageAdaptiveGroups(
            limit: 10000
            filter: { date_geq: $start, date_leq: $end }
          ) {
            max { databaseSizeBytes }
          }
        }
      }
    }
    "#
}

fn r2_query() -> &'static str {
    r#"
    query R2AccountAnalytics($accountTag: string!, $startDate: Time, $endDate: Time) {
      viewer {
        accounts(filter: { accountTag: $accountTag }) {
          r2OperationsAdaptiveGroups(
            limit: 10000
            filter: { datetime_geq: $startDate, datetime_leq: $endDate }
          ) {
            sum { requests }
            dimensions { datetime actionType }
          }
          r2StorageAdaptiveGroups(
            limit: 10000
            filter: { datetime_geq: $startDate, datetime_leq: $endDate }
            orderBy: [datetime_DESC]
          ) {
            max { objectCount uploadCount payloadSize metadataSize }
            dimensions { datetime }
          }
        }
      }
    }
    "#
}

fn kv_query() -> &'static str {
    r#"
    query KvAccountAnalytics($accountTag: string!, $start: Date, $end: Date) {
      viewer {
        accounts(filter: { accountTag: $accountTag }) {
          kvOperationsAdaptiveGroups(
            limit: 10000
            filter: { date_geq: $start, date_leq: $end }
          ) {
            sum { requests }
            dimensions { date actionType }
          }
          kvStorageAdaptiveGroups(
            limit: 10000
            filter: { date_geq: $start, date_leq: $end }
            orderBy: [date_DESC]
          ) {
            max { keyCount byteCount }
            dimensions { date }
          }
        }
      }
    }
    "#
}

fn zone_traffic_query() -> &'static str {
    r#"
    query ZoneTraffic($zoneTags: [string!], $datetimeStart: Time, $datetimeEnd: Time) {
      viewer {
        zones(filter: { zoneTag_in: $zoneTags }) {
          httpRequestsAdaptiveGroups(
            limit: 10000
            orderBy: [count_DESC]
            filter: { datetime_geq: $datetimeStart, datetime_lt: $datetimeEnd, requestSource: "eyeball" }
          ) {
            count
            sum { visits edgeResponseBytes }
            dimensions { clientRequestHTTPHost }
          }
        }
      }
    }
    "#
}

fn zone_security_query() -> &'static str {
    r#"
    query ZoneSecurity($zoneTags: [string!], $datetimeStart: Time, $datetimeEnd: Time) {
      viewer {
        zones(filter: { zoneTag_in: $zoneTags }) {
          firewallEventsAdaptiveGroups(
            limit: 10000
            filter: { datetime_geq: $datetimeStart, datetime_lt: $datetimeEnd }
          ) {
            count
            dimensions { action }
          }
        }
      }
    }
    "#
}

fn zone_security_events_query() -> &'static str {
    r#"
    query ZoneSecurityEvents($zoneTags: [string!], $datetimeStart: Time, $datetimeEnd: Time) {
      viewer {
        zones(filter: { zoneTag_in: $zoneTags }) {
          firewallEventsAdaptive(
            limit: 10000
            orderBy: [datetime_DESC]
            filter: { datetime_geq: $datetimeStart, datetime_lt: $datetimeEnd }
          ) {
            action
          }
        }
      }
    }
    "#
}

fn usage_panels(
    summary: &MetricSummary,
    points: &HashMap<String, Vec<(String, u64)>>,
) -> Vec<UsagePanel> {
    vec![
        UsagePanel {
            id: "workers".into(),
            title: "Workers".into(),
            value: compact(summary.worker_requests),
            detail: format!("{} errors", compact(summary.worker_errors)),
            tone: if summary.worker_errors > 0 {
                "warn".into()
            } else {
                "good".into()
            },
            points: point_series(points, "workers"),
        },
        UsagePanel {
            id: "d1".into(),
            title: "D1".into(),
            value: compact(summary.d1_queries),
            detail: summary
                .d1_latency_p90_ms
                .map(|value| format!("{value:.1} ms p90"))
                .unwrap_or_else(|| "latency unavailable".into()),
            tone: "neutral".into(),
            points: point_series(points, "d1"),
        },
        UsagePanel {
            id: "r2".into(),
            title: "R2".into(),
            value: bytes(summary.r2_storage_bytes),
            detail: format!("{} operations", compact(summary.r2_operations)),
            tone: "neutral".into(),
            points: point_series(points, "r2"),
        },
        UsagePanel {
            id: "kv".into(),
            title: "KV".into(),
            value: compact(summary.kv_operations),
            detail: format!("{} stored", bytes(summary.kv_storage_bytes)),
            tone: "neutral".into(),
            points: point_series(points, "kv"),
        },
        UsagePanel {
            id: "observability".into(),
            title: "Observability".into(),
            value: compact(summary.worker_log_events + summary.worker_trace_events),
            detail: format!(
                "{} audit events / {} Logpush jobs",
                compact(summary.audit_events),
                compact(summary.logpush_enabled_jobs)
            ),
            tone: if summary.audit_failures > 0 || summary.collector_api_errors > 0 {
                "warn".into()
            } else {
                "good".into()
            },
            points: Vec::new(),
        },
    ]
}

fn health_panels(issues: &[String]) -> Vec<ServiceHealth> {
    let inventory_issue = issue_contains(issues, &["inventory", "binding metadata"]);
    let analytics_issue = issue_contains(issues, &["metrics", "analytics"]);
    let observability_issue = issue_contains(issues, &["observability", "logpush", "audit"]);
    let collector_issue = issue_contains(issues, &["collector", "http", "graphql"]);
    let optional_observability_issue = issues.iter().any(|issue| {
        is_optional_scope_issue(issue)
            && issue_matches(issue, &["observability", "logpush", "audit"])
    });
    let blocking_issue_count = issues
        .iter()
        .filter(|issue| !is_optional_scope_issue(issue))
        .count();

    vec![
        ServiceHealth {
            id: "inventory".into(),
            service: "Inventory".into(),
            status: if inventory_issue { "warn" } else { "ok" }.into(),
            label: if inventory_issue { "Partial" } else { "Fresh" }.into(),
            detail: if inventory_issue {
                "Some inventory endpoints failed or need additional read scopes.".into()
            } else {
                "Resource inventory collectors returned data.".into()
            },
        },
        ServiceHealth {
            id: "graphql".into(),
            service: "GraphQL metrics".into(),
            status: if analytics_issue { "warn" } else { "ok" }.into(),
            label: if analytics_issue { "Partial" } else { "Fresh" }.into(),
            detail: if analytics_issue {
                "One or more analytics datasets were unavailable for this token or plan.".into()
            } else {
                "Account and product analytics collectors completed.".into()
            },
        },
        ServiceHealth {
            id: "observability".into(),
            service: "Observability".into(),
            status: if observability_issue { "warn" } else { "ok" }.into(),
            label: if observability_issue {
                "Scoped"
            } else if optional_observability_issue {
                "Limited"
            } else {
                "Checked"
            }
            .into(),
            detail: if observability_issue {
                "Audit Logs, Logpush, or Workers Observability needs additional read scopes or setup.".into()
            } else if optional_observability_issue {
                "Core observability completed; one or more optional Cloudflare coverage checks were unavailable.".into()
            } else {
                "Audit, Logpush, and Workers Observability collectors completed.".into()
            },
        },
        ServiceHealth {
            id: "collector".into(),
            service: "Collector API".into(),
            status: if collector_issue { "warn" } else { "ok" }.into(),
            label: if blocking_issue_count == 0 {
                "Clean"
            } else {
                "See issues"
            }
            .into(),
            detail: if issues.is_empty() {
                "Cedar recorded no Cloudflare API failures during this sync.".into()
            } else if blocking_issue_count == 0 {
                "Only optional Cloudflare write-gated endpoints were unavailable.".into()
            } else {
                format!(
                    "{} collector issue(s) recorded during this sync.",
                    blocking_issue_count
                )
            },
        },
    ]
}

fn issue_contains(issues: &[String], needles: &[&str]) -> bool {
    issues
        .iter()
        .filter(|issue| !is_optional_scope_issue(issue))
        .any(|issue| issue_matches(issue, needles))
}

fn issue_matches(issue: &str, needles: &[&str]) -> bool {
    let normalized = issue.to_lowercase();
    needles.iter().any(|needle| normalized.contains(needle))
}

fn is_optional_scope_issue(issue: &str) -> bool {
    let normalized = issue.to_lowercase();
    normalized.starts_with("optional ")
        || normalized.contains(" optional checks scoped")
        || normalized.contains("write-gated")
        || normalized.contains("cloudflare requires logs write")
        || normalized.contains("cloudflare requires workers observability write")
}

fn is_graphql_path_access_error(error: &str) -> bool {
    error
        .to_lowercase()
        .contains("does not have access to the path")
}

fn estimate_workers_paid_plan_cost(summary: &MetricSummary, range: &str) -> CostProjection {
    const BASE_USD: f64 = 5.0;
    const MILLION: f64 = 1_000_000.0;
    const GIB: f64 = 1024_f64 * 1024_f64 * 1024_f64;

    let factor = monthly_projection_factor(range);
    let worker_requests = summary.worker_requests as f64 * factor;
    let worker_cpu_ms = summary.worker_cpu_time_ms.unwrap_or_default() * factor;
    let d1_rows_read = summary.d1_rows_read as f64 * factor;
    let d1_rows_written = summary.d1_rows_written as f64 * factor;
    let r2_class_a = summary.r2_class_a_operations as f64 * factor;
    let r2_class_b = summary.r2_class_b_operations as f64 * factor;
    let kv_reads = summary.kv_read_operations as f64 * factor;
    let kv_writes = summary.kv_write_operations as f64 * factor;
    let kv_deletes = summary.kv_delete_operations as f64 * factor;
    let kv_lists = summary.kv_list_operations as f64 * factor;

    let worker_request_overage = overage(worker_requests, 10_000_000.0) / MILLION * 0.30;
    let worker_cpu_overage = overage(worker_cpu_ms, 30_000_000.0) / MILLION * 0.02;

    let d1_read_overage = overage(d1_rows_read, 25_000_000_000.0) / MILLION * 0.001;
    let d1_write_overage = overage(d1_rows_written, 50_000_000.0) / MILLION * 1.00;
    let d1_storage_overage = overage(summary.d1_storage_bytes as f64 / GIB, 5.0) * 0.75;

    let r2_storage_overage = overage(summary.r2_storage_bytes as f64 / GIB, 10.0).ceil() * 0.015;
    let r2_class_a_overage = (overage(r2_class_a, 1_000_000.0) / MILLION).ceil() * 4.50;
    let r2_class_b_overage = (overage(r2_class_b, 10_000_000.0) / MILLION).ceil() * 0.36;

    let kv_read_overage = overage(kv_reads, 10_000_000.0) / MILLION * 0.50;
    let kv_write_overage = overage(kv_writes, 1_000_000.0) / MILLION * 5.00;
    let kv_delete_overage = overage(kv_deletes, 1_000_000.0) / MILLION * 5.00;
    let kv_list_overage = overage(kv_lists, 1_000_000.0) / MILLION * 5.00;
    let kv_storage_overage = overage(summary.kv_storage_bytes as f64 / GIB, 1.0) * 0.50;

    let overage = worker_request_overage
        + worker_cpu_overage
        + d1_read_overage
        + d1_write_overage
        + d1_storage_overage
        + r2_storage_overage
        + r2_class_a_overage
        + r2_class_b_overage
        + kv_read_overage
        + kv_write_overage
        + kv_delete_overage
        + kv_list_overage
        + kv_storage_overage;

    CostProjection {
        base: BASE_USD,
        overage: round_cents(overage),
        total: round_cents(BASE_USD + overage),
    }
}

fn monthly_projection_factor(range: &str) -> f64 {
    match range {
        "24h" => 30.0,
        "7d" => 30.0 / 7.0,
        _ => 1.0,
    }
}

fn overage(value: f64, included: f64) -> f64 {
    (value - included).max(0.0)
}

fn round_cents(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn point_series(points: &HashMap<String, Vec<(String, u64)>>, key: &str) -> Vec<u32> {
    let Some(items) = points.get(key) else {
        return Vec::new();
    };
    if items.is_empty() {
        return Vec::new();
    }

    let mut buckets: HashMap<String, u64> = HashMap::new();
    for (bucket, value) in items {
        *buckets.entry(bucket.clone()).or_default() += *value;
    }

    let mut ordered = buckets.into_iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    let mut values = ordered
        .into_iter()
        .map(|(_, value)| value)
        .collect::<Vec<_>>();
    if values.len() > 12 {
        let chunk = (values.len() as f64 / 12.0).ceil() as usize;
        values = values
            .chunks(chunk)
            .map(|chunk| chunk.iter().copied().sum::<u64>())
            .collect::<Vec<_>>();
    }

    values
        .into_iter()
        .map(|value| value.min(u32::MAX as u64) as u32)
        .collect()
}

fn result_array(value: &Value) -> Vec<&Value> {
    let Some(result) = value.get("result") else {
        return Vec::new();
    };

    if let Some(items) = result.as_array() {
        return items.iter().collect();
    }

    for key in [
        "items",
        "buckets",
        "namespaces",
        "databases",
        "projects",
        "scripts",
        "jobs",
        "events",
        "keys",
        "destinations",
    ] {
        if let Some(items) = result.get(key).and_then(Value::as_array) {
            return items.iter().collect();
        }
    }

    Vec::new()
}

fn result_items_value(items: Vec<Value>) -> Value {
    json!({ "result": items })
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn field_as_string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(|item| {
        item.as_str()
            .map(ToOwned::to_owned)
            .or_else(|| item.as_u64().map(|number| number.to_string()))
            .or_else(|| item.as_i64().map(|number| number.to_string()))
    })
}

fn bool_field(value: &Value, key: &str) -> Option<bool> {
    value.get(key).and_then(|item| {
        item.as_bool().or_else(|| {
            item.as_str()
                .and_then(|text| match normalized_action(text).as_str() {
                    "true" | "enabled" | "on" | "yes" => Some(true),
                    "false" | "disabled" | "off" | "no" => Some(false),
                    _ => None,
                })
        })
    })
}

fn first_string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| string_field(value, key))
}

fn first_f64_field(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| f64_field(value, key))
}

fn percent_encode_path_segment(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                vec![byte as char]
            }
            _ => {
                let encoded = format!("%{byte:02X}");
                encoded.chars().collect()
            }
        })
        .collect()
}

fn percent_encode_query_value(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                vec![byte as char]
            }
            b' ' => vec!['+'],
            _ => {
                let encoded = format!("%{byte:02X}");
                encoded.chars().collect()
            }
        })
        .collect()
}

fn sanitize_destination(value: &str) -> String {
    let without_query = value.split('?').next().unwrap_or(value);
    let truncated = without_query.chars().take(80).collect::<String>();
    if without_query.chars().count() > 80 {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn u64_field(value: &Value, key: &str) -> u64 {
    value
        .get(key)
        .and_then(|item| {
            item.as_u64()
                .or_else(|| item.as_f64().map(|number| number as u64))
        })
        .unwrap_or_default()
}

fn f64_field(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(|item| {
        item.as_f64()
            .or_else(|| item.as_u64().map(|number| number as f64))
    })
}

fn compact(value: u64) -> String {
    if value >= 1_000_000_000 {
        format!("{:.1}B", value as f64 / 1_000_000_000.0)
    } else if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn bytes(value: u64) -> String {
    if value >= 1024_u64.pow(4) {
        format!("{:.1} TB", value as f64 / 1024_f64.powi(4))
    } else if value >= 1024_u64.pow(3) {
        format!("{:.1} GB", value as f64 / 1024_f64.powi(3))
    } else if value >= 1024_u64.pow(2) {
        format!("{:.1} MB", value as f64 / 1024_f64.powi(2))
    } else if value >= 1024 {
        format!("{:.1} KB", value as f64 / 1024.0)
    } else {
        format!("{value} B")
    }
}

fn keyring_entry() -> keyring::Entry {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .expect("static keyring service/user are valid")
}

fn read_token() -> AppResult<String> {
    Ok(keyring_entry().get_password()?)
}

fn write_token(token: &str) -> AppResult<()> {
    keyring_entry().set_password(token)?;
    Ok(())
}

fn read_account(state: &Backend) -> AppResult<Option<Account>> {
    let db = state
        .db
        .lock()
        .map_err(|_| AppError::Message("Local database lock failed.".into()))?;
    let id = get_config(&db, "account_id")?;
    let name = get_config(&db, "account_name")?;
    Ok(match (id, name) {
        (Some(id), Some(name)) => Some(Account { id, name }),
        _ => None,
    })
}

fn write_account(state: &Backend, account: &Account) -> AppResult<()> {
    let db = state
        .db
        .lock()
        .map_err(|_| AppError::Message("Local database lock failed.".into()))?;
    set_config(&db, "account_id", &account.id)?;
    set_config(&db, "account_name", &account.name)?;
    Ok(())
}

fn get_config(db: &Connection, key: &str) -> AppResult<Option<String>> {
    let mut statement = db.prepare("SELECT value FROM config WHERE key = ?1")?;
    let value = statement.query_row(params![key], |row| row.get::<_, String>(0));
    match value {
        Ok(value) => Ok(Some(value)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(AppError::Database(error)),
    }
}

fn set_config(db: &Connection, key: &str, value: &str) -> AppResult<()> {
    db.execute(
        "INSERT INTO config (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

fn save_snapshot(
    state: &Backend,
    account: &Account,
    snapshot: &DashboardSnapshot,
) -> AppResult<()> {
    let db = state
        .db
        .lock()
        .map_err(|_| AppError::Message("Local database lock failed.".into()))?;
    let payload = serde_json::to_string(snapshot)?;
    db.execute(
        "INSERT INTO snapshots (account_id, range_key, generated_at, payload) VALUES (?1, ?2, ?3, ?4)",
        params![account.id, snapshot.range, snapshot.generated_at, payload],
    )?;
    db.execute(
        "DELETE FROM snapshots
         WHERE account_id = ?1
           AND range_key = ?2
           AND id NOT IN (
             SELECT id FROM snapshots
             WHERE account_id = ?1 AND range_key = ?2
             ORDER BY generated_at DESC
             LIMIT 1
           )",
        params![account.id, snapshot.range],
    )?;
    Ok(())
}

fn read_fresh_snapshot(
    state: &Backend,
    account: &Account,
    range: &str,
) -> AppResult<Option<DashboardSnapshot>> {
    let Some(snapshot) = read_latest_snapshot(state, account, range)? else {
        return Ok(None);
    };

    Ok(snapshot_is_fresh(&snapshot).then_some(snapshot))
}

fn read_latest_snapshot(
    state: &Backend,
    account: &Account,
    range: &str,
) -> AppResult<Option<DashboardSnapshot>> {
    let db = state
        .db
        .lock()
        .map_err(|_| AppError::Message("Local database lock failed.".into()))?;
    let mut statement = db.prepare(
        "SELECT payload FROM snapshots
         WHERE account_id = ?1 AND range_key = ?2
         ORDER BY generated_at DESC
         LIMIT 1",
    )?;
    let payload = statement.query_row(params![account.id, range], |row| row.get::<_, String>(0));
    let payload = match payload {
        Ok(payload) => payload,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(error) => return Err(AppError::Database(error)),
    };

    let mut snapshot: DashboardSnapshot = serde_json::from_str(&payload)?;
    hydrate_cached_snapshot(&mut snapshot, account);
    Ok(Some(snapshot))
}

fn hydrate_cached_snapshot(snapshot: &mut DashboardSnapshot, account: &Account) {
    snapshot.live = true;
    snapshot.cached = true;
    snapshot.account = Some(account.clone());
    snapshot.expires_at = parsed_snapshot_time(snapshot)
        .map(|generated_at| cache_expires_at(generated_at, &snapshot.range));
}

fn snapshot_is_fresh(snapshot: &DashboardSnapshot) -> bool {
    parsed_snapshot_time(snapshot)
        .map(|generated_at| generated_at + cache_ttl(&snapshot.range) > Utc::now())
        .unwrap_or(false)
}

fn parsed_snapshot_time(snapshot: &DashboardSnapshot) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&snapshot.generated_at)
        .ok()
        .map(|datetime| datetime.with_timezone(&Utc))
}

fn cache_expires_at(generated_at: DateTime<Utc>, range: &str) -> String {
    (generated_at + cache_ttl(range)).to_rfc3339()
}

fn cache_ttl(range: &str) -> Duration {
    match range {
        "30d" => Duration::minutes(30),
        "7d" => Duration::minutes(15),
        _ => Duration::minutes(5),
    }
}

fn normalize_range(range: &str) -> &str {
    match range {
        "24h" | "7d" | "30d" => range,
        _ => "24h",
    }
}

fn open_database() -> AppResult<Connection> {
    let mut dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Cedar");
    fs::create_dir_all(&dir)?;
    dir.push("cedar.sqlite3");

    let db = Connection::open(dir)?;
    initialize_database(&db)?;
    Ok(db)
}

fn initialize_database(db: &Connection) -> AppResult<()> {
    db.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS config (
          key TEXT PRIMARY KEY,
          value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS snapshots (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          account_id TEXT NOT NULL,
          range_key TEXT NOT NULL,
          generated_at TEXT NOT NULL,
          payload TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS snapshots_account_range_idx
          ON snapshots (account_id, range_key, generated_at DESC);
        ",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visual_qa_backend_keeps_preferences_in_memory() {
        let backend = Backend::new_visual_qa().expect("visual QA backend should initialize");

        backend
            .set_preference("visual-qa", "isolated")
            .expect("in-memory preference should save");

        assert_eq!(
            backend
                .preference("visual-qa")
                .expect("preference should load"),
            Some("isolated".into())
        );
    }

    fn utc(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn metric_time_slices_split_long_ranges_into_daily_windows() {
        let slices = metric_time_slices_between(
            utc("2026-05-01T00:00:00Z"),
            utc("2026-05-31T00:00:00Z"),
            Duration::days(1),
        );

        assert_eq!(slices.len(), 30);
        assert_eq!(slices[0].start_time, "2026-05-01T00:00:00.000Z");
        assert_eq!(slices[0].end_time, "2026-05-02T00:00:00.000Z");
        assert_eq!(slices.last().unwrap().end_time, "2026-05-31T00:00:00.000Z");

        for slice in slices {
            let start = utc(&slice.start_time);
            let end = utc(&slice.end_time);
            assert!(start < end);
            assert!(end - start <= Duration::days(1));
        }
    }

    #[test]
    fn zone_batches_include_all_discovered_zones() {
        let zones = (0..22)
            .map(|index| ZoneInfo {
                id: format!("zone-{index}"),
                name: format!("zone-{index}.example"),
                status: "active".into(),
            })
            .collect::<Vec<_>>();

        let batches = zone_batches(&zones);

        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].len(), 10);
        assert_eq!(batches[1].len(), 10);
        assert_eq!(batches[2].len(), 2);
    }

    #[test]
    fn parse_zone_traffic_reads_all_returned_zones() {
        let value = json!({
            "data": {
                "viewer": {
                    "zones": [
                        {
                            "httpRequestsAdaptiveGroups": [
                                {
                                    "count": 3,
                                    "sum": { "cachedRequests": 1 },
                                    "dimensions": { "clientRequestHTTPHost": "alpha.example" }
                                }
                            ]
                        },
                        {
                            "httpRequestsAdaptiveGroups": [
                                {
                                    "count": 7,
                                    "sum": { "cachedRequests": 4 },
                                    "dimensions": { "clientRequestHTTPHost": "beta.example" }
                                }
                            ]
                        }
                    ]
                }
            }
        });
        let mut summary = ZoneSummary::default();
        let mut host_totals = HashMap::new();

        parse_zone_traffic(&value, &mut summary, &mut host_totals);

        assert_eq!(summary.requests, 10);
        assert_eq!(summary.cache_hit_ratio, Some(0.5));
        assert_eq!(host_totals.get("alpha.example"), Some(&3));
        assert_eq!(host_totals.get("beta.example"), Some(&7));
    }

    #[test]
    fn paged_path_adds_page_params_to_plain_and_filtered_paths() {
        assert_eq!(
            paged_path("/accounts", 2, 100),
            "/accounts?page=2&per_page=100"
        );
        assert_eq!(
            paged_path("/zones?account.id=abc", 3, 50),
            "/zones?account.id=abc&page=3&per_page=50"
        );
    }

    #[test]
    fn result_total_pages_reads_cloudflare_result_info() {
        let value = json!({
            "result": [],
            "result_info": {
                "page": 1,
                "per_page": 100,
                "total_pages": 4
            }
        });

        assert_eq!(result_total_pages(&value), Some(4));
    }

    #[test]
    fn cloudflare_retention_limit_parses_compact_units() {
        let mixed = parse_cloudflare_retention_limit(
            r#"viewer.zones.3.httpRequestsAdaptiveGroups: zone "abc" cannot request data older than 1w1d, but your query requests data from 4w2d4s ago"#,
        )
        .unwrap();

        assert_eq!(mixed.duration, Duration::days(8));
        assert_eq!(mixed.label, "1w1d");

        let seconds = parse_cloudflare_retention_limit(
            "viewer.zones.0.firewallEventsAdaptiveGroups: cannot request data older than 2678400s",
        )
        .unwrap();

        assert_eq!(seconds.duration, Duration::seconds(2_678_400));
        assert_eq!(seconds.label, "2678400s");
    }

    #[test]
    fn retention_limited_start_keeps_inside_cloudflare_window() {
        let window = MetricWindow {
            range: "30d".into(),
            start: utc("2026-05-01T00:00:00Z"),
            end: utc("2026-05-31T00:00:00Z"),
            start_time: String::new(),
            end_time: String::new(),
            start_ms: 0,
            end_ms: 0,
            start_date: String::new(),
            end_date: String::new(),
        };
        let limit = CloudflareRetentionLimit {
            duration: Duration::days(8),
            label: "1w1d".into(),
        };

        assert_eq!(
            retention_limited_start(&window, &limit),
            utc("2026-05-23T00:05:00Z")
        );
    }

    #[test]
    fn collector_errors_ignore_optional_capability_probes() {
        let mut collector = CollectorTelemetryBuilder::default();
        collector.record(
            "GET",
            "/accounts/account-123/logpush/jobs",
            Some(403),
            12.0,
            false,
            None,
            Some("HTTP 403: Authentication error".into()),
        );
        collector.record(
            "POST",
            "/graphql",
            Some(200),
            24.0,
            false,
            None,
            Some(
                "viewer.zones.firewallEventsAdaptiveGroups: does not have access to the path"
                    .into(),
            ),
        );
        collector.record(
            "POST",
            "/graphql",
            Some(200),
            22.0,
            false,
            None,
            Some(
                "viewer.zones.3.httpRequestsAdaptiveGroups: zone \"abc\" cannot request data older than 1w1d, but your query requests data from 4w2d4s ago"
                    .into(),
            ),
        );
        collector.record(
            "GET",
            "/accounts/account-123/workers/scripts",
            Some(500),
            8.0,
            false,
            None,
            Some("HTTP 500".into()),
        );

        let telemetry = collector.finish();

        assert_eq!(telemetry.api_errors, 1);
        assert_eq!(
            telemetry
                .endpoints
                .iter()
                .filter(|endpoint| endpoint.optional)
                .count(),
            3
        );
    }
}
