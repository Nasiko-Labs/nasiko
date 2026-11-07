//! One page of a tiered conversation, bounded on both tiers (TRD §8.1, C1).
//!
//! The hot side is a keyset SQL read and the cold side a filtered, ordered Lance
//! scan, each capped at the page size; the two are merged (hot wins on a shared
//! id, as in [`chat_messages::merge`]) and cut to the page. Only then are the
//! sidecar bodies of the page's external rows read, so paging a long archive
//! costs one page of rows and bodies per request, not the whole conversation.
//! A row ranked k-th in the merged order is ranked at most k-th on its own tier,
//! so `limit` rows from each tier always cover the merged page.

use anyhow::Result;
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use lance::dataset::scanner::ColumnOrdering;
use uuid::Uuid;

use crate::chat_messages::{self, ChatMessageRow, ColdMessage};
use crate::{EventStore, cold, metrics};

/// A `(timestamp, id)` keyset bound, the composite the SQL paths use.
pub type Keyset = (DateTime<Utc>, Uuid);

/// What to read: rows strictly after `after` (oldest first) or strictly before
/// `before` (newest first), or the newest / oldest rows with neither.
#[derive(Debug, Clone, Copy)]
pub struct PageQuery {
    pub after: Option<Keyset>,
    pub before: Option<Keyset>,
    /// Read oldest first. Implied by `after`; otherwise newest first.
    pub ascending: bool,
    /// `system` rows (HITL continuations) are hidden from transcripts.
    pub include_system: bool,
    pub limit: usize,
}

impl PageQuery {
    fn ascending(&self) -> bool {
        self.after.is_some() || self.ascending
    }
}

impl EventStore {
    /// One page of `session_id`'s messages, in read order (oldest first when
    /// ascending, else newest first), at most `q.limit` rows.
    pub async fn chat_page(&self, session_id: &str, q: PageQuery) -> Result<Vec<ChatMessageRow>> {
        let hot: Vec<ChatMessageRow> = self.hot_page(session_id, &q).await?;
        let started = std::time::Instant::now();
        let cold_rows: Vec<ColdMessage> = self.cold_page(session_id, &q).await?;
        let page: Vec<(ChatMessageRow, bool)> = take_page(hot, cold_rows, q.ascending(), q.limit);
        let rows: Vec<ChatMessageRow> = self.hydrate(page).await?;
        metrics::metrics().cold_read(chat_messages::TABLE, "chat_page", started.elapsed());
        Ok(rows)
    }

    async fn hot_page(&self, session_id: &str, q: &PageQuery) -> Result<Vec<ChatMessageRow>> {
        let dir: &str = if q.ascending() { "ASC" } else { "DESC" };
        let system: &str = if q.include_system {
            ""
        } else {
            " AND m.role <> 'system'"
        };
        let (bound, key): (&str, Option<Keyset>) = match (q.after, q.before) {
            (Some(a), _) => (" AND (m.timestamp, m.id) > ($2, $3)", Some(a)),
            (None, Some(b)) => (" AND (m.timestamp, m.id) < ($2, $3)", Some(b)),
            (None, None) => ("", None),
        };
        let limit_arg: &str = if key.is_some() { "$4" } else { "$2" };
        let sql: String = format!(
            "SELECT {} FROM chat_messages m JOIN chat_sessions s USING (session_id)
             WHERE m.session_id = $1{system}{bound}
             ORDER BY m.timestamp {dir}, m.id {dir} LIMIT {limit_arg}",
            chat_messages::COLUMNS
        );
        let mut query = sqlx::query_as::<_, ChatMessageRow>(&sql).bind(session_id);
        if let Some((ts, id)) = key {
            query = query.bind(ts).bind(id);
        }
        Ok(query
            .bind(i64::try_from(q.limit).unwrap_or(i64::MAX))
            .fetch_all(&self.db)
            .await?)
    }

    async fn cold_page(&self, session_id: &str, q: &PageQuery) -> Result<Vec<ColdMessage>> {
        let Some(ds) = cold::open(&self.uri(chat_messages::TABLE)).await? else {
            return Ok(vec![]);
        };
        let order = |c: &str| {
            if q.ascending() {
                ColumnOrdering::asc_nulls_last(c.to_owned())
            } else {
                ColumnOrdering::desc_nulls_last(c.to_owned())
            }
        };
        let mut s = ds.scan();
        s.filter(&cold_filter(session_id, q))?
            .order_by(Some(vec![order("timestamp"), order("id")]))?
            .limit(Some(i64::try_from(q.limit).unwrap_or(i64::MAX)), None)?;
        let batches: Vec<arrow_array::RecordBatch> =
            s.try_into_stream().await?.try_collect().await?;
        let mut out: Vec<ColdMessage> = Vec::new();
        for b in &batches {
            out.extend(chat_messages::from_record_batch(b)?);
        }
        Ok(out)
    }

    /// Reads sidecar bodies for the page's external rows only.
    async fn hydrate(&self, page: Vec<(ChatMessageRow, bool)>) -> Result<Vec<ChatMessageRow>> {
        let ext: Vec<String> = external_ids(&page)
            .iter()
            .map(|id| cold::lit(&id.to_string()))
            .collect();
        let mut bodies: std::collections::HashMap<Uuid, String> = Default::default();
        if !ext.is_empty() {
            let f: String = format!("id IN ({})", ext.join(", "));
            for b in cold::scan(&self.uri(chat_messages::BODIES), &f).await? {
                bodies.extend(chat_messages::bodies_from_record_batch(&b)?);
            }
        }
        Ok(page
            .into_iter()
            .map(|(mut r, external)| {
                if external {
                    r.content = bodies.remove(&r.id).unwrap_or_default();
                }
                r
            })
            .collect())
    }
}

