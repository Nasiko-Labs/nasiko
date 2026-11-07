//! `trace_usage` → Arrow. Pure: no IO, unit-tested below.
//!
//! The Postgres key is `(trace_id, agent_name)`; cold rows also carry `row_key`
//! (`trace_id/agent_name`) so crash recovery can read one column back.
//! Sorted by `(user_id, started_at)` like `token_usage` (TRD §3.0).

use std::sync::Arc;

use anyhow::{Context, Result};
use arrow_array::{
    Array, BooleanArray, Float64Array, Int32Array, Int64Array, RecordBatch, StringArray,
    TimestampMicrosecondArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub const TABLE: &str = "trace_usage";
pub const INDEXES: [&str; 4] = ["user_id", "agent_id", "session_id", "row_key"];

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct TraceUsageRow {
    pub trace_id: String,
    pub agent_name: String,
    pub session_id: Option<String>,
    pub agent_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cost_usd: f64,
    pub prompt_cost_usd: f64,
    pub completion_cost_usd: f64,
    pub latency_ms: Option<i64>,
    pub tool_call_count: i64,
    pub error_count: Option<i32>,
    pub cost_estimated: Option<bool>,
    pub started_at: DateTime<Utc>,
    pub materialized_at: DateTime<Utc>,
    /// `agent` / `coding` / `acp`.
    pub source: String,
    /// Router metadata merged per trace (JSONB as JSON text): brevity / minimal_code
    /// arms, key_source, operation_type.
    pub metadata: String,
}

pub const COLUMNS: &str = "trace_id, agent_name, session_id, agent_id, user_id, model, provider, \
    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, cost_usd, \
    prompt_cost_usd, completion_cost_usd, latency_ms, tool_call_count, error_count, \
    cost_estimated, started_at, materialized_at, source, metadata::text AS metadata";

/// Cold key for one row; `trace_id` is hex, so `/` never collides.
pub fn row_key(trace_id: &str, agent_name: &str) -> String {
    format!("{trace_id}/{agent_name}")
}

pub fn sort_rows(rows: &mut [TraceUsageRow]) {
    rows.sort_by(|a, b| {
        (a.user_id, a.started_at, &a.trace_id, &a.agent_name).cmp(&(
            b.user_id,
            b.started_at,
            &b.trace_id,
            &b.agent_name,
        ))
    });
}

pub fn schema() -> SchemaRef {
    let ts = || DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()));
    let mut f = vec![Field::new("row_key", DataType::Utf8, false)];
    for (n, null) in [
        ("trace_id", false),
        ("agent_name", false),
        ("session_id", true),
        ("agent_id", true),
        ("user_id", true),
        ("model", true),
        ("provider", true),
    ] {
        f.push(Field::new(n, DataType::Utf8, null));
    }
    for n in [
        "input_tokens",
        "output_tokens",
        "cache_read_tokens",
        "cache_creation_tokens",
    ] {
        f.push(Field::new(n, DataType::Int64, false));
    }
    for n in ["cost_usd", "prompt_cost_usd", "completion_cost_usd"] {
        f.push(Field::new(n, DataType::Float64, false));
    }
    f.extend([
        Field::new("latency_ms", DataType::Int64, true),
        Field::new("tool_call_count", DataType::Int64, false),
        Field::new("error_count", DataType::Int32, true),
        Field::new("cost_estimated", DataType::Boolean, true),
        Field::new("started_at", ts(), false),
        Field::new("materialized_at", ts(), false),
        Field::new("batch_id", DataType::Utf8, false),
        Field::new("source", DataType::Utf8, false),
        Field::new("metadata", DataType::Utf8, false),
    ]);
    Arc::new(Schema::new(f))
}

