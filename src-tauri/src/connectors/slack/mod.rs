//! Slack connector.
//!
//! Per workspace, resolves the current user via `auth.test`, then uses
//! `search.messages` (`<@me> after:<today>`) to find messages that mention me
//! today, grouped by channel. Requires a user token (`xoxp`) with `search:read`
//! - bot tokens cannot search. Token lives in the OS keychain.
//!
//! Owned modules:
//!   - `api`    thin Slack Web API client + wire types (auth.test, search.messages)
//!   - `token`  user-token resolution (keychain or `SLACK_USER_TOKEN`)

mod api;
mod token;

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, Utc};

use crate::engine::connector::{Connector, ConnectorError, ConnectorMeta, FetchCtx, Snapshot};
use crate::engine::i18n;
use crate::engine::panel::{Cell, Column, Panel, Stat, TableSpec};

use api::{Match, SlackClient, SlackError};

/// Guard against a runaway paging loop; 20 pages * 100 = 2000 mentions/day is
/// far more than any human receives, so this only bounds pathological cases.
const MAX_SEARCH_PAGES: u32 = 20;

pub struct SlackConnector;

impl SlackConnector {
    pub fn new() -> Self {
        SlackConnector
    }
}

#[async_trait]
impl Connector for SlackConnector {
    fn meta(&self) -> ConnectorMeta {
        ConnectorMeta {
            id: "slack".into(),
            name: "Slack".into(),
            icon: "slack".into(),
            default_refresh_secs: 60,
        }
    }

    async fn fetch(&self, _ctx: &FetchCtx) -> Result<Snapshot, ConnectorError> {
        // Use the first workspace configured in Settings; fall back to the default.
        // TODO: honor `_ctx.timezone` (today is pinned to IST) and support more
        // than one workspace.
        let cfg = crate::engine::config::load();
        let label = cfg
            .slack
            .workspaces
            .first()
            .map(|w| w.label.clone())
            .unwrap_or_else(|| token::DEFAULT_LABEL.to_string());
        let Some(resolved) = token::resolve(&label) else {
            return Ok(Snapshot::needs_auth(i18n::t("slack.needsAuth")));
        };

        if !resolved.is_user_token() {
            // A bot/app token can authenticate but can never call search.messages.
            return Ok(Snapshot::needs_auth(i18n::t("slack.needsUserToken")));
        }

        let client = SlackClient::new(resolved.token).map_err(map_hard_error)?;

        // 1. Who am I / which workspace? Needed for the mention query and labels.
        let identity = match client.auth_test().await {
            Ok(id) => id,
            Err(e) if e.is_auth_problem() => {
                return Ok(Snapshot::needs_auth(auth_hint(&e)));
            }
            Err(e) if e.is_rate_limited() => return Err(ConnectorError::RateLimited),
            Err(e) => return Err(map_hard_error(e)),
        };

        let Some(user_id) = identity.user_id.clone() else {
            return Ok(Snapshot::needs_auth(i18n::t("slack.noUserId")));
        };

        // 2. IST "today" bounds. The client-side `since` filter is the source of
        //    truth; the `after:` query bound only narrows the server search.
        let ist = FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("IST offset is valid");
        let now_ist = Utc::now().with_timezone(&ist);
        let today = now_ist.date_naive();
        let since_utc: DateTime<Utc> = today
            .and_hms_opt(0, 0, 0)
            .expect("midnight is valid")
            .and_local_timezone(ist)
            .single()
            .expect("IST offset never yields ambiguous local times")
            .with_timezone(&Utc);

        // Slack's `after:` is day-granular and exclusive, evaluated in the
        // workspace timezone (which may differ from IST). Query from the day
        // *before* IST-today so no early-morning mention is dropped, then trim
        // precisely with `since_utc` below.
        let after_date = today.pred_opt().unwrap_or(today);
        let query = format!("<@{}> after:{}", user_id, after_date.format("%Y-%m-%d"));

        // 3. Page through the matches.
        let matches = match collect_matches(&client, &query).await {
            Ok(m) => m,
            Err(e) if e.is_auth_problem() => return Ok(Snapshot::needs_auth(auth_hint(&e))),
            Err(e) if e.is_rate_limited() => return Err(ConnectorError::RateLimited),
            Err(e) => return Err(map_hard_error(e)),
        };

        // 4. Refine against IST midnight and group by channel.
        let groups = group_by_channel(&matches, since_utc);

        let panels = build_panels(&identity, &groups);
        Ok(Snapshot::ok(panels, Some(self.meta().default_refresh_secs)))
    }
}

