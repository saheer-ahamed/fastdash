// The widget: the whole app, shrunk to the two numbers worth glancing at while
// you work on something else.
//
// It is the same window as the dashboard, resized by the backend (see
// `src-tauri/src/pip.rs`), so this component simply replaces the app shell while
// widget mode is on. That also means it inherits nothing from the dashboard's
// fetch loop, which is the point: nothing here polls. A tab fetches once when
// you first open it with nothing to show, and after that only when you press
// Refresh. A widget parked on screen all day costs no API budget.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AppConfig, Health, Panel, Snapshot } from "./types";
import { t } from "./i18n";
import { todayRange } from "./range";

/// Which connectors have a tab. A connector that is not connected has none -
/// an empty tab explaining it is not set up is exactly the kind of thing a
/// glanceable widget has no room for.
export interface PipAvailability {
  github: boolean;
  claude: boolean;
}

/// The three shapes the one window has, mirroring `pip::Mode` in Rust. The
/// backend owns the geometry; this is only the name the frontend asks by.
export type WindowMode = "dashboard" | "widget" | "tiny";

/// How long the widget may sit unwatched before it folds itself into the
/// minimized square. Long enough that glancing away mid-thought does not lose
/// the panel, short enough that a widget forgotten behind other work stops
/// covering it.
const IDLE_MINIMIZE_MS = 5000;

/// The control that shrinks the app into the widget.
///
/// It lives in the topbar of every page rather than in one of them, because it
/// acts on the window rather than on what is being shown: a control that came
/// and went as you moved between Settings and a dashboard would read as
/// something you had lost. The window's own title bar is the OS's, so the
/// topbar is as close to it as the app can put a button.
export function PipToggle({ onOpen }: { onOpen: () => void }) {
  return (
    <button
      className="pip-toggle"
      onClick={onOpen}
      title={t("pip.openHint")}
      aria-label={t("pip.openHint")}
    >
      {"⤢"}
    </button>
  );
}

type Tab = "github" | "claude";

const TAB_LABEL: Record<Tab, string> = { github: "GitHub", claude: "Claude" };

/// One view the widget can show: a connector, and for GitHub the account within
/// it. Everything - the cache, the in-flight flag, the fetch - is keyed on this
/// rather than on the tab, so two accounts are two separate readings and
/// switching between them can never paint one under the other's name.
type View = { tab: Tab; account: string | null };

/// Cache key for a view. A label cannot contain `|`, so this cannot collide.
const viewKey = (v: View) => `${v.tab}|${v.account ?? ""}`;

