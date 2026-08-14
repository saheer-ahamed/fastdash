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
import { getCurrentWindow } from "@tauri-apps/api/window";
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
import Welcome from "./Welcome";
import Connectors from "./connectors/ConnectorsPage";
import Pip, { PipToggle, type PipAvailability, type WindowMode } from "./Pip";
import Tiny from "./Tiny";
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

// Cadence a connector asks to be polled at while its tab is being watched.
// Clamped so a misdeclared 0 can never become a busy-loop.
const cadenceMs = (meta: ConnectorMeta | undefined) =>
  Math.max(1, meta?.defaultRefreshSecs ?? 60) * 1000;

// Whether a snapshot is still young enough to leave alone.
const isFresh = (snap: Snapshot | undefined, cadence: number) =>
  !!snap && Date.now() - new Date(snap.fetchedAt).getTime() < cadence;

export default function App() {
  const [connectors, setConnectors] = useState<ConnectorMeta[]>([]);
  // Whether the connector list has come back yet. Until it has, `active` is null
  // for a reason nobody should be shown copy about - "nothing connected" in the
  // frame before the list lands would flash at everyone who does have one.
  const [listed, setListed] = useState(false);
  const [active, setActive] = useState<string | null>(null);
  const [page, setPage] = useState<Page | null>(null);
  // Which connector's form the Connectors page should open on, when it was
  // reached from a first-run card rather than the sidebar. Read once as that
  // page mounts, so it seeds the sub-tab without owning it afterwards.
  const [openConnector, setOpenConnector] = useState<string | null>(null);
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
  // Fetching is gated on the app actually being watched: the window has focus
  // and the connector's own tab is the thing on screen. `active` alone is not
  // enough - a pinned page (Connectors / Settings) covers the dashboard.
  const focused = useWindowFocus();
  const live = page === null ? active : null;
  // GitHub view state lives here, not in <GithubView>, so it survives tab
  // switches. Otherwise leaving and re-entering the GitHub tab unmounts the
  // component, drops its cache, and flashes "Loading..." on every return.
  const github = useGithubState(range, focused && live === "github");
  // Whether the widget is the thing on screen, and which tabs it can offer.
  // Only these two connectors have a widget reading, and only a connected one
  // gets a tab - so with neither connected there is nothing to shrink into and
  // the toggle is not offered at all. Read off the same `configured` flag the
  // sidebar filters on, so the two can never disagree about what is set up.
  const [mode, setMode] = useState<WindowMode>("dashboard");
  const pipAvailable: PipAvailability = useMemo(
    () => ({
      github: connectors.some((c) => c.id === "github" && c.configured),
      claude: connectors.some((c) => c.id === "claude" && c.configured),
    }),
    [connectors],
  );
  const canPip = pipAvailable.github || pipAvailable.claude;

  // Resize the window first and swap the view only once it has actually
  // resized: painting the widget into a full-size window, or the dashboard into
  // a 300px one, is a visible flash of the wrong layout either way.
  const goMode = useCallback((next: WindowMode) => {
    invoke("set_pip_mode", { mode: next })
      .then(() => setMode(next))
      .catch((e) => console.error(e));
  }, []);
  // Stable callbacks, not inline arrows: the widget's idle timer is keyed on
  // its handlers, so a fresh identity on every render of this component would
  // restart the countdown before it could ever finish.
  const openPip = useCallback(() => goMode("widget"), [goMode]);
  const minimizePip = useCallback(() => goMode("tiny"), [goMode]);
  const exitPip = useCallback(() => goMode("dashboard"), [goMode]);

  // Read the connector list: once on startup, and again whenever a connector's
  // settings are saved. Whether a connector is connected is the backend's
  // answer, per its own credentials - the frontend knows no per-connector rules,
  // which is what keeps adding a connector a zero-UI-change job.
  const reloadConnectors = useCallback(
    () =>
      invoke<ConnectorMeta[]>("list_connectors")
        .then((cs) => {
          setConnectors(cs);
          const visible = cs.filter((c) => c.configured);
          // Keep the current selection while it is still connected, otherwise
          // fall back to the first one. Set in the same update as the list, so
          // there is never a frame rendering a dashboard for a tab that has just
          // disappeared from the sidebar.
          setActive((cur) =>
            cur && visible.some((c) => c.id === cur) ? cur : (visible[0]?.id ?? null),
          );
        })
        .catch((e) => console.error(e))
        .finally(() => setListed(true)),
    [],
  );

  useEffect(() => {
    reloadConnectors();
  }, [reloadConnectors]);

  // Apply the saved language on startup.
  useEffect(() => {
    invoke<AppConfig>("get_config")
      .then((cfg) => {
        setLocale(cfg.locale);
        setLang(cfg.locale);
      })
      .catch(() => {});
  }, []);

  // Every backend fetch emits `connector:update`, whoever asked for it, so a
  // snapshot lands here exactly once no matter which path produced it.
  useEffect(() => {
    const unlisten = listen<ConnectorUpdate>("connector:update", (e) => {
      // File it under the range it actually covers, so a fetch that finishes
      // after the date filter moved cannot overwrite the range now on screen.
      const key = snapKey(e.payload.id, e.payload.range);
      setSnapshots((s) => ({ ...s, [key]: e.payload.snapshot }));
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  const refresh = useCallback((id: string, r: DateRange) => {
    setLoading(true);
    return invoke<Snapshot>("fetch_connector", { id, range: r })
      .then((snap) => setSnapshots((s) => ({ ...s, [snapKey(id, r)]: snap })))
      .catch((e) => console.error(e))
      .finally(() => setLoading(false));
  }, []);

  // After a connector's settings are saved its cached snapshots describe the old
  // settings, so drop them - the tab refetches when it is next opened rather
  // than now, while the Connectors page is what's on screen. The connector list
  // is re-read too, since connecting or disconnecting one is exactly what makes
  // its sidebar tab appear or vanish, and it must do so without a restart. The
  // GitHub account list follows for the same reason: an account added just now
  // shows up as a sub-tab straight away.
  const onConnectorSaved = useCallback(
    (id: string) => {
      setSnapshots((s) =>
        Object.fromEntries(Object.entries(s).filter(([k]) => !k.startsWith(`${id}|`))),
      );
      reloadConnectors();
      if (id === "github") {
        github.reloadAccounts();
        github.clear();
      }
    },
    [github, reloadConnectors],
  );

  // Switch language: update the frontend catalog and re-render the chrome. Panel
  // strings are baked into snapshots by the backend, so every cached one is now
  // in the old language - dropping them all makes the next tab the user opens
  // fetch a fresh copy. Nothing is refetched here: this is reached from
  // Settings, so there is no dashboard on screen to refetch.
  const onLocaleChange = useCallback(
    (next: string) => {
      setLocale(next);
      setLang(next);
      setSnapshots({});
      github.clear();
    },
    [github],
  );

  // Keep the dashboard on screen fresh - and only that one. A fetch happens only
  // while its own tab is showing and the window has focus; switching away or
  // clicking out of the app tears the timer down, so a dashboard nobody is
  // looking at never spends API budget. Coming back refetches at once if what we
  // have has aged past the connector's cadence, and otherwise paints the cache.
  //
  // Only today polls: a past range cannot change, so it is fetched once and left
  // to the Refresh button.
  //
  // GitHub is exempt - its tab renders per-account views that fetch themselves
  // (see `useGithubState`), so a connector-level fetch here would spend a second
  // round of the same rate-limited API on a snapshot nothing renders.
  useEffect(() => {
    if (!focused || !live || live === "github") return;
    const cadence = cadenceMs(connectors.find((c) => c.id === live));
    const key = snapKey(live, range);
    let stopped = false;
    // A fetch slower than the cadence must not get a second one piled on top.
    // Deliberately scoped to this run of the effect rather than a ref that
    // outlives it: a flag that survives can only ever get stuck, and a stuck
    // flag would silently stop the connector refreshing for the whole session.
    let inFlight = false;

    const fetchNow = () => {
      if (stopped || inFlight) return;
      inFlight = true;
      refresh(live, range).finally(() => {
        inFlight = false;
      });
    };

    // On arrival, unlike on the interval, only fetch if what we already have has
    // aged out. Flipping between tabs or bouncing focus then costs nothing.
    const start = async () => {
      let current = snapshotsRef.current[key];
      // Nothing on screen for this view yet: the backend may still be holding a
      // snapshot from earlier in the session (the frontend can reload without it
      // restarting), so paint that before deciding to fetch. It only ever holds
      // today, so any other range goes straight to the fetch below.
      if (!current && rangeKey(range) === rangeKey(todayRange())) {
        const cached = await invoke<Snapshot | null>("get_cached", { id: live }).catch(
          () => null,
        );
        if (stopped) return;
        if (cached) {
          current = cached;
          setSnapshots((s) => (s[key] ? s : { ...s, [key]: cached }));
        }
      }
      if (!isFresh(current, cadence)) fetchNow();
    };
    start();

    if (rangeKey(range) !== rangeKey(todayRange())) {
      return () => {
        stopped = true;
      };
    }
    const id = window.setInterval(fetchNow, cadence);
    return () => {
      stopped = true;
      window.clearInterval(id);
    };
  }, [focused, live, connectors, range, refresh]);

  // Jump from a first-run card straight to that connector's setup form, rather
  // than dropping the user on the Connectors page to find it themselves.
  const openConnectorTab = useCallback((id: string) => {
    setOpenConnector(id);
    setPage("connectors");
  }, []);

  const onRange = useCallback((next: DateRange, p: PresetId) => {
    setRange(next);
    setPreset(p);
  }, []);

  const snap = active ? snapshots[snapKey(active, range)] : undefined;
  const activeName = connectors.find((c) => c.id === active)?.name ?? "";
  // The connectors with a dashboard worth opening. `connectors` keeps every one
  // the backend returned, because the polling cadence is read off it by id.
  const visible = useMemo(() => connectors.filter((c) => c.configured), [connectors]);

  // A sidebar dot reports the health of what that tab last loaded, so a tab not
  // opened yet stays idle rather than claiming a status nothing measured.
  // GitHub keeps its snapshots per (account, org) view, so its dot follows the
  // sub-tab the user has selected.
  const dotStatus = (id: string): Health | undefined =>
    id === "github"
      ? github.label
        ? github.snaps[viewKey(github.label, github.org, range)]?.status
        : undefined
      : snapshots[snapKey(id, range)]?.status;

  // Widget mode replaces the whole shell rather than rendering inside it: the
  // window is 300px wide by this point, and a sidebar would be most of it. The
  // minimized square replaces even that: by then the window is 34px, which is
  // room for one glyph.
  if (mode === "tiny") {
    return <Tiny onExpand={openPip} />;
  }
  if (mode === "widget") {
    return (
      <Pip
        available={pipAvailable}
        watched={focused}
        onMinimize={minimizePip}
        onExit={exitPip}
      />
    );
  }

  // Built once and handed to whichever topbar is on screen, so every page
  // offers the same control in the same place - and pages with nothing to
  // shrink into (nothing connected) get nothing.
  const pipToggle = canPip ? <PipToggle onOpen={openPip} /> : null;

  return (
    <div className="app">
      <UpdateBanner />
      <aside className="sidebar">
        <div className="brand">fastdash</div>
        <nav>
          {/* Only the connected ones get a tab. The Connectors page below still
              lists every connector on purpose - it is where you go to connect
              one, so filtering it too would make an unconnected connector
              permanently unreachable. */}
          {visible.map((c) => (
            <button
              key={c.id}
              className={"tab" + (!page && c.id === active ? " active" : "")}
              onClick={() => {
                setPage(null);
                setActive(c.id);
              }}
            >
              <span className={"dot " + statusClass(dotStatus(c.id))} />
              {c.name}
            </button>
          ))}
        </nav>
        <div className="sidebar-footer">
          <button
            className={"tab" + (page === "connectors" ? " active" : "")}
            // Clearing the deep link matters: without it a card clicked earlier
            // would keep re-seeding the sub-tab on every later visit here.
            onClick={() => {
              setOpenConnector(null);
              setPage("connectors");
            }}
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
          <Connectors
            initialId={openConnector ?? undefined}
            onRefresh={onConnectorSaved}
            pipToggle={pipToggle}
          />
        ) : page === "settings" ? (
          <>
            <header className="topbar">
              <h1>{t("app.settings")}</h1>
              <div className="actions">{pipToggle}</div>
            </header>
            <Settings onLocaleChange={onLocaleChange} />
          </>
        ) : active === null ? (
          // Nothing connected, so there is no connector to name in a topbar and
          // no range to filter - just the way in. Held back until the list has
          // actually answered, so it never flashes on the way to a dashboard.
          listed && <Welcome onConnect={openConnectorTab} />
        ) : active === "github" ? (
          <GithubView
            state={github}
            range={range}
            preset={preset}
            onRange={onRange}
            pipToggle={pipToggle}
          />
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
                {pipToggle}
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

// Whether the app is the window being watched. Every fetch in the app is gated
// on this: a dashboard nobody is looking at must not poll, and an app sitting in
// the background must make no network calls at all.
//
// Two signals, unioned, because neither one is sufficient on its own:
//
//   - The DOM focus/blur pair says whether the *webview* holds focus. That is
//     the signal that tracks the app being used, because the page is what the
//     user clicks into, and it is the only signal available under `npm run dev`,
//     where this runs in a plain browser with no Tauri API.
//   - The window's own focus event says whether the *native window* holds focus.
//     Tauri reports a window as focused only while it is both active and holding
//     keyboard focus, and on Windows the WebView2 content is a child window that
//     takes that keyboard focus away as soon as the app is used - so from
//     startup onwards this alone reads as unfocused for an app sitting right in
//     front of the user. It is still needed for the mirror case: grabbing the
//     native frame to drag or resize moves focus off the webview while the
//     window plainly has it.
//
// Either signal being true means the app is being watched; both false means it
// is not. Both start out assumed focused - the app is launched into the
// foreground - and are corrected as soon as the listeners below attach.
function useWindowFocus(): boolean {
  const [domFocused, setDomFocused] = useState(true);
  // `null` until the desktop listener attaches, and forever in a plain browser,
  // so an absent window signal never props the union up on its own.
  const [windowFocused, setWindowFocused] = useState<boolean | null>(null);

  useEffect(() => {
    const on = () => setDomFocused(true);
    const off = () => setDomFocused(false);
    window.addEventListener("focus", on);
    window.addEventListener("blur", off);
    setDomFocused(document.hasFocus());
    return () => {
      window.removeEventListener("focus", on);
      window.removeEventListener("blur", off);
    };
  }, []);

  useEffect(() => {
    let stopped = false;
    let unlisten: (() => void) | undefined;

    // Subscribe before reading the current state, so a change landing in between
    // is caught rather than dropped.
    const attach = async () => {
      const win = getCurrentWindow();
      const off = await win.onFocusChanged(({ payload }) => setWindowFocused(payload));
      if (stopped) {
        off();
        return;
      }
      unlisten = off;
      setWindowFocused(await win.isFocused());
    };
    // No Tauri API (the browser dev server): the DOM signal carries it alone.
    attach().catch(() => {});

    return () => {
      stopped = true;
      unlisten?.();
    };
  }, []);

  return domFocused || windowFocused === true;
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
// `github_fetch` and refreshes on this cadence for as long as it is the thing
// on screen - see `useGithubState`.
const GITHUB_REFRESH_MS = 60_000;

// The backend's marker for "a newer request took over". Not an error: the view
// keeps whatever it already had, and the request that superseded it will paint.
const SUPERSEDED = "superseded";

// Stable cache key for an (account, org, range) view. ` ` can't appear in a
// label or org, so it's a safe separator.
const viewKey = (label: string, org: string | null, range: DateRange) =>
  `${label} ${org ?? ""} ${rangeKey(range)}`;

// A configured scope may carry an `org:`, `user:` or `author:` qualifier (see
// `scope_qualifier` in the Rust connector). Chips show the bare name; the
// qualified value is what gets stored and sent, so only the display text is
// stripped. Entries are normalized whitespace-free on save, but the `\s*` keeps
// a config written before that from rendering as a ragged chip.
const scopeName = (scope: string) => scope.replace(/^\s*(org|user|author)\s*:\s*/, "");

// The GitHub view's persistent state. Held above <GithubView> (in <App>) so it
// outlives tab switches: the cached snapshots, the loading flags, and the
// selected account/org all survive leaving and re-entering the GitHub tab, so
// returning shows cached data instantly and refreshes silently in the topbar
// button instead of flashing "Loading...".
//
// Living above the view is exactly why `watched` exists: the hook stays mounted
// for the life of the app, so it must be told when the GitHub tab is the thing
// on screen and the window has focus. Nothing fetches unless it is.
type GithubState = ReturnType<typeof useGithubState>;

function useGithubState(range: DateRange, watched: boolean) {
  const [accounts, setAccounts] = useState<GithubAccount[]>([]);
  const [label, setLabel] = useState<string | null>(null);
  // null = the account's "All orgs" view.
  const [org, setOrg] = useState<string | null>(null);
  // Last-fetched snapshot per view, kept so switching tabs shows cached data
  // instantly instead of a loading flash (refresh happens in the background).
  const [snaps, setSnaps] = useState<Record<string, Snapshot>>({});
  // Which views have a fetch in flight, so each tab's refresh spins on its own.
  const [loadingKeys, setLoadingKeys] = useState<Record<string, boolean>>({});
  // Views whose last fetch failed. The cached snapshot stays on screen, but the
  // topbar says so - a silently swallowed failure looks exactly like a refresh
  // that fetched nothing new.
  const [failedKeys, setFailedKeys] = useState<Record<string, boolean>>({});
  // Monotonic request counter per view. Only the newest request for a view may
  // paint: overlapping fetches resolve in whatever order the network decides,
  // and without this an older one landing late repaints stale data over fresh -
  // which then sat there until the app was relaunched.
  const requestIds = useRef<Record<string, number>>({});
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
  // flags the view as loading and overlays the result when it arrives. Anything
  // a newer request for the same view has already superseded is dropped, so the
  // view can only ever move forward in time. `force` is the manual Refresh
  // button, which skips the backend's short reuse window.
  const load = useCallback(
    (lbl: string, o: string | null, r: DateRange, force = false) => {
      const key = viewKey(lbl, o, r);
      const id = (requestIds.current[key] = (requestIds.current[key] ?? 0) + 1);
      const current = () => requestIds.current[key] === id;

      setLoadingKeys((l) => ({ ...l, [key]: true }));
      invoke<Snapshot>("github_fetch", { label: lbl, org: o, range: r, force })
        .then((s) => {
          if (!current()) return;
          setSnaps((m) => ({ ...m, [key]: s }));
          setFailedKeys((f) => ({ ...f, [key]: false }));
        })
        .catch((e) => {
          // The backend cancels the fetch for a view the user has left; that is
          // the intended outcome, not a failure to report.
          if (String(e).includes(SUPERSEDED) || !current()) return;
          console.error(e);
          setFailedKeys((f) => ({ ...f, [key]: true }));
        })
        .finally(() => {
          // A superseded request must not clear the spinner its successor owns.
          if (current()) setLoadingKeys((l) => ({ ...l, [key]: false }));
        });
    },
    [],
  );

  // Forget every cached view. Used when something makes them all wrong at once -
  // a language change, or edited accounts - so the next view opened refetches
  // instead of painting stale panels.
  const clear = useCallback(() => setSnaps({}), []);

  // Keep the selected view fresh: refetch if its cached snapshot is missing or
  // older than the refresh cadence - flipping between recently-loaded views (or
  // tabs, or ranges) then costs nothing. A periodic interval keeps the selected
  // view fresh, and the manual Refresh button always forces a fetch.
  //
  // All of it hangs off `watched`: the tab has to be the thing on screen and the
  // window has to have focus. Leaving the tab or clicking out of the app stops
  // the polling dead, and returning picks it back up - the cache above survives
  // in between, so a return is usually free.
  //
  // Only the live view - today - polls. A past day can no longer change, and a
  // multi-day range costs several paginated Search queries per org, which would
  // eat GitHub's 30-requests-a-minute search budget if re-run every minute. Those
  // are fetched once, and the Refresh button is always there.
  const key = rangeKey(range);
  useEffect(() => {
    if (!watched || !label) return;
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
  }, [watched, label, org, key, load]);

  return {
    accounts,
    label,
    setLabel,
    org,
    setOrg,
    snaps,
    loadingKeys,
    failedKeys,
    load,
    reloadAccounts,
    clear,
  };
}

function GithubView({
  state,
  range,
  preset,
  onRange,
  pipToggle,
}: {
  state: GithubState;
  range: DateRange;
  preset: PresetId;
  onRange: (next: DateRange, preset: PresetId) => void;
  /** The widget toggle, rendered by whoever owns it - see `PipToggle`. */
  pipToggle: ReactNode;
}) {
  const { accounts, label, setLabel, org, setOrg, snaps, loadingKeys, failedKeys, load } =
    state;

  const activeAccount = accounts.find((a) => a.label === label);
  const key = label ? viewKey(label, org, range) : null;
  const snap = key ? snaps[key] : undefined;
  const loading = key ? !!loadingKeys[key] : false;
  const failed = key ? !!failedKeys[key] : false;

  if (accounts.length === 0) {
    return (
      <>
        <header className="topbar">
          <h1>GitHub</h1>
          <div className="actions">{pipToggle}</div>
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
          {failed && !loading ? (
            <span className="stale" role="status">
              {t("app.refreshFailed")}
            </span>
          ) : (
            snap && (
              <span className="muted">
                {t("app.updated", { time: fetchedLabel(snap.fetchedAt) })}
              </span>
            )
          )}
          <button
            className="refresh"
            disabled={loading || !label}
            onClick={() => label && load(label, org, range, true)}
            aria-label={t("app.refresh")}
          >
            {loading && <span className="spinner" aria-hidden />}
            {t("app.refresh")}
          </button>
          {pipToggle}
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
              {scopeName(o)}
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
  // Misconfigured is not a transient failure - retrying cannot fix it, and the
  // backend already localized copy naming what to change. Hiding that behind
  // the generic "we'll keep trying" line would leave the user with no way to
  // find out which setting is wrong, so it is shown to everyone.
  if (status.state === "misconfigured") {
    return <div className={"banner " + statusClass(status)}>{status.message}</div>;
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
    case "misconfigured":
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
