//! Rollups over parsed turns.
//!
//! Three scopes live side by side, because the panels mean different things:
//!
//! * **range-scoped** (`per_model`, `effort`, `sessions`, `messages`,
//!   `per_day`, `range_tokens`) - only turns whose IST day falls inside the
//!   selected date filter. This is what the token-usage panels report on.
//! * **all-time** (`all_time_tokens`, `all_time_sessions`, `per_month`) - the
//!   full history, so the headline totals and the month table stay stable while
//!   the filter moves.
//! * **now-relative** (`five_hour_tokens`, `current_week_tokens`,
//!   `current_month_tokens`) - live windows that back the plan-limit meters and
//!   ignore the filter entirely, exactly like Claude's own usage page.
//!
//! Day/week/month boundaries use IST (fixed +05:30, no DST) so they line up with
//! the user's local calendar without pulling in chrono-tz. The rolling 5-hour
//! block is timezone-independent (last 5h of wall clock).

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};

use crate::engine::range::{self, DateRange};

use super::parse::Turn;

/// Per-model token totals.
#[derive(Debug, Clone, Default)]
pub struct ModelTotals {
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub turns: u64,
}

impl ModelTotals {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

/// Effort split, measured two ways.
#[derive(Debug, Clone, Default)]
pub struct EffortSplit {
    pub effort: String,
    pub output_tokens: u64,
    pub turns: u64,
}

/// One IST calendar day of total token usage.
#[derive(Debug, Clone)]
pub struct DayPoint {
    pub date: NaiveDate,
    pub total_tokens: u64,
}

/// One IST calendar month of total token usage.
#[derive(Debug, Clone)]
pub struct MonthPoint {
    /// e.g. "Jul 2026".
    pub label: String,
    pub total_tokens: u64,
}

/// Everything the panels need, derived from local transcripts. See the module
/// docs for which fields follow the date filter and which do not.
#[derive(Debug, Clone, Default)]
pub struct Aggregate {
    /// Total tokens in the selected range.
    pub range_tokens: u64,
    /// Sorted by [`ModelTotals::total`] descending. Range-scoped.
    pub per_model: Vec<ModelTotals>,
    /// Sorted by output tokens descending. Range-scoped.
    pub effort: Vec<EffortSplit>,
    /// Distinct sessions touched in the range.
    pub sessions: usize,
    /// Assistant turns (messages) in the range.
    pub messages: usize,
    /// Range days with usage, ascending.
    pub per_day: Vec<DayPoint>,

    /// Tokens across every transcript, ignoring the filter.
    pub all_time_tokens: u64,
    /// Distinct sessions across every transcript.
    pub all_time_sessions: usize,
    /// IST calendar-month totals, most recent month first. All-time history.
    pub per_month: Vec<MonthPoint>,

