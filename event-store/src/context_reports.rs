//! `context_reports` ↔ Arrow. Pure: no IO, unit-tested below.
//!
//! Rolled up into `context_reports_daily` (agent and price frozen at tiering time).

use std::sync::Arc;

use anyhow::{Context, Result};
use arrow_array::{
    Array, BooleanArray, Int64Array, RecordBatch, StringArray, TimestampMicrosecondArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub const TABLE: &str = "context_reports";
pub const INDEXES: [&str; 3] = ["id", "user_id", "trace_id"];
pub const COLUMNS: &str = "id, user_id, agent_id, session_id, trace_id, span_id, strategy, level, \
     budget::bigint AS budget, fallback, pool_messages::bigint AS pool_messages, pool_tokens, \
     kept_messages::bigint AS kept_messages, kept_tokens, compressed_bytes_saved, created_at";

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ContextReportRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub agent_id: Option<Uuid>,
    pub session_id: Option<String>,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub strategy: Option<String>,
    pub level: Option<String>,
    pub budget: Option<i64>,
    pub fallback: bool,
    pub pool_messages: i64,
    pub pool_tokens: i64,
    pub kept_messages: i64,
    pub kept_tokens: i64,
    pub compressed_bytes_saved: i64,
    pub created_at: DateTime<Utc>,
}

pub fn sort_rows(rows: &mut [ContextReportRow]) {
    rows.sort_by_key(|r| (r.user_id, r.created_at, r.id));
}

const STR: [(&str, bool); 8] = [
    ("id", false),
    ("user_id", false),
    ("agent_id", true),
    ("session_id", true),
    ("trace_id", true),
    ("span_id", true),
    ("strategy", true),
    ("level", true),
];

pub fn schema() -> SchemaRef {
    let mut f: Vec<Field> = STR
        .iter()
        .map(|(n, null)| Field::new(*n, DataType::Utf8, *null))
        .collect();
    f.push(Field::new("budget", DataType::Int64, true));
    f.push(Field::new("fallback", DataType::Boolean, false));
    for n in [
        "pool_messages",
        "pool_tokens",
        "kept_messages",
        "kept_tokens",
        "compressed_bytes_saved",
    ] {
        f.push(Field::new(n, DataType::Int64, false));
    }
    f.push(Field::new(
        "created_at",
        DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
        false,
    ));
    f.push(Field::new("batch_id", DataType::Utf8, false));
    Arc::new(Schema::new(f))
}

fn strs(r: &ContextReportRow) -> [Option<String>; 8] {
    [
        Some(r.id.to_string()),
        Some(r.user_id.to_string()),
        r.agent_id.map(|a| a.to_string()),
        r.session_id.clone(),
        r.trace_id.clone(),
        r.span_id.clone(),
        r.strategy.clone(),
        r.level.clone(),
    ]
}

pub fn to_record_batch(rows: &[ContextReportRow], batch_id: Uuid) -> Result<RecordBatch> {
    let s: Vec<[Option<String>; 8]> = rows.iter().map(strs).collect();
    let mut cols: Vec<Arc<dyn Array>> = (0..STR.len())
        .map(|i| {
            Arc::new(StringArray::from(
                s.iter().map(|r| r[i].clone()).collect::<Vec<_>>(),
            )) as Arc<dyn Array>
        })
        .collect();
    let i = |f: &dyn Fn(&ContextReportRow) -> Option<i64>| -> Arc<dyn Array> {
        Arc::new(Int64Array::from(rows.iter().map(f).collect::<Vec<_>>()))
    };
    cols.push(i(&|r| r.budget));
    cols.push(Arc::new(BooleanArray::from(
        rows.iter().map(|r| r.fallback).collect::<Vec<_>>(),
    )));
    cols.push(i(&|r| Some(r.pool_messages)));
    cols.push(i(&|r| Some(r.pool_tokens)));
    cols.push(i(&|r| Some(r.kept_messages)));
    cols.push(i(&|r| Some(r.kept_tokens)));
    cols.push(i(&|r| Some(r.compressed_bytes_saved)));
    cols.push(Arc::new(
        TimestampMicrosecondArray::from(
            rows.iter()
                .map(|r| r.created_at.timestamp_micros())
                .collect::<Vec<_>>(),
        )
        .with_timezone("UTC"),
    ));
    cols.push(Arc::new(StringArray::from(vec![
        batch_id.to_string();
        rows.len()
    ])));
    Ok(RecordBatch::try_new(schema(), cols)?)
}

