// Which guided walkthroughs this install has already seen.
//
// A "seen" flag is a UI-only preference, so it lives in localStorage next to the
// theme and the dev-mode flag (see theme.ts / devmode.ts) rather than in the
// Rust-backed config - it describes this machine's onboarding, not the user's
// settings, and losing it costs nothing worse than one extra walkthrough.

const KEY = "fastdash.tourSeen";

/** Whether the named walkthrough has already run to completion (or been skipped). */
export function tourSeen(id: string): boolean {
  try {
    return localStorage.getItem(`${KEY}.${id}`) === "true";
  } catch {
    // Private-mode/blocked storage: treat it as unseen rather than crashing the
    // page - the worst case is the walkthrough offering itself again.
    return false;
  }
}

/** Remember that the named walkthrough is done, so it never auto-opens again. */
export function markTourSeen(id: string): void {
  try {
    localStorage.setItem(`${KEY}.${id}`, "true");
  } catch {
    /* storage unavailable - nothing to persist */
  }
}
