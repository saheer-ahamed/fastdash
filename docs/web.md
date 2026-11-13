# fastdash in the browser

One UI. `crates/fastdash-app` is the GPUI window, and it is also the wasm target. There is no second dashboard.

## What blocks the window

The desktop app depends on `gpui` **0.2.2** from crates.io, pinned because GPUI is pre-1.0 and breaks between releases (`crates/fastdash-app/Cargo.toml`).

That published crate has no browser platform. Its `src/platform/` tree is macOS, Windows, and Linux (Wayland and X11) only. Asking the full window to compile for `wasm32-unknown-unknown` fails inside GPUI's own dependency tree, before any fastdash view is reached:

```
error: The target OS is "unknown" or "none", so it's unsupported by the errno crate.
```

`errno` is pulled in by GPUI's native platform crates (`rustix` / `async-io`). Even if that compile error were patched, there is still no `Platform` implementation for `wasm32-unknown-unknown`, so the app could not open a window.

A browser backend does exist, but not as a release of this crate:

- Zed merged [GPUI on the web](https://github.com/zed-industries/zed/pull/50228) on 26 Feb 2026 (`14f37ed`). It lives in the Zed workspace as `gpui_web` plus `gpui_platform`, not on crates.io.
- By the time that landed, examples had already moved from `Application::new().run(...)` (what fastdash calls) to `gpui_platform::application().run(...)`, plus `gpui_platform::web_init()` for wasm.
- Current Zed `main` still prints version `0.2.2` and has not shipped a new crates.io release. Depending on that git tree would retarget the desktop app off the pinned API. This repo's rule is that the pin stays, because a GPUI bump breaks the window.

So the wasm target of `fastdash-app` compiles only `src/proxy.rs`. The views, the window, the tray, and the taskbar stay `cfg`'d to the desktop build. CI checks and builds that wasm target so the split cannot rot. It does **not** produce a GPUI canvas.

### Closest path that keeps this one codebase

1. Stay on GPUI 0.2.2 for desktop. Do not fork a second UI.
2. Keep `src/proxy.rs` next to the window. It rewrites `https://api.github.com/...`, `https://api.anthropic.com/...`, and `https://*.sentry.io/...` into `/api/proxy/...` on the same origin. When Zed publishes a GPUI that can open a window in wasm, the connectors call this instead of putting a token in `Authorization`, and the same views render.
3. Until that publish, the Cloudflare Worker below is the secret holder. It is not a dashboard. `cloudflare/public/index.html` is only the host page the wasm module will boot in (the same shape as Zed's `gpui_web` example: a document the canvas is appended to). Today it says why the module is absent.

What will still be desktop-only even after GPUI grows a web backend, because those APIs are not in a browser:

- The Windows taskbar readout and the tray icon.
- The OS keychain. Tokens for the browser copy live in the Worker.
- Claude plan-limit meters. They read Claude Code's login under `~/.claude`. Anthropic does not offer that login to third parties. An Admin API key, stored on the Worker, is the web path for billed usage.
- The in-app updater and single-instance mutex.
- Self-hosted Sentry at an arbitrary origin. SaaS (`sentry.io` and `*.sentry.io`) is proxied. A session may also name one extra hostname; the desktop app can still talk to any origin with the keychain token.

Fetch behavior does not change on the desktop: the window still decides when to fetch, and nothing in the core polls. The wasm build does not fetch yet, because the views are not linked.

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

Open the `workers.dev` URL. It shows the host page, not the dashboards. `POST /api/session` is how a client stores a token. There is no GPUI canvas until the crate above ships a web platform and this repo points the wasm target at it.

### 7. Custom domain (optional)

Workers & Pages → fastdash → Settings → Domains. The session cookie is `SameSite=Lax` and `Secure` on https, so the future wasm app and `/api` must stay on that one origin.
