# fastdash

A super-fast desktop dashboard for Claude usage, with pluggable connectors (GitHub, Slack).

Built with Tauri v2 (Rust core) and a React + TypeScript frontend.

## What it shows

- **Claude**: token usage (total and per model), efforts used, weekly usage, the current 5-hour window, reset countdown, and cost - read from local `~/.claude` transcripts, with official `/usage` numbers overlaid when available.
- **GitHub**: per selected org, today's per-contributor PR counts (opened / merged / closed-without-merge / open), line contributions (based on PRs merged today), and the PR list with repos.
- **Slack**: per workspace, the channels that mentioned me today.

## Prerequisites

- Rust (stable, MSVC toolchain on Windows)
- Node.js 18+ and npm
- On Windows: WebView2 (preinstalled on Windows 11) and the C++ build tools

## Development

```bash
npm install
npm run tauri dev
```

## Build

```bash
npm run tauri build
```

## Architecture

See [DESIGN.md](./DESIGN.md).

The core is connector-agnostic: every connector implements one `Connector` trait and emits generic render `Panel`s, so the UI never learns connector specifics.
Each connector is developed in its own worktree.

## Status

Scaffold: core engine, connector trait, generic panel renderer, and three connector stubs are wired.
Connector implementations are in progress.
