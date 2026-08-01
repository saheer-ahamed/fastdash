//! The dashboard's date filter: one inclusive range of calendar days, shared by
//! every connector.
//!
//! The UI owns the presets (Today, Yesterday, Last 7 days, ...) and always sends
//! resolved dates, so the backend has exactly one contract - a start and an end
//! day - and no preset list to keep in sync across two languages. The human
//! label is derived back here (`label`) so panel titles are worded identically
//! no matter which connector renders them.
//!
//! Days are IST (+05:30, no DST), the same boundary the connectors already use
//! for "today"; see `AppConfig::timezone` for the setting this will eventually
//! read.

use chrono::{Duration, FixedOffset, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::engine::i18n;

/// The IST fixed offset (UTC+05:30); `east_opt` only fails on out-of-range.
pub fn ist() -> FixedOffset {
    FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("IST offset is in range")
}

/// Today's IST calendar day.
pub fn today_ist() -> NaiveDate {
    Utc::now().with_timezone(&ist()).date_naive()
}

/// An inclusive span of calendar days, `start <= end`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DateRange {
    pub start: NaiveDate,
    pub end: NaiveDate,
}

impl DateRange {
    /// The default range: today only.
    pub fn today() -> Self {
        let today = today_ist();
        DateRange {
            start: today,
            end: today,
        }
    }

    /// The range with its ends in order, so a reversed custom range still means
    /// what the user drew rather than searching an empty window.
    pub fn normalized(&self) -> DateRange {
        if self.start <= self.end {
            self.clone()
        } else {
            DateRange {
                start: self.end,
                end: self.start,
            }
        }
    }

    pub fn is_single_day(&self) -> bool {
        self.start == self.end
    }

    pub fn contains(&self, day: NaiveDate) -> bool {
        day >= self.start && day <= self.end
    }

    /// RFC3339 bounds for GitHub's search qualifiers, e.g.
    /// `2026-07-18T00:00:00+05:30..2026-07-20T23:59:59+05:30`.
    pub fn ist_bounds(&self) -> String {
        format!(
            "{start}T00:00:00+05:30..{end}T23:59:59+05:30",
            start = self.start.format("%Y-%m-%d"),
            end = self.end.format("%Y-%m-%d")
        )
    }

    /// How the range is named in panel titles, relative to `today`. Recognizes
    /// the presets the UI offers and falls back to the literal span.
    pub fn label_on(&self, today: NaiveDate) -> String {
        if self.is_single_day() {
            return if self.start == today {
                i18n::t("range.today")
            } else if self.start == today - Duration::days(1) {
                i18n::t("range.yesterday")
            } else {
                i18n::tf("range.singleDay", &[("date", &pretty(self.start))])
            };
        }

        if self.end == today {
            if self.start == today - Duration::days(6) {
                return i18n::t("range.last7");
            }
            if self.start == today - Duration::days(29) {
                return i18n::t("range.last30");
            }
        }

        i18n::tf(
            "range.span",
            &[("start", &pretty(self.start)), ("end", &pretty(self.end))],
        )
    }

    /// [`Self::label_on`] against today's IST day.
    pub fn label(&self) -> String {
        self.label_on(today_ist())
    }
}

impl Default for DateRange {
    fn default() -> Self {
        DateRange::today()
    }
}

/// `2026-07-24` -> `Jul 24, 2026`.
fn pretty(day: NaiveDate) -> String {
    day.format("%b %-d, %Y").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn range(start: NaiveDate, end: NaiveDate) -> DateRange {
        DateRange { start, end }
    }

    #[test]
    fn labels_match_the_ui_presets() {
        let today = day(2026, 7, 27);
        let label = |start, end| range(start, end).label_on(today);

        assert_eq!(label(today, today), "Today");
        assert_eq!(label(day(2026, 7, 26), day(2026, 7, 26)), "Yesterday");
        assert_eq!(label(day(2026, 7, 21), today), "Last 7 days");
        assert_eq!(label(day(2026, 6, 28), today), "Last 30 days");
        // Month-to-date is no longer a preset: it reads as the span it is.
        assert_eq!(label(day(2026, 7, 1), today), "Jul 1, 2026 - Jul 27, 2026");
    }

    #[test]
    fn arbitrary_ranges_fall_back_to_the_dates() {
        let today = day(2026, 7, 27);
        assert_eq!(
            range(day(2026, 3, 2), day(2026, 3, 2)).label_on(today),
            "Mar 2, 2026"
        );
        assert_eq!(
            range(day(2026, 3, 2), day(2026, 4, 5)).label_on(today),
            "Mar 2, 2026 - Apr 5, 2026"
        );
    }

    #[test]
    fn bounds_cover_both_end_days_in_full() {
        let bounds = range(day(2026, 7, 18), day(2026, 7, 20)).ist_bounds();
        assert_eq!(
            bounds,
            "2026-07-18T00:00:00+05:30..2026-07-20T23:59:59+05:30"
        );
    }

    #[test]
    fn reversed_ranges_are_ordered_not_empty() {
        let r = range(day(2026, 7, 20), day(2026, 7, 18)).normalized();
        assert_eq!(r.start, day(2026, 7, 18));
        assert_eq!(r.end, day(2026, 7, 20));
        assert!(r.contains(day(2026, 7, 19)));
    }
}
