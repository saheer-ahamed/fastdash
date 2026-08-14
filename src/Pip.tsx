// The widget: the whole app, shrunk to the two numbers worth glancing at while
// you work on something else.
//
// It is the same window as the dashboard, resized by the backend (see
// `src-tauri/src/pip.rs`), so this component simply replaces the app shell while
// widget mode is on. It draws what it is given and owns nothing that has to
// outlive it: what it is looking at, and everything it has fetched, is held by
// `App` (see `pipstate.ts`), because this component is unmounted every time the
// window changes shape.

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Health, Panel, Snapshot } from "./types";
import type { PipState, Tab } from "./pipstate";
import { t } from "./i18n";

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

const TAB_LABEL: Record<Tab, string> = { github: "GitHub", claude: "Claude" };

export default function Pip({
  state,
  watched,
  onMinimize,
  onExit,
}: {
  state: PipState;
  /** Whether the app holds focus - see `useWindowFocus` in `App.tsx`. */
  watched: boolean;
  onMinimize: () => void;
  onExit: () => void;
}) {
  const { tabs, tab, setTab, accounts, account, setAccount, snap, busy, pending, refresh } =
    state;
  // Whether the pointer is over the widget. Focus alone is the wrong test for
  // "is this being looked at": the widget is a thing you glance at while typing
  // in another app, and it never has focus then - but a pointer resting on it
  // is someone reading it, and folding it away under their cursor would be
  // taking it away mid-glance.
  const [hovered, setHovered] = useState(false);

  // Left alone for a few seconds, the widget gets out of the way by itself.
  // The timer is restarted by the effect re-running, so any moment of attention
  // - focus returning, the pointer arriving - buys another full interval rather
  // than a fraction of one.
  useEffect(() => {
    if (watched || hovered) return;
    const timer = setTimeout(onMinimize, IDLE_MINIMIZE_MS);
    return () => clearTimeout(timer);
  }, [watched, hovered, onMinimize]);

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
            onClick={refresh}
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
