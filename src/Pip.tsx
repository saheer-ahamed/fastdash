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
import type { Health, Panel, Snapshot } from "./types";
import { t } from "./i18n";
import { todayRange } from "./range";

/// Which connectors have a tab. A connector that is not connected has none -
/// an empty tab explaining it is not set up is exactly the kind of thing a
/// glanceable widget has no room for.
export interface PipAvailability {
  github: boolean;
  claude: boolean;
}

type Tab = "github" | "claude";

const TAB_LABEL: Record<Tab, string> = { github: "GitHub", claude: "Claude" };

export default function Pip({
  available,
  onExit,
}: {
  available: PipAvailability;
  onExit: () => void;
}) {
  const tabs = useMemo(
    () => (["github", "claude"] as Tab[]).filter((id) => available[id]),
    [available],
  );
  const [tab, setTab] = useState<Tab>(tabs[0] ?? "github");
  const [snaps, setSnaps] = useState<Partial<Record<Tab, Snapshot>>>({});
  const [loading, setLoading] = useState<Partial<Record<Tab, boolean>>>({});
  // Read inside the auto-fetch effect without making it a dependency, which
  // would re-run it - and fetch again - every time a snapshot lands.
  const snapsRef = useRef(snaps);
  snapsRef.current = snaps;
  const loadingRef = useRef(loading);
  loadingRef.current = loading;

  // A connector disconnected while the widget was open takes its tab with it.
  useEffect(() => {
    if (!available[tab] && tabs.length > 0) setTab(tabs[0]);
  }, [available, tab, tabs]);

  const load = useCallback((which: Tab) => {
    // The Refresh button is a button, so it can be pressed twice: a second
    // fetch of the same tab piled on the first would spend the rate limit
    // twice to paint the same numbers.
    if (loadingRef.current[which]) return;
    setLoading((l) => ({ ...l, [which]: true }));
    const call =
      which === "github"
        ? invoke<Snapshot>("pip_github", { range: todayRange() })
        : invoke<Snapshot>("pip_claude");
    call
      .then((snap) => setSnaps((s) => ({ ...s, [which]: snap })))
      .catch((e) => console.error(e))
      .finally(() => setLoading((l) => ({ ...l, [which]: false })));
  }, []);

  // Fetch on arrival at a tab that has nothing to show, and only then. Coming
  // back to a tab already loaded paints what it had - however old that is -
  // because the alternative is a widget that quietly fetches every time the eye
  // passes over it.
  useEffect(() => {
    if (!available[tab]) return;
    if (snapsRef.current[tab]) return;
    load(tab);
  }, [tab, available, load]);

  const snap = snaps[tab];
  const busy = !!loading[tab];

  return (
    <div className="pip">
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
            onClick={() => load(tab)}
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
        </div>
      </header>

      <div className="pip-body">
        {snap ? <PipSnapshot snapshot={snap} /> : busy ? null : (
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
