// In-app auto-update. On launch we ask GitHub (via the release `latest.json`)
// whether a newer signed build exists. If so we download it, install it, and
// relaunch into the new version. Signature verification is handled natively by
// tauri-plugin-updater against the pubkey baked into tauri.conf.json - an
// unsigned or tampered artifact is rejected before it is ever installed.
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

// Guard so a failed update check never blocks app startup.
export async function runUpdateCheck(): Promise<void> {
  try {
    const update = await check();
    if (!update) return;

    // Pull the whole payload; the callback lets us log progress without a UI.
    await update.downloadAndInstall((event) => {
      if (event.event === "Started") {
        console.info(`[updater] downloading v${update.version}`);
      } else if (event.event === "Finished") {
        console.info("[updater] download finished, installing");
      }
    });

    // The new version is staged; relaunch to run it.
    await relaunch();
  } catch (err) {
    // Offline, rate-limited, or no release yet - all non-fatal. The user keeps
    // running the current version and we retry on the next launch.
    console.warn("[updater] update check failed:", err);
  }
}
