# fastdash

A super-fast desktop dashboard for Claude usage, with pluggable connectors (GitHub, Slack).

Built with Tauri v2 (Rust core) and a React + TypeScript frontend.

## What it shows

- **Claude**: token usage (total and per model), efforts used, weekly usage, the current 5-hour window, reset countdown, and cost - read from local `~/.claude` transcripts, with official `/usage` numbers overlaid when available.
- **GitHub**: per selected org, today's per-contributor PR counts (opened / merged / closed-without-merge / open), line contributions (based on PRs merged today), and the PR list with repos.
- **Slack**: per workspace, the channels that mentioned me today.

## Install

Windows 10 or 11, 64-bit.
Latest builds: <https://github.com/saheer-ahamed/fastdash/releases/latest>

From the terminal (no admin rights, no SmartScreen prompt):

```powershell
irm https://raw.githubusercontent.com/saheer-ahamed/fastdash/main/docs/install.ps1 | iex
```

Or with [Scoop](https://scoop.sh):

```powershell
scoop bucket add fastdash https://github.com/saheer-ahamed/fastdash
scoop install fastdash
```

Or grab the `.exe` installer from the [latest release](https://github.com/saheer-ahamed/fastdash/releases/latest).

Builds are not yet code-signed, so the browser-downloaded installer trips SmartScreen ("More info" then "Run anyway").
The terminal and Scoop paths above are unaffected, because only browsers apply the Mark-of-the-Web that SmartScreen keys off.
Every release ships `SHA256SUMS.txt` so downloads can be verified.

## Prerequisites

These are for building from source; installing a release needs none of them.

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
