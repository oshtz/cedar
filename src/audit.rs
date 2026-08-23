use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::backend::{CollectorEndpoint, DashboardSnapshot, ResourceRow};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum WorkerAuditPreference {
    #[default]
    Normal,
    Critical,
    Ignore,
}

pub(crate) type WorkerAuditPreferences = HashMap<String, WorkerAuditPreference>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Tone {
    Good,
    Warn,
    Bad,
    Neutral,
}

impl Tone {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Good => "good",
            Self::Warn => "warn",
            Self::Bad => "bad",
            Self::Neutral => "neutral",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Section {
    Overview,
    Resources,
    Workers,
    Billing,
    Connection,
    Settings,
}

#[derive(Clone, Debug)]
pub(crate) struct AuditFinding {
    pub(crate) title: String,
    pub(crate) detail: String,
    pub(crate) tone: Tone,
    pub(crate) section: Option<Section>,
    pub(crate) action: Option<String>,
    pub(crate) evidence: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct SnapshotChange {
    pub(crate) title: String,
    pub(crate) detail: String,
    pub(crate) tone: Tone,
}

pub(crate) fn is_optional_scope_issue(issue: &str) -> bool {
    let normalized = issue.to_lowercase();
    normalized.starts_with("optional ")
        || normalized.contains(" optional checks scoped")
        || normalized.contains("write-gated")
        || normalized.contains("cloudflare requires logs write")
        || normalized.contains("cloudflare requires workers observability write")
}

pub(crate) fn unique_actionable_issues(issues: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    issues
        .iter()
        .filter_map(|issue| {
            let issue = issue.trim();
            if issue.is_empty() || is_optional_scope_issue(issue) || !seen.insert(issue.to_owned())
            {
                None
            } else {
                Some(issue.to_owned())
            }
        })
        .collect()
}

pub(crate) fn build_audit_findings(
    snapshot: &DashboardSnapshot,
    preferences: &WorkerAuditPreferences,
) -> Vec<AuditFinding> {
    if snapshot.resources.is_empty() {
        return vec![finding(
            "No account inventory yet",
            "Connect Cloudflare and sync to produce an audit snapshot.",
            Tone::Neutral,
            Some(Section::Connection),
        )];
    }

    let mut findings = Vec::new();
    let actionable = unique_actionable_issues(&snapshot.issues);
    let optional = snapshot
        .issues
        .iter()
        .filter(|issue| is_optional_scope_issue(issue))
        .cloned()
        .collect::<Vec<_>>();
    let workers = snapshot
        .resources
        .iter()
        .filter(|resource| resource.kind == "worker")
        .collect::<Vec<_>>();
    let audited_workers = workers
        .iter()
        .copied()
        .filter(|worker| worker_preference(worker, preferences) != WorkerAuditPreference::Ignore)
        .collect::<Vec<_>>();
    let uncovered = audited_workers
        .iter()
        .copied()
        .filter(|worker| !has_worker_observability(worker))
        .collect::<Vec<_>>();
    let critical_uncovered = uncovered
        .iter()
        .filter(|worker| worker_preference(worker, preferences) == WorkerAuditPreference::Critical)
        .count();
    let coverage_tone = if snapshot.metrics.worker_errors > 0 || critical_uncovered > 0 {
        Tone::Warn
    } else {
        Tone::Neutral
    };

    for issue in actionable.into_iter().take(3) {
        let mut item = finding(
            "Sync issue",
            &issue,
            if issue.to_lowercase().contains("failed") {
                Tone::Bad
            } else {
                Tone::Warn
            },
            Some(Section::Connection),
        );
        item.action =
            Some("Fix the token scope or Cloudflare API issue, then run the audit again.".into());
        item.evidence.push(issue);
        findings.push(item);
    }

    if snapshot.collector.api_errors > 0 {
        let mut item = finding(
            "Collector errors",
            &format!(
                "{} Cloudflare API calls failed during the last sync.",
                compact_number(snapshot.collector.api_errors)
            ),
            Tone::Bad,
            Some(Section::Connection),
        );
        item.action = Some("Open Connection, check token scopes, then rerun the audit.".into());
        item.evidence = snapshot
            .collector
            .endpoints
            .iter()
            .filter(|endpoint| !endpoint.ok && !endpoint.optional)
            .take(4)
            .map(format_endpoint)
            .collect();
        findings.push(item);
    }

    if snapshot
        .collector
        .rate_limit_remaining
        .as_deref()
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|remaining| remaining < 100)
    {
        let remaining = snapshot
            .collector
            .rate_limit_remaining
            .as_deref()
            .unwrap_or_default();
        let mut item = finding(
            "Cloudflare rate limit is low",
            &format!("{remaining} API calls remain on the last observed rate-limit header."),
            Tone::Warn,
            Some(Section::Connection),
        );
        item.action =
            Some("Wait before forcing another sync if Cloudflare starts returning 429s.".into());
        findings.push(item);
    }

