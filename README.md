# fastdash

A super-fast desktop dashboard for Claude usage, with pluggable connectors (Claude, GitHub and Sentry today; Slack planned).

Built with Tauri v2 (Rust core) and a React + TypeScript frontend.

## What it shows

- **Claude**: token usage (total and per model), efforts used, the current 5-hour window, the weekly windows, reset countdowns, and cost.
  Numbers come from this machine's local `~/.claude` transcripts by default; the plan-limit meters use Claude Code's own login, and connecting an Anthropic Console organization replaces the estimates with official token counts and the amount actually billed.
- **GitHub**: per selected org, per-contributor PR counts for the selected range (opened / merged / closed-without-merge / open), line contributions (based on PRs merged in range), the PR list with repos, and the contribution heatmap.
- **Sentry**: per organization, the unresolved issues that fired in the selected range - how many, how many are new, the event total, which projects they came from, and the issue list with events, users affected, and when each was last seen.
- **Slack** (planned, not yet available): per workspace, the channels that mentioned me today.

Every panel is generic: connectors emit render `Panel`s and the UI never learns connector specifics, so adding a connector needs no UI change.

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

The Rust core builds on macOS too (including keychain-backed credentials), but releases are produced on Windows only, so there is no macOS download yet.

## Updating

fastdash updates itself.
On launch it checks the GitHub releases feed and, if a newer signed build exists, offers a non-blocking toast to install and restart - the download never starts without a click.
The check runs once at startup, so an already-running app notices a new release on its next launch.

Scoop users can still update through Scoop if they prefer (`scoop update fastdash`); both paths pull from the same release.

## The date filter

One filter at the top of the app applies to every dashboard: Today (the default), Yesterday, Last 7 days, Last 30 days, or a custom span.
Presets resolve to concrete calendar days in your local timezone, and every connector fetch carries that span.

Two readings deliberately ignore it, because they are "right now" values rather than a period: Claude's plan-limit meters and the GitHub contribution heatmap.

## Refreshing

Nothing polls on a timer in the background.
The frontend drives every fetch, and only for the dashboard currently on screen while the app window has focus - switching tabs or clicking away costs nothing, and a cached result younger than the connector's TTL is reused instead of refetched.

Fetches are also serialized per connector: starting a new one cancels the previous, so flipping between sub-tabs cannot burn through the GitHub Search budget or repaint stale numbers out of order.

## The widget

The button at the end of the topbar shrinks the whole app into a small always-on-top panel you can leave in the corner of the screen while you work.
It carries one tab per connected connector, and only for GitHub and Claude: your own PRs merged and created today plus the lines those merged PRs touched, and Claude's live session and weekly plan meters.
GitHub adds a row of sub-tabs when more than one account is configured, so each login's own numbers are one click apart.
Drag it by its header, and press the arrow in its top-right corner to grow back into the dashboard - at exactly the size and position it had before.

Nothing in the widget refreshes on a timer, not even while it is on screen.
A tab fetches once when you first open it with nothing to show, and after that only when you press its refresh button, so the timestamp along the bottom is part of the reading.
Each account is its own reading, cached separately, so switching between them repaints rather than refetches.

The button is absent when neither GitHub nor Claude is connected, since there would be nothing to put in it.

## Settings

Under **Settings -> General**:

- **Theme**: System, Dark, Light, Midnight, Amber, Green, or Paper.
- **Language**: English today; the string catalog under `locales/<lang>/` is shared by the Rust backend and the frontend, so a new language is a set of JSON files plus one registration.
- **Timezone** (IANA, e.g. `Asia/Kolkata`): the day boundary the date filter and the daily rollups use.
- **Filter bot authors**: drops dependabot and similar from the GitHub contributor tables.

**Settings -> About** shows the running version, and hides a developer mode behind repeated clicks on it.

## Connecting Anthropic Console

Optional.
Without it, the Claude dashboard still works from local transcripts; with it, the usage and cost tiles switch from estimates to Anthropic's own numbers.

Go to **Settings -> Claude**, then paste an **Admin API key** (`sk-ant-admin01-...`) created in Console under **Settings -> Admin keys**.
The key is stored in the OS keychain, and Console shows it only once.

Two limits are Anthropic's, not fastdash's:

- Admin keys require the admin role in a Claude Console **organization**; they are not offered to individual accounts.
- The reports cover **Console-billed** usage only. Claude Code running against a Pro or Max subscription is not billed through Console and never appears there, so an empty report is a normal outcome and is reported in words rather than as zeros. That is also why the local transcript scan stays.

There is no "Sign in with Claude" button because Anthropic runs no third-party OAuth client registration, and their policy reserves subscription OAuth tokens for Claude Code and claude.ai.

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

### Scopes beyond an org

An account's tracked scope does not have to be an organization.
A personal repository owner works, and an `author:<login>` scope counts the PRs a person wrote anywhere the token can see, which is the way to include work outside the orgs you listed.

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

## Where credentials live

Every token and API key is stored in the OS keychain (Windows Credential Manager, Keychain on macOS), never in the config file and never in the repo.
Config holds labels, orgs and preferences only.

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

`npm run dev` alone runs the frontend in a browser, which is enough for UI work but has no Tauri backend.

Gates that CI and the git hooks enforce:

```bash
npm run lint
npm run typecheck
cd src-tauri && cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings
```

Commits, branches and PR titles follow Conventional Commits; see [CLAUDE.md](./CLAUDE.md) for the exact rules and where each one is enforced.

## Build

```bash
npm run tauri build
```

Releases are automatic: merging to `main` computes the next version from the Conventional Commits since the last tag, builds the signed installers, and publishes the GitHub release.
Never bump a version or push a `v*` tag by hand.

## Architecture

See [DESIGN.md](./DESIGN.md).

- `src-tauri/src/engine/` - connector-agnostic core: the `Connector` trait, registry, config, keychain secrets, snapshot cache, the shared fetch path, i18n, and the shared date filter.
- `src-tauri/src/connectors/` - self-contained connectors behind that trait.
- `src-tauri/src/ipc.rs` - the Tauri command surface exposed to the frontend.
- `src-tauri/src/pip.rs` - widget mode: shrinking the one window and putting it back.
- `src/` - the React frontend, which only ever renders generic `Panel`s.

## Status

The core engine, connector trait, generic panel renderer, and the Claude, GitHub and Sentry connectors are all shipping.
The Slack connector is planned but not yet implemented.