pub fn to_record_batch(rows: &[TraceUsageRow], batch_id: Uuid) -> Result<RecordBatch> {
    let s = |f: &dyn Fn(&TraceUsageRow) -> Option<String>| -> Arc<dyn Array> {
        Arc::new(StringArray::from(rows.iter().map(f).collect::<Vec<_>>()))
    };
    let i64c = |f: &dyn Fn(&TraceUsageRow) -> i64| -> Arc<dyn Array> {
        Arc::new(Int64Array::from(rows.iter().map(f).collect::<Vec<_>>()))
    };
    let f64c = |f: &dyn Fn(&TraceUsageRow) -> f64| -> Arc<dyn Array> {
        Arc::new(Float64Array::from(rows.iter().map(f).collect::<Vec<_>>()))
    };
    let tsc = |f: &dyn Fn(&TraceUsageRow) -> DateTime<Utc>| -> Arc<dyn Array> {
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
        s(&|r| Some(row_key(&r.trace_id, &r.agent_name))),
        s(&|r| Some(r.trace_id.clone())),
        s(&|r| Some(r.agent_name.clone())),
        s(&|r| r.session_id.clone()),
        s(&|r| r.agent_id.map(|v| v.to_string())),
        s(&|r| r.user_id.map(|v| v.to_string())),
        s(&|r| r.model.clone()),
        s(&|r| r.provider.clone()),
        i64c(&|r| r.input_tokens),
        i64c(&|r| r.output_tokens),
        i64c(&|r| r.cache_read_tokens),
        i64c(&|r| r.cache_creation_tokens),
        f64c(&|r| r.cost_usd),
        f64c(&|r| r.prompt_cost_usd),
        f64c(&|r| r.completion_cost_usd),
        Arc::new(Int64Array::from(
            rows.iter().map(|r| r.latency_ms).collect::<Vec<_>>(),
        )),
        i64c(&|r| r.tool_call_count),
        Arc::new(Int32Array::from(
            rows.iter().map(|r| r.error_count).collect::<Vec<_>>(),
        )),
        Arc::new(BooleanArray::from(
            rows.iter().map(|r| r.cost_estimated).collect::<Vec<_>>(),
        )),
        tsc(&|r| r.started_at),
        tsc(&|r| r.materialized_at),
        Arc::new(StringArray::from(vec![bid.as_str(); rows.len()])),
        s(&|r| Some(r.source.clone())),
        s(&|r| Some(r.metadata.clone())),
    ];
    RecordBatch::try_new(schema(), cols).context("trace_usage record batch")
}

