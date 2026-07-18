# fastdash - Design

The source of truth for what we are building and how the pieces fit.
Each connector is built in isolation against the contract in "The isolation contract" below.

## Goals

A super-fast, simple desktop dashboard.
One glance shows Claude usage; connectors add GitHub and Slack views.
"Fast" means the UI always reads a warm in-memory cache and never blocks on the network.

## Stack

- Tauri v2 desktop shell (small binary, native speed, cross-platform).
- Rust core (`src-tauri/`) for all fetching, parsing, and aggregation.
- React + TypeScript frontend (`src/`) as a thin, generic renderer.
- Secrets in the OS keychain via the `keyring` crate (Windows Credential Manager).
- Non-secret config in `%APPDATA%/fastdash/config.toml`.

## The isolation contract

Every connector (Claude included) plugs into the core through one trait, so a new connector touches zero core and zero frontend code.

```rust
#[async_trait]
pub trait Connector: Send + Sync {
    fn meta(&self) -> ConnectorMeta;                       // id, name, icon, refresh cadence
    async fn fetch(&self, ctx: &FetchCtx) -> Result<Snapshot, ConnectorError>;
}
```

A `fetch` returns a `Snapshot { status, panels, fetched_at, next_refresh_secs }`.
`status` is a `Health` enum (`Ok` / `NeedsAuth` / `RateLimited` / `Error`) that drives the sidebar status dot and any banner.

Data crosses the Rust to JS boundary only as generic render primitives, so the frontend never learns what "GitHub" is:

- `StatCards` - KPI tiles (tokens, cost, mentions today).
- `Meter` - a progress bar with an optional limit (5h window, weekly usage).
- `Table` - sortable tables (PR counts, per-model tokens, line contributions).
- `BarList` - horizontal bars (effort split, top contributors).
- `List` - a vertical list of links (Slack mentions, PR list).

The core stays agnostic: `Registry` holds the connectors, a scheduler refreshes each on its own interval into a cache, and the UI reads the cache.
The engine types live in `src-tauri/src/engine/`; connectors live in `src-tauri/src/connectors/<name>/`.

## Connector: Claude

Fully offline for tokens, effort, and cost; the official limit and reset are best-effort.

Source data is the local transcripts under `~/.claude/projects/**/*.jsonl`.
Each assistant turn is one JSONL line carrying `timestamp`, `effort` (for example `"medium"`), `session_id`, and a `message` object with `model` plus a full `usage` block (input, output, `cache_creation` 1h/5m, `cache_read`, `service_tier`, and web-search/web-fetch counts).

Everything the panel needs is derivable from that, except the official weekly limit percentage and the exact reset clock, which live on Anthropic's servers.

Owned modules (in `connectors/claude/`):

- `parse` - a `notify`-based watcher that reads only bytes appended to the active session file, plus a cold full-scan on startup.
- `aggregate` - rollups by model, effort, day, week, and 5-hour block.
- `usage_api` - the official `/usage` pull using the OAuth token at `~/.claude/.credentials.json` (also check `~/.claude/stats-cache.json`), with an automatic fallback to limits computed from local history so the bars never go blank.
- `pricing` - token to cost using a per-model pricing table (input, output, cache-write, cache-read classes).

Panels: a `Meter` for the 5h window and one for weekly usage, `StatCards` for total tokens and cost, a `Table` of per-model tokens, and a `BarList` for the effort split.

## Connector: GitHub

Config supports multiple accounts, each with its own PAT in the keychain: work (`saheer-zro`) and personal (`saheer-ahamed`).
Per account the user selects organizations from a checklist populated by `/user/orgs`.

Fetch strategy avoids listing all repos and PRs.
It uses the Search API for the date-filtered sets, then a single batched GraphQL call to enrich only that set with additions, deletions, state, and author.

- Opened today: `org:X type:pr created:<range>`
- Merged today: `org:X type:pr merged:<range>`
- Closed, not merged: `org:X type:pr closed:<range> is:unmerged`
- Still open: `org:X type:pr created:<range> is:open`

"Today" uses IST datetime bounds, not a bare date, so PRs near midnight are attributed correctly: `created:2026-07-18T00:00:00+05:30..2026-07-18T23:59:59+05:30`.

Three tables:

1. Contributor by Opened / Merged / Closed-no-merge / Open (columns independent), sorted by total.
2. Contributor line contributions: Additions / Deletions / Net / number of PRs.
   Decision: line contributions are based on PRs **merged today** (not opened today).
3. PR list: Repo, PR title (link), Author, State, +/-, time.

Rate limits: Search is 30 requests/minute authenticated, which is ample; cache per (org, day), refresh every 60s or on manual refresh, and honor `X-RateLimit-Remaining` with backoff.
Optionally filter bot authors (dependabot and similar).

This mirrors the logic already proven in the `daily-pr-stats` skill.

## Connector: Slack

Config picks a workspace; multiple workspaces are supported, each with its own token.

Auth is the only real setup cost: `search.messages` requires a user token (`xoxp`) with `search:read`; bot tokens cannot search.
For v1 the user creates a minimal Slack app, adds the `search:read` user scope, installs it, and pastes the User OAuth Token once.
The token lives in the keychain.

Fetch: `auth.test` resolves the current user id and workspace name, then `search.messages` with `<@Uxxxx> after:<today> sort:timestamp` returns messages that mention me today, grouped by channel and refined against IST midnight client-side.

Panels: `StatCards` for total mentions and active channels, and a `Table`/`List` of Channel, mentions today, last time, and preview linking to the Slack permalink.

Fallback: if a workspace forbids `search:read`, degrade to scanning `conversations.list` + `conversations.history`, which is heavier and needs more scopes, so search is the default.

## Refresh cadences

- Claude: file-watch driven, effectively instant (2s debounce).
- GitHub: every 60s or manual.
- Slack: every 60s or manual.

All are async, independent, and non-blocking; the UI always reads the cache.

## Parallel build plan

The connectors are isolated behind the trait, so they are built concurrently in separate git worktrees:

- `feat/claude` - the Claude connector modules.
- `feat/github` - the GitHub connector.
- `feat/slack` - the Slack connector.
- `feat/core` - config loading, keychain, and the refresh scheduler.

Shared surfaces (`engine/connector.rs`, `engine/panel.rs`) are stable; connectors depend on them but not on each other.