    if snapshot.metrics.worker_errors > 0 {
        let mut item = finding(
            "Worker errors",
            &format!(
                "{} Worker errors in the selected {} range.",
                compact_number(snapshot.metrics.worker_errors),
                snapshot.range
            ),
            Tone::Warn,
            Some(Section::Workers),
        );
        item.action =
            Some("Open Workers and inspect scripts with warning/quiet status first.".into());
        item.evidence = name_evidence(
            audited_workers
                .iter()
                .copied()
                .filter(|worker| worker.status != "healthy"),
            preferences,
        );
        findings.push(item);
    }

    if !uncovered.is_empty() {
        let mut item = finding(
            "Worker observability coverage",
            &format!(
                "{} of {} audited Workers have no logs, traces, destinations, or Logpush metadata.",
                compact_number(uncovered.len() as u64),
                compact_number(audited_workers.len() as u64)
            ),
            coverage_tone,
            Some(Section::Workers),
        );
        item.action =
            Some("Prioritize logs/traces for production or traffic-bearing scripts.".into());
        item.evidence = name_evidence(uncovered.iter().copied(), preferences);
        findings.push(item);
    }

    if !snapshot.observability.gaps.is_empty() {
        let mut item = finding(
            "Worker telemetry gaps",
            &format!(
                "{} Workers Observability checks did not produce full coverage.",
                compact_number(snapshot.observability.gaps.len() as u64)
            ),
            Tone::Warn,
            Some(Section::Workers),
        );
        item.action =
            Some("Verify Workers Observability access and telemetry configuration.".into());
        item.evidence = snapshot
            .observability
            .gaps
            .iter()
            .take(4)
            .cloned()
            .collect();
        findings.push(item);
    }

    let logpush_blocked = optional
        .iter()
        .any(|issue| issue.to_lowercase().contains("logpush"));
    if !logpush_blocked && !audited_workers.is_empty() && snapshot.logpush.workers_trace_jobs == 0 {
        let mut item = finding(
            "Worker trace Logpush coverage",
            "No Worker trace Logpush jobs were found for this account.",
            coverage_tone,
            Some(Section::Workers),
        );
        item.action =
            Some("Add Worker trace Logpush only when durable incident logs matter.".into());
        findings.push(item);
    }

    if snapshot.logpush.disabled_jobs > 0 {
        let mut item = finding(
            "Disabled Logpush jobs",
            &format!(
                "{} Logpush jobs are disabled.",
                snapshot.logpush.disabled_jobs
            ),
            Tone::Warn,
            Some(Section::Workers),
        );
        item.action =
            Some("Enable or delete disabled Logpush jobs so coverage is unambiguous.".into());
        item.evidence = snapshot
            .logpush
            .recent
            .iter()
            .filter(|job| !job.enabled)
            .take(4)
            .map(|job| {
                format!(
                    "{} {}: {}",
                    if job.enabled { "enabled" } else { "disabled" },
                    job.dataset,
                    job.name
                )
            })
            .collect();
        findings.push(item);
    }

