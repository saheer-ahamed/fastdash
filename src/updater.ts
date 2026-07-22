// In-app auto-update. On launch the app asks GitHub whether a newer signed
// release exists (`checkForUpdate`); if one does, the UI surfaces a banner and
// `installUpdate` downloads the signed installer, runs it, and relaunches into
// the new version.
//
// The update is verified against the public key baked into
// `src-tauri/tauri.conf.json` (`plugins.updater.pubkey`) before it is ever run,
// so a tampered or unsigned artifact is rejected. The private half signs the
// artifacts in CI and never leaves the repo secrets.
//
// Everything here degrades quietly: in `tauri dev`, when offline, or when the
// release has no `latest.json` yet, `check()` throws and we simply report "no
// update" rather than surfacing an error to the user.

import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export type { Update };

// Returns the pending update, or null when the app is already current (or the
// check could not run - offline, dev build, no published release yet).
export async function checkForUpdate(): Promise<Update | null> {
  try {
    return await check();
  } catch (e) {
    // Not fatal: a failed check just means we stay on the current version.
    console.warn("update check failed", e);
    return null;
  }
}

// Download + verify + install the update, then relaunch into it. Rejects if the
// download or signature check fails, so callers can restore the UI and let the
// user retry.
export async function installUpdate(update: Update): Promise<void> {
  await update.downloadAndInstall();
  await relaunch();
}
