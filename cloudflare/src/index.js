// Secret broker for the fastdash GPUI app.
//
// The browser never receives a GitHub, Anthropic, or Sentry token. A person
// posts one once; it is sealed and stored under an HttpOnly session. Later
// the app (once GPUI can run here) calls `/api/proxy/...` with that cookie,
// and this Worker adds the credential on the way to the upstream.
//
// There is no second dashboard in this file. The page under `public/` hosts
// the GPUI wasm module (`fastdash.js`), which calls back into `/api`.

const COOKIE = "fastdash_session";
const SESSION_SECONDS = 60 * 60 * 24 * 30;

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (request.method !== "GET" && request.method !== "HEAD" && !originAllowed(request, url)) {
      return json({ error: "cross-origin request blocked" }, 403);
    }
    if (url.pathname === "/api/health") return json({ ok: true });
    if (url.pathname === "/api/session" && request.method === "POST") return createSession(request, env);
    if (url.pathname === "/api/session" && request.method === "DELETE") return deleteSession(request, env);
    if (url.pathname === "/api/session" && request.method === "GET") return sessionStatus(request, env);
    if (url.pathname.startsWith("/api/proxy/")) return proxy(request, env, url);
    return env.ASSETS.fetch(request);
  },
};

function originAllowed(request, url) {
  const origin = request.headers.get("Origin");
  if (!origin) return true;
  return origin === url.origin;
}

async function createSession(request, env) {
  let body;
  try {
    body = await request.json();
  } catch {
    return json({ error: "expected a JSON body" }, 400);
  }
  const github = clean(body.githubToken);
  const anthropic = clean(body.anthropicAdminKey);
  const sentry = clean(body.sentryToken);
  const sentryHost = clean(body.sentryHost);
  if (!github && !anthropic && !sentry) {
    return json({ error: "provide a GitHub token, an Anthropic admin key, or a Sentry token" }, 400);
  }
  const record = {};
  if (github) {
    if (github.length < 10 || github.length > 500) return json({ error: "that does not look like a GitHub token" }, 400);
    const login = await githubLogin(github);
    if (!login.ok) return login.response;
    if (!loginAllowed(env, login.login)) {
      return json({ error: "this server only accepts GitHub accounts its operator listed" }, 403);
    }
    record.github = await seal(env, github);
    record.login = login.login;
  }
  if (anthropic) {
    if (anthropic.length < 20 || anthropic.length > 400) return json({ error: "that does not look like an Admin API key" }, 400);
    record.anthropic = await seal(env, anthropic);
  }
  if (sentry) {
    if (sentry.startsWith("sntrys_")) {
      return json({ error: "an organization auth token (sntrys_) cannot read issues" }, 400);
    }
    if (sentry.length < 10 || sentry.length > 2000) return json({ error: "that does not look like a Sentry token" }, 400);
    record.sentry = await seal(env, sentry);
    if (sentryHost) {
      if (!/^[a-z0-9.-]+$/i.test(sentryHost) || sentryHost.includes("..")) {
        return json({ error: "Sentry host must be a hostname, with no scheme or path" }, 400);
      }
      record.sentryHost = sentryHost;
    }
  }
  const id = crypto.randomUUID().replaceAll("-", "") + crypto.randomUUID().replaceAll("-", "");
  await env.SESSIONS.put(id, JSON.stringify(record), { expirationTtl: SESSION_SECONDS });
  const headers = new Headers({ "content-type": "application/json", "cache-control": "no-store" });
  headers.append("set-cookie", cookie(id, SESSION_SECONDS, new URL(request.url).protocol === "https:"));
  return new Response(JSON.stringify({ login: record.login || null, github: Boolean(github), anthropic: Boolean(anthropic), sentry: Boolean(sentry) }), { headers });
}

async function deleteSession(request, env) {
  const id = readCookie(request);
  if (id) await env.SESSIONS.delete(id);
  const headers = new Headers({ "content-type": "application/json" });
  headers.append("set-cookie", cookie("", 0, new URL(request.url).protocol === "https:"));
  return new Response(JSON.stringify({ ok: true }), { headers });
}

async function sessionStatus(request, env) {
  const record = await load(request, env);
  if (!record) return json({ signedIn: false });
  return json({
    signedIn: true,
    login: record.login || null,
    github: Boolean(record.github),
    anthropic: Boolean(record.anthropic),
    sentry: Boolean(record.sentry),
  });
}

async function proxy(request, env, url) {
  const record = await load(request, env);
  if (!record) return json({ error: "sign in required" }, 401);
  const routed = route(url.pathname, url.search, record);
  if (routed.error) return json({ error: routed.error }, routed.status);
  const headers = new Headers();
  for (const name of ["accept", "content-type", "x-github-api-version", "anthropic-version", "anthropic-beta"]) {
    const value = request.headers.get(name);
    if (value) headers.set(name, value);
  }
  headers.set("user-agent", "fastdash");
  if (routed.auth === "github") headers.set("authorization", `Bearer ${routed.token}`);
  if (routed.auth === "anthropic") {
    headers.set("x-api-key", routed.token);
    if (!headers.has("anthropic-version")) headers.set("anthropic-version", "2023-06-01");
  }
  if (routed.auth === "sentry") headers.set("authorization", `Bearer ${routed.token}`);
  const init = { method: request.method, headers };
  if (request.method !== "GET" && request.method !== "HEAD") {
    const body = await request.arrayBuffer();
    if (body.byteLength > 1024 * 1024) return json({ error: "request body is too large" }, 413);
    init.body = body;
  }
  const upstream = await fetch(routed.url, init);
  const responseHeaders = new Headers();
  for (const name of ["content-type", "x-ratelimit-remaining", "x-ratelimit-reset", "retry-after", "link"]) {
    const value = upstream.headers.get(name);
    if (value) responseHeaders.set(name, value);
  }
  responseHeaders.set("cache-control", "no-store");
  return new Response(upstream.body, { status: upstream.status, headers: responseHeaders });
}