    let failed_events = snapshot
        .audit
        .recent
        .iter()
        .filter(|event| {
            let result = event.result.to_lowercase();
            result.contains("fail") || result.contains("error") || result.contains("deny")
        })
        .count();
    let audit_failures = snapshot.audit.failures.max(failed_events);
    if audit_failures > 0 {
        let mut item = finding(
            "Failed Cloudflare audit actions",
            &format!(
                "{audit_failures} failed audit-log events in the selected {} range.",
                snapshot.range
            ),
            Tone::Warn,
            Some(Section::Connection),
        );
        item.action =
            Some("Review failed account actions before treating the snapshot as clean.".into());
        findings.push(item);
    }

    let attention = snapshot
        .resources
        .iter()
        .filter(|resource| resource.status == "warning" || resource.status == "unknown")
        .collect::<Vec<_>>();
    if !attention.is_empty() {
        let mut item = finding(
            "Resources need attention",
            &format!(
                "{} resources are warning or unknown: {}.",
                attention.len(),
                name_list(&attention)
            ),
            Tone::Warn,
            Some(Section::Resources),
        );
        item.action = Some("Inspect warning or unknown rows in the resource table.".into());
        item.evidence = name_evidence(attention.iter().copied(), preferences);
        findings.push(item);
    }

    let quiet = audited_workers
        .iter()
        .copied()
        .filter(|worker| worker.status == "quiet")
        .collect::<Vec<_>>();
    if !quiet.is_empty() {
        let mut item = finding(
            "Quiet workers",
            &format!(
                "{} Workers had no request metrics in the selected {} range.",
                quiet.len(),
                snapshot.range
            ),
            Tone::Neutral,
            Some(Section::Workers),
        );
        item.action =
            Some("Confirm quiet is expected for cron, queue, or low-traffic scripts.".into());
        item.evidence = name_evidence(quiet.iter().copied(), preferences);
        findings.push(item);
    }

    if snapshot.metrics.cost_overage_usd.unwrap_or_default() > 0.0 {
        let overage = snapshot.metrics.cost_overage_usd.unwrap_or_default();
        let mut item = finding(
            "Projected Workers overage",
            &format!(
                "{} over the Workers Paid base from current usage projection.",
                money(overage)
            ),
            Tone::Warn,
            Some(Section::Billing),
        );
        item.action =
            Some("Open Cost and check the allowance drivers before the month closes.".into());
        findings.push(item);
    }

    if !optional.is_empty() {
        let mut item = finding(
            "Scoped coverage gaps",
            &format!(
                "{} optional checks were blocked by token scope, plan, or endpoint access.",
                optional.len()
            ),
            Tone::Neutral,
            Some(Section::Connection),
        );
        item.action = Some(
            "Use a full audit token when Logpush or Workers Observability coverage matters.".into(),
        );
        item.evidence = optional.into_iter().take(4).collect();
        findings.push(item);
    }

    if findings.is_empty() {
        let mut item = finding(
            "No audit findings",
            "Inventory, collector, observability, Logpush, and usage checks did not surface obvious action items.",
            Tone::Good,
            Some(Section::Overview),
        );
        item.action = Some("Copy the report or rerun after the next infrastructure change.".into());
        findings.push(item);
    }