/// Fetch every page of results for `query`, respecting Slack's paging metadata.
async fn collect_matches(client: &SlackClient, query: &str) -> Result<Vec<Match>, SlackError> {
    let mut out = Vec::new();
    let mut page = 1u32;
    loop {
        let messages = client.search_messages(query, page).await?;
        out.extend(messages.matches);

        let total_pages = messages
            .paging
            .and_then(|p| p.pages)
            .unwrap_or(1)
            .min(MAX_SEARCH_PAGES);
        if page >= total_pages {
            break;
        }
        page += 1;
    }
    Ok(out)
}

/// A channel's mentions for today, rolled up.
struct ChannelGroup {
    name: String,
    count: usize,
    last_ts: DateTime<Utc>,
    last_permalink: Option<String>,
    last_preview: String,
}

/// Keep only matches at/after IST midnight, then group by channel and remember
/// the most recent message per channel (for last-time, preview, permalink).
fn group_by_channel(matches: &[Match], since_utc: DateTime<Utc>) -> Vec<ChannelGroup> {
    let mut by_channel: HashMap<String, ChannelGroup> = HashMap::new();

    for m in matches {
        let Some(ts) = m.ts.as_deref().and_then(parse_slack_ts) else {
            continue;
        };
        if ts < since_utc {
            continue;
        }

        let channel = m.channel.as_ref();
        let channel_id = channel
            .and_then(|c| c.id.clone())
            .unwrap_or_else(|| "unknown".into());
        let channel_name = channel
            .and_then(|c| c.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| channel_id.clone());

        let entry = by_channel.entry(channel_id).or_insert_with(|| ChannelGroup {
            name: channel_name,
            count: 0,
            last_ts: ts,
            last_permalink: None,
            last_preview: String::new(),
        });
        entry.count += 1;
        // Track the newest message in the channel for the display columns.
        if ts >= entry.last_ts || entry.last_preview.is_empty() {
            entry.last_ts = ts;
            entry.last_permalink = m.permalink.clone();
            entry.last_preview = clean_preview(m.text.as_deref().unwrap_or_default());
        }
    }

    let mut groups: Vec<ChannelGroup> = by_channel.into_values().collect();
    // Busiest channels first; break ties by most recent activity.
    groups.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| b.last_ts.cmp(&a.last_ts)));
    groups
}

/// StatCards + a "Mentions today" table linking each channel to its latest
/// message permalink.
fn build_panels(identity: &api::AuthTest, groups: &[ChannelGroup]) -> Vec<Panel> {
    let ist = FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("IST offset is valid");
    let total: usize = groups.iter().map(|g| g.count).sum();
    let active_channels = groups.len();

    let workspace = identity
        .team
        .clone()
        .or_else(|| identity.url.clone())
        .unwrap_or_else(|| "Slack".into());

    let mut panels = vec![Panel::StatCards {
        title: Some(i18n::t("slack.title")),
        stats: vec![
            Stat {
                label: i18n::t("slack.mentionsToday"),
                value: total.to_string(),
                sub: Some(workspace),
            },
            Stat {
                label: i18n::t("slack.activeChannels"),
                value: active_channels.to_string(),
                sub: None,
            },
        ],
    }];

    if groups.is_empty() {
        return panels;
    }

    let rows: Vec<Vec<Cell>> = groups
        .iter()
        .map(|g| {
            let last_time = g.last_ts.with_timezone(&ist).format("%H:%M").to_string();
            vec![
                Cell {
                    text: format!("#{}", g.name),
                    href: g.last_permalink.clone(),
                },
                Cell {
                    text: g.count.to_string(),
                    href: None,
                },
                Cell {
                    text: last_time,
                    href: None,
                },
                Cell {
                    text: g.last_preview.clone(),
                    href: g.last_permalink.clone(),
                },
            ]
        })
        .collect();

    panels.push(Panel::Table(TableSpec {
        title: Some(i18n::t("slack.mentionsToday")),
        columns: vec![
            Column {
                key: "channel".into(),
                label: i18n::t("slack.columnChannel"),
                numeric: false,
            },
            Column {
                key: "mentions".into(),
                label: i18n::t("slack.columnMentions"),
                numeric: true,
            },
            Column {
                key: "last".into(),
                label: i18n::t("slack.columnLastTime"),
                numeric: false,
            },
            Column {
                key: "preview".into(),
                label: i18n::t("slack.columnPreview"),
                numeric: false,
            },
        ],
        rows,
    }));

    panels
}

