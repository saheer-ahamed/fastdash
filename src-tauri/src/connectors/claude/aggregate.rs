//! Rollups over parsed turns: totals, per-model, per-effort, per-day, current
//! week, and a rolling 5-hour block.
//!
//! Day/week boundaries use IST (fixed +05:30, no DST) so "today" and "this
//! week" line up with the user's local calendar without pulling in chrono-tz.
//! The rolling 5-hour block is timezone-independent (last 5h of wall clock).

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, Utc};

use super::parse::Turn;

/// IST offset: +05:30, fixed year-round.
fn ist() -> FixedOffset {
    FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("IST offset is valid")
}

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

/// Everything the panels need, derived from local transcripts.
#[derive(Debug, Clone, Default)]
pub struct Aggregate {
    pub total_input: u64,
    pub total_output: u64,
    pub total_cache_read: u64,
    pub total_cache_write: u64,
    /// Sorted by [`ModelTotals::total`] descending.
    pub per_model: Vec<ModelTotals>,
    /// Sorted by output tokens descending.
    pub effort: Vec<EffortSplit>,
    pub sessions: usize,
    /// Total assistant turns (messages) seen.
    pub messages: usize,
    /// Sorted by date ascending.
    pub per_day: Vec<DayPoint>,
    /// Sum of all tokens for turns in the current IST week (from Monday 00:00).
    pub current_week_tokens: u64,
    /// Sum of all tokens for turns in the last rolling 5 hours.
    pub five_hour_tokens: u64,
}

impl Aggregate {
    pub fn total_tokens(&self) -> u64 {
        self.total_input + self.total_output + self.total_cache_read + self.total_cache_write
    }

    /// Total output tokens across all efforts (denominator for effort shares).
    pub fn total_effort_output(&self) -> u64 {
        self.effort.iter().map(|e| e.output_tokens).sum()
    }
}

/// Build all rollups. `now` is injected for testability.
pub fn build(turns: &[Turn], now: DateTime<Utc>) -> Aggregate {
    let mut agg = Aggregate::default();
    agg.messages = turns.len();

    let mut models: HashMap<String, ModelTotals> = HashMap::new();
    let mut efforts: HashMap<String, EffortSplit> = HashMap::new();
    let mut sessions: HashSet<&str> = HashSet::new();
    let mut days: BTreeMap<NaiveDate, u64> = BTreeMap::new();

    let ist = ist();
    let now_ist = now.with_timezone(&ist);
    let today = now_ist.date_naive();
    let week_start =
        today - Duration::days(today.weekday().num_days_from_monday() as i64);
    let five_hour_cutoff = now - Duration::hours(5);

    for t in turns {
        let line_total =
            t.input_tokens + t.output_tokens + t.cache_read_tokens + t.cache_write_tokens;

        agg.total_input += t.input_tokens;
        agg.total_output += t.output_tokens;
        agg.total_cache_read += t.cache_read_tokens;
        agg.total_cache_write += t.cache_write_tokens;

        let m = models.entry(t.model.clone()).or_insert_with(|| ModelTotals {
            model: t.model.clone(),
            ..Default::default()
        });
        m.input += t.input_tokens;
        m.output += t.output_tokens;
        m.cache_read += t.cache_read_tokens;
        m.cache_write += t.cache_write_tokens;
        m.turns += 1;

        let e = efforts.entry(t.effort.clone()).or_insert_with(|| EffortSplit {
            effort: t.effort.clone(),
            ..Default::default()
        });
        e.output_tokens += t.output_tokens;
        e.turns += 1;

        if !t.session_id.is_empty() {
            sessions.insert(t.session_id.as_str());
        }

        let turn_ist_date = t.timestamp.with_timezone(&ist).date_naive();
        *days.entry(turn_ist_date).or_insert(0) += line_total;

        if turn_ist_date >= week_start {
            agg.current_week_tokens += line_total;
        }
        if t.timestamp >= five_hour_cutoff {
            agg.five_hour_tokens += line_total;
        }
    }

    let mut per_model: Vec<ModelTotals> = models.into_values().collect();
    per_model.sort_by(|a, b| b.total().cmp(&a.total()));
    agg.per_model = per_model;

    let mut effort: Vec<EffortSplit> = efforts.into_values().collect();
    effort.sort_by(|a, b| b.output_tokens.cmp(&a.output_tokens));
    agg.effort = effort;

    agg.sessions = sessions.len();
    agg.per_day = days
        .into_iter()
        .map(|(date, total_tokens)| DayPoint { date, total_tokens })
        .collect();

    agg
}
