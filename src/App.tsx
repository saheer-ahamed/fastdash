import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AppConfig,
  Cell,
  ConnectorMeta,
  ConnectorUpdate,
  DateRange,
  GithubAccount,
  HeatDay,
  Health,
  Panel,
  Snapshot,
} from "./types";
import Settings from "./Settings";
import Connectors from "./connectors/ConnectorsPage";
import RangeFilter from "./RangeFilter";
import { rangeKey, todayRange, type PresetId } from "./range";
import { getLocale, setLocale, t } from "./i18n";
import { useDevMode } from "./devmode";
import { checkForUpdate, installUpdate, type Update } from "./updater";

// Which of the two pinned bottom pages is showing, if either.
type Page = "connectors" | "settings";

// Cache slot for a connector's snapshot: one per (connector, date range), so
// flipping between ranges shows what was already fetched instead of a blank.
const snapKey = (id: string, range: DateRange) => `${id}|${rangeKey(range)}`;

export default function App() {
  const [connectors, setConnectors] = useState<ConnectorMeta[]>([]);
  const [active, setActive] = useState<string | null>(null);
  const [page, setPage] = useState<Page | null>(null);
  const [snapshots, setSnapshots] = useState<Record<string, Snapshot>>({});
  // Latest snapshots, readable inside the seeding effect without making it a
  // dependency (which would re-run it on every fetch).
  const snapshotsRef = useRef(snapshots);
  snapshotsRef.current = snapshots;
  const [loading, setLoading] = useState(false);
  // The date filter, shared by every connector so switching tabs keeps showing
  // the same days. Defaults to today; `preset` is only which chip is lit.
  const [range, setRange] = useState<DateRange>(todayRange);
  const [preset, setPreset] = useState<PresetId>("today");
  // Bumped on language change to re-render chrome that calls t().
  const [, setLang] = useState("en");
  // GitHub view state lives here, not in <GithubView>, so it survives tab
  // switches. Otherwise leaving and re-entering the GitHub tab unmounts the
  // component, drops its cache, and flashes "Loading..." on every return.
  const github = useGithubState(range);

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
      // File it under the range it actually covers. The scheduler only ever
      // refreshes today, so this never disturbs a historical range on screen.
      const key = snapKey(e.payload.id, e.payload.range);
      setSnapshots((s) => ({ ...s, [key]: e.payload.snapshot }));
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  const refresh = useCallback((id: string, r: DateRange) => {
    setLoading(true);
    invoke<Snapshot>("fetch_connector", { id, range: r })
      .then((snap) => setSnapshots((s) => ({ ...s, [snapKey(id, r)]: snap })))
      .catch((e) => console.error(e))
      .finally(() => setLoading(false));
  }, []);

  // After a connector's settings are saved: refetch its dashboard, and re-read
  // the GitHub account list so an account added just now shows up as a sub-tab
  // without restarting the app.
  const onConnectorSaved = useCallback(
    (id: string) => {
      refresh(id, range);
      if (id === "github") github.reloadAccounts();
    },
    [refresh, range, github],
  );

  // Switch language: update the frontend catalog, re-render chrome, and re-fetch
  // every connector so backend panel strings come back in the new language.
  const onLocaleChange = useCallback(
    (next: string) => {
      setLocale(next);
      setLang(next);
      connectors.forEach((c) => refresh(c.id, range));
    },
    [connectors, refresh, range],
  );

  // On showing a connector - or moving the date filter - seed instantly from
  // whatever is already cached and fetch otherwise. The warm backend cache only
  // holds today, so any other range goes straight to a fetch.
  useEffect(() => {
    if (!active) return;
    const key = snapKey(active, range);
    if (snapshotsRef.current[key]) return;
    if (rangeKey(range) !== rangeKey(todayRange())) {
      refresh(active, range);
      return;
    }
    invoke<Snapshot | null>("get_cached", { id: active })
      .then((snap) => {
        if (snap) setSnapshots((s) => ({ ...s, [key]: snap }));
        else refresh(active, range);
      })
      .catch(() => refresh(active, range));
  }, [active, range, refresh]);

  const onRange = useCallback((next: DateRange, p: PresetId) => {
    setRange(next);
    setPreset(p);
  }, []);

  const snap = active ? snapshots[snapKey(active, range)] : undefined;
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
              <span className={"dot " + statusClass(snapshots[snapKey(c.id, range)]?.status)} />
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
          <GithubView state={github} range={range} preset={preset} onRange={onRange} />
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
                  onClick={() => active && refresh(active, range)}
                >
                  {loading ? t("app.refreshing") : t("app.refresh")}
                </button>
              </div>
            </header>

            <RangeFilter range={range} preset={preset} onChange={onRange} />

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

// Stable cache key for an (account, org, range) view. ` ` can't appear in a
// label or org, so it's a safe separator.
const viewKey = (label: string, org: string | null, range: DateRange) =>
  `${label} ${org ?? ""} ${rangeKey(range)}`;

// The GitHub view's persistent state. Held above <GithubView> (in <App>) so it
// outlives tab switches: the cached snapshots, the loading flags, and the
// selected account/org all survive leaving and re-entering the GitHub tab, so
// returning shows cached data instantly and refreshes silently in the topbar
// button instead of flashing "Loading...".
type GithubState = ReturnType<typeof useGithubState>;

function useGithubState(range: DateRange) {
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
  const load = useCallback((lbl: string, o: string | null, r: DateRange) => {
    const key = viewKey(lbl, o, r);
    setLoadingKeys((l) => ({ ...l, [key]: true }));
    invoke<Snapshot>("github_fetch", { label: lbl, org: o, range: r })
      .then((s) => setSnaps((m) => ({ ...m, [key]: s })))
      .catch((e) => console.error(e))
      .finally(() => setLoadingKeys((l) => ({ ...l, [key]: false })));
  }, []);

  // Keep the selected view fresh: refetch if its cached snapshot is missing or
  // older than the refresh cadence - flipping between recently-loaded views (or
  // tabs, or ranges) then costs nothing. A periodic interval keeps the active
  // view fresh, and the manual Refresh button always forces a fetch. This runs
  // while the GitHub tab is mounted; the cache above persists even when it isn't.
  //
  // Only the live view - today - polls. A past day can no longer change, and a
  // multi-day range costs several paginated Search queries per org, which would
  // eat GitHub's 30-requests-a-minute search budget if re-run every minute. Those
  // are fetched once, and the Refresh button is always there.
  const key = rangeKey(range);
  useEffect(() => {
    if (!label) return;
    const cached = snapsRef.current[viewKey(label, org, range)];
    const fresh =
      cached && Date.now() - new Date(cached.fetchedAt).getTime() < GITHUB_REFRESH_MS;
    if (!fresh) load(label, org, range);
    if (key !== rangeKey(todayRange())) return;
    const id = window.setInterval(() => load(label, org, range), GITHUB_REFRESH_MS);
    return () => window.clearInterval(id);
    // `range` is compared by its stable key, so a new object with the same days
    // does not restart the interval.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [label, org, key, load]);

  return { accounts, label, setLabel, org, setOrg, snaps, loadingKeys, load, reloadAccounts };
}

function GithubView({
  state,
  range,
  preset,
  onRange,
}: {
  state: GithubState;
  range: DateRange;
  preset: PresetId;
  onRange: (next: DateRange, preset: PresetId) => void;
}) {
  const { accounts, label, setLabel, org, setOrg, snaps, loadingKeys, load } = state;

  const activeAccount = accounts.find((a) => a.label === label);
  const key = label ? viewKey(label, org, range) : null;
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
            onClick={() => label && load(label, org, range)}
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

      <RangeFilter range={range} preset={preset} onChange={onRange} />

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
    case "note":
      return (
        <section className="card">
          {panel.title && <h2>{panel.title}</h2>}
          <p className="note-msg muted">{panel.message}</p>
        </section>
      );
    case "heatmap":
      return <HeatmapView panel={panel} />;
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

// A GitHub-style year grid: one column per week (Sunday-first, as GitHub lays
// it out), one row per weekday, with month labels above and a year rail on the
// right. The backend supplies the shade level per day, so this only paints.
const MS_PER_DAY = 86_400_000;
// Which weekday rows get a label (Mon / Wed / Fri, like GitHub).
const LABELLED_ROWS = [1, 3, 5];

function HeatmapView({ panel }: { panel: Extract<Panel, { kind: "heatmap" }> }) {
  const [label, setLabel] = useState<string | null>(null);
  const active = panel.years.find((y) => y.label === label) ?? panel.years[0];
  const grid = useMemo(() => layoutHeatmap(active?.days ?? []), [active]);

  if (!active) return null;

  return (
    <section className="card heatmap-card">
      {panel.title && <h2>{panel.title}</h2>}
      <div className="heatmap-body">
        <div className="heatmap-main">
          <div className="heatmap-summary">{active.summary}</div>
          <div className="heatmap-scroll">
            <div className="heatmap-chart">
              <div className="heatmap-corner" />
              <div
                className="heatmap-months"
                style={{ gridTemplateColumns: `repeat(${grid.columns}, var(--heat-cell))` }}
              >
                {grid.months.map((m) => (
                  <span key={m.col} style={{ gridColumn: m.col + 1 }}>
                    {m.label}
                  </span>
                ))}
              </div>
              <div className="heatmap-weekdays">
                {LABELLED_ROWS.map((row) => (
                  <span key={row} style={{ gridRow: row + 1 }}>
                    {weekdayLabel(row)}
                  </span>
                ))}
              </div>
              <div
                className="heatmap-cells"
                style={{ gridTemplateColumns: `repeat(${grid.columns}, var(--heat-cell))` }}
              >
                {grid.cells.map((c) => (
                  <span
                    key={c.day.date}
                    className={"heat-cell l" + c.day.level}
                    style={{ gridColumn: c.col + 1, gridRow: c.row + 1 }}
                    title={c.day.tooltip}
                  />
                ))}
              </div>
            </div>
          </div>
          <div className="heatmap-legend muted">
            <span>{t("heatmap.less")}</span>
            {[0, 1, 2, 3, 4].map((l) => (
              <span key={l} className={"heat-cell l" + l} />
            ))}
            <span>{t("heatmap.more")}</span>
          </div>
        </div>

        {panel.years.length > 1 && (
          <div className="heatmap-years">
            {panel.years.map((y) => (
              <button
                key={y.label}
                className={"heatmap-year" + (y.label === active.label ? " active" : "")}
                onClick={() => setLabel(y.label)}
              >
                {y.label}
              </button>
            ))}
          </div>
        )}
      </div>
    </section>
  );
}

interface HeatCell {
  day: HeatDay;
  /** 0-based week column. */
  col: number;
  /** 0-based weekday row, Sunday first. */
  row: number;
}

// Place every day on the (week, weekday) grid and derive the month labels.
// The grid starts on the Sunday of the first day's week, so a window that
// begins mid-week leaves the leading cells of column 0 empty, as GitHub does.
function layoutHeatmap(days: HeatDay[]): {
  cells: HeatCell[];
  columns: number;
  months: { col: number; label: string }[];
} {
  if (days.length === 0) return { cells: [], columns: 0, months: [] };

  const first = parseDay(days[0].date);
  const start = new Date(first);
  start.setDate(start.getDate() - start.getDay());

  const cells = days.map((day) => {
    const index = Math.round((parseDay(day.date).getTime() - start.getTime()) / MS_PER_DAY);
    return { day, col: Math.floor(index / 7), row: index % 7 };
  });
  const columns = cells[cells.length - 1].col + 1;

  // Label a column when its first day starts a new month; drop labels that
  // would sit on top of the next one (the leading partial month).
  const months: { col: number; label: string }[] = [];
  let seen = -1;
  for (const cell of cells) {
    const month = parseDay(cell.day.date).getMonth();
    if (month !== seen) {
      seen = month;
      if (months[months.length - 1]?.col !== cell.col) {
        months.push({ col: cell.col, label: monthLabel(cell.day.date) });
      }
    }
  }
  const spaced = months.filter(
    (m, i) => i === months.length - 1 || months[i + 1].col - m.col >= 3,
  );

  return { cells, columns, months: spaced };
}

// Parse `YYYY-MM-DD` as a local calendar date. `new Date(iso)` would read it as
// UTC midnight and shift the day west of Greenwich.
function parseDay(iso: string): Date {
  const [y, m, d] = iso.split("-").map(Number);
  return new Date(y, m - 1, d);
}

function monthLabel(iso: string): string {
  return parseDay(iso).toLocaleDateString(getLocale(), { month: "short" });
}

// Short weekday name for a Sunday-first row index.
function weekdayLabel(row: number): string {
  // 2026-07-26 is a Sunday; offset from it to name the row.
  const d = new Date(2026, 6, 26 + row);
  return d.toLocaleDateString(getLocale(), { weekday: "short" });
}

const PAGE_SIZE = 15;

type SortDir = "asc" | "desc";
type SortState = { col: number; dir: SortDir };

// The direction a column starts in: counts read best largest-first, names A-Z.
const firstDir = (numeric: boolean): SortDir => (numeric ? "desc" : "asc");

// What a cell compares as: its explicit sort key when the backend supplied one
// (formatted values like "1.2M" or "Jul 24, 14:30" sort wrong as text), the
// number its text parses to in a numeric column, otherwise the text itself.
// `null` means "no value" - those rows sink to the bottom either way.
function cellValue(cell: Cell | undefined, numeric: boolean): number | string | null {
  if (!cell) return null;
  if (cell.sort != null) return cell.sort;
  const text = cell.text.trim();
  if (text === "") return null;
  if (numeric) {
    const n = Number(text.replace(/[^0-9.+-]/g, ""));
    return Number.isNaN(n) ? null : n;
  }
  return text;
}

function compareValues(a: number | string | null, b: number | string | null): number {
  if (a === null || b === null) return a === b ? 0 : a === null ? 1 : -1;
  if (typeof a === "number" && typeof b === "number") return a - b;
  return String(a).localeCompare(String(b), getLocale(), {
    numeric: true,
    sensitivity: "base",
  });
}

// A table that sorts on any column and paginates client-side once it exceeds
// PAGE_SIZE rows. Sorting is off by default: connectors order their rows
// meaningfully (most merged first, and so on), and a third click restores it.
function TableView({ panel }: { panel: Extract<Panel, { kind: "table" }> }) {
  const [page, setPage] = useState(0);
  const [sort, setSort] = useState<SortState | null>(null);

  const sorted = useMemo(() => {
    const col = sort && sort.col < panel.columns.length ? sort : null;
    if (!col) return panel.rows;
    const numeric = panel.columns[col.col]?.numeric ?? false;
    // Decorate with the original index so equal rows keep the backend order.
    return panel.rows
      .map((row, i) => ({ row, i, value: cellValue(row[col.col], numeric) }))
      .sort((a, b) => {
        // Blanks sink in both directions, so the sign applies only to real values.
        if (a.value === null || b.value === null) {
          return compareValues(a.value, b.value) || a.i - b.i;
        }
        const cmp = compareValues(a.value, b.value) * (col.dir === "asc" ? 1 : -1);
        return cmp || a.i - b.i;
      })
      .map((d) => d.row);
  }, [panel.rows, panel.columns, sort]);

  // asc -> desc -> unsorted, starting in whichever direction reads best.
  function toggleSort(ci: number) {
    const start = firstDir(panel.columns[ci]?.numeric ?? false);
    setPage(0);
    setSort((cur) => {
      if (!cur || cur.col !== ci) return { col: ci, dir: start };
      if (cur.dir === start) return { col: ci, dir: start === "asc" ? "desc" : "asc" };
      return null;
    });
  }

  const total = sorted.length;
  const pages = Math.max(1, Math.ceil(total / PAGE_SIZE));
  const paginated = total > PAGE_SIZE;
  const clamped = Math.min(page, pages - 1);
  const start = clamped * PAGE_SIZE;
  const rows = paginated ? sorted.slice(start, start + PAGE_SIZE) : sorted;

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
              {panel.columns.map((col, ci) => {
                const active = sort?.col === ci;
                return (
                  <th
                    key={col.key}
                    className={col.numeric ? "num" : ""}
                    aria-sort={
                      active ? (sort.dir === "asc" ? "ascending" : "descending") : "none"
                    }
                  >
                    <button
                      type="button"
                      className={`th-sort${active ? " active" : ""}`}
                      onClick={() => toggleSort(ci)}
                      // The hint is the more useful thing to say when there is
                      // one; the caret already advertises the sorting.
                      title={col.hint ?? t("table.sortBy", { column: col.label })}
                    >
                      <span className={col.hint ? "hinted" : ""}>{col.label}</span>
                      <span className="caret" aria-hidden="true">
                        {active ? (sort.dir === "asc" ? "↑" : "↓") : "↕"}
                      </span>
                    </button>
                  </th>
                );
              })}
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
