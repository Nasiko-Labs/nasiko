//! `chat_messages` ↔ Arrow. Pure: no IO, unit-tested below (TRD §8.1, §11).
//!
//! Messages of a session idle past `chat_idle_days` move to Lance; the session
//! stays writable (new turns go to Postgres, `load_session` merges by `(ts, id)`).
//! `user_id` is copied from `chat_sessions` so erasure and search filter cold rows
//! without a join.
//!
//! Large bodies: a `content` longer than `chat_body_inline_max` bytes is written to
//! the sidecar dataset `chat_message_bodies` (`id`, `user_id`, `session_id`,
//! `content`, `batch_id`) and the main row carries `content = NULL`,
//! `content_external = true`. Why a sidecar Lance dataset, not raw object-store
//! blobs: it rides the same `cold_path`/credentials, the same `batch_id` crash
//! recovery and the same `user_id` erasure + purge, so no second lifecycle to own.
//! The main dataset stays small, so session loads and scans read little; the
//! sidecar is opened only for the ids that need it.
//!
//! JSONB columns are stored as their Postgres text form; `cost_usd` as decimal text.

use std::sync::Arc;

use anyhow::{Context, Result};
use arrow_array::{
    Array, BooleanArray, Int32Array, RecordBatch, StringArray, TimestampMicrosecondArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub const TABLE: &str = "chat_messages";
pub const BODIES: &str = "chat_message_bodies";
pub const INDEXES: [&str; 3] = ["id", "session_id", "user_id"];
pub const BODY_INDEXES: [&str; 2] = ["id", "user_id"];

/// Select list over `chat_messages m JOIN chat_sessions s`.
pub const COLUMNS: &str = "m.id, m.session_id, s.user_id, m.role, m.content, \
    m.file_parts::text AS file_parts, m.has_file_parts, m.timestamp, m.input_tokens, \
    m.output_tokens, m.model, m.duration_ms, m.cost_usd::text AS cost_usd, m.usage_estimated, \
    m.trace_id, m.metadata::text AS metadata, m.cache_read_tokens, m.cache_creation_tokens, \
    m.external_turn_id";

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ChatMessageRow {
    pub id: Uuid,
    pub session_id: String,
    pub user_id: Uuid,
    pub role: String,
    pub content: String,
    pub file_parts: Option<String>,
    pub has_file_parts: bool,
    pub timestamp: DateTime<Utc>,
    pub input_tokens: Option<i32>,
    pub output_tokens: Option<i32>,
    pub model: Option<String>,
    pub duration_ms: Option<i32>,
    pub cost_usd: Option<String>,
    pub usage_estimated: Option<bool>,
    pub trace_id: Option<String>,
    pub metadata: Option<String>,
    pub cache_read_tokens: Option<i32>,
    pub cache_creation_tokens: Option<i32>,
    pub external_turn_id: Option<String>,
}

/// A cold row as read from the main dataset; `content` is empty when external.
#[derive(Debug, Clone, PartialEq)]
pub struct ColdMessage {
    pub row: ChatMessageRow,
    pub content_external: bool,
}

/// Lance batches for one tiering batch: main rows plus sidecar bodies, if any.
pub struct Batches {
    pub main: RecordBatch,
    pub bodies: Option<RecordBatch>,
}

pub fn sort_rows(rows: &mut [ChatMessageRow]) {
    rows.sort_by(|a, b| {
        (&a.session_id, a.timestamp, a.id).cmp(&(&b.session_id, b.timestamp, b.id))
    });
}

fn ts() -> DataType {
    DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()))
}

pub fn schema() -> SchemaRef {
    let u = |n: &str, null: bool| Field::new(n, DataType::Utf8, null);
    let i = |n: &str| Field::new(n, DataType::Int32, true);
    Arc::new(Schema::new(vec![
        u("id", false),
        u("session_id", false),
        u("user_id", false),
        u("role", false),
        u("content", true),
        Field::new("content_external", DataType::Boolean, false),
        u("file_parts", true),
        Field::new("has_file_parts", DataType::Boolean, false),
        Field::new("timestamp", ts(), false),
        i("input_tokens"),
        i("output_tokens"),
        u("model", true),
        i("duration_ms"),
        u("cost_usd", true),
        Field::new("usage_estimated", DataType::Boolean, true),
        u("trace_id", true),
        u("metadata", true),
        i("cache_read_tokens"),
        i("cache_creation_tokens"),
        u("external_turn_id", true),
        u("batch_id", false),
    ]))
}

pub fn body_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("user_id", DataType::Utf8, false),
        Field::new("session_id", DataType::Utf8, false),
        Field::new("content", DataType::Utf8, false),
        Field::new("batch_id", DataType::Utf8, false),
    ]))
}