    findings.truncate(6);
    findings
}

pub(crate) fn diff_snapshots(
    previous: &DashboardSnapshot,
    next: &DashboardSnapshot,
) -> Vec<SnapshotChange> {
    if previous.generated_at.is_empty()
        || previous.resources.is_empty()
        || previous.range != next.range
    {
        return Vec::new();
    }

    let previous_rows = previous
        .resources
        .iter()
        .map(|row| (resource_key(row), row))
        .collect::<HashMap<_, _>>();
    let next_rows = next
        .resources
        .iter()
        .map(|row| (resource_key(row), row))
        .collect::<HashMap<_, _>>();
    let added = next
        .resources
        .iter()
        .filter(|row| !previous_rows.contains_key(&resource_key(row)))
        .collect::<Vec<_>>();
    let removed = previous
        .resources
        .iter()
        .filter(|row| !next_rows.contains_key(&resource_key(row)))
        .collect::<Vec<_>>();
    let changed = next
        .resources
        .iter()
        .filter(|row| {
            previous_rows
                .get(&resource_key(row))
                .is_some_and(|old| old.status != row.status)
        })
        .collect::<Vec<_>>();

    let mut changes = Vec::new();
    if !added.is_empty() {
        changes.push(SnapshotChange {
            title: "Resources added".into(),
            detail: format!("{} new resources: {}.", added.len(), name_list(&added)),
            tone: Tone::Good,
        });
    }
    if !removed.is_empty() {
        changes.push(SnapshotChange {
            title: "Resources removed".into(),
            detail: format!(
                "{} resources disappeared: {}.",
                removed.len(),
                name_list(&removed)
            ),
            tone: Tone::Warn,
        });
    }
    if !changed.is_empty() {
        changes.push(SnapshotChange {
            title: "Status changed".into(),
            detail: format!(
                "{} resources changed health state: {}.",
                changed.len(),
                name_list(&changed)
            ),
            tone: Tone::Warn,
        });
    }
    if let (Some(before), Some(after)) = (previous.metrics.cost_usd, next.metrics.cost_usd)
        && (after - before).abs() >= 0.01
    {
        changes.push(SnapshotChange {
            title: "Cost projection moved".into(),
            detail: format!("{} to {}.", money(before), money(after)),
            tone: if after > before {
                Tone::Warn
            } else {
                Tone::Good
            },
        });
    }
    changes.truncate(8);
    changes
}

pub(crate) fn build_audit_report(
    snapshot: &DashboardSnapshot,
    findings: &[AuditFinding],
    changes: &[SnapshotChange],
) -> String {
    let account = snapshot
        .account
        .as_ref()
        .map(|account| account.name.as_str())
        .unwrap_or("Unknown account");
    let source = if snapshot.live {
        "Live Cloudflare API"
    } else if snapshot.cached {
        "Local cache"
    } else {
        "Empty"
    };
    let workers = snapshot
        .resources
        .iter()
        .filter(|row| row.kind == "worker")
        .count();
    let optional_gaps = snapshot
        .issues
        .iter()
        .filter(|issue| is_optional_scope_issue(issue))
        .count();
    let mut lines = vec![
        format!("# Cedar audit - {account}"),
        String::new(),
        format!(
            "Generated: {}",
            if snapshot.generated_at.is_empty() {
                "Not synced"
            } else {
                &snapshot.generated_at
            }
        ),
        format!("Range: {}", snapshot.range),
        format!("Source: {source}"),
        String::new(),
        "## Inventory".into(),
        format!("- Workers: {}", snapshot.inventory.workers),
        format!("- Pages: {}", snapshot.inventory.pages),
        format!("- D1: {}", snapshot.inventory.d1),
        format!(
            "- R2: {} ({})",
            snapshot.inventory.r2,
            format_bytes(snapshot.metrics.r2_storage_bytes)
        ),
        format!(
            "- KV: {} ({})",
            snapshot.inventory.kv,
            format_bytes(snapshot.metrics.kv_storage_bytes)
        ),
        format!("- Zones: {}", snapshot.inventory.zones),
        String::new(),
        "## Coverage".into(),
        format!(
            "- Collector: {} API calls, {} errors",
            snapshot.collector.api_calls, snapshot.collector.api_errors
        ),
        format!(
            "- Audit logs: {} events, {} failures",
            snapshot.audit.events, snapshot.audit.failures
        ),
        format!(
            "- Logpush: {}/{} jobs enabled, {} Worker trace jobs",
            snapshot.logpush.enabled_jobs,
            snapshot.logpush.jobs,
            snapshot.logpush.workers_trace_jobs
        ),
        format!(
            "- Worker observability config: {}/{} Workers configured, {} full-sample, {} destinations",
            snapshot.observability.configured_workers,
            workers,
            snapshot.observability.full_sample_workers,
            snapshot.observability.destinations
        ),
        format!("- Scope gaps: {optional_gaps} optional checks blocked"),
        String::new(),
        "## Findings".into(),
    ];
    for finding in findings {
        lines.push(format!(
            "- [{}] {}: {}",
            finding.tone.label(),
            finding.title,
            finding.detail
        ));
        if let Some(action) = &finding.action {
            lines.push(format!("  - Action: {action}"));
        }
        for evidence in finding.evidence.iter().take(4) {
            lines.push(format!("  - Evidence: {evidence}"));
        }
    }
    lines.extend([String::new(), "## Recent changes".into()]);
    if changes.is_empty() {
        lines.push("- No tracked changes since the previous snapshot.".into());
    } else {
        lines.extend(changes.iter().map(|change| {
            format!(
                "- [{}] {}: {}",
                change.tone.label(),
                change.title,
                change.detail
            )
        }));
    }
    lines.extend([
        String::new(),
        "## Usage".into(),
        format!(
            "- Worker requests: {}",
            compact_number(snapshot.metrics.worker_requests)
        ),
        format!(
            "- Worker errors: {}",
            compact_number(snapshot.metrics.worker_errors)
        ),
        format!(
            "- D1 queries: {}",
            compact_number(snapshot.metrics.d1_queries)
        ),
        format!(
            "- R2 operations: {}",
            compact_number(snapshot.metrics.r2_operations)
        ),
        format!(
            "- KV operations: {}",
            compact_number(snapshot.metrics.kv_operations)
        ),
        format!(
            "- Workers cost projection: {}",
            snapshot
                .metrics
                .cost_usd
                .map(money)
                .unwrap_or_else(|| "N/A".into())
        ),
    ]);
    lines.join("\n")
}

pub(crate) fn compact_number(value: u64) -> String {
    for (threshold, suffix) in [
        (1_000_000_000_000_u64, "T"),
        (1_000_000_000, "B"),
        (1_000_000, "M"),
        (1_000, "K"),
    ] {
        if value >= threshold {
            let scaled = value as f64 / threshold as f64;
            return if scaled >= 10.0 {
                format!("{scaled:.0}{suffix}")
            } else {
                format!("{scaled:.1}{suffix}")
            };
        }
    }
    value.to_string()
}

pub(crate) fn format_bytes(bytes: u64) -> String {
    if bytes == 0 {
        return "0 B".into();
    }
    let units = ["B", "KB", "MB", "GB", "TB"];
    let index = ((bytes as f64).ln() / 1024_f64.ln()).floor().max(0.0) as usize;
    let index = index.min(units.len() - 1);
    let value = bytes as f64 / 1024_f64.powi(index as i32);
    if value >= 10.0 || index == 0 {
        format!("{value:.0} {}", units[index])
    } else {
        format!("{value:.1} {}", units[index])
    }
}

pub(crate) fn money(value: f64) -> String {
    format!("${value:.2}")
}

fn finding(title: &str, detail: &str, tone: Tone, section: Option<Section>) -> AuditFinding {
    AuditFinding {
        title: title.into(),
        detail: detail.into(),
        tone,
        section,
        action: None,
        evidence: Vec::new(),
    }
}

fn resource_key(row: &ResourceRow) -> String {
    format!("{}-{}", row.kind, row.id)
}

fn worker_preference(
    row: &ResourceRow,
    preferences: &WorkerAuditPreferences,
) -> WorkerAuditPreference {
    preferences
        .get(&resource_key(row))
        .copied()
        .unwrap_or_default()
}

fn has_worker_observability(worker: &ResourceRow) -> bool {
    worker.observability.as_ref().is_some_and(|observability| {
        observability.enabled.unwrap_or(false)
            || observability.logs_enabled.unwrap_or(false)
            || observability.traces_enabled.unwrap_or(false)
            || observability.invocation_logs.unwrap_or(false)
            || observability.logpush.unwrap_or(false)
            || !observability.destinations.is_empty()
    })
}

fn name_evidence<'a>(
    rows: impl Iterator<Item = &'a ResourceRow>,
    preferences: &WorkerAuditPreferences,
) -> Vec<String> {
    rows.take(4)
        .map(|resource| {
            let critical = resource.kind == "worker"
                && worker_preference(resource, preferences) == WorkerAuditPreference::Critical;
            format!(
                "{} ({}, {}{})",
                resource.name,
                resource.kind,
                resource.status,
                if critical { ", critical" } else { "" }
            )
        })
        .collect()
}

