// Developer mode: a hidden flag, unlocked the classic way by tapping the version
// line in Settings five times. When on, developer-only affordances appear - most
// notably the crash screen (see ErrorBoundary) surfaces the full technical stack
// instead of tucking it behind a collapsed section.
//
// Persisted in localStorage like the theme (see theme.ts), and exposed through a
// tiny pub/sub so both the hook-based Settings UI and the class-based
// ErrorBoundary can react to changes without a shared store or prop drilling.

import { useSyncExternalStore } from "react";

const STORAGE_KEY = "fastdash.devMode";

/** How many taps unlock (or re-lock) developer mode. */
export const DEV_MODE_TAPS = 5;

const listeners = new Set<() => void>();

export function isDevMode(): boolean {
  return localStorage.getItem(STORAGE_KEY) === "true";
}

/** Persist the flag and notify every subscriber. */
export function setDevMode(on: boolean): void {
  localStorage.setItem(STORAGE_KEY, String(on));
  listeners.forEach((fn) => fn());
}

/** Subscribe to dev-mode changes; returns an unsubscribe function. */
export function onDevModeChange(fn: () => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

/**
 * React hook reading the current dev-mode flag. Re-renders the calling component
 * whenever the flag is toggled (e.g. from Settings), so dev-only affordances
 * appear and disappear live without a shared store or prop drilling.
 */
export function useDevMode(): boolean {
  return useSyncExternalStore(onDevModeChange, isDevMode);
}
