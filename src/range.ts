// The dashboard's date filter: an inclusive span of calendar days, sent to every
// connector fetch. Presets live here (the frontend owns them) and always resolve
// to concrete dates, so the backend has a single contract - a start and an end
// day - and derives panel titles from it.

import type { DateRange } from "./types";

export type PresetId = "today" | "yesterday" | "last7" | "last30" | "thisMonth" | "custom";

// Chip order in the filter bar. "custom" is last and reveals the two date
// inputs instead of resolving to a fixed span.
export const PRESETS: { id: PresetId; labelKey: string }[] = [
  { id: "today", labelKey: "range.today" },
  { id: "yesterday", labelKey: "range.yesterday" },
  { id: "last7", labelKey: "range.last7" },
  { id: "last30", labelKey: "range.last30" },
  { id: "thisMonth", labelKey: "range.thisMonth" },
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
    case "thisMonth": {
      const now = new Date();
      return { start: toISODate(new Date(now.getFullYear(), now.getMonth(), 1)), end: today };
    }
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