/// Cold rows → `TraceUsageRow`. Inverse of [`to_record_batch`] (drops `row_key`/`batch_id`).
pub fn from_record_batch(b: &RecordBatch) -> Result<Vec<TraceUsageRow>> {
    fn col<'a, T: 'static>(b: &'a RecordBatch, n: &str) -> Result<&'a T> {
        b.column_by_name(n)
            .with_context(|| format!("trace_usage column {n} missing"))?
            .as_any()
            .downcast_ref::<T>()
            .with_context(|| format!("trace_usage column {n} has wrong type"))
    }
    let st = |n: &str| col::<StringArray>(b, n);
    let (trace, agent, session, agent_id, user_id, model, provider, source, meta) = (
        st("trace_id")?,
        st("agent_name")?,
        st("session_id")?,
        st("agent_id")?,
        st("user_id")?,
        st("model")?,
        st("provider")?,
        st("source")?,
        st("metadata")?,
    );
    let i = |n: &str| col::<Int64Array>(b, n);
    let (tin, tout, cr, cc, lat, tools) = (
        i("input_tokens")?,
        i("output_tokens")?,
        i("cache_read_tokens")?,
        i("cache_creation_tokens")?,
        i("latency_ms")?,
        i("tool_call_count")?,
    );
    let f = |n: &str| col::<Float64Array>(b, n);
    let (cost, pcost, ccost) = (
        f("cost_usd")?,
        f("prompt_cost_usd")?,
        f("completion_cost_usd")?,
    );
    let errs = col::<Int32Array>(b, "error_count")?;
    let est = col::<BooleanArray>(b, "cost_estimated")?;
    let started = col::<TimestampMicrosecondArray>(b, "started_at")?;
    let mat = col::<TimestampMicrosecondArray>(b, "materialized_at")?;

    let opt = |a: &StringArray, r: usize| (!a.is_null(r)).then(|| a.value(r).to_string());
    let uuid = |a: &StringArray, r: usize| -> Result<Option<Uuid>> {
        opt(a, r)
            .map(|s| Uuid::parse_str(&s).with_context(|| format!("bad uuid {s}")))
            .transpose()
    };
    let ts = |a: &TimestampMicrosecondArray, r: usize| {
        DateTime::from_timestamp_micros(a.value(r)).context("bad trace_usage timestamp")
    };
    let mut out = Vec::with_capacity(b.num_rows());
    for r in 0..b.num_rows() {
        out.push(TraceUsageRow {
            trace_id: trace.value(r).to_string(),
            agent_name: agent.value(r).to_string(),
            session_id: opt(session, r),
            agent_id: uuid(agent_id, r)?,
            user_id: uuid(user_id, r)?,
            model: opt(model, r),
            provider: opt(provider, r),
            input_tokens: tin.value(r),
            output_tokens: tout.value(r),
            cache_read_tokens: cr.value(r),
            cache_creation_tokens: cc.value(r),
            cost_usd: cost.value(r),
            prompt_cost_usd: pcost.value(r),
            completion_cost_usd: ccost.value(r),
            latency_ms: (!lat.is_null(r)).then(|| lat.value(r)),
            tool_call_count: tools.value(r),
            error_count: (!errs.is_null(r)).then(|| errs.value(r)),
            cost_estimated: (!est.is_null(r)).then(|| est.value(r)),
            started_at: ts(started, r)?,
            materialized_at: ts(mat, r)?,
            source: source.value(r).to_string(),
            metadata: meta.value(r).to_string(),
        });
    }
    Ok(out)
}

/// Hot + cold rows for one read, hot winning on `(trace_id, agent_name)`; ordered by
/// `started_at` then key. A row is either hot or cold after a committed batch, but a
/// crash between Lance append and the Postgres delete can briefly leave it in both.
pub fn merge(hot: Vec<TraceUsageRow>, cold: Vec<TraceUsageRow>) -> Vec<TraceUsageRow> {
    let keys: std::collections::HashSet<(String, String)> = hot
        .iter()
        .map(|r| (r.trace_id.clone(), r.agent_name.clone()))
        .collect();
    let mut out = hot;
    out.extend(
        cold.into_iter()
            .filter(|r| !keys.contains(&(r.trace_id.clone(), r.agent_name.clone()))),
    );
    out.sort_by(|a, b| {
        (a.started_at, &a.trace_id, &a.agent_name).cmp(&(b.started_at, &b.trace_id, &b.agent_name))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn round_trips_through_arrow() {
        let mut a = row("t1", Some(Uuid::from_u128(7)), 10);
        a.session_id = Some("s".into());
        a.latency_ms = Some(42);
        a.cost_estimated = Some(true);
        let b = row("t2", None, 5);
        let batch = to_record_batch(&[a.clone(), b.clone()], Uuid::nil()).unwrap();
        assert_eq!(from_record_batch(&batch).unwrap(), vec![a, b]);
    }

    #[test]
    fn merge_prefers_hot_and_orders_by_time() {
        let mut hot = row("t1", None, 10);
        hot.cost_usd = 9.0;
        let cold = vec![row("t1", None, 10), row("t0", None, 1)];
        let m = merge(vec![hot], cold);
        let ids: Vec<_> = m.iter().map(|r| r.trace_id.as_str()).collect();
        assert_eq!(ids, ["t0", "t1"]);
        assert_eq!(m[1].cost_usd, 9.0);
    }

    fn row(trace: &str, user: Option<Uuid>, secs: i64) -> TraceUsageRow {
        TraceUsageRow {
            trace_id: trace.into(),
            agent_name: "a".into(),
            session_id: None,
            agent_id: None,
            user_id: user,
            model: Some("m".into()),
            provider: None,
            input_tokens: 3,
            output_tokens: 4,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            cost_usd: 0.5,
            prompt_cost_usd: 0.2,
            completion_cost_usd: 0.3,
            latency_ms: None,
            tool_call_count: 2,
            error_count: Some(1),
            cost_estimated: None,
            started_at: Utc.timestamp_opt(secs, 0).unwrap(),
            materialized_at: Utc.timestamp_opt(secs, 0).unwrap(),
            source: "agent".into(),
            metadata: "{}".into(),
        }
    }

    #[test]
    fn batch_matches_schema_and_keys() {
        let rows = vec![row("t1", None, 10), row("t2", Some(Uuid::nil()), 5)];
        let b = to_record_batch(&rows, Uuid::nil()).unwrap();
        assert_eq!(b.num_rows(), 2);
        assert_eq!(b.schema(), schema());
        let keys = b
            .column_by_name("row_key")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(keys.value(0), "t1/a");
        let tools = b
            .column_by_name("tool_call_count")
            .unwrap()
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(tools.value(1), 2);
    }

    #[test]
    fn sort_groups_by_user_then_time() {
        let u = Some(Uuid::from_u128(1));
        let mut rows = vec![row("b", u, 9), row("a", u, 1), row("c", None, 50)];
        sort_rows(&mut rows);
        let order: Vec<_> = rows.iter().map(|r| r.trace_id.as_str()).collect();
        assert_eq!(order, ["c", "a", "b"]);
    }
}
