# fastdash

A super-fast desktop dashboard for Claude usage, with pluggable connectors (GitHub and Sentry, with Slack planned).

Built with Tauri v2 (Rust core) and a React + TypeScript frontend.

## What it shows

- **Claude**: token usage (total and per model), efforts used, weekly usage, the current 5-hour window, reset countdown, and cost - read from local `~/.claude` transcripts, with official `/usage` numbers overlaid when available.
- **GitHub**: per selected org, today's per-contributor PR counts (opened / merged / closed-without-merge / open), line contributions (based on PRs merged today), and the PR list with repos.
- **Sentry**: per organization, the unresolved issues that fired in the selected range - how many, how many are new, the event total, which projects they came from, and the issue list with events, users affected, and when each was last seen.
- **Slack** (planned, not yet available): per workspace, the channels that mentioned me today.

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

## Updating

fastdash updates itself.
On launch it checks the GitHub releases feed and, if a newer signed build exists, downloads and installs it - no need to re-run the installer or the Scoop command.

Scoop users can still update through Scoop if they prefer (`scoop update fastdash`); both paths pull from the same release.

## Connecting GitHub

Add an account under **Connectors -> GitHub**, give it a label and the orgs to track, then authorize it one of three ways.

### Connect with GitHub (device flow)

The button requests exactly the scopes fastdash needs - `repo`, `read:org`, `read:user` - so nothing is left to configure.

One catch that is not about scopes: an organization can restrict third-party OAuth apps.
Until an org owner grants fastdash access to that org, GitHub hides the org's repositories from the token entirely - `/user/orgs` comes back empty and searches fail - no matter which scopes were approved.
fastdash detects this and links you to the grant page.
Personal access tokens are not subject to that policy, which is the usual reason to reach for one.

### Classic personal access token

The only option that lights up every panel.

| Scope | What it unlocks |
|-------|-----------------|
| `repo` | PRs in private org repositories - the counts, line contributions, and PR list |
| `read:org` | Resolving the orgs the account belongs to |
| `read:user` | **Private activity in the contribution heatmap** |

Without `read:user` the heatmap is not an error - GitHub simply leaves private activity out and reports a total of 0, so a busy year renders empty.
If it is still empty with `read:user` set, turn on **Settings -> Public profile -> Include private contributions on my profile** on GitHub.

### Fine-grained personal access token

Set the **resource owner** to the organization you want to track, and either grant access to all repositories or pick the ones that should count.

| Permission | Level | What it unlocks |
|------------|-------|-----------------|
| Repository -> Metadata | Read-only | Mandatory; GitHub enables it with any other repository permission |
| Repository -> Pull requests | Read-only | PR data, including the additions/deletions behind line contributions |
| Repository -> Issues | Read-only | The search endpoint fastdash uses covers issues and PRs together |
| Repository -> Contents | Read-only | Makes the repository visible to search at all |

No account-level permission is needed, and none of them helps the heatmap: fine-grained tokens have no equivalent of `read:user`, so the contribution grid is limited to public activity.
Use a classic token if you want it complete.

Two more constraints come from GitHub, not fastdash:

- A fine-grained token belongs to **one** resource owner, so it cannot span several orgs. Tracking two orgs this way means two account rows, each with its own token.
- If the org enforces a token policy, an owner has to approve the token before it works.

## Connecting Sentry

Add a connection under **Connectors -> Sentry**, give it a label and an auth token, then leave the rest at their defaults unless one of the notes below applies.

### The token

Create a **user auth token** at **Sentry Settings -> Account -> User Auth Tokens** with three read scopes.

| Scope | What it unlocks |
|-------|-----------------|
| `event:read` | The issue stream itself - every number on the dashboard comes from it |
| `project:read` | The project each issue belongs to, and the events-by-project breakdown |
| `org:read` | Discovering which organizations the token can see, so the Organizations field can be left empty |

An **internal integration** token (Settings -> Developer Settings) works the same way and is the better choice for a shared or team setup, since it is not tied to one person's account.

Sentry's third token kind does not work, and cannot be made to: an **organization auth token** (`sntrys_...`) exists for CI - uploading releases and source maps - and has no `event:read` scope to grant.
fastdash recognizes the prefix and says so rather than telling you to tick a box that is not there.

### Sentry URL

Leave it empty for `sentry.io`.

- **EU-region organizations** are served from `https://de.sentry.io`; the `sentry.io` host will not find them.
- **Self-hosted** installs use their own origin, e.g. `https://sentry.example.com` - the part of the URL before `/organizations/`. A pasted `/api/0` suffix is stripped for you.

### Organizations

The slug from your Sentry URL, the part after `/organizations/`.
Leave the field empty to report on **every** organization the token can see, which is what `org:read` is for.
Naming them explicitly is the way to skip that discovery call, and the only way to work with a token that has no `org:read`.

Two things the connector does deliberately:

- It reports on **unresolved** issues only. That is a state filter, not a date one, so an issue that first fired months ago and is still erroring today shows up - which is the point. **New issues** is the stat that separates the two.
- **Events** counts what happened inside the selected date range, not an issue's lifetime total. Projects are not narrowable: the connector asks for every project the token can read in one request and derives the breakdown from the results, rather than spending a round trip per project.

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

Scaffold: core engine, connector trait, generic panel renderer, and the Claude, GitHub and Sentry connectors are wired.
Connector implementations are in progress; the Slack connector is planned but not yet implemented.