function route(pathname, search, record) {
  if (pathname.startsWith("/api/proxy/github/") || pathname === "/api/proxy/github") {
    if (!record._github) return { error: "no GitHub token stored for this session", status: 401 };
    const path = pathname.slice("/api/proxy/github".length) || "/";
    return { url: `https://api.github.com${path}${search}`, auth: "github", token: record._github };
  }
  if (pathname.startsWith("/api/proxy/anthropic/") || pathname === "/api/proxy/anthropic") {
    if (!record._anthropic) return { error: "no Anthropic admin key stored for this session", status: 401 };
    const path = pathname.slice("/api/proxy/anthropic".length) || "/";
    return { url: `https://api.anthropic.com${path}${search}`, auth: "anthropic", token: record._anthropic };
  }
  const sentry = pathname.match(/^\/api\/proxy\/sentry\/([^/]+)(\/.*)?$/);
  if (sentry) {
    if (!record._sentry) return { error: "no Sentry token stored for this session", status: 401 };
    const host = sentry[1].toLowerCase();
    if (!sentryHostAllowed(host, record)) return { error: "that Sentry host is not allowed", status: 400 };
    const path = sentry[2] || "/";
    return { url: `https://${host}${path}${search}`, auth: "sentry", token: record._sentry };
  }
  return { error: "unknown proxy route", status: 404 };
}

function sentryHostAllowed(host, record) {
  if (host === "sentry.io" || (host.endsWith(".sentry.io") && !host.endsWith("..sentry.io"))) {
    const prefix = host === "sentry.io" ? "ok" : host.slice(0, -".sentry.io".length);
    if (prefix && /^[a-z0-9.-]+$/.test(prefix)) return true;
  }
  return Boolean(record.sentryHost) && host === String(record.sentryHost).toLowerCase();
}

async function load(request, env) {
  const id = readCookie(request);
  if (!id) return null;
  const raw = await env.SESSIONS.get(id);
  if (!raw) return null;
  const record = JSON.parse(raw);
  if (record.github) record._github = await openSecret(env, record.github);
  if (record.anthropic) record._anthropic = await openSecret(env, record.anthropic);
  if (record.sentry) record._sentry = await openSecret(env, record.sentry);
  return record;
}

async function githubLogin(token) {
  const response = await fetch("https://api.github.com/user", {
    headers: {
      authorization: `Bearer ${token}`,
      accept: "application/vnd.github+json",
      "user-agent": "fastdash",
    },
  });
  if (response.status === 401) return { ok: false, response: json({ error: "GitHub rejected that token" }, 400) };
  if (!response.ok) return { ok: false, response: json({ error: "could not reach GitHub" }, 502) };
  const body = await response.json();
  if (!body.login) return { ok: false, response: json({ error: "GitHub did not return a login" }, 502) };
  return { ok: true, login: body.login };
}

function loginAllowed(env, login) {
  const list = String(env.FASTDASH_WEB_ALLOWED_LOGINS || "")
    .split(",")
    .map((item) => item.trim().toLowerCase())
    .filter(Boolean);
  if (list.length === 0) return true;
  return list.includes(login.toLowerCase());
}

function clean(value) {
  return typeof value === "string" ? value.trim() : "";
}

function readCookie(request) {
  const raw = request.headers.get("cookie") || "";
  for (const part of raw.split(";")) {
    const trimmed = part.trim();
    if (trimmed.startsWith(`${COOKIE}=`)) return trimmed.slice(COOKIE.length + 1);
  }
  return "";
}

function cookie(value, maxAge, secure) {
  const flag = secure ? "; Secure" : "";
  return `${COOKIE}=${value}; HttpOnly; Path=/; SameSite=Lax; Max-Age=${maxAge}${flag}`;
}

function json(body, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", "cache-control": "no-store" },
  });
}

async function seal(env, plaintext) {
  const key = await aesKey(env);
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const cipher = await crypto.subtle.encrypt({ name: "AES-GCM", iv }, key, new TextEncoder().encode(plaintext));
  const packed = new Uint8Array(iv.length + cipher.byteLength);
  packed.set(iv, 0);
  packed.set(new Uint8Array(cipher), iv.length);
  let binary = "";
  for (const byte of packed) binary += String.fromCharCode(byte);
  return btoa(binary);
}

async function openSecret(env, packed) {
  const bytes = Uint8Array.from(atob(packed), (char) => char.charCodeAt(0));
  const key = await aesKey(env);
  const clear = await crypto.subtle.decrypt({ name: "AES-GCM", iv: bytes.slice(0, 12) }, key, bytes.slice(12));
  return new TextDecoder().decode(clear);
}

async function aesKey(env) {
  const raw = String(env.FASTDASH_WEB_SECRET_KEY || "").trim();
  if (!/^[0-9a-fA-F]{64}$/.test(raw)) throw new Error("FASTDASH_WEB_SECRET_KEY must be 64 hex characters");
  const bytes = new Uint8Array(raw.match(/../g).map((byte) => parseInt(byte, 16)));
  return crypto.subtle.importKey("raw", bytes, "AES-GCM", false, ["encrypt", "decrypt"]);
}
