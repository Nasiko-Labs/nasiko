//! `tool_calls` ↔ Arrow. Pure: no IO, unit-tested below.
//!
//! No rollup: per-call rows are read back per user/session from Lance. Shares the
//! 90-day `trace_usage` hot window. The Postgres key is `(trace_id, call_id)`;
//! cold rows carry `row_key` (`trace_id/call_id`) so crash recovery reads one column.
//! JSONB bodies are stored as UTF-8 text.
//!
//! Large bodies: when `arguments` or `output` is longer than `tool_body_inline_max`
//! bytes, both go to the sidecar dataset `tool_call_bodies` (`row_key`, `user_id`,
//! `session_id`, `arguments`, `output`, `batch_id`) and the main row carries NULLs
//! plus `body_external = true`. Same rationale as `chat_message_bodies`: one
//! lifecycle (batch recovery, erasure, purge) and a small main dataset.

use std::sync::Arc;

use anyhow::{Context, Result};
use arrow_array::{Array, Int64Array, RecordBatch, StringArray, TimestampMicrosecondArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub const TABLE: &str = "tool_calls";
pub const BODIES: &str = "tool_call_bodies";
pub const INDEXES: [&str; 3] = ["row_key", "user_id", "session_id"];
pub const BODY_INDEXES: [&str; 2] = ["row_key", "user_id"];
pub const COLUMNS: &str = "trace_id, call_id, user_id, agent_id, agent_name, session_id, \
    tool_name, tool_kind, status, error, arguments::text AS arguments, output::text AS output, \
    duration_ms, started_at, created_at";

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ToolCallRow {
    pub trace_id: String,
    pub call_id: String,
    pub user_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub agent_name: String,
    pub session_id: Option<String>,
    pub tool_name: String,
    pub tool_kind: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub arguments: Option<String>,
    pub output: Option<String>,
    pub duration_ms: Option<i64>,
    pub started_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

pub fn row_key(trace_id: &str, call_id: &str) -> String {
    format!("{trace_id}/{call_id}")
}

/// Inverse of `row_key`. Trace ids never contain `/`; call ids may.
pub fn split_key(key: &str) -> Option<(&str, &str)> {
    key.split_once('/')
}

pub fn sort_rows(rows: &mut [ToolCallRow]) {
    rows.sort_by(|a, b| {
        (a.user_id, a.started_at, &a.trace_id, &a.call_id).cmp(&(
            b.user_id,
            b.started_at,
            &b.trace_id,
            &b.call_id,
        ))
    });
}

const STR: [(&str, bool); 12] = [
    ("row_key", false),
    ("trace_id", false),
    ("call_id", false),
    ("user_id", true),
    ("agent_id", true),
    ("agent_name", false),
    ("session_id", true),
    ("tool_name", false),
    ("tool_kind", true),
    ("status", false),
    ("error", true),
    ("arguments", true),
];

fn ts() -> DataType {
    DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()))
}

pub fn schema() -> SchemaRef {
    let mut f: Vec<Field> = STR
        .iter()
        .map(|(n, null)| Field::new(*n, DataType::Utf8, *null))
        .collect();
    f.push(Field::new("output", DataType::Utf8, true));
    f.push(Field::new("body_external", DataType::Boolean, false));
    f.push(Field::new("duration_ms", DataType::Int64, true));
    f.push(Field::new("started_at", ts(), false));
    f.push(Field::new("created_at", ts(), false));
    f.push(Field::new("batch_id", DataType::Utf8, false));
    Arc::new(Schema::new(f))
}

pub fn body_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("row_key", DataType::Utf8, false),
        Field::new("user_id", DataType::Utf8, true),
        Field::new("session_id", DataType::Utf8, true),
        Field::new("arguments", DataType::Utf8, true),
        Field::new("output", DataType::Utf8, true),
        Field::new("batch_id", DataType::Utf8, false),
    ]))
}

/// Lance batches for one tiering batch: main rows plus sidecar bodies, if any.
pub struct Batches {
    pub main: RecordBatch,
    pub bodies: Option<RecordBatch>,
}

/// A cold row from the main dataset; `arguments`/`output` are `None` when external.
#[derive(Debug, Clone, PartialEq)]
pub struct ColdToolCall {
    pub row: ToolCallRow,
    pub body_external: bool,
}

