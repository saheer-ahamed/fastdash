# fastdash - project instructions

A super-fast Claude usage + connectors dashboard.
Tauri v2 (Rust backend, `src-tauri/`) + React/TypeScript frontend (`src/`), built and released through GitHub Actions.

These project rules add to my global `~/.claude/CLAUDE.md`; where they overlap, the stricter of the two wins.

## Conventions (enforced - a violation blocks the commit or the PR)

### Commit messages - Conventional Commits, required

Every commit message must be a [Conventional Commit](https://www.conventionalcommits.org): `type(optional-scope): subject`.
This is not cosmetic: the release workflow parses these to compute the next version, so a malformed message either breaks versioning or silently ships nothing.

Allowed types (defined once in `commitlint.config.js`, mirror any change here):

| Type | Meaning | Release effect |
|------|---------|----------------|
| `feat` | New user-facing capability | minor bump |
| `fix` | Bug fix | patch bump |
| `perf` | Performance improvement | patch bump |
| `refactor` | Behavior-preserving code change | none |
| `style` | Formatting only (fmt / prettier / eslint) | none |
| `docs` | Documentation only | none |
| `test` | Tests only | none |
| `build` | Build system, deps, bundling | none |
| `ci` | CI / workflows | none |
| `chore` | Maintenance, releases, tooling | none |
| `revert` | Reverts a previous commit | none |

A breaking change is marked with `!` after the type/scope (`feat!:`) or a `BREAKING CHANGE:` footer, and forces a **major** bump.

Subject rules: lower-case first letter, no trailing period, imperative mood, header <= 100 chars.
Suggested scopes (not hard-enforced, but keep them consistent): `ui`, `rust`, `engine`, `connectors`, `release`, `deps`, `ci`, `docs`.

Examples:

```
feat(ui): add developer mode toggle
fix(engine): stop scheduler double-firing on config reload
chore(release): v0.2.0
```

### Branch names - `type/short-description`, required

Branches must match `^(feat|fix|perf|refactor|style|docs|test|build|ci|chore|revert)/[a-z0-9._-]+$`.
Examples: `feat/dev-mode`, `fix/scheduler-race`, `chore/husky-pre-commit`.

### Pull requests

The **PR title** must itself be a Conventional Commit, because a squash-merge turns the title into the commit subject on `main`, and that subject is what the release workflow reads.
Fill in the PR template, including the release-impact box.

### Where enforcement lives (defense in depth)

- Local, pre-commit: `.husky/pre-commit` runs eslint + `tsc` (frontend) and `cargo fmt --check` + `clippy -D warnings` (Rust) on staged files.
- Local, commit-msg: `.husky/commit-msg` runs commitlint against the message.
- CI, on every PR: `.github/workflows/pr-checks.yml` validates the PR title, the branch name, and every commit in the PR.

Wire the `pr-checks` jobs in as **required status checks** (Settings -> Branches -> `main`) so a failing check actually blocks the merge.
`git commit --no-verify` bypasses the local hooks in a genuine emergency only; CI still enforces on the PR.

## Releasing - fully automatic, do not tag or bump by hand

`.github/workflows/release.yml` runs on every push to `main`:

1. Analyzes the Conventional Commits since the last tag and computes the next version (no releasable commits -> no release).
2. Bumps `package.json`, `package-lock.json`, `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, and `src-tauri/tauri.conf.json` via `scripts/bump-version.sh` - the single source of truth for a bump.
3. Commits `chore(release): vX.Y.Z [skip ci]` and pushes the tag to `main`.
4. Builds the signed installers + portable zip and publishes the GitHub release (not a draft).
5. Uploads `latest.json` so installed apps self-update via the in-app updater.

Never bump the version or push a `v*` tag manually; let the merge do it.
`workflow_dispatch` on the workflow is the only manual escape hatch (forces a chosen bump).

### In-app auto-update

The app checks GitHub on launch (`src/updater.ts`) and self-updates from the signed release.
Update artifacts are signed with a keypair: the **public** key lives in `src-tauri/tauri.conf.json` (`plugins.updater.pubkey`); the **private** key and its password are the GitHub repo secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
The private key (`fastdash.key`) must never be committed - it is gitignored.
Updates ride the signed NSIS installer; Scoop and `install.ps1` keep using the portable zip from the same release.

## Layout

- `src-tauri/src/engine/` - connector-agnostic core: `Connector` trait, registry, config, secrets (OS keychain), snapshot cache, scheduler, i18n.
- `src-tauri/src/connectors/` - self-contained connectors behind the trait; adding one needs zero UI changes.
- `src-tauri/src/ipc.rs` - the Tauri command surface exposed to the frontend.
- `src/` - React frontend; the UI only ever renders generic `Panel`s.

## Build & check commands

- `npm run dev` - Vite dev server; `npm run tauri dev` - full desktop app.
- `npm run lint` / `npm run typecheck` - frontend gates.
- In `src-tauri/`: `cargo fmt --all`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo check`.
