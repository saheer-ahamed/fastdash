// What the widget is looking at, kept outside the widget.
//
// The widget (`Pip.tsx`) is unmounted every time the window changes shape -
// opening the dashboard, folding into the square - and state held inside it
// would go with it: coming back would land on the first tab and the first
// account, throwing away both the arrangement the user chose and the readings
// already paid for. So it lives here, held by `App`, which is always mounted.
//
// Nothing here polls. A view fetches once, when the widget arrives at it with
// nothing to show, and after that only when Refresh is pressed - so a widget
// parked on screen all day costs no API budget.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AppConfig, Snapshot } from "./types";
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

export type Tab = "github" | "claude";

/// One view the widget can show: a connector, and for GitHub the account within
/// it. Everything - the cache, the in-flight flag, the fetch - is keyed on this
/// rather than on the tab, so two accounts are two separate readings and
/// switching between them can never paint one under the other's name.
type View = { tab: Tab; account: string | null };

/// Cache key for a view. A label cannot contain `|`, so this cannot collide.
const viewKey = (v: View) => `${v.tab}|${v.account ?? ""}`;

/// Everything the widget is looking at, and what it has already fetched.
export interface PipState {
  tabs: Tab[];
  tab: Tab;
  setTab: (tab: Tab) => void;
  accounts: string[];
  account: string | null;
  setAccount: (label: string | null) => void;
  /** The reading for the current view, if one has been fetched. */
  snap: Snapshot | undefined;
  /** Whether the current view is mid-fetch. */
  busy: boolean;
  /** Whether there is nothing to draw yet but something is on its way. */
  pending: boolean;
  /** Fetch the current view, whether or not it already has a reading. */
  refresh: () => void;
}

/// `active` is whether the widget is the thing on screen. Only then does this
/// fetch: a dashboard must not be quietly filling the widget's cache in the
/// background.
export function usePipState(active: boolean, available: PipAvailability): PipState {
  const tabs = useMemo(
    () => (["github", "claude"] as Tab[]).filter((id) => available[id]),
    [available],
  );
  const [tab, setTab] = useState<Tab>("github");
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

  // A connector disconnected takes its tab with it.
  useEffect(() => {
    if (!available[tab] && tabs.length > 0) setTab(tabs[0]);
  }, [available, tab, tabs]);

  // The account only qualifies the GitHub view; Claude has one reading.
  const key = viewKey({ tab, account: tab === "github" ? account : null });

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
  // passes over it, including every time it is unfolded from the square.
  //
  // The GitHub view waits for the account list, so the first fetch is filed
  // under the account it actually belongs to rather than under `null` and then
  // fetched a second time - two calls on the same rate limit for one number -
  // when the label lands a moment later.
  const waitingForAccounts = tab === "github" && !accountsRead;
  useEffect(() => {
    if (!active || !available[tab] || waitingForAccounts) return;
    if (snapsRef.current[key]) return;
    load({ tab, account: tab === "github" ? account : null });
  }, [active, tab, account, key, available, waitingForAccounts, load]);

  const busy = !!loading[key];
  // Keyed on the view's two parts rather than on a view object, which is
  // rebuilt every render and would hand the header a new Refresh every time.
  const refresh = useCallback(
    () => load({ tab, account: tab === "github" ? account : null }),
    [load, tab, account],
  );

  return {
    tabs,
    tab,
    setTab,
    accounts,
    account,
    setAccount,
    snap: snaps[key],
    busy,
    // Everything between opening the widget and having something to draw: the
    // config read, the gap before the effect fires, and the fetch itself. They
    // are one wait as far as the user is concerned, and the window is far too
    // small for an unexplained blank to read as anything but broken.
    pending: busy || waitingForAccounts,
    refresh,
  };
}
