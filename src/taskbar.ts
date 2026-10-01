// Fills the taskbar readout.
//
// The readout lives in the Windows taskbar and is drawn by Rust
// (`src-tauri/src/taskbar/`). Nothing refreshes it on a timer: it is filled
// once when the app starts and again when what it should show changes, and
// after that only when the user clicks the refresh icon that appears when the
// readout is hovered.

import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

/// Fill the readout whenever `key` changes - the connector list loaded, a
/// connector saved. `null` means not yet known, and holds it back until it is.
export function useTaskbarReadout(key: string | null): void {
  useEffect(() => {
    if (key === null) return;
    void invoke("taskbar_refresh").catch((e) => console.error("taskbar refresh", e));
  }, [key]);
}
