# fastdash in the browser

One UI. `crates/fastdash-app` is the GPUI window on the desktop and the same window in the browser. There is no second dashboard.

## How the window is built

GPUI on crates.io (0.2.2) has no browser platform. This repo depends on Zed's tree instead, pinned to one commit in `crates/fastdash-app/Cargo.toml`:

`https://github.com/zed-industries/zed` @ `f0fa51bb510fc91f6da7ec55f3e0f317f1f975ec`

That commit is Zed `main` as of 14 Nov 2026. It contains the web platform from [GPUI on the web](https://github.com/zed-industries/zed/pull/50228) (`gpui_web` and `gpui_platform`). The browser build calls `gpui_platform::web_init()` and `gpui_platform::single_threaded_web()`. Single-threaded is the stable-Rust path: Zed's default `multithreaded` feature needs nightly, `build-std`, and cross-origin isolation, which a static Worker page does not use.

Desktop entry points stay on the same GPUI types. macOS and Linux go through `gpui_platform::application()` (font-kit on macOS, Wayland and X11 on Linux). Windows constructs `gpui_windows::WindowsPlatform` directly so GPUI's own manifest is not embedded beside `resources/fastdash.manifest.xml`.

`scripts/build-web.sh` runs `cargo build --target wasm32-unknown-unknown` and `wasm-bindgen --target web`, and writes `cloudflare/public/fastdash.js` plus `fastdash_bg.wasm`. `cloudflare/public/index.html` imports that module. The generated files are gitignored.

```bash
bash scripts/build-web.sh          # release
bash scripts/build-web.sh --debug  # faster local iteration
```

### What the browser build does not do

These stay desktop-only. The web build hides them or shows a note. It does not crash.

- The Windows taskbar readout and the tray icon.
- The OS keychain, the in-app updater, and the single-instance mutex.
- Claude Code transcripts and plan-limit meters under `~/.claude`. An Admin API key posted to the Worker is the web path for billed Console usage.
- Device Flow. The browser sign-in form posts a token instead.
- Self-hosted Sentry at an arbitrary origin. SaaS (`sentry.io` and `*.sentry.io`) is proxied. The sign-in form can also name one extra hostname; the desktop app can still talk to any origin with the keychain token.

Fetch behavior does not change on the desktop: the window still decides when to fetch, and nothing in the core polls. In the browser the same connectors run, but every GitHub, Anthropic, and Sentry URL is rewritten by `engine/proxy.rs` (re-exported from `crates/fastdash-app/src/proxy.rs`) onto `/api/proxy/...`. `Authorization` and `x-api-key` are stripped before the request leaves the page. The session cookie is the credential.

The browser font is IBM Plex Mono (Regular and Bold), SIL Open Font License, in `crates/fastdash-app/resources/fonts/`. Desktop builds still pick an installed monospace face.

## The Worker

`cloudflare/src/index.js` is the whole server. No container. Workers static assets serve the host page. `/api/*` hits the Worker first.

| Call | What it does |
| --- | --- |
| `POST /api/session` | Body `{ "githubToken", "anthropicAdminKey", "sentryToken", "sentryHost" }`. Any subset. The token is checked (GitHub: `GET /user`), sealed with AES-GCM, stored in KV, and forgotten. The response is `{ login, github, anthropic, sentry }` and an HttpOnly cookie. The token is not in the body coming back. |
| `GET /api/session` | Which credentials exist. No secrets. |
| `DELETE /api/session` | Drops the KV row and the cookie. |
| `/api/proxy/github/...` | `https://api.github.com/...` with `Authorization: Bearer` from the session. |
| `/api/proxy/anthropic/...` | `https://api.anthropic.com/...` with `x-api-key`. |
| `/api/proxy/sentry/<host>/...` | `https://<host>/...` with `Authorization: Bearer`, and only if `<host>` is `sentry.io`, `*.sentry.io`, or the one hostname stored with the session. |

`src/proxy.rs` is the matching map. A request the browser makes to the Worker must not include the upstream token; the Worker does not copy an inbound `Authorization` header onto the upstream call. It sets its own.

## Deploy

Nothing here creates the account or runs the deploy. The workflow is **Actions → Deploy web → Run workflow**. It does not run on push.

Workers static assets and a Worker script do not need the Workers Paid plan. KV on the free plan is enough for a personal dashboard.

### 1. Cloudflare account

Sign up at <https://dash.cloudflare.com>. Note the account id (Workers & Pages → Overview, the hex id).

### 2. KV namespace

From the repo, after `npm ci --prefix cloudflare` and `npx wrangler login` (or with `CLOUDFLARE_API_TOKEN` set):

```bash
npx --prefix cloudflare wrangler kv namespace create SESSIONS
npx --prefix cloudflare wrangler kv namespace create SESSIONS --preview
```

Paste both ids into `wrangler.jsonc` in place of `replace-with-wrangler-kv-namespace-create`. The placeholder will not deploy.

### 3. API token for GitHub Actions

My Profile → API Tokens → Create Custom Token. Account-owned. Account resources: this account.

| Permission | Access |
| --- | --- |
| Account → Workers Scripts | Edit |
| Account → Workers KV Storage | Edit |
| Account → Account Settings | Read |
| User → User Details | Read |

The first deploy creates the Worker. If it is rejected because the Worker does not exist yet, add Workers **Admin** at the product scope (Cloudflare requires Admin to create a Worker; Edit is enough after that) and run the workflow again.

This token does not need Containers or R2. There is no container and no R2 bucket.

### 4. GitHub Actions secrets

Repo → Settings → Secrets and variables → Actions:

| Name | Value |
| --- | --- |
| `CLOUDFLARE_API_TOKEN` | the token from step 3 |
| `CLOUDFLARE_ACCOUNT_ID` | the hex account id |

### 5. Worker secret

The encryption key for tokens in KV. Losing it makes every stored token unreadable; people connect again.

```bash
openssl rand -hex 32
npx --prefix cloudflare wrangler secret put FASTDASH_WEB_SECRET_KEY
```

Optional, recommended on a URL other people can open:

```bash
npx --prefix cloudflare wrangler secret put FASTDASH_WEB_ALLOWED_LOGINS
```

Value: comma-separated GitHub logins, for example `octocat,hubot`. Unset means any GitHub user who can reach the Worker can store a token in your KV namespace.

Do not set `GITHUB_TOKEN` or an Anthropic key as a Worker variable. Those would be the operator's credentials, attached to whoever called the proxy. Each session carries its own.

### 6. Deploy

GitHub → Actions → **Deploy web** → Run workflow.

Open the `workers.dev` URL. The host page loads the GPUI app. The sign-in form posts `{ githubToken, anthropicAdminKey, sentryToken, sentryHost }` to `POST /api/session`. After that, Refresh on a connected tab fetches through `/api/proxy/...`. The taskbar, the tray, local Claude transcripts, and the updater are not in this build.

### 7. Custom domain (optional)

Workers & Pages → fastdash → Settings → Domains. The session cookie is `SameSite=Lax` and `Secure` on https, so the future wasm app and `/api` must stay on that one origin.