/// Sidecar body of one call.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallBody {
    pub row_key: String,
    pub arguments: Option<String>,
    pub output: Option<String>,
}

fn is_external(r: &ToolCallRow, inline_max: usize) -> bool {
    let len = |s: &Option<String>| s.as_ref().map_or(0, String::len);
    len(&r.arguments) > inline_max || len(&r.output) > inline_max
}

/// Splits rows whose `arguments` or `output` exceed `inline_max` bytes into the sidecar.
pub fn to_record_batches(
    rows: &[ToolCallRow],
    batch_id: Uuid,
    inline_max: usize,
) -> Result<Batches> {
    let ext: Vec<bool> = rows.iter().map(|r| is_external(r, inline_max)).collect();
    let main = main_batch(rows, &ext, batch_id)?;
    let big: Vec<&ToolCallRow> = rows
        .iter()
        .zip(&ext)
        .filter(|(_, e)| **e)
        .map(|(r, _)| r)
        .collect();
    let bodies = if big.is_empty() {
        None
    } else {
        let s = |f: &dyn Fn(&ToolCallRow) -> Option<String>| -> Arc<dyn Array> {
            Arc::new(StringArray::from(
                big.iter().map(|r| f(r)).collect::<Vec<Option<String>>>(),
            ))
        };
        let bid = batch_id.to_string();
        let cols: Vec<Arc<dyn Array>> = vec![
            s(&|r| Some(row_key(&r.trace_id, &r.call_id))),
            s(&|r| r.user_id.map(|u| u.to_string())),
            s(&|r| r.session_id.clone()),
            s(&|r| r.arguments.clone()),
            s(&|r| r.output.clone()),
            s(&|_| Some(bid.clone())),
        ];
        Some(RecordBatch::try_new(body_schema(), cols)?)
    };
    Ok(Batches { main, bodies })
}

/// All bodies inline (no sidecar).
pub fn to_record_batch(rows: &[ToolCallRow], batch_id: Uuid) -> Result<RecordBatch> {
    main_batch(rows, &vec![false; rows.len()], batch_id)
}

fn main_batch(rows: &[ToolCallRow], ext: &[bool], batch_id: Uuid) -> Result<RecordBatch> {
    let body = |f: &dyn Fn(&ToolCallRow) -> Option<String>| -> Arc<dyn Array> {
        Arc::new(StringArray::from(
            rows.iter()
                .zip(ext)
                .map(|(r, e)| if *e { None } else { f(r) })
                .collect::<Vec<Option<String>>>(),
        ))
    };
    let s = |f: &dyn Fn(&ToolCallRow) -> Option<String>| -> Arc<dyn Array> {
        Arc::new(StringArray::from(rows.iter().map(f).collect::<Vec<_>>()))
    };
    let t = |f: &dyn Fn(&ToolCallRow) -> DateTime<Utc>| -> Arc<dyn Array> {
        Arc::new(
            TimestampMicrosecondArray::from(
                rows.iter()
                    .map(|r| f(r).timestamp_micros())
                    .collect::<Vec<_>>(),
            )
            .with_timezone("UTC"),
        )
    };
    let bid = batch_id.to_string();
    let cols: Vec<Arc<dyn Array>> = vec![
        s(&|r| Some(row_key(&r.trace_id, &r.call_id))),
        s(&|r| Some(r.trace_id.clone())),
        s(&|r| Some(r.call_id.clone())),
        s(&|r| r.user_id.map(|u| u.to_string())),
        s(&|r| r.agent_id.map(|u| u.to_string())),
        s(&|r| Some(r.agent_name.clone())),
        s(&|r| r.session_id.clone()),
        s(&|r| Some(r.tool_name.clone())),
        s(&|r| r.tool_kind.clone()),
        s(&|r| Some(r.status.clone())),
        s(&|r| r.error.clone()),
        body(&|r| r.arguments.clone()),
        body(&|r| r.output.clone()),
        Arc::new(arrow_array::BooleanArray::from(ext.to_vec())),
        Arc::new(Int64Array::from(
            rows.iter().map(|r| r.duration_ms).collect::<Vec<_>>(),
        )),
        t(&|r| r.started_at),
        t(&|r| r.created_at),
        s(&|_| Some(bid.clone())),
    ];
    Ok(RecordBatch::try_new(schema(), cols)?)
}