fn strs<T>(rows: &[T], f: impl Fn(&T) -> Option<String>) -> Arc<dyn Array> {
    Arc::new(StringArray::from(
        rows.iter().map(f).collect::<Vec<Option<String>>>(),
    ))
}
fn ints(rows: &[ChatMessageRow], f: impl Fn(&ChatMessageRow) -> Option<i32>) -> Arc<dyn Array> {
    Arc::new(Int32Array::from(
        rows.iter().map(f).collect::<Vec<Option<i32>>>(),
    ))
}
fn bools(rows: &[ChatMessageRow], f: impl Fn(&ChatMessageRow) -> Option<bool>) -> Arc<dyn Array> {
    Arc::new(BooleanArray::from(
        rows.iter().map(f).collect::<Vec<Option<bool>>>(),
    ))
}

/// Splits bodies longer than `inline_max` bytes into the sidecar batch.
pub fn to_record_batches(
    rows: &[ChatMessageRow],
    batch_id: Uuid,
    inline_max: usize,
) -> Result<Batches> {
    let bid = batch_id.to_string();
    let ext = |r: &ChatMessageRow| r.content.len() > inline_max;
    let ts_arr: Arc<dyn Array> = Arc::new(
        TimestampMicrosecondArray::from(
            rows.iter()
                .map(|r| r.timestamp.timestamp_micros())
                .collect::<Vec<i64>>(),
        )
        .with_timezone("UTC"),
    );
    let cols: Vec<Arc<dyn Array>> = vec![
        strs(rows, |r| Some(r.id.to_string())),
        strs(rows, |r| Some(r.session_id.clone())),
        strs(rows, |r| Some(r.user_id.to_string())),
        strs(rows, |r| Some(r.role.clone())),
        strs(rows, |r| (!ext(r)).then(|| r.content.clone())),
        bools(rows, |r| Some(ext(r))),
        strs(rows, |r| r.file_parts.clone()),
        bools(rows, |r| Some(r.has_file_parts)),
        ts_arr,
        ints(rows, |r| r.input_tokens),
        ints(rows, |r| r.output_tokens),
        strs(rows, |r| r.model.clone()),
        ints(rows, |r| r.duration_ms),
        strs(rows, |r| r.cost_usd.clone()),
        bools(rows, |r| r.usage_estimated),
        strs(rows, |r| r.trace_id.clone()),
        strs(rows, |r| r.metadata.clone()),
        ints(rows, |r| r.cache_read_tokens),
        ints(rows, |r| r.cache_creation_tokens),
        strs(rows, |r| r.external_turn_id.clone()),
        strs(rows, |_| Some(bid.clone())),
    ];
    let main = RecordBatch::try_new(schema(), cols)?;
    let big: Vec<&ChatMessageRow> = rows.iter().filter(|r| ext(r)).collect();
    let bodies = if big.is_empty() {
        None
    } else {
        let cols: Vec<Arc<dyn Array>> = vec![
            strs(&big, |r| Some(r.id.to_string())),
            strs(&big, |r| Some(r.user_id.to_string())),
            strs(&big, |r| Some(r.session_id.clone())),
            strs(&big, |r| Some(r.content.clone())),
            strs(&big, |_| Some(bid.clone())),
        ];
        Some(RecordBatch::try_new(body_schema(), cols)?)
    };
    Ok(Batches { main, bodies })
}

