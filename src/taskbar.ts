// Keeps the taskbar readout current.
//
// The readout lives in the Windows taskbar and is drawn by Rust
// (`src-tauri/src/taskbar/`), but like every other fetch in the app it is the
// frontend that decides when to refresh it. It is the one reading that is
// refreshed on a timer: its whole point is to be right while the dashboard is
// closed, so it cannot wait for the user to look. The page keeps running while
// the window is hidden, which is what lets this loop outlive a closed window.

import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

/// How often the readout is refreshed. Claude's `/usage` is cached for 45s in
/// Rust, and GitHub costs two Search queries per account per tick against a
/// budget of 30 a minute, so this is well inside both.
const REFRESH_MS = 2 * 60 * 1000;

/// Refresh the readout now and then every `REFRESH_MS`. `key` changes whenever
/// the answer might have - a connector saved, the readout reconfigured - and
/// restarts the loop so the change shows at once rather than on the next tick.
/// `null` means not yet known, and holds the loop back until it is.
export function useTaskbarReadout(key: string | null): void {
  useEffect(() => {
    if (key === null) return;
    const refresh = () =>
      void invoke("taskbar_refresh").catch((e) => console.error("taskbar refresh", e));
    refresh();
    const timer = window.setInterval(refresh, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [key]);
}