export default function Pip({
  available,
  watched,
  onMinimize,
  onExit,
}: {
  available: PipAvailability;
  /** Whether the app holds focus - see `useWindowFocus` in `App.tsx`. */
  watched: boolean;
  onMinimize: () => void;
  onExit: () => void;
}) {
  const tabs = useMemo(
    () => (["github", "claude"] as Tab[]).filter((id) => available[id]),
    [available],
  );
  const [tab, setTab] = useState<Tab>(tabs[0] ?? "github");
  // Whether the pointer is over the widget. Focus alone is the wrong test for
  // "is this being looked at": the widget is a thing you glance at while typing
  // in another app, and it never has focus then - but a pointer resting on it
  // is someone reading it, and folding it away under their cursor would be
  // taking it away mid-glance.
  const [hovered, setHovered] = useState(false);
  // The configured GitHub accounts, in the order the Connectors page lists
  // them. Read once: the widget cannot reach the settings that change it.
  const [accounts, setAccounts] = useState<string[]>([]);
  const [account, setAccount] = useState<string | null>(null);
  // Whether the account list has come back. The GitHub view waits for it, so
  // it is also the difference between "nothing to show" and "not yet asked".
  const [accountsRead, setAccountsRead] = useState(false);
  const [snaps, setSnaps] = useState<Record<string, Snapshot>>({});
  const [loading, setLoading] = useState<Record<string, boolean>>({});
  // Read inside the auto-fetch effect without making it a dependency, which
  // would re-run it - and fetch again - every time a snapshot lands.
  const snapsRef = useRef(snaps);
  snapsRef.current = snaps;
  const loadingRef = useRef(loading);
  loadingRef.current = loading;

  useEffect(() => {
    let cancelled = false;
    invoke<AppConfig>("get_config")
      .then((cfg) => {
        if (cancelled) return;
        const labels = cfg.github.accounts.map((a) => a.label);
        setAccounts(labels);
        // Pick the first account up front rather than leaving it null: the
        // fetch would resolve null to the first account anyway, and then the
        // sub-tab row would light up nothing while showing that account.
        setAccount(labels[0] ?? null);
      })
      .catch((e) => console.error(e))
      // A config read that fails must still release the GitHub view, or the
      // widget waits for an account list that is never coming. The fetch then
      // runs unlabelled, which the backend resolves to the first account.
      .finally(() => {
        if (!cancelled) setAccountsRead(true);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // A connector disconnected while the widget was open takes its tab with it.
  useEffect(() => {
    if (!available[tab] && tabs.length > 0) setTab(tabs[0]);
  }, [available, tab, tabs]);

  // Left alone for a few seconds, the widget gets out of the way by itself.
  // The timer is restarted by the effect re-running, so any moment of attention
  // - focus returning, the pointer arriving - buys another full interval rather
  // than a fraction of one.
  useEffect(() => {
    if (watched || hovered) return;
    const timer = setTimeout(onMinimize, IDLE_MINIMIZE_MS);
    return () => clearTimeout(timer);
  }, [watched, hovered, onMinimize]);

  // The account only qualifies the GitHub view; Claude has one reading.
  const view: View = { tab, account: tab === "github" ? account : null };
  const key = viewKey(view);

  const load = useCallback((which: View) => {
    // The Refresh button is a button, so it can be pressed twice: a second
    // fetch of the same view piled on the first would spend the rate limit
    // twice to paint the same numbers.
    const k = viewKey(which);
    if (loadingRef.current[k]) return;
    setLoading((l) => ({ ...l, [k]: true }));
    const call =
      which.tab === "github"
        ? invoke<Snapshot>("pip_github", {
            label: which.account,
            range: todayRange(),
          })
        : invoke<Snapshot>("pip_claude");
    call
      .then((snap) => setSnaps((s) => ({ ...s, [k]: snap })))
      .catch((e) => console.error(e))
      .finally(() => setLoading((l) => ({ ...l, [k]: false })));
  }, []);

  // Fetch on arrival at a view that has nothing to show, and only then. Coming
  // back to one already loaded paints what it had - however old that is -
  // because the alternative is a widget that quietly fetches every time the eye
  // passes over it.
  //
  // The GitHub view waits for the account list, so the first fetch is filed
  // under the account it actually belongs to rather than under `null` and then
  // fetched a second time - two calls on the same rate limit for one number -
  // when the label lands a moment later.
  const waitingForAccounts = tab === "github" && !accountsRead;
  useEffect(() => {
    if (!available[tab] || waitingForAccounts) return;
    if (snapsRef.current[key]) return;
    load({ tab, account: tab === "github" ? account : null });
  }, [tab, account, key, available, waitingForAccounts, load]);

  const snap = snaps[key];
  const busy = !!loading[key];
  // Everything between opening the widget and having something to draw: the
  // config read, the gap before the effect fires, and the fetch itself. They
  // are one wait as far as the user is concerned, and the window is far too
  // small for an unexplained blank to read as anything but broken.
  const pending = busy || waitingForAccounts;

  return (
    <div
      className="pip"
      onPointerEnter={() => setHovered(true)}
      onPointerLeave={() => setHovered(false)}
    >
      {/* The whole header drags the window: with the title bar gone this is the
          only way to move the widget. Buttons inside it opt out, or a click on
          Refresh would register as the start of a drag. */}
      <header className="pip-head" data-tauri-drag-region>
        <div className="pip-tabs" data-tauri-drag-region>
          {tabs.map((id) => (
            <button
              key={id}
              className={"pip-tab" + (id === tab ? " active" : "")}
              onClick={() => setTab(id)}
            >
              {TAB_LABEL[id]}
            </button>
          ))}
        </div>
        <div className="pip-head-actions">
          <button
            className="pip-icon"
            onClick={() => load(view)}
            disabled={busy}
            title={t("pip.refresh")}
            aria-label={t("pip.refresh")}
          >
            {busy ? <span className="spinner" aria-hidden /> : "↻"}
          </button>
          <button
            className="pip-icon"
            onClick={onExit}
            title={t("pip.exit")}
            aria-label={t("pip.exit")}
          >
            {"↗"}
          </button>
          {/* The window has no title bar of its own, so the last two controls
              every window is expected to have live here. Minimize folds the
              widget into the square rather than into the taskbar: a widget the
              taskbar swallowed is one the user has to go and find. */}
          <button
            className="pip-icon"
            onClick={onMinimize}
            title={t("pip.minimize")}
            aria-label={t("pip.minimize")}
          >
            {"–"}
          </button>
          <button
            className="pip-icon pip-close"
            onClick={() => void invoke("close_app").catch((e) => console.error(e))}
            title={t("pip.close")}
            aria-label={t("pip.close")}
          >
            {"✕"}
          </button>
        </div>
      </header>

      {/* One row per GitHub account, as the dashboard has. Only when there is
          more than one: a single account is already named in the footer, and a
          lone chip in a 300px window is a row spent saying nothing. */}
      {tab === "github" && accounts.length > 1 && (
        <div className="pip-accounts">
          {accounts.map((label) => (
            <button
              key={label}
              className={"pip-account" + (label === account ? " active" : "")}
              onClick={() => setAccount(label)}
              title={label}
            >
              {label}
            </button>
          ))}
        </div>
      )}

      <div className="pip-body">
        {snap ? (
          <PipSnapshot snapshot={snap} />
        ) : pending ? (
          <p className="pip-loading muted" role="status">
            <span className="spinner" aria-hidden />
            {t("app.loading")}
          </p>
        ) : (
          <p className="pip-status muted">{t("pip.empty")}</p>
        )}
      </div>

      {/* Nothing here refreshes on its own, so how old the numbers are is part
          of what they mean: without the stamp, yesterday's reading and this
          minute's look identical. */}
      {snap && (
        <footer className="pip-foot muted">
          <span className="pip-subject">{subjectOf(snap)}</span>
          <span>{t("app.updated", { time: fetchedLabel(snap.fetchedAt) })}</span>
        </footer>
      )}
    </div>
  );
}

/// Who the numbers are about, when the connector says so - the GitHub reading
/// carries the login it counted, which is worth showing when the account with
/// the token is not the one you assumed.
function subjectOf(snapshot: Snapshot): string {
  const stats = snapshot.panels.find((p) => p.kind === "statCards");
  return (stats?.kind === "statCards" && stats.title) || "";
}

function fetchedLabel(iso: string): string {
  return new Date(iso).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

function PipSnapshot({ snapshot }: { snapshot: Snapshot }) {
  const message = statusMessage(snapshot.status);
  if (message) return <p className="pip-status">{message}</p>;
  if (snapshot.panels.length === 0) {
    return <p className="pip-status muted">{t("pip.empty")}</p>;
  }
  return (
    <>
      {snapshot.panels.map((panel, i) => (
        <PipPanel key={i} panel={panel} />
      ))}
    </>
  );
}

// The one line a failure gets. The dashboard can afford a banner over the data
// it still has; the widget has no data to keep, so the status is the content.
// Copy the user can act on is shown verbatim, the rest gets the short line -
// a raw HTTP string would not fit and would not help.
function statusMessage(status: Health): string | null {
  switch (status.state) {
    case "ok":
      return null;
    case "needsAuth":
    case "misconfigured":
      return status.message;
    default:
      return t("pip.failed");
  }
}

// Only the two panel kinds the widget asks its backend for. Anything else is a
// panel this view was not designed to hold, and is dropped rather than allowed
// to overflow a 300px window.
function PipPanel({ panel }: { panel: Panel }) {
  if (panel.kind === "statCards") {
    return (
      <dl className="pip-stats">
        {panel.stats.map((s, i) => (
          <div key={i} className="pip-stat">
            <dt>{s.label}</dt>
            <dd>{s.value}</dd>
          </div>
        ))}
      </dl>
    );
  }

  if (panel.kind === "meter") {
    const pct = panel.limit ? Math.min(100, (panel.used / panel.limit) * 100) : 0;
    return (
      <div className="pip-meter">
        <div className="pip-meter-head">
          <span className="pip-meter-label">{panel.label}</span>
          <span className="pip-meter-pct">
            {panel.caption ?? `${Math.round(pct)}%`}
          </span>
        </div>
        <div className="meter-track">
          <div className="meter-fill" style={{ width: `${pct}%` }} />
        </div>
        {panel.sub && <div className="pip-meter-sub muted">{panel.sub}</div>}
      </div>
    );
  }

  return null;
}