fn col<'a, T: 'static>(b: &'a RecordBatch, n: &str) -> Result<&'a T> {
    b.column_by_name(n)
        .with_context(|| format!("missing {n}"))?
        .as_any()
        .downcast_ref::<T>()
        .with_context(|| format!("{n} has unexpected type"))
}

pub fn from_record_batch(b: &RecordBatch) -> Result<Vec<ColdMessage>> {
    let s = |n: &str| col::<StringArray>(b, n);
    let i = |n: &str| col::<Int32Array>(b, n);
    let bo = |n: &str| col::<BooleanArray>(b, n);
    let (id, sess, user, role, content, fparts) = (
        s("id")?,
        s("session_id")?,
        s("user_id")?,
        s("role")?,
        s("content")?,
        s("file_parts")?,
    );
    let (model, cost, trace, meta, turn) = (
        s("model")?,
        s("cost_usd")?,
        s("trace_id")?,
        s("metadata")?,
        s("external_turn_id")?,
    );
    let (inp, out, dur, cread, ccreate) = (
        i("input_tokens")?,
        i("output_tokens")?,
        i("duration_ms")?,
        i("cache_read_tokens")?,
        i("cache_creation_tokens")?,
    );
    let (ext, hasf, est) = (
        bo("content_external")?,
        bo("has_file_parts")?,
        bo("usage_estimated")?,
    );
    let tsa = col::<TimestampMicrosecondArray>(b, "timestamp")?;
    let os = |a: &StringArray, k: usize| (!a.is_null(k)).then(|| a.value(k).to_string());
    let oi = |a: &Int32Array, k: usize| (!a.is_null(k)).then(|| a.value(k));
    (0..b.num_rows())
        .map(|k| {
            Ok(ColdMessage {
                content_external: ext.value(k),
                row: ChatMessageRow {
                    id: Uuid::parse_str(id.value(k))?,
                    session_id: sess.value(k).to_string(),
                    user_id: Uuid::parse_str(user.value(k))?,
                    role: role.value(k).to_string(),
                    content: os(content, k).unwrap_or_default(),
                    file_parts: os(fparts, k),
                    has_file_parts: hasf.value(k),
                    timestamp: DateTime::from_timestamp_micros(tsa.value(k))
                        .context("timestamp out of range")?,
                    input_tokens: oi(inp, k),
                    output_tokens: oi(out, k),
                    model: os(model, k),
                    duration_ms: oi(dur, k),
                    cost_usd: os(cost, k),
                    usage_estimated: (!est.is_null(k)).then(|| est.value(k)),
                    trace_id: os(trace, k),
                    metadata: os(meta, k),
                    cache_read_tokens: oi(cread, k),
                    cache_creation_tokens: oi(ccreate, k),
                    external_turn_id: os(turn, k),
                },
            })
        })
        .collect()
}

/// `(id, content)` pairs from a sidecar batch.
pub fn bodies_from_record_batch(b: &RecordBatch) -> Result<Vec<(Uuid, String)>> {
    let (id, content) = (
        col::<StringArray>(b, "id")?,
        col::<StringArray>(b, "content")?,
    );
    (0..b.num_rows())
        .map(|k| Ok((Uuid::parse_str(id.value(k))?, content.value(k).to_string())))
        .collect()
}

/// Hot rows win over cold copies of the same id (tiering window, TRD §8);
/// result sorted by `(timestamp, id)`. Pure.
pub fn merge(hot: Vec<ChatMessageRow>, cold: Vec<ChatMessageRow>) -> Vec<ChatMessageRow> {
    let seen: std::collections::HashSet<Uuid> = hot.iter().map(|r| r.id).collect();
    let mut out = hot;
    out.extend(cold.into_iter().filter(|r| !seen.contains(&r.id)));
    out.sort_by_key(|r| (r.timestamp, r.id));
    out
}

/// Lowercased search terms; empty input → no terms.
pub fn terms(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Lance filter: every term appears in `column` (case-insensitive substring).
/// `%`/`_` in terms are matched literally via `ESCAPE`-free replacement.
pub fn like_filter(column: &str, terms: &[String]) -> String {
    terms
        .iter()
        .map(|t| {
            let t = t.replace(['%', '_'], "");
            format!(
                "lower({column}) LIKE {}",
                crate::cold::lit(&format!("%{t}%"))
            )
        })
        .collect::<Vec<String>>()
        .join(" AND ")
}

/// True when `content` contains every term (case-insensitive). Pure.
pub fn matches(content: &str, terms: &[String]) -> bool {
    let c = content.to_lowercase();
    terms.iter().all(|t| c.contains(t.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(micros: i64, content: &str) -> ChatMessageRow {
        ChatMessageRow {
            id: Uuid::from_u128(micros as u128),
            session_id: "s1".into(),
            user_id: Uuid::from_u128(7),
            role: "user".into(),
            content: content.into(),
            file_parts: Some("[]".into()),
            has_file_parts: false,
            timestamp: DateTime::from_timestamp_micros(micros).unwrap(),
            input_tokens: Some(3),
            output_tokens: None,
            model: None,
            duration_ms: None,
            cost_usd: Some("0.00100000".into()),
            usage_estimated: Some(true),
            trace_id: None,
            metadata: None,
            cache_read_tokens: None,
            cache_creation_tokens: Some(1),
            external_turn_id: None,
        }
    }

    #[test]
    fn round_trips_and_splits_large_bodies() {
        let rows = vec![row(1, "short"), row(2, &"x".repeat(100))];
        let b = to_record_batches(&rows, Uuid::new_v4(), 10).unwrap();
        let back = from_record_batch(&b.main).unwrap();
        assert_eq!(back[0].row, rows[0]);
        assert!(back[1].content_external);
        assert_eq!(back[1].row.content, "");
        let bodies = bodies_from_record_batch(&b.bodies.unwrap()).unwrap();
        assert_eq!(bodies, vec![(rows[1].id, rows[1].content.clone())]);
    }

    #[test]
    fn merge_prefers_hot_and_orders() {
        let mut hot = row(2, "hot");
        hot.content = "edited".into();
        let out = merge(vec![hot, row(3, "new")], vec![row(1, "a"), row(2, "b")]);
        let c: Vec<&str> = out.iter().map(|r| r.content.as_str()).collect();
        assert_eq!(c, ["a", "edited", "new"]);
    }

    #[test]
    fn search_terms() {
        let t = terms("Blue  WHALE");
        assert!(matches("a blue whale here", &t));
        assert!(!matches("blue only", &t));
        assert_eq!(
            like_filter("content", &t),
            "lower(content) LIKE '%blue%' AND lower(content) LIKE '%whale%'"
        );
    }
}