/// Rows with any external bodies left as `None`; fill them with [`fill_bodies`].
pub fn from_record_batch(b: &RecordBatch) -> Result<Vec<ToolCallRow>> {
    Ok(cold_from_record_batch(b)?
        .into_iter()
        .map(|c| c.row)
        .collect())
}

/// `body_external` is absent in datasets written before the sidecar; read as `false`.
pub fn cold_from_record_batch(b: &RecordBatch) -> Result<Vec<ColdToolCall>> {
    let ext: Option<&arrow_array::BooleanArray> = b
        .column_by_name("body_external")
        .and_then(|c| c.as_any().downcast_ref::<arrow_array::BooleanArray>());
    let rows: Vec<ToolCallRow> = rows_from_record_batch(b)?;
    Ok(rows
        .into_iter()
        .enumerate()
        .map(|(i, row)| ColdToolCall {
            row,
            body_external: ext.is_some_and(|e| e.value(i)),
        })
        .collect())
}

/// Bodies from a sidecar batch.
pub fn bodies_from_record_batch(b: &RecordBatch) -> Result<Vec<ToolCallBody>> {
    let col = |n: &str| -> Result<&StringArray> {
        b.column_by_name(n)
            .with_context(|| format!("missing {n}"))?
            .as_any()
            .downcast_ref::<StringArray>()
            .with_context(|| format!("{n} not utf8"))
    };
    let (key, args, out) = (col("row_key")?, col("arguments")?, col("output")?);
    let opt = |a: &StringArray, i: usize| (!a.is_null(i)).then(|| a.value(i).to_string());
    Ok((0..b.num_rows())
        .map(|i| ToolCallBody {
            row_key: key.value(i).to_string(),
            arguments: opt(args, i),
            output: opt(out, i),
        })
        .collect())
}

/// Puts sidecar bodies back on external rows. Pure.
pub fn fill_bodies(cold: Vec<ColdToolCall>, bodies: Vec<ToolCallBody>) -> Vec<ToolCallRow> {
    let mut by_key: std::collections::HashMap<String, ToolCallBody> =
        bodies.into_iter().map(|b| (b.row_key.clone(), b)).collect();
    cold.into_iter()
        .map(|mut c| {
            if c.body_external
                && let Some(b) = by_key.remove(&row_key(&c.row.trace_id, &c.row.call_id))
            {
                c.row.arguments = b.arguments;
                c.row.output = b.output;
            }
            c.row
        })
        .collect()
}

fn rows_from_record_batch(b: &RecordBatch) -> Result<Vec<ToolCallRow>> {
    let col = |n: &str| -> Result<&StringArray> {
        b.column_by_name(n)
            .with_context(|| format!("missing {n}"))?
            .as_any()
            .downcast_ref::<StringArray>()
            .with_context(|| format!("{n} not utf8"))
    };
    let tcol = |n: &str| -> Result<&TimestampMicrosecondArray> {
        b.column_by_name(n)
            .with_context(|| format!("missing {n}"))?
            .as_any()
            .downcast_ref::<TimestampMicrosecondArray>()
            .with_context(|| format!("{n} not timestamp"))
    };
    let opt = |a: &StringArray, i: usize| (!a.is_null(i)).then(|| a.value(i).to_string());
    let uuid = |a: &StringArray, i: usize| -> Result<Option<Uuid>> {
        opt(a, i)
            .map(|s| Uuid::parse_str(&s))
            .transpose()
            .map_err(Into::into)
    };
    let (trace, call, user, agent, name, sess) = (
        col("trace_id")?,
        col("call_id")?,
        col("user_id")?,
        col("agent_id")?,
        col("agent_name")?,
        col("session_id")?,
    );
    let (tool, kind, status, err, args, out) = (
        col("tool_name")?,
        col("tool_kind")?,
        col("status")?,
        col("error")?,
        col("arguments")?,
        col("output")?,
    );
    let dur = b
        .column_by_name("duration_ms")
        .context("missing duration_ms")?
        .as_any()
        .downcast_ref::<Int64Array>()
        .context("duration_ms not int64")?;
    let (started, created) = (tcol("started_at")?, tcol("created_at")?);
    let at = |a: &TimestampMicrosecondArray, i: usize| {
        DateTime::from_timestamp_micros(a.value(i)).context("timestamp out of range")
    };
    (0..b.num_rows())
        .map(|i| {
            Ok(ToolCallRow {
                trace_id: trace.value(i).to_string(),
                call_id: call.value(i).to_string(),
                user_id: uuid(user, i)?,
                agent_id: uuid(agent, i)?,
                agent_name: name.value(i).to_string(),
                session_id: opt(sess, i),
                tool_name: tool.value(i).to_string(),
                tool_kind: opt(kind, i),
                status: status.value(i).to_string(),
                error: opt(err, i),
                arguments: opt(args, i),
                output: opt(out, i),
                duration_ms: (!dur.is_null(i)).then(|| dur.value(i)),
                started_at: at(started, i)?,
                created_at: at(created, i)?,
            })
        })
        .collect()
}

