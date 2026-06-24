<h1>
  <img src="src-tauri/icons/icon.png" alt="Cedar app icon" width="28" height="28">
  Cedar
</h1>

<p>
  <strong>A local-first Cloudflare audit-to-dev-report app for developers who want inventory, drift, coverage, and cost signals without pretending the dashboard already answers that.</strong>
</p>

<p>
  <img alt="Tauri" src="https://img.shields.io/badge/Tauri-000000?style=flat&logo=tauri&logoColor=white">
  <img alt="React" src="https://img.shields.io/badge/React-000000?style=flat&logo=react&logoColor=white">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-000000?style=flat&logo=rust&logoColor=white">
  <img alt="TypeScript" src="https://img.shields.io/badge/TypeScript-000000?style=flat&logo=typescript&logoColor=white">
  <img alt="Vite" src="https://img.shields.io/badge/Vite-000000?style=flat&logo=vite&logoColor=white">
  <img alt="SQLite" src="https://img.shields.io/badge/SQLite-000000?style=flat&logo=sqlite&logoColor=white">
  <img alt="Cloudflare" src="https://img.shields.io/badge/Cloudflare-000000?style=flat&logo=cloudflare&logoColor=white">
</p>

Cedar is a personal learning project built around a real Cloudflare visibility problem. It turns Cloudflare inventory, usage, sync health, recent drift, observability coverage, and Workers cost projection into a desktop audit view and a paste-ready developer report without sending your token or snapshots to a hosted service.

## Why this exists

I built Cedar to learn the parts of desktop software that only show up in a real app: Tauri, a Rust backend, React UI state, OS keychain storage, local SQLite snapshots, Cloudflare REST/GraphQL edge cases, and signed desktop release packaging.

The practical goal is narrower: run a local account audit, get a developer report, and stop treating six dashboard tabs as a system view.

## Who this is for

- Developers running several Cloudflare resources who want a developer report instead of another dashboard tab
- People studying a Tauri + Rust + React app that talks to a large external API and stores data locally
- Not a fit for teams that need invoice-grade billing, multi-user workflows, guaranteed maintenance, or full observability across every Cloudflare plan

## Status

Personal learning project. Issues and forks are welcome, but I make no support or roadmap promises.

## What it does

- Lists accounts, zones, Workers, Pages, D1, R2, and KV
- Produces a local account audit with actionable findings and recent snapshot changes
- Copies a Markdown developer report for issues, PRs, and handoff notes
- Shows Workers, zone, D1, R2, and KV usage across `24h`, `7d`, and `30d`
- Surfaces Audit Logs, Logpush job coverage, Workers Observability, and zone security/event summaries when scoped
- Shows collector diagnostics for Cloudflare API calls, failures, latency, rate-limit headers, and Ray IDs
- Tracks health per Cloudflare area so one API failure does not hide everything else
- Keeps recent snapshots in local SQLite for history and faster reloads
- Estimates Workers Paid usage from analytics-derived activity
- Shows Worker binding links only when Cloudflare returns metadata that matches discovered resources

## Run

```powershell
npm install
npm run desktop
```

For UI-only development:

```powershell
npm run dev
```

Live Cloudflare access requires the Tauri desktop runtime.

## Cloudflare Coverage

Cedar paginates Cloudflare REST list collectors for accounts, Workers, D1, R2, KV, zones, Audit Logs, and Logpush jobs. Pages projects are fetched without explicit `page` or `per_page` options because Cloudflare can reject those list options for that endpoint. Zone GraphQL analytics run in batches of 10 across every discovered zone. Accounts with many zones will make more Cloudflare API calls, and Cloudflare plan/token limits can still make individual datasets unavailable. A defensive 100-page REST guard fails the affected collector loudly instead of silently truncating large accounts.

Cloudflare's retired Zone Analytics REST endpoint is not used; Cedar relies on current GraphQL datasets. Security Events use `firewallEventsAdaptiveGroups` first and fall back to raw `firewallEventsAdaptive` rows when Cloudflare denies the aggregate path. Traffic-level Security Analytics still uses `httpRequestsAdaptiveGroups`.

Cedar only stores the connected token locally in the OS keychain and stores snapshots in local SQLite.

## Token Scopes

Cedar can run in two token modes:

| Mode | Scopes | What to expect |
| --- | --- | --- |
| Minimal read-only | Account Settings Read, Account Analytics Read, Workers Scripts Read, Cloudflare Pages Read, Zone Read, Zone Analytics Read, Audit Logs Read, D1 Read, Workers R2 Storage Read, Workers KV Storage Read | Inventory, usage, zones, audit logs, and local cache. Logpush and Workers Observability panels may show scoped gaps. |
| Full observability | Minimal scopes plus Logs Edit and Workers Observability Edit | Adds account/zone Logpush inventory, Workers Observability keys, destinations, and telemetry checks. |

Cedar's token setup lets you open Cloudflare's token form in either mode. Full observability pre-fills:

- Account Settings Read, Account Analytics Read, Workers Scripts Read, Workers Observability Edit
- Cloudflare Pages Read, Zone Read, Zone Analytics Read, Logs Edit, Audit Logs Read
- D1 Read, Workers R2 Storage Read, Workers KV Storage Read

Cloudflare token-template URLs use `edit` for permissions the dashboard labels as write/edit. Cedar requests those only where Cloudflare write-gates read-like observability endpoints. Minimal read-only mode omits Logs Edit and Workers Observability Edit.

- Logpush job inventory requires Logs Edit/Write, even for listing jobs.
- Workers Observability telemetry queries require Workers Observability Edit/Write.

When Cedar is already connected to an account, the token button opens Cloudflare's account token form. Account tokens require permission to create account-owned tokens, typically Super Administrator access. If you use the first-run user token form instead, confirm that Account scopes include Logs Edit and that Zone scopes apply to all monitored zones with Zone Analytics Read and Logs Edit. Cloudflare can show account-level Logs and zone-level Logs as separate rows; Cedar needs both rows for full account and zone Logpush inventory.

## License

MIT. See [LICENSE](LICENSE).

## Scripts

| Command | Purpose |
| --- | --- |
| `npm run dev` | Start Vite at `127.0.0.1:5188` |
| `npm run desktop` | Start the Tauri desktop app |
| `npm run typecheck` | Type-check the frontend without emitting files |
| `npm run lint` | Run the frontend lint/type gate |
| `npm test` | Run frontend unit tests |
| `npm run build` | Type-check and build the frontend |
| `npm run preview` | Preview the built frontend at `127.0.0.1:4173` |
| `npm run desktop:build` | Package the desktop app |
| `npm run desktop:build:no-bundle` | Build the Windows app executable without installers for portable packaging |
| `npm run release:check` | Verify release metadata, version alignment, and release workflow shape |
| `npm run validate` | Run the frontend lint, tests, and build gates |

Rust validation:

```powershell
cd src-tauri
cargo fmt --check
cargo test
cargo check
```

## Layout

```text
src/              React UI
src-tauri/src/    Rust commands, Cloudflare collectors, keychain, SQLite
src-tauri/icons/  App icons and platform assets
public/           Static frontend assets
```

## Limits

- Cost is a Workers Paid projection, not invoice-grade billing.
- Cedar does not call Cloudflare's gated billing usage APIs.
- Advanced observability collectors degrade gracefully when the token, plan, or account has no access to a dataset.