    /// Tokens in the current IST week (from Monday 00:00).
    pub current_week_tokens: u64,
    /// Tokens in the last rolling 5 hours.
    pub five_hour_tokens: u64,
    /// Tokens in the current IST calendar month.
    pub current_month_tokens: u64,
}

impl Aggregate {
    /// Total output tokens across all efforts (denominator for effort shares).
    pub fn total_effort_output(&self) -> u64 {
        self.effort.iter().map(|e| e.output_tokens).sum()
    }
}

/// Build all rollups for `range`. `now` is injected for testability.
pub fn build(turns: &[Turn], now: DateTime<Utc>, range: &DateRange) -> Aggregate {
    let mut agg = Aggregate::default();

    let mut models: HashMap<String, ModelTotals> = HashMap::new();
    let mut efforts: HashMap<String, EffortSplit> = HashMap::new();
    let mut sessions: HashSet<&str> = HashSet::new();
    let mut all_sessions: HashSet<&str> = HashSet::new();
    let mut days: BTreeMap<NaiveDate, u64> = BTreeMap::new();
    let mut months: BTreeMap<(i32, u32), u64> = BTreeMap::new();

    let ist = range::ist();
    let now_ist = now.with_timezone(&ist);
    let today = now_ist.date_naive();
    let week_start = today - Duration::days(today.weekday().num_days_from_monday() as i64);
    let five_hour_cutoff = now - Duration::hours(5);

    for t in turns {
        let line_total =
            t.input_tokens + t.output_tokens + t.cache_read_tokens + t.cache_write_tokens;
        let turn_ist_date = t.timestamp.with_timezone(&ist).date_naive();

        // --- all-time and now-relative: every turn counts ---
        agg.all_time_tokens += line_total;
        if !t.session_id.is_empty() {
            all_sessions.insert(t.session_id.as_str());
        }

        let (ty, tm) = (turn_ist_date.year(), turn_ist_date.month());
        *months.entry((ty, tm)).or_insert(0) += line_total;

        if turn_ist_date >= week_start {
            agg.current_week_tokens += line_total;
        }
        if t.timestamp >= five_hour_cutoff {
            agg.five_hour_tokens += line_total;
        }
        if ty == now_ist.year() && tm == now_ist.month() {
            agg.current_month_tokens += line_total;
        }

        // --- range-scoped: only turns inside the date filter ---
        if !range.contains(turn_ist_date) {
            continue;
        }

        agg.range_tokens += line_total;
        agg.messages += 1;

        let m = models
            .entry(t.model.clone())
            .or_insert_with(|| ModelTotals {
                model: t.model.clone(),
                ..Default::default()
            });
        m.input += t.input_tokens;
        m.output += t.output_tokens;
        m.cache_read += t.cache_read_tokens;
        m.cache_write += t.cache_write_tokens;
        m.turns += 1;

        let e = efforts
            .entry(t.effort.clone())
            .or_insert_with(|| EffortSplit {
                effort: t.effort.clone(),
                ..Default::default()
            });
        e.output_tokens += t.output_tokens;
        e.turns += 1;

        if !t.session_id.is_empty() {
            sessions.insert(t.session_id.as_str());
        }

        *days.entry(turn_ist_date).or_insert(0) += line_total;
    }

    let mut per_model: Vec<ModelTotals> = models.into_values().collect();
    per_model.sort_by_key(|m| std::cmp::Reverse(m.total()));
    agg.per_model = per_model;

    let mut effort: Vec<EffortSplit> = efforts.into_values().collect();
    effort.sort_by_key(|e| std::cmp::Reverse(e.output_tokens));
    agg.effort = effort;

    agg.sessions = sessions.len();
    agg.all_time_sessions = all_sessions.len();
    agg.per_day = days
        .into_iter()
        .map(|(date, total_tokens)| DayPoint { date, total_tokens })
        .collect();
    agg.per_month = months
        .into_iter()
        .rev()
        .map(|((y, m), total_tokens)| {
            let label = NaiveDate::from_ymd_opt(y, m, 1)
                .map(|d| d.format("%b %Y").to_string())
                .unwrap_or_default();
            MonthPoint {
                label,
                total_tokens,
            }
        })
        .collect();

    agg
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(day: &str, session: &str, output: u64) -> Turn {
        Turn {
            timestamp: DateTime::parse_from_rfc3339(day)
                .unwrap()
                .with_timezone(&Utc),
            session_id: session.to_string(),
            model: "claude-opus-5".to_string(),
            effort: "high".to_string(),
            input_tokens: 0,
            output_tokens: output,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cache_write_1h_tokens: 0,
            cache_write_5m_tokens: 0,
            service_tier: None,
        }
    }

    fn range(start: &str, end: &str) -> DateRange {
        DateRange {
            start: NaiveDate::parse_from_str(start, "%Y-%m-%d").unwrap(),
            end: NaiveDate::parse_from_str(end, "%Y-%m-%d").unwrap(),
        }
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-27T12:00:00+05:30")
            .unwrap()
            .with_timezone(&Utc)
    }

    /// The filter drives the per-model/session rollups; the all-time totals and
    /// the month history stay whole regardless of what is selected.
    #[test]
    fn range_scopes_usage_but_not_all_time() {
        let turns = vec![
            turn("2026-07-27T10:00:00+05:30", "s1", 100), // today
            turn("2026-07-26T10:00:00+05:30", "s2", 30),  // yesterday
            turn("2026-05-02T10:00:00+05:30", "s3", 7),   // months back
        ];

        let today = build(&turns, now(), &range("2026-07-27", "2026-07-27"));
        assert_eq!(today.range_tokens, 100);
        assert_eq!(today.messages, 1);
        assert_eq!(today.sessions, 1);
        assert_eq!(today.all_time_tokens, 137);
        assert_eq!(today.all_time_sessions, 3);
        // Jul 2026 (both recent turns) and May 2026 - month history is all-time.
        assert_eq!(today.per_month.len(), 2);

        let week = build(&turns, now(), &range("2026-07-21", "2026-07-27"));
        assert_eq!(week.range_tokens, 130);
        assert_eq!(week.sessions, 2);
        assert_eq!(week.all_time_tokens, 137);
    }

    /// A range in the past must not disturb the live plan-limit windows.
    #[test]
    fn live_windows_ignore_the_range() {
        let turns = vec![turn("2026-07-27T10:00:00+05:30", "s1", 100)];
        let past = build(&turns, now(), &range("2026-05-01", "2026-05-31"));
        assert_eq!(past.range_tokens, 0);
        assert_eq!(past.current_month_tokens, 100);
        assert_eq!(past.current_week_tokens, 100);
        assert_eq!(past.five_hour_tokens, 100);
    }
}