/// Hot rows win over cold copies of the same `(trace_id, call_id)`; result is
/// sorted by `(started_at, call_id)`. Pure.
pub fn merge(hot: Vec<ToolCallRow>, cold: Vec<ToolCallRow>) -> Vec<ToolCallRow> {
    let seen: std::collections::HashSet<(String, String)> = hot
        .iter()
        .map(|r| (r.trace_id.clone(), r.call_id.clone()))
        .collect();
    let mut out = hot;
    out.extend(
        cold.into_iter()
            .filter(|r| !seen.contains(&(r.trace_id.clone(), r.call_id.clone()))),
    );
    out.sort_by(|a, b| (a.started_at, &a.call_id).cmp(&(b.started_at, &b.call_id)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_dedupes_and_prefers_hot() {
        let mut hot_b = row(1, 2_000, "b");
        hot_b.status = "failed".into();
        let cold = vec![row(1, 1_000, "a"), row(1, 2_000, "b")];
        let out = merge(vec![hot_b, row(1, 3_000, "c")], cold);
        let ids: Vec<&str> = out.iter().map(|r| r.call_id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        assert_eq!(out[1].status, "failed");
    }

    fn row(user: u128, micros: i64, call: &str) -> ToolCallRow {
        ToolCallRow {
            trace_id: "t1".into(),
            call_id: call.into(),
            user_id: Some(Uuid::from_u128(user)),
            agent_id: None,
            agent_name: "claude".into(),
            session_id: Some("s1".into()),
            tool_name: "Bash".into(),
            tool_kind: None,
            status: "succeeded".into(),
            error: None,
            arguments: Some("{\"cmd\":\"ls\"}".into()),
            output: None,
            duration_ms: Some(12),
            started_at: DateTime::from_timestamp_micros(micros).unwrap(),
            created_at: DateTime::from_timestamp_micros(micros).unwrap(),
        }
    }

    #[test]
    fn round_trips_through_arrow() {
        let mut rows = vec![row(2, 2_000_000, "a/b"), row(1, 1_000_000, "c")];
        sort_rows(&mut rows);
        assert_eq!(rows[0].user_id, Some(Uuid::from_u128(1)));
        let b = to_record_batch(&rows, Uuid::new_v4()).unwrap();
        assert_eq!(from_record_batch(&b).unwrap(), rows);
    }

    #[test]
    fn large_bodies_go_to_the_sidecar_and_come_back() {
        let mut big = row(1, 1_000_000, "big");
        big.output = Some("x".repeat(64));
        let small = row(1, 2_000_000, "small");
        let rows = vec![big.clone(), small.clone()];
        let b = to_record_batches(&rows, Uuid::new_v4(), 32).unwrap();
        let cold = cold_from_record_batch(&b.main).unwrap();
        assert!(cold[0].body_external && !cold[1].body_external);
        assert_eq!(cold[0].row.output, None);
        assert_eq!(cold[0].row.arguments, None);
        assert_eq!(cold[1].row, small);
        let bodies = bodies_from_record_batch(&b.bodies.unwrap()).unwrap();
        assert_eq!(bodies.len(), 1);
        assert_eq!(fill_bodies(cold, bodies), rows);
    }

    #[test]
    fn small_bodies_need_no_sidecar() {
        let b = to_record_batches(&[row(1, 1, "a")], Uuid::new_v4(), 1024).unwrap();
        assert!(b.bodies.is_none());
    }

    #[test]
    fn key_splits_on_first_slash() {
        let k = row_key("t1", "a/b");
        assert_eq!(split_key(&k), Some(("t1", "a/b")));
    }
}
