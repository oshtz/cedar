# Cedar

<img src="assets/icon.png" alt="Cedar app icon" width="64" height="64">

A local-first Cloudflare audit and developer-report app, built as a native Rust desktop application with GPUI.

Cedar turns Cloudflare inventory, usage, sync health, recent drift, observability coverage, and Workers cost projections into an operational view and a paste-ready Markdown report. Credentials and snapshots stay local.

## Stack

- Rust 2024
- GPUI with `gpui-component`
- Embedded Geist and Geist Mono typography
- Tokio and Reqwest for Cloudflare API work
- SQLite for snapshots and preferences
- The operating-system keychain for the Cloudflare token

There is no browser runtime, WebView, JavaScript, React, Vite, or Tauri layer.

## Features

- Accounts, zones, Workers, Pages, D1, R2, and KV inventory
- Local account audits with actionable findings
- Snapshot comparison and recent-change reporting
- Markdown developer-report copy
- Workers, zone, D1, R2, and KV usage for `24h`, `7d`, and `30d`
- Audit Logs, Logpush, Workers Observability, and zone security signals when permitted
- Per-collector API diagnostics, latency, rate-limit headers, failures, and Ray IDs
- Analytics-derived Workers Paid cost projection
- Graceful partial results when Cloudflare scopes or plan access are limited

## Run

Install the stable Rust toolchain, then:

```powershell
cargo run
```

The first build compiles GPUI and its graphics stack, so it takes longer than later builds.

## Validate

```powershell
cargo fmt --check
cargo test --locked
cargo check --locked
```

Build an optimized native executable:

```powershell
cargo build --release --locked
```

On Windows, the result is `target/release/cedar.exe` and has no WebView runtime dependency.

Build the complete Windows release artifact set locally:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/package-windows.ps1
```

That command builds the optimized GPUI executable, creates a standalone `.exe` and a `.zip`,
generates SHA-256 checksums, verifies the archive, and launches the packaged native app for a short
startup smoke test. Artifacts are written to `dist-release/windows/`.

## Release

GitHub Actions builds native Windows x64 and universal macOS releases from `v*` tags. The pipeline
validates that the tag exactly matches the version in `Cargo.toml`, runs the Rust quality gates,
signs when configured, packages the app, verifies every checksum, and creates a draft GitHub
Release. There is no Tauri bundler, updater manifest, JavaScript build, WebView payload, or Enigma
portable wrapper in the release path.

Release assets:

- `Cedar_<version>_windows-x64.exe`
- `Cedar_<version>_windows-x64.zip`
- `SHA256SUMS-windows.txt`
- `Cedar_<version>_macos.dmg`
- `Cedar_<version>_macos.app.zip`
- `SHA256SUMS-macos.txt`

See [RELEASING.md](RELEASING.md) for the tag, signing, notarization, verification, and publishing
checklist.

### Visual QA

Cedar includes an offline visual-QA mode backed by deterministic fixture data and an in-memory
database. Capture every major surface in dark and light themes at the minimum, standard, and wide
window presets:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/visual-qa.ps1
```

Run a focused capture while iterating:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/visual-qa.ps1 `
  -Scenario shortcuts `
  -Theme dark `
  -Viewport minimum `
  -SkipBuild
```

Captures and their SHA-256 manifest are written to `artifacts/visual-qa/`. The capture runner opens
and foregrounds each native window briefly so GPU-rendered GPUI content is captured accurately.
List or launch fixture states directly with:

```powershell
target/release/cedar.exe --list-visual-qa
target/release/cedar.exe --visual-qa workers-inspector --theme light --viewport 1440x960
```

## Data and credentials

Cedar preserves its existing local data contracts:

- Database: the platform local-data directory under `Cedar/cedar.sqlite3`
- Keychain service: `cedar`
- Keychain entry: `cloudflare-api-token`

Preferences that previously lived in browser local storage now live in the SQLite configuration table.

## Cloudflare coverage

Cedar paginates Cloudflare REST collectors for accounts, Workers, D1, R2, KV, zones, Audit Logs, and Logpush jobs. Pages projects are fetched without explicit pagination options because Cloudflare may reject them for that endpoint. Zone GraphQL analytics run in batches across discovered zones.

Cedar uses current GraphQL datasets instead of the retired Zone Analytics REST endpoint. Security Events use `firewallEventsAdaptiveGroups` first and fall back to raw `firewallEventsAdaptive` rows when necessary.

### Token scopes

| Mode | Scopes | Result |
| --- | --- | --- |
| Minimal read-only | Account Settings Read, Account Analytics Read, Workers Scripts Read, Cloudflare Pages Read, Zone Read, Zone Analytics Read, Audit Logs Read, D1 Read, Workers R2 Storage Read, Workers KV Storage Read | Inventory, usage, zones, audit logs, and local cache |
| Full observability | Minimal scopes plus Logs Edit and Workers Observability Edit | Adds Logpush inventory, Workers Observability keys, destinations, and telemetry checks |

Cloudflare write-gates some read-like observability endpoints. Logpush inventory requires Logs Edit, and Workers Observability telemetry requires Workers Observability Edit.

## Layout

```text
src/main.rs       GPUI application entry point
src/ui.rs         Native window, state, and interface
src/backend.rs    Cloudflare collectors, keychain, and SQLite
src/audit.rs      Audit findings, snapshot diff, and report generation
assets/           App icons and packaging assets
scripts/          Native packaging, signing, smoke-test, and visual-QA tooling
```

## Status and limits

Cedar is a personal learning project. Issues and forks are welcome, but there are no support or roadmap promises.

- Cost is a Workers Paid projection, not invoice-grade billing.
- Cedar does not call Cloudflare's gated billing usage APIs.
- Advanced collectors degrade gracefully when the token, plan, or account cannot access a dataset.
- GPUI is still pre-1.0, so framework API upgrades may require code changes.

## License

MIT. See [LICENSE](LICENSE).

Geist font files are distributed under the SIL Open Font License 1.1. See `assets/fonts/OFL-Geist.txt`.
