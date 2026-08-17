// The dashboard's date filter: an inclusive span of calendar days, sent to every
// connector fetch. Presets live here (the frontend owns them) and always resolve
// to concrete dates, so the backend has a single contract - a start and an end
// day - and derives panel titles from it.

import { useEffect, useState } from "react";
import type { DateRange } from "./types";

export type PresetId = "today" | "yesterday" | "last7" | "last30" | "custom";

// Chip order in the filter bar. "custom" is last and reveals the two date
// inputs instead of resolving to a fixed span.
export const PRESETS: { id: PresetId; labelKey: string }[] = [
  { id: "today", labelKey: "range.today" },
  { id: "yesterday", labelKey: "range.yesterday" },
  { id: "last7", labelKey: "range.last7" },
  { id: "last30", labelKey: "range.last30" },
  { id: "custom", labelKey: "range.custom" },
];

// `YYYY-MM-DD` for a local calendar date. `toISOString()` would convert to UTC
// first and shift the day for anyone east of Greenwich.
export function toISODate(d: Date): string {
  const m = `${d.getMonth() + 1}`.padStart(2, "0");
  const day = `${d.getDate()}`.padStart(2, "0");
  return `${d.getFullYear()}-${m}-${day}`;
}

function daysAgo(n: number): Date {
  const d = new Date();
  d.setDate(d.getDate() - n);
  return d;
}

/// The span a preset resolves to right now. "custom" has no fixed span, so it
/// keeps whatever range is already selected; callers pass that in.
export function presetRange(id: PresetId, current?: DateRange): DateRange {
  const today = toISODate(new Date());
  switch (id) {
    case "today":
      return { start: today, end: today };
    case "yesterday": {
      const y = toISODate(daysAgo(1));
      return { start: y, end: y };
    }
    case "last7":
      return { start: toISODate(daysAgo(6)), end: today };
    case "last30":
      return { start: toISODate(daysAgo(29)), end: today };
    case "custom":
      return current ?? { start: today, end: today };
  }
}

export const todayRange = (): DateRange => presetRange("today");

export function isSameRange(a: DateRange, b: DateRange): boolean {
  return a.start === b.start && a.end === b.end;
}

// Stable cache key for a range. `_` can't appear in an ISO date.
export const rangeKey = (r: DateRange): string => `${r.start}_${r.end}`;

// Milliseconds from `now` to the next local midnight, never zero or negative so
// a timer built on it always moves forward.
export function msUntilNextDay(now: Date = new Date()): number {
  const midnight = new Date(now);
  midnight.setHours(24, 0, 0, 0);
  return Math.max(1, midnight.getTime() - now.getTime());
}

// Today's local calendar day, re-read as the day rolls over.
//
// Every preset except "custom" is relative to today, and the app is long-lived:
// left running past midnight it would otherwise keep fetching - and keep
// polling - the day it was opened on, with the "Today" chip still lit. The
// numbers then never move again until the app is relaunched, which is exactly
// what a stale connector looks like.
//
// Two triggers, because neither alone is enough. The timer is aimed at the next
// midnight, but a timer does not run while the machine sleeps and fires late
// when it wakes; focus and visibility catch the case where the app was suspended
// across the boundary. Both funnel into the same re-read, and the state only
// changes when the day string actually does, so a spurious wakeup costs nothing.
export function useToday(): string {
  const [today, setToday] = useState(() => toISODate(new Date()));

  useEffect(() => {
    let timer = 0;

    const check = () => {
      setToday((cur) => {
        const now = toISODate(new Date());
        return now === cur ? cur : now;
      });
      // Re-aimed after every check rather than set on an interval: a fixed
      // 24h interval would drift off midnight, and the wake-up checks below
      // must not leave a stale timeout aimed at yesterday's boundary.
      schedule();
    };

    const schedule = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(check, msUntilNextDay());
    };

    schedule();
    window.addEventListener("focus", check);
    document.addEventListener("visibilitychange", check);
    return () => {
      window.clearTimeout(timer);
      window.removeEventListener("focus", check);
      document.removeEventListener("visibilitychange", check);
    };
  }, []);

  return today;
}