fn name_list(rows: &[&ResourceRow]) -> String {
    let names = rows
        .iter()
        .take(3)
        .map(|row| row.name.as_str())
        .collect::<Vec<_>>();
    if rows.len() > names.len() {
        format!("{} +{}", names.join(", "), rows.len() - names.len())
    } else {
        names.join(", ")
    }
}

fn format_endpoint(endpoint: &CollectorEndpoint) -> String {
    let mut parts = vec![format!("{} {}", endpoint.method, endpoint.path)];
    if let Some(status) = endpoint.status {
        parts.push(status.to_string());
    }
    parts.push(format!("{:.0} ms", endpoint.duration_ms));
    if let Some(ray_id) = &endpoint.ray_id {
        parts.push(format!("ray {ray_id}"));
    }
    if let Some(error) = &endpoint.error {
        parts.push(error.clone());
    }
    parts.join(" / ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Account;

    fn resource(id: &str, name: &str, kind: &str, status: &str) -> ResourceRow {
        ResourceRow {
            id: id.into(),
            name: name.into(),
            kind: kind.into(),
            status: status.into(),
            primary_metric: "0 requests".into(),
            secondary_metric: "No telemetry".into(),
            updated_at: None,
            bindings: None,
            observability: None,
        }
    }

    fn snapshot(resources: Vec<ResourceRow>) -> DashboardSnapshot {
        DashboardSnapshot {
            generated_at: "2026-08-23T12:00:00Z".into(),
            range: "24h".into(),
            resources,
            ..DashboardSnapshot::default()
        }
    }

    #[test]
    fn actionable_issues_drop_optional_blanks_and_duplicates() {
        let issues = vec![
            "Worker collector failed".into(),
            "optional Logpush check scoped".into(),
            "Worker collector failed".into(),
            " ".into(),
        ];

        assert_eq!(
            unique_actionable_issues(&issues),
            ["Worker collector failed"]
        );
    }

    #[test]
    fn ignored_worker_is_removed_from_observability_finding() {
        let worker = resource("ignored", "cron-worker", "worker", "quiet");
        let snapshot = snapshot(vec![worker]);
        let preferences = HashMap::from([("worker-ignored".into(), WorkerAuditPreference::Ignore)]);

        let findings = build_audit_findings(&snapshot, &preferences);

        assert!(
            findings
                .iter()
                .all(|finding| finding.title != "Worker observability coverage")
        );
    }

    #[test]
    fn snapshot_diff_tracks_add_remove_status_and_cost() {
        let mut previous = snapshot(vec![
            resource("stable", "api", "worker", "healthy"),
            resource("removed", "old-bucket", "r2", "healthy"),
        ]);
        previous.metrics.cost_usd = Some(5.0);
        let mut next = snapshot(vec![
            resource("stable", "api", "worker", "warning"),
            resource("added", "new-db", "d1", "healthy"),
        ]);
        next.metrics.cost_usd = Some(8.0);

        let changes = diff_snapshots(&previous, &next);
        let titles = changes
            .iter()
            .map(|change| change.title.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            titles,
            [
                "Resources added",
                "Resources removed",
                "Status changed",
                "Cost projection moved"
            ]
        );
    }

    #[test]
    fn report_contains_account_findings_changes_and_usage() {
        let mut snapshot = snapshot(vec![resource("api", "api", "worker", "healthy")]);
        snapshot.account = Some(Account {
            id: "account-id".into(),
            name: "Production".into(),
        });
        snapshot.live = true;
        snapshot.metrics.worker_requests = 12_500;
        let findings = vec![finding(
            "Review worker",
            "One script needs attention.",
            Tone::Warn,
            Some(Section::Workers),
        )];
        let changes = vec![SnapshotChange {
            title: "Status changed".into(),
            detail: "api became warning.".into(),
            tone: Tone::Warn,
        }];

        let report = build_audit_report(&snapshot, &findings, &changes);

        assert!(report.contains("# Cedar audit - Production"));
        assert!(report.contains("Source: Live Cloudflare API"));
        assert!(report.contains("[warn] Review worker"));
        assert!(report.contains("[warn] Status changed"));
        assert!(report.contains("Worker requests: 12K"));
    }
}
