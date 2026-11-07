//! NAS-1161: row-level `token_usage` reads from the cold (Lance) dataset; the hot table is retired (0079) and drained there.
//!
//! Both stores are asked for the same page; results are merged newest-first on
//! `(created_at, id)` and de-duplicated by `id`. Querying Postgres for the whole
//! range is cheap (it only holds hot rows), and de-duplication covers the window
//! in which a batch is already in Lance but not yet deleted from Postgres. Lance
//! is skipped entirely when the range starts after the newest tiered row
//! (`tier_batches.max_ts`), so recent-only queries never touch object storage.
//!
//! Only filters backed by Lance BTREE indexes (or cheap to scan within them) are
//! offered here. Free-text / metadata filters stay with callers and apply to hot
//! data only.

use anyhow::Result;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::token_usage::{self, TABLE, TokenUsageRow};
use crate::{EventStore, cold, ts_lit};

/// Equality filters; every set field must match. `user_ids` empty = any user
/// (callers scoping to an org pass its member ids).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsageFilter {
    pub user_ids: Vec<Uuid>,
    pub agent_id: Option<Uuid>,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub operation_type: Option<String>,
}

/// Position after the last row of a page; the next page holds strictly older rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
}

impl Cursor {
    fn of(r: &TokenUsageRow) -> Self {
        Self {
            created_at: r.created_at,
            id: r.id,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsagePage {
    pub rows: Vec<TokenUsageRow>,
    /// `None` when there are no older rows.
    pub next: Option<Cursor>,
}

/// `[from, to)` plus paging.
#[derive(Debug, Clone, Copy)]
pub struct UsageRange {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

fn newest_first(a: &TokenUsageRow, b: &TokenUsageRow) -> std::cmp::Ordering {
    (b.created_at, b.id).cmp(&(a.created_at, a.id))
}

/// Merge two result sets into one page. Pure; unit-tested.
/// Each input may hold up to `limit + 1` rows; the extra row decides `next`.
pub fn merge_page(hot: Vec<TokenUsageRow>, cold: Vec<TokenUsageRow>, limit: usize) -> UsagePage {
    let mut seen = std::collections::HashSet::new();
    let mut rows: Vec<TokenUsageRow> = hot
        .into_iter()
        .chain(cold)
        .filter(|r| seen.insert(r.id))
        .collect();
    rows.sort_by(newest_first);
    let more = rows.len() > limit;
    rows.truncate(limit);
    let next = if more {
        rows.last().map(Cursor::of)
    } else {
        None
    };
    UsagePage { rows, next }
}

/// Lance needs the cold tier only if the range reaches at or below the newest tiered row.
pub fn needs_cold(range: &UsageRange, cold_max: Option<DateTime<Utc>>) -> bool {
    cold_max.is_some_and(|m| range.from <= m)
}

/// Lance filter string for the cold side. Pure; unit-tested.
pub fn lance_filter(f: &UsageFilter, range: &UsageRange, cursor: Option<Cursor>) -> String {
    let mut parts = vec![
        format!("created_at >= {}", ts_lit(range.from)),
        format!("created_at < {}", ts_lit(range.to)),
    ];
    if !f.user_ids.is_empty() {
        let ids: Vec<String> = f
            .user_ids
            .iter()
            .map(|u| cold::lit(&u.to_string()))
            .collect();
        parts.push(format!("user_id IN ({})", ids.join(", ")));
    }
    let eq = [
        ("agent_id", f.agent_id.map(|u| u.to_string())),
        ("session_id", f.session_id.clone()),
        ("model", f.model.clone()),
        ("provider", f.provider.clone()),
        ("operation_type", f.operation_type.clone()),
    ];
    for (col, v) in eq {
        if let Some(v) = v {
            parts.push(format!("{col} = {}", cold::lit(&v)));
        }
    }
    if let Some(c) = cursor {
        let t = ts_lit(c.created_at);
        parts.push(format!(
            "(created_at < {t} OR (created_at = {t} AND id < {}))",
            cold::lit(&c.id.to_string())
        ));
    }
    parts.join(" AND ")
}

impl EventStore {
    /// Newest `created_at` that has reached the cold tier (`live` batches).
    pub async fn cold_max(&self) -> Result<Option<DateTime<Utc>>> {
        Ok(sqlx::query_scalar(
            "SELECT max(max_ts) FROM tier_batches WHERE table_name = $1 AND state = 'live'",
        )
        .bind(TABLE)
        .fetch_one(&self.db)
        .await?)
    }

    /// Every cold `token_usage` row matching `filter` in `[from, to)`, unordered.
    /// For rows no rollup keeps (e.g. `mcp_tool_call`); callers aggregate and add hot rows.
    /// Empty without touching Lance when the range ends above the cold tier.
    pub async fn cold_rows(
        &self,
        filter: &UsageFilter,
        range: UsageRange,
    ) -> Result<Vec<TokenUsageRow>> {
        let mut out: Vec<TokenUsageRow> = Vec::new();
        if !needs_cold(&range, self.cold_max().await?) {
            return Ok(out);
        }
        for b in cold::scan(&self.uri(TABLE), &lance_filter(filter, &range, None)).await? {
            out.extend(token_usage::from_record_batch(&b)?);
        }
        Ok(out)
    }

    /// One page of `token_usage` rows, newest first, from the cold tier.
    pub async fn query(
        &self,
        filter: &UsageFilter,
        range: UsageRange,
        cursor: Option<Cursor>,
        limit: usize,
    ) -> Result<UsagePage> {
        let fetch = limit + 1;
        // token_usage is retired (0079); its rows are drained to the cold dataset.
        let hot: Vec<TokenUsageRow> = Vec::new();
        let mut cold_rows = Vec::new();
        if needs_cold(&range, self.cold_max().await?) {
            // Lance has no ORDER BY + LIMIT pushdown we rely on; the BTREE-filtered
            // scan is sorted here. Fine for per-user/agent ranges (M7 bench).
            for b in cold::scan(&self.uri(TABLE), &lance_filter(filter, &range, cursor)).await? {
                cold_rows.extend(token_usage::from_record_batch(&b)?);
            }
            cold_rows.sort_by(newest_first);
            cold_rows.truncate(fetch);
        }
        Ok(merge_page(hot, cold_rows, limit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token_usage::tests::row;
    use chrono::TimeZone;

    fn t(s: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + s, 0).unwrap()
    }

    #[test]
    fn merges_newest_first_and_dedupes() {
        let shared = row(1, 5);
        let hot = vec![row(1, 9), shared.clone()];
        let cold = vec![shared.clone(), row(1, 1)];
        let p = merge_page(hot, cold, 10);
        let secs: Vec<_> = p
            .rows
            .iter()
            .map(|r| r.created_at.timestamp() - 1_700_000_000)
            .collect();
        assert_eq!(secs, [9, 5, 1]);
        assert_eq!(p.next, None);
    }

    #[test]
    fn next_cursor_only_when_more() {
        let rows: Vec<_> = (0..4).map(|s| row(1, s)).collect();
        let p = merge_page(rows, vec![], 3);
        assert_eq!(p.rows.len(), 3);
        assert_eq!(p.next, Some(Cursor::of(&p.rows[2])));
        assert_eq!(p.next.unwrap().created_at, t(1));
    }

    #[test]
    fn ties_break_on_id() {
        let (mut a, mut b) = (row(1, 0), row(1, 0));
        a.id = Uuid::from_u128(1);
        b.id = Uuid::from_u128(2);
        let p = merge_page(vec![a], vec![b], 10);
        assert_eq!(p.rows[0].id, Uuid::from_u128(2));
    }

    #[test]
    fn cold_skipped_for_recent_ranges() {
        let r = UsageRange {
            from: t(100),
            to: t(200),
        };
        assert!(!needs_cold(&r, None));
        assert!(!needs_cold(&r, Some(t(50))));
        assert!(needs_cold(&r, Some(t(100))));
    }

    #[test]
    fn lance_filter_shape() {
        let f = UsageFilter {
            user_ids: vec![Uuid::from_u128(1)],
            model: Some("o'x".into()),
            ..Default::default()
        };
        let r = UsageRange {
            from: t(0),
            to: t(10),
        };
        let c = Cursor {
            created_at: t(5),
            id: Uuid::from_u128(7),
        };
        let s = lance_filter(&f, &r, Some(c));
        assert!(s.contains("user_id IN ('00000000-0000-0000-0000-000000000001')"));
        assert!(s.contains("model = 'o''x'"));
        assert!(s.contains("id < '00000000-0000-0000-0000-000000000007'"));
        assert!(!s.contains("agent_id"));
    }
}