/// Parse a Slack `ts` string (`"1610000000.000200"`) into a UTC datetime.
fn parse_slack_ts(ts: &str) -> Option<DateTime<Utc>> {
    let mut parts = ts.split('.');
    let secs: i64 = parts.next()?.parse().ok()?;
    let nanos: u32 = match parts.next() {
        // Slack's fraction is microseconds (6 digits); pad to nanoseconds.
        Some(frac) => format!("{:0<9}", frac).get(..9)?.parse().ok()?,
        None => 0,
    };
    DateTime::from_timestamp(secs, nanos)
}

/// Turn Slack message markup into a short, readable preview.
///
/// Strips `<...>` tokens (user/channel mentions, links) - keeping the human
/// label after `|` when present - decodes the common HTML entities Slack emits,
/// collapses whitespace, and truncates.
fn clean_preview(text: &str) -> String {
    const MAX_LEN: usize = 90;

    let mut cleaned = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            // Consume until the matching '>'; keep only the label after '|'.
            let mut token = String::new();
            for tc in chars.by_ref() {
                if tc == '>' {
                    break;
                }
                token.push(tc);
            }
            if let Some((_, label)) = token.split_once('|') {
                cleaned.push_str(label);
            }
            // Bare `<@U123>` / `<#C123>` / `<http...>` (no label) are dropped.
        } else {
            cleaned.push(c);
        }
    }

    let cleaned = cleaned
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">");
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");

    if collapsed.chars().count() > MAX_LEN {
        let truncated: String = collapsed.chars().take(MAX_LEN).collect();
        format!("{}…", truncated.trim_end())
    } else {
        collapsed
    }
}

/// A user-facing hint for an auth-class Slack error.
fn auth_hint(e: &SlackError) -> String {
    match e.api_code() {
        Some("missing_scope") => i18n::t("slack.hintMissingScope"),
        Some("not_allowed_token_type") => i18n::t("slack.hintNotUserToken"),
        Some("token_revoked" | "token_expired" | "invalid_auth" | "not_authed") => {
            i18n::t("slack.hintInvalidToken")
        }
        Some("account_inactive") => i18n::t("slack.hintAccountInactive"),
        _ => i18n::t("slack.needsAuth"),
    }
}

/// Map a non-auth, non-rate-limit Slack error onto the connector's error type.
fn map_hard_error(e: SlackError) -> ConnectorError {
    ConnectorError::Other(e.to_string())
}