pub fn from_record_batch(b: &RecordBatch) -> Result<Vec<ContextReportRow>> {
    fn get<'a, T: 'static>(b: &'a RecordBatch, n: &str) -> Result<&'a T> {
        b.column_by_name(n)
            .with_context(|| format!("missing {n}"))?
            .as_any()
            .downcast_ref::<T>()
            .with_context(|| format!("{n} wrong type"))
    }
    let s = |n: &str| get::<StringArray>(b, n);
    let i = |n: &str| get::<Int64Array>(b, n);
    let opt = |a: &StringArray, k: usize| (!a.is_null(k)).then(|| a.value(k).to_string());
    let (id, user, agent, sess, trace, span, strat, level) = (
        s("id")?,
        s("user_id")?,
        s("agent_id")?,
        s("session_id")?,
        s("trace_id")?,
        s("span_id")?,
        s("strategy")?,
        s("level")?,
    );
    let budget = i("budget")?;
    let fb = get::<BooleanArray>(b, "fallback")?;
    let (pm, pt, km, kt, cb) = (
        i("pool_messages")?,
        i("pool_tokens")?,
        i("kept_messages")?,
        i("kept_tokens")?,
        i("compressed_bytes_saved")?,
    );
    let at = get::<TimestampMicrosecondArray>(b, "created_at")?;
    (0..b.num_rows())
        .map(|k| {
            Ok(ContextReportRow {
                id: Uuid::parse_str(id.value(k))?,
                user_id: Uuid::parse_str(user.value(k))?,
                agent_id: opt(agent, k).map(|a| Uuid::parse_str(&a)).transpose()?,
                session_id: opt(sess, k),
                trace_id: opt(trace, k),
                span_id: opt(span, k),
                strategy: opt(strat, k),
                level: opt(level, k),
                budget: (!budget.is_null(k)).then(|| budget.value(k)),
                fallback: fb.value(k),
                pool_messages: pm.value(k),
                pool_tokens: pt.value(k),
                kept_messages: km.value(k),
                kept_tokens: kt.value(k),
                compressed_bytes_saved: cb.value(k),
                created_at: DateTime::from_timestamp_micros(at.value(k))
                    .context("created_at out of range")?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_arrow() {
        let mk = |u: u128, m: i64, agent: Option<Uuid>| ContextReportRow {
            id: Uuid::new_v4(),
            user_id: Uuid::from_u128(u),
            agent_id: agent,
            session_id: None,
            trace_id: Some("t".into()),
            span_id: None,
            strategy: Some("pacms".into()),
            level: None,
            budget: Some(100),
            fallback: false,
            pool_messages: 10,
            pool_tokens: 1000,
            kept_messages: 4,
            kept_tokens: 400,
            compressed_bytes_saved: 7,
            created_at: DateTime::from_timestamp_micros(m).unwrap(),
        };
        let mut rows = vec![
            mk(2, 2_000_000, Some(Uuid::from_u128(9))),
            mk(1, 1_000_000, None),
        ];
        sort_rows(&mut rows);
        assert_eq!(rows[0].user_id, Uuid::from_u128(1));
        let b = to_record_batch(&rows, Uuid::new_v4()).unwrap();
        assert_eq!(from_record_batch(&b).unwrap(), rows);
    }
}