/// The Lance filter for one page: the session, visibility and the keyset bound
/// written out as `(timestamp, id) <op> (t, i)`. Ids are lowercase hyphenated
/// UUID strings, whose string order is the UUID byte order Postgres sorts by.
fn cold_filter(session_id: &str, q: &PageQuery) -> String {
    let mut parts: Vec<String> = vec![format!("session_id = {}", cold::lit(session_id))];
    if !q.include_system {
        parts.push("role <> 'system'".into());
    }
    let bound = |(t, id): Keyset, op: &str| {
        let (ts, id): (String, String) = (ts_lit(t), cold::lit(&id.to_string()));
        format!("(timestamp {op} {ts} OR (timestamp = {ts} AND id {op} {id}))")
    };
    match (q.after, q.before) {
        (Some(a), _) => parts.push(bound(a, ">")),
        (None, Some(b)) => parts.push(bound(b, "<")),
        (None, None) => {}
    }
    parts.join(" AND ")
}

fn ts_lit(t: DateTime<Utc>) -> String {
    format!("TIMESTAMP '{}'", t.format("%Y-%m-%d %H:%M:%S%.6f"))
}

/// Merge both tiers' bounded reads into the page, in read order. Hot rows win
/// over a cold copy of the same id. The flag marks rows whose body is external.
fn take_page(
    hot: Vec<ChatMessageRow>,
    cold_rows: Vec<ColdMessage>,
    ascending: bool,
    limit: usize,
) -> Vec<(ChatMessageRow, bool)> {
    let seen: std::collections::HashSet<Uuid> = hot.iter().map(|r| r.id).collect();
    let mut all: Vec<(ChatMessageRow, bool)> = hot.into_iter().map(|r| (r, false)).collect();
    all.extend(
        cold_rows
            .into_iter()
            .filter(|m| !seen.contains(&m.row.id))
            .map(|m| (m.row, m.content_external)),
    );
    all.sort_by_key(|(r, _)| (r.timestamp, r.id));
    if !ascending {
        all.reverse();
    }
    all.truncate(limit);
    all
}

fn external_ids(page: &[(ChatMessageRow, bool)]) -> Vec<Uuid> {
    page.iter()
        .filter(|(_, ext)| *ext)
        .map(|(r, _)| r.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn row(secs: i64) -> ChatMessageRow {
        ChatMessageRow {
            id: Uuid::from_u128(secs as u128),
            session_id: "s".into(),
            user_id: Uuid::nil(),
            role: "user".into(),
            content: format!("m{secs}"),
            file_parts: None,
            has_file_parts: false,
            timestamp: Utc.timestamp_opt(secs, 0).unwrap(),
            input_tokens: None,
            output_tokens: None,
            model: None,
            duration_ms: None,
            cost_usd: None,
            usage_estimated: None,
            trace_id: None,
            metadata: None,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            external_turn_id: None,
        }
    }

    fn cold(secs: i64, external: bool) -> ColdMessage {
        ColdMessage {
            row: row(secs),
            content_external: external,
        }
    }

    fn secs(p: &[(ChatMessageRow, bool)]) -> Vec<i64> {
        p.iter().map(|(r, _)| r.timestamp.timestamp()).collect()
    }

    #[test]
    fn merges_tiers_in_read_order_and_cuts_to_the_page() {
        let hot: Vec<ChatMessageRow> = vec![row(5), row(4)];
        let cold_rows: Vec<ColdMessage> = vec![cold(3, false), cold(2, true)];
        let newest = take_page(hot.clone(), cold_rows.clone(), false, 3);
        assert_eq!(secs(&newest), vec![5, 4, 3]);
        let oldest = take_page(hot, cold_rows, true, 3);
        assert_eq!(secs(&oldest), vec![2, 3, 4]);
    }

    #[test]
    fn hot_wins_over_its_cold_copy() {
        let mut fresh: ChatMessageRow = row(3);
        fresh.content = "hot".into();
        let page = take_page(vec![fresh], vec![cold(3, true)], true, 10);
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].0.content, "hot");
        assert!(!page[0].1, "the hot row has its own body");
    }

    #[test]
    fn bodies_are_read_only_for_the_page() {
        // Three external cold rows, a page of two: only those two bodies are fetched.
        let page = take_page(
            vec![],
            vec![cold(1, true), cold(2, true), cold(3, true)],
            false,
            2,
        );
        assert_eq!(
            external_ids(&page),
            vec![Uuid::from_u128(3), Uuid::from_u128(2)]
        );
    }

    #[test]
    fn filter_carries_session_visibility_and_composite_bound() {
        let k: Keyset = (Utc.timestamp_opt(10, 0).unwrap(), Uuid::from_u128(7));
        let q = PageQuery {
            after: None,
            before: Some(k),
            ascending: false,
            include_system: false,
            limit: 5,
        };
        let f: String = cold_filter("it's", &q);
        assert!(f.starts_with("session_id = 'it''s'"), "{f}");
        assert!(f.contains("role <> 'system'"));
        assert!(f.contains(
            "(timestamp < TIMESTAMP '1970-01-01 00:00:10.000000' OR \
             (timestamp = TIMESTAMP '1970-01-01 00:00:10.000000' AND \
             id < '00000000-0000-0000-0000-000000000007'))"
        ));
        let all = PageQuery {
            include_system: true,
            ..q
        };
        assert!(
            !cold_filter(
                "s",
                &PageQuery {
                    before: None,
                    ..all
                }
            )
            .contains("role")
        );
    }

    #[test]
    fn uuid_strings_sort_like_uuids() {
        let a = Uuid::from_u128(0x0a);
        let b = Uuid::from_u128(0xf0_0000_0000_0000_0000);
        assert_eq!(a < b, a.to_string() < b.to_string());
    }
}