// TODO(feat/slack): `search:read` fallback. When a workspace forbids the scope
// (or a workspace policy blocks search entirely), degrade to enumerating
// channels via `conversations.list` and scanning `conversations.history` for
// `<@me>` mentions since IST midnight. That path is heavier and needs the
// `channels:history` / `groups:history` / `im:history` / `mpim:history` +
// `conversations.list` scopes, so search stays the default. Left unimplemented
// until a real workspace exercises the forbidden-search case.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::connector::Health;

    fn ist() -> FixedOffset {
        FixedOffset::east_opt(5 * 3600 + 30 * 60).unwrap()
    }

    fn make_match(ts: &str, channel_id: &str, channel_name: &str, text: &str) -> Match {
        Match {
            ts: Some(ts.to_string()),
            text: Some(text.to_string()),
            permalink: Some(format!("https://acme.slack.com/archives/{channel_id}/p{ts}")),
            channel: Some(api::Channel {
                id: Some(channel_id.to_string()),
                name: Some(channel_name.to_string()),
            }),
        }
    }

    #[test]
    fn parses_slack_ts_to_microsecond_precision() {
        let dt = parse_slack_ts("1610000000.000200").expect("valid ts");
        assert_eq!(dt.timestamp(), 1_610_000_000);
        assert_eq!(dt.timestamp_subsec_micros(), 200);
        // Missing fraction is treated as .0.
        assert_eq!(parse_slack_ts("1610000000").unwrap().timestamp(), 1_610_000_000);
        assert!(parse_slack_ts("not-a-ts").is_none());
    }

    #[test]
    fn clean_preview_strips_markup_and_entities() {
        let out = clean_preview("hey <@U123> see <https://x.io|the doc> &amp; ping <#C1|general>");
        assert_eq!(out, "hey see the doc & ping general");
    }

    #[test]
    fn clean_preview_truncates_long_text() {
        let long = "x".repeat(200);
        let out = clean_preview(&long);
        // 90 chars plus the ellipsis.
        assert_eq!(out.chars().count(), 91);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn groups_by_channel_filtering_before_ist_midnight() {
        // Since = 2021-01-07 00:00 IST == 2021-01-06 18:30 UTC.
        let since = DateTime::from_timestamp(1_609_951_200, 0).unwrap();
        let old = since.timestamp() - 60; // one minute before the window
        let a1 = since.timestamp() + 10;
        let a2 = since.timestamp() + 20;
        let b1 = since.timestamp() + 5;

        let matches = vec![
            make_match(&format!("{old}.000000"), "C1", "general", "too old"),
            make_match(&format!("{a1}.000000"), "C1", "general", "first"),
            make_match(&format!("{a2}.000000"), "C1", "general", "latest here"),
            make_match(&format!("{b1}.000000"), "C2", "random", "other channel"),
        ];

        let groups = group_by_channel(&matches, since);
        assert_eq!(groups.len(), 2, "two active channels, stale match dropped");

        // Busiest channel (C1, 2 mentions) sorts first.
        assert_eq!(groups[0].name, "general");
        assert_eq!(groups[0].count, 2);
        // Latest message wins for preview + permalink.
        assert_eq!(groups[0].last_preview, "latest here");
        assert!(groups[0].last_permalink.as_deref().unwrap().contains(&a2.to_string()));

        assert_eq!(groups[1].name, "random");
        assert_eq!(groups[1].count, 1);
    }

    #[test]
    fn build_panels_emits_statcards_and_table() {
        let since = DateTime::from_timestamp(1_609_951_200, 0).unwrap();
        let matches = vec![
            make_match(&format!("{}.000000", since.timestamp() + 1), "C1", "general", "hi"),
            make_match(&format!("{}.000000", since.timestamp() + 2), "C1", "general", "again"),
        ];
        let groups = group_by_channel(&matches, since);
        let identity = api::AuthTest {
            ok: true,
            error: None,
            url: Some("https://acme.slack.com/".into()),
            team: Some("Acme".into()),
            user: Some("me".into()),
            user_id: Some("U123".into()),
            team_id: Some("T123".into()),
        };

        let panels = build_panels(&identity, &groups);
        assert_eq!(panels.len(), 2, "StatCards + Table");

        match &panels[0] {
            Panel::StatCards { stats, .. } => {
                assert_eq!(stats[0].value, "2"); // mentions today
                assert_eq!(stats[1].value, "1"); // active channels
            }
            other => panic!("expected StatCards, got {other:?}"),
        }
        match &panels[1] {
            Panel::Table(spec) => {
                assert_eq!(spec.columns.len(), 4);
                assert_eq!(spec.rows.len(), 1);
                let channel_cell = &spec.rows[0][0];
                assert_eq!(channel_cell.text, "#general");
                assert!(channel_cell.href.is_some(), "channel links to the permalink");
            }
            other => panic!("expected Table, got {other:?}"),
        }
    }

    #[test]
    fn user_token_prefix_is_enforced() {
        assert!(token::ResolvedToken { token: "xoxp-abc".into() }.is_user_token());
        assert!(!token::ResolvedToken { token: "xoxb-abc".into() }.is_user_token());
    }

    #[tokio::test]
    async fn no_token_returns_needs_auth() {
        std::env::remove_var("SLACK_USER_TOKEN");
        // Only exercise the offline no-token path. If this machine happens to
        // have a `fastdash/slack/default` keychain token, skip rather than make
        // a live API call (keeps the test hermetic and non-flaky).
        if token::resolve(token::DEFAULT_LABEL).is_some() {
            return;
        }
        let ctx = FetchCtx {
            timezone: "Asia/Kolkata".into(),
        };
        let snap = SlackConnector::new().fetch(&ctx).await.expect("fetch ok");
        assert!(
            matches!(snap.status, Health::NeedsAuth { .. }),
            "no token should yield NeedsAuth, got {:?}",
            snap.status
        );
        assert!(snap.panels.is_empty());
    }
}
