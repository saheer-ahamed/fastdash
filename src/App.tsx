import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AppConfig,
  ConnectorMeta,
  ConnectorUpdate,
  GithubAccount,
  Health,
  Panel,
  Snapshot,
} from "./types";
import Settings from "./Settings";
import Connectors from "./connectors/ConnectorsPage";
import { setLocale, t } from "./i18n";
import { useDevMode } from "./devmode";
import { checkForUpdate, installUpdate, type Update } from "./updater";

// Which of the two pinned bottom pages is showing, if either.
type Page = "connectors" | "settings";

export default function App() {
  const [connectors, setConnectors] = useState<ConnectorMeta[]>([]);
  const [active, setActive] = useState<string | null>(null);
  const [page, setPage] = useState<Page | null>(null);
  const [snapshots, setSnapshots] = useState<Record<string, Snapshot>>({});
  const [loading, setLoading] = useState(false);
  // Bumped on language change to re-render chrome that calls t().
  const [, setLang] = useState("en");
  // GitHub view state lives here, not in <GithubView>, so it survives tab
  // switches. Otherwise leaving and re-entering the GitHub tab unmounts the
  // component, drops its cache, and flashes "Loading..." on every return.
  const github = useGithubState();

  useEffect(() => {
    invoke<ConnectorMeta[]>("list_connectors")
      .then((cs) => {
        setConnectors(cs);
        if (cs.length > 0) setActive(cs[0].id);
      })
      .catch((e) => console.error(e));
  }, []);

  // Apply the saved language on startup.
  useEffect(() => {
    invoke<AppConfig>("get_config")
      .then((cfg) => {
        setLocale(cfg.locale);
        setLang(cfg.locale);
      })
      .catch(() => {});
  }, []);

  // Live updates: the scheduler emits `connector:update` on every refresh, so
  // panels update on their own cadence without the UI polling.
  useEffect(() => {
    const unlisten = listen<ConnectorUpdate>("connector:update", (e) => {
      setSnapshots((s) => ({ ...s, [e.payload.id]: e.payload.snapshot }));
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  const refresh = useCallback((id: string) => {
    setLoading(true);
    invoke<Snapshot>("fetch_connector", { id })
      .then((snap) => setSnapshots((s) => ({ ...s, [id]: snap })))
      .catch((e) => console.error(e))
      .finally(() => setLoading(false));
  }, []);

  // After a connector's settings are saved: refetch its dashboard, and re-read
  // the GitHub account list so an account added just now shows up as a sub-tab
  // without restarting the app.
  const onConnectorSaved = useCallback(
    (id: string) => {
      refresh(id);
      if (id === "github") github.reloadAccounts();
    },
    [refresh, github],
  );

  // Switch language: update the frontend catalog, re-render chrome, and re-fetch
  // every connector so backend panel strings come back in the new language.
  const onLocaleChange = useCallback(
    (next: string) => {
      setLocale(next);
      setLang(next);
      connectors.forEach((c) => refresh(c.id));
    },
    [connectors, refresh],
  );

  // On first showing a connector, seed instantly from the warm cache, then let
  // the live event stream keep it fresh.
  useEffect(() => {
    if (!active) return;
    invoke<Snapshot | null>("get_cached", { id: active })
      .then((snap) => {
        if (snap) setSnapshots((s) => ({ ...s, [active]: snap }));
        else refresh(active);
      })
      .catch(() => refresh(active));
  }, [active, refresh]);

  const snap = active ? snapshots[active] : undefined;
  const activeName = connectors.find((c) => c.id === active)?.name ?? "";

  return (
    <div className="app">
      <UpdateBanner />
      <aside className="sidebar">
        <div className="brand">fastdash</div>
        <nav>
          {connectors.map((c) => (
            <button
              key={c.id}
              className={"tab" + (!page && c.id === active ? " active" : "")}
              onClick={() => {
                setPage(null);
                setActive(c.id);
              }}
            >
              <span className={"dot " + statusClass(snapshots[c.id]?.status)} />
              {c.name}
            </button>
          ))}
        </nav>
        <div className="sidebar-footer">
          <button
            className={"tab" + (page === "connectors" ? " active" : "")}
            onClick={() => setPage("connectors")}
          >
            <span className="dot idle" />
            {t("app.connectors")}
          </button>
          <button
            className={"tab" + (page === "settings" ? " active" : "")}
            onClick={() => setPage("settings")}
          >
            <span className="dot idle" />
            {t("app.settings")}
          </button>
        </div>
      </aside>

      <main className="content">
        {page === "connectors" ? (
          <Connectors onRefresh={onConnectorSaved} />
        ) : page === "settings" ? (
          <>
            <header className="topbar">
              <h1>{t("app.settings")}</h1>
            </header>
            <Settings onLocaleChange={onLocaleChange} />
          </>
        ) : active === "github" ? (
          <GithubView state={github} />
        ) : (
          <>
            <header className="topbar">
              <h1>{activeName}</h1>
              <div className="actions">
                {snap && (
                  <span className="muted">
                    {t("app.updated", { time: fetchedLabel(snap.fetchedAt) })}
                  </span>
                )}
                <button
                  className="refresh"
                  disabled={loading || !active}
                  onClick={() => active && refresh(active)}
                >
                  {loading ? t("app.refreshing") : t("app.refresh")}
                </button>
              </div>
            </header>

            {snap ? (
              <SnapshotView snapshot={snap} />
            ) : (
              <div className="empty">{t("app.loading")}</div>
            )}
          </>
        )}
      </main>
    </div>
  );
}

// A non-blocking toast that appears only when a newer signed release exists.
// It checks once on launch (quietly ignoring offline/dev builds), then lets the
// user install on their own schedule - the download + relaunch happens on click,
// never automatically. Dismissing hides it until the next launch.
function UpdateBanner() {
  const [update, setUpdate] = useState<Update | null>(null);
  const [installing, setInstalling] = useState(false);
  const [failed, setFailed] = useState(false);
  const [dismissed, setDismissed] = useState(false);

  useEffect(() => {
    let cancelled = false;
    checkForUpdate()
      .then((u) => {
        if (!cancelled) setUpdate(u);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  if (!update || dismissed) return null;

  async function install() {
    if (!update) return;
    setInstalling(true);
    setFailed(false);
    try {
      // Resolves into a relaunch on success, so nothing runs after this.
      await installUpdate(update);
    } catch (e) {
      console.error("update install failed", e);
      setInstalling(false);
      setFailed(true);
    }
  }

  return (
    <div className="update-toast" role="status">
      <span className="update-msg">
        {failed
          ? t("update.failed")
          : t("update.available", { version: `v${update.version}` })}
      </span>
      <div className="update-actions">
        <button className="save-btn" onClick={install} disabled={installing}>
          {installing ? t("update.installing") : t("update.install")}
        </button>
        {!installing && (
          <button className="link-btn" onClick={() => setDismissed(true)}>
            {t("update.dismiss")}
          </button>
        )}
      </div>
    </div>
  );
}

// Open a link in the OS default browser. Tauri's webview ignores
// `target="_blank"`, so panel links route through the backend `open_external`.
function openExternal(url: string) {
  invoke("open_external", { url }).catch((e) => console.error(e));
}

// A panel link that opens in the external browser instead of navigating the
// webview. Keeps a real `href` for accessibility but intercepts the click.
function ExtLink({ href, children }: { href: string; children: ReactNode }) {
  return (
    <a
      href={href}
      onClick={(e) => {
        e.preventDefault();
        openExternal(href);
      }}
    >
      {children}
    </a>
  );
}

// The GitHub dashboard: one sub-tab per connected account, with an org filter
// (All + each org) inside the account. Each (account, org) view fetches via
// `github_fetch` and self-refreshes on the connector cadence.
const GITHUB_REFRESH_MS = 60_000;

// Stable cache key for an (account, org) view. ` ` can't appear in a label
// or org, so it's a safe separator.
const viewKey = (label: string, org: string | null) => `${label} ${org ?? ""}`;

// The GitHub view's persistent state. Held above <GithubView> (in <App>) so it
// outlives tab switches: the cached snapshots, the loading flags, and the
// selected account/org all survive leaving and re-entering the GitHub tab, so
// returning shows cached data instantly and refreshes silently in the topbar
// button instead of flashing "Loading...".
type GithubState = ReturnType<typeof useGithubState>;

function useGithubState() {
  const [accounts, setAccounts] = useState<GithubAccount[]>([]);
  const [label, setLabel] = useState<string | null>(null);
  // null = the account's "All orgs" view.
  const [org, setOrg] = useState<string | null>(null);
  // Last-fetched snapshot per view, kept so switching tabs shows cached data
  // instantly instead of a loading flash (refresh happens in the background).
  const [snaps, setSnaps] = useState<Record<string, Snapshot>>({});
  // Which views have a fetch in flight, so each tab's refresh spins on its own.
  const [loadingKeys, setLoadingKeys] = useState<Record<string, boolean>>({});
  // Latest snapshots, readable inside the switch effect without making it a
  // dependency (which would reset the refresh interval on every fetch).
  const snapsRef = useRef(snaps);
  snapsRef.current = snaps;

  // Read the configured accounts: once on startup, and again whenever they are
  // edited on the Connectors page. Keeps the current selection if it still
  // exists, otherwise falls back to the first account.
  const reloadAccounts = useCallback(() => {
    invoke<AppConfig>("get_config")
      .then((cfg) => {
        const next = cfg.github.accounts;
        setAccounts(next);
        setLabel((cur) =>
          cur && next.some((a) => a.label === cur) ? cur : (next[0]?.label ?? null),
        );
      })
      .catch((e) => console.error(e));
  }, []);

  useEffect(() => {
    reloadAccounts();
  }, [reloadAccounts]);

  // Fetch one view in the background: never clears the cached snapshot, only
  // flags the view as loading and overlays the result when it arrives.
  const load = useCallback((lbl: string, o: string | null) => {
    const key = viewKey(lbl, o);
    setLoadingKeys((l) => ({ ...l, [key]: true }));
    invoke<Snapshot>("github_fetch", { label: lbl, org: o })
      .then((s) => setSnaps((m) => ({ ...m, [key]: s })))
      .catch((e) => console.error(e))
      .finally(() => setLoadingKeys((l) => ({ ...l, [key]: false })));
  }, []);

  // Keep the selected view fresh: refetch if its cached snapshot is missing or
  // older than the refresh cadence - flipping between recently-loaded views (or
  // tabs) then costs nothing. A periodic interval keeps the active view fresh,
  // and the manual Refresh button always forces a fetch. This runs while the
  // GitHub tab is mounted; the cache above persists even when it isn't.
  useEffect(() => {
    if (!label) return;
    const cached = snapsRef.current[viewKey(label, org)];
    const fresh =
      cached && Date.now() - new Date(cached.fetchedAt).getTime() < GITHUB_REFRESH_MS;
    if (!fresh) load(label, org);
    const id = window.setInterval(() => load(label, org), GITHUB_REFRESH_MS);
    return () => window.clearInterval(id);
  }, [label, org, load]);

  return { accounts, label, setLabel, org, setOrg, snaps, loadingKeys, load, reloadAccounts };
}

function GithubView({ state }: { state: GithubState }) {
  const { accounts, label, setLabel, org, setOrg, snaps, loadingKeys, load } = state;

  const activeAccount = accounts.find((a) => a.label === label);
  const key = label ? viewKey(label, org) : null;
  const snap = key ? snaps[key] : undefined;
  const loading = key ? !!loadingKeys[key] : false;

  if (accounts.length === 0) {
    return (
      <>
        <header className="topbar">
          <h1>GitHub</h1>
        </header>
        <div className="empty">{t("github.noAccounts")}</div>
      </>
    );
  }

  return (
    <>
      <header className="topbar">
        <h1>GitHub</h1>
        <div className="actions">
          {snap && (
            <span className="muted">
              {t("app.updated", { time: fetchedLabel(snap.fetchedAt) })}
            </span>
          )}
          <button
            className="refresh"
            disabled={loading || !label}
            onClick={() => label && load(label, org)}
            aria-label={t("app.refresh")}
          >
            {loading && <span className="spinner" aria-hidden />}
            {t("app.refresh")}
          </button>
        </div>
      </header>

      <div className="subtabs">
        {accounts.map((a) => (
          <button
            key={a.label}
            className={"subtab" + (a.label === label ? " active" : "")}
            onClick={() => {
              setLabel(a.label);
              setOrg(null);
            }}
          >
            {a.label}
          </button>
        ))}
      </div>

      {activeAccount && activeAccount.orgs.length > 1 && (
        <div className="org-filter">
          <button
            className={"chip" + (org === null ? " active" : "")}
            onClick={() => setOrg(null)}
          >
            {t("github.allOrgs")}
          </button>
          {activeAccount.orgs.map((o) => (
            <button
              key={o}
              className={"chip" + (org === o ? " active" : "")}
              onClick={() => setOrg(o)}
            >
              {o}
            </button>
          ))}
        </div>
      )}

      {snap ? (
        <SnapshotView snapshot={snap} />
      ) : loading ? (
        <div className="empty">{t("app.loading")}</div>
      ) : null}
    </>
  );
}

function SnapshotView({ snapshot }: { snapshot: Snapshot }) {
  return (
    <div className="panels">
      <StatusBanner status={snapshot.status} />
      {snapshot.panels.map((panel, i) => (
        <PanelView key={i} panel={panel} />
      ))}
    </div>
  );
}

function StatusBanner({ status }: { status: Health }) {
  const devMode = useDevMode();
  if (status.state === "ok") return null;

  // needsAuth and rateLimited already carry human-friendly, actionable copy
  // (the backend localizes needsAuth; rateLimited is a fixed frontend string),
  // so they read fine for everyone.
  if (status.state === "needsAuth") {
    return <div className={"banner " + statusClass(status)}>{status.message}</div>;
  }
  if (status.state === "rateLimited") {
    return <div className={"banner " + statusClass(status)}>{t("status.rateLimited")}</div>;
  }

  // A generic fetch/parse/HTTP failure. Everyday users see a plain, reassuring
  // message; the raw technical string (e.g. "github returned status 422: ...")
  // is developer-only, shown as a secondary line just in developer mode.
  return (
    <div className={"banner " + statusClass(status)}>
      <span className="banner-msg">{t("status.error")}</span>
      {devMode && status.message && (
        <span className="banner-tech">{status.message}</span>
      )}
    </div>
  );
}

function PanelView({ panel }: { panel: Panel }) {
  switch (panel.kind) {
    case "statCards":
      return (
        <section className="card">
          {panel.title && <h2>{panel.title}</h2>}
          <div className="stat-grid">
            {panel.stats.map((s, i) => (
              <div key={i} className="stat">
                <div className="stat-value">{s.value}</div>
                <div className="stat-label">{s.label}</div>
                {s.sub && <div className="stat-sub">{s.sub}</div>}
              </div>
            ))}
          </div>
        </section>
      );
    case "heading":
      return (
        <div className="section-heading">
          <h2>{panel.title}</h2>
          {panel.badge && <span className="badge">{panel.badge}</span>}
        </div>
      );
    case "meter": {
      const pct = panel.limit ? Math.min(100, (panel.used / panel.limit) * 100) : 0;
      return (
        <section className="card meter-card">
          <div className="meter-head">
            <div className="meter-label-group">
              <span className="meter-label">{panel.label}</span>
              {panel.sub && <span className="meter-sub muted">{panel.sub}</span>}
            </div>
            <span className="meter-pct">{panel.caption ?? `${Math.round(pct)}%`}</span>
          </div>
          <div className="meter-track">
            <div className="meter-fill" style={{ width: `${pct}%` }} />
          </div>
        </section>
      );
    }
    case "table":
      return <TableView panel={panel} />;
    case "barList":
      return (
        <section className="card">
          {panel.title && <h2>{panel.title}</h2>}
          <div className="bars">
            {panel.bars.map((b, i) => (
              <div key={i} className="bar-row">
                <span className="bar-label">{b.label}</span>
                <span className="bar-track">
                  <span className="bar-fill" style={{ width: `${Math.min(100, b.value * 100)}%` }} />
                </span>
                <span className="bar-value muted">{b.display ?? ""}</span>
              </div>
            ))}
          </div>
        </section>
      );
    case "list":
      return (
        <section className="card">
          {panel.title && <h2>{panel.title}</h2>}
          <ul className="list">
            {panel.items.map((item, i) => (
              <li key={i}>
                <div className="list-main">
                  {item.href ? (
                    <ExtLink href={item.href}>{item.title}</ExtLink>
                  ) : (
                    <span>{item.title}</span>
                  )}
                  {item.subtitle && <span className="muted"> {item.subtitle}</span>}
                </div>
                {item.meta && <span className="muted">{item.meta}</span>}
              </li>
            ))}
          </ul>
        </section>
      );
  }
}

const PAGE_SIZE = 15;

// A table that paginates client-side once it exceeds PAGE_SIZE rows.
function TableView({ panel }: { panel: Extract<Panel, { kind: "table" }> }) {
  const [page, setPage] = useState(0);
  const total = panel.rows.length;
  const pages = Math.max(1, Math.ceil(total / PAGE_SIZE));
  const paginated = total > PAGE_SIZE;
  const clamped = Math.min(page, pages - 1);
  const start = clamped * PAGE_SIZE;
  const rows = paginated ? panel.rows.slice(start, start + PAGE_SIZE) : panel.rows;

  useEffect(() => {
    if (page > pages - 1) setPage(0);
  }, [page, pages]);

  return (
    <section className="card">
      {panel.title && <h2>{panel.title}</h2>}
      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              {panel.columns.map((col) => (
                <th key={col.key} className={col.numeric ? "num" : ""}>
                  {col.label}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((row, ri) => (
              <tr key={start + ri}>
                {row.map((cell, ci) => (
                  <td key={ci} className={panel.columns[ci]?.numeric ? "num" : ""}>
                    {cell.href ? (
                      <ExtLink href={cell.href}>{cell.text}</ExtLink>
                    ) : (
                      cell.text
                    )}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {paginated && (
        <div className="pager">
          <button disabled={clamped === 0} onClick={() => setPage(clamped - 1)}>
            {t("pager.prev")}
          </button>
          <span className="muted">
            {t("pager.range", {
              start: start + 1,
              end: Math.min(start + PAGE_SIZE, total),
              total,
            })}
          </span>
          <button disabled={clamped >= pages - 1} onClick={() => setPage(clamped + 1)}>
            {t("pager.next")}
          </button>
        </div>
      )}
    </section>
  );
}

function statusClass(status: Health | undefined): string {
  switch (status?.state) {
    case "ok":
      return "ok";
    case "needsAuth":
    case "rateLimited":
      return "warn";
    case "error":
      return "err";
    default:
      return "idle";
  }
}

function fetchedLabel(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}
