//! `token_usage` ↔ Arrow. Pure: no IO, unit-tested below.
//!
//! Columns keep their Postgres names, plus `batch_id`. `jsonb` → UTF-8 string,
//! `DECIMAL(10,8)` → `Decimal128(18, 8)`, UUIDs → strings, `timestamptz` →
//! microseconds UTC. Rows are sorted by `(user_id, created_at)` (TRD §3.0).

use std::sync::Arc;

use anyhow::{Context, Result};
use arrow_array::{
    Array, BooleanArray, Decimal128Array, Int32Array, RecordBatch, StringArray,
    TimestampMicrosecondArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use uuid::Uuid;

pub const TABLE: &str = "token_usage";
const COST_SCALE: u32 = 8;

/// One `token_usage` row as read for tiering.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct TokenUsageRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub agent_id: Option<Uuid>,
    pub operation_type: String,
    pub request_id: Option<String>,
    pub session_id: Option<String>,
    pub provider: String,
    pub model: String,
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub total_tokens: i32,
    pub cache_creation_input_tokens: Option<i32>,
    pub cache_read_input_tokens: Option<i32>,
    pub cached_tokens: Option<i32>,
    pub audio_tokens: Option<i32>,
    pub reasoning_tokens: Option<i32>,
    pub accepted_prediction_tokens: Option<i32>,
    pub rejected_prediction_tokens: Option<i32>,
    pub completion_tokens_details: Option<serde_json::Value>,
    pub prompt_tokens_details: Option<serde_json::Value>,
    pub cost_usd: Option<Decimal>,
    pub latency_ms: Option<i32>,
    pub ttft_ms: Option<i32>,
    pub streaming: Option<bool>,
    pub finish_reason: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

/// Postgres column list, in schema order (without `batch_id`).
pub const COLUMNS: &str = "id, user_id, agent_id, operation_type, request_id, session_id, \
    provider, model, input_tokens, output_tokens, total_tokens, cache_creation_input_tokens, \
    cache_read_input_tokens, cached_tokens, audio_tokens, reasoning_tokens, \
    accepted_prediction_tokens, rejected_prediction_tokens, completion_tokens_details, \
    prompt_tokens_details, cost_usd, latency_ms, ttft_ms, streaming, finish_reason, metadata, \
    created_at";

const INT_COLUMNS: [&str; 12] = [
    "input_tokens",
    "output_tokens",
    "total_tokens",
    "cache_creation_input_tokens",
    "cache_read_input_tokens",
    "cached_tokens",
    "audio_tokens",
    "reasoning_tokens",
    "accepted_prediction_tokens",
    "rejected_prediction_tokens",
    "latency_ms",
    "ttft_ms",
];

pub fn schema() -> SchemaRef {
    let ts = DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()));
    let mut f = vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("user_id", DataType::Utf8, false),
        Field::new("agent_id", DataType::Utf8, true),
        Field::new("operation_type", DataType::Utf8, false),
        Field::new("request_id", DataType::Utf8, true),
        Field::new("session_id", DataType::Utf8, true),
        Field::new("provider", DataType::Utf8, false),
        Field::new("model", DataType::Utf8, false),
    ];
    for (i, name) in INT_COLUMNS.iter().enumerate() {
        // input/output/total are NOT NULL in Postgres.
        f.push(Field::new(*name, DataType::Int32, i >= 3));
    }
    f.extend([
        Field::new("completion_tokens_details", DataType::Utf8, true),
        Field::new("prompt_tokens_details", DataType::Utf8, true),
        Field::new("cost_usd", DataType::Decimal128(18, COST_SCALE as i8), true),
        Field::new("streaming", DataType::Boolean, true),
        Field::new("finish_reason", DataType::Utf8, true),
        Field::new("metadata", DataType::Utf8, true),
        Field::new("created_at", ts, false),
        Field::new("batch_id", DataType::Utf8, false),
    ]);
    Arc::new(Schema::new(f))
}

fn int_values(r: &TokenUsageRow) -> [Option<i32>; 12] {
    [
        Some(r.input_tokens),
        Some(r.output_tokens),
        Some(r.total_tokens),
        r.cache_creation_input_tokens,
        r.cache_read_input_tokens,
        r.cached_tokens,
        r.audio_tokens,
        r.reasoning_tokens,
        r.accepted_prediction_tokens,
        r.rejected_prediction_tokens,
        r.latency_ms,
        r.ttft_ms,
    ]
}

/// Sort key for cold batches: `(user_id, created_at, id)`.
pub fn sort_rows(rows: &mut [TokenUsageRow]) {
    rows.sort_by_key(|r| (r.user_id, r.created_at, r.id));
}

fn cost_mantissa(d: Decimal) -> i128 {
    let mut d = d;
    d.rescale(COST_SCALE);
    d.mantissa()
}

/// Rows → one Arrow batch tagged with `batch_id`. Caller sorts first.
pub fn to_record_batch(rows: &[TokenUsageRow], batch_id: Uuid) -> Result<RecordBatch> {
    let s = |f: fn(&TokenUsageRow) -> Option<String>| -> Arc<dyn Array> {
        Arc::new(StringArray::from(rows.iter().map(f).collect::<Vec<_>>()))
    };
    let json = |f: fn(&TokenUsageRow) -> &Option<serde_json::Value>| -> Arc<dyn Array> {
        Arc::new(StringArray::from(
            rows.iter()
                .map(|r| f(r).as_ref().map(|v| v.to_string()))
                .collect::<Vec<_>>(),
        ))
    };
    let mut cols: Vec<Arc<dyn Array>> = vec![
        s(|r| Some(r.id.to_string())),
        s(|r| Some(r.user_id.to_string())),
        s(|r| r.agent_id.map(|a| a.to_string())),
        s(|r| Some(r.operation_type.clone())),
        s(|r| r.request_id.clone()),
        s(|r| r.session_id.clone()),
        s(|r| Some(r.provider.clone())),
        s(|r| Some(r.model.clone())),
    ];
    let ints: Vec<[Option<i32>; 12]> = rows.iter().map(int_values).collect();
    for i in 0..12 {
        cols.push(Arc::new(Int32Array::from(
            ints.iter().map(|v| v[i]).collect::<Vec<_>>(),
        )));
    }
    cols.push(json(|r| &r.completion_tokens_details));
    cols.push(json(|r| &r.prompt_tokens_details));
    cols.push(Arc::new(
        Decimal128Array::from(
            rows.iter()
                .map(|r| r.cost_usd.map(cost_mantissa))
                .collect::<Vec<_>>(),
        )
        .with_precision_and_scale(18, COST_SCALE as i8)?,
    ));
    cols.push(Arc::new(BooleanArray::from(
        rows.iter().map(|r| r.streaming).collect::<Vec<_>>(),
    )));
    cols.push(s(|r| r.finish_reason.clone()));
    cols.push(json(|r| &r.metadata));
    cols.push(Arc::new(
        TimestampMicrosecondArray::from(
            rows.iter()
                .map(|r| r.created_at.timestamp_micros())
                .collect::<Vec<_>>(),
        )
        .with_timezone("UTC"),
    ));
    let bid = batch_id.to_string();
    cols.push(Arc::new(StringArray::from(vec![bid.as_str(); rows.len()])));
    RecordBatch::try_new(schema(), cols).context("token_usage record batch")
}

/// Arrow batch → rows (typed cold reader).
pub fn from_record_batch(b: &RecordBatch) -> Result<Vec<TokenUsageRow>> {
    let str_col = |name: &str| -> Result<&StringArray> {
        b.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .with_context(|| format!("column {name}"))
    };
    let int_col = |name: &str| -> Result<&Int32Array> {
        b.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Int32Array>())
            .with_context(|| format!("column {name}"))
    };
    let opt = |a: &StringArray, i: usize| (!a.is_null(i)).then(|| a.value(i).to_owned());
    let oint = |a: &Int32Array, i: usize| (!a.is_null(i)).then(|| a.value(i));
    let ojson = |a: &StringArray, i: usize| -> Result<Option<serde_json::Value>> {
        opt(a, i)
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .map_err(Into::into)
    };
    let uuid = |s: &str| Uuid::parse_str(s).with_context(|| format!("uuid {s}"));

    let (id, user, agent, op, req, sess, prov, model) = (
        str_col("id")?,
        str_col("user_id")?,
        str_col("agent_id")?,
        str_col("operation_type")?,
        str_col("request_id")?,
        str_col("session_id")?,
        str_col("provider")?,
        str_col("model")?,
    );
    let ints: Vec<&Int32Array> = INT_COLUMNS
        .iter()
        .map(|n| int_col(n))
        .collect::<Result<_>>()?;
    let (ctd, ptd, fin, meta) = (
        str_col("completion_tokens_details")?,
        str_col("prompt_tokens_details")?,
        str_col("finish_reason")?,
        str_col("metadata")?,
    );
    let cost = b
        .column_by_name("cost_usd")
        .and_then(|c| c.as_any().downcast_ref::<Decimal128Array>())
        .context("column cost_usd")?;
    let streaming = b
        .column_by_name("streaming")
        .and_then(|c| c.as_any().downcast_ref::<BooleanArray>())
        .context("column streaming")?;
    let ts = b
        .column_by_name("created_at")
        .and_then(|c| c.as_any().downcast_ref::<TimestampMicrosecondArray>())
        .context("column created_at")?;

    (0..b.num_rows())
        .map(|i| {
            Ok(TokenUsageRow {
                id: uuid(id.value(i))?,
                user_id: uuid(user.value(i))?,
                agent_id: opt(agent, i).map(|s| uuid(&s)).transpose()?,
                operation_type: op.value(i).to_owned(),
                request_id: opt(req, i),
                session_id: opt(sess, i),
                provider: prov.value(i).to_owned(),
                model: model.value(i).to_owned(),
                input_tokens: ints[0].value(i),
                output_tokens: ints[1].value(i),
                total_tokens: ints[2].value(i),
                cache_creation_input_tokens: oint(ints[3], i),
                cache_read_input_tokens: oint(ints[4], i),
                cached_tokens: oint(ints[5], i),
                audio_tokens: oint(ints[6], i),
                reasoning_tokens: oint(ints[7], i),
                accepted_prediction_tokens: oint(ints[8], i),
                rejected_prediction_tokens: oint(ints[9], i),
                latency_ms: oint(ints[10], i),
                ttft_ms: oint(ints[11], i),
                completion_tokens_details: ojson(ctd, i)?,
                prompt_tokens_details: ojson(ptd, i)?,
                cost_usd: (!cost.is_null(i))
                    .then(|| Decimal::from_i128_with_scale(cost.value(i), COST_SCALE)),
                streaming: (!streaming.is_null(i)).then(|| streaming.value(i)),
                finish_reason: opt(fin, i),
                metadata: ojson(meta, i)?,
                created_at: DateTime::from_timestamp_micros(ts.value(i)).context("created_at")?,
            })
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use chrono::TimeZone;

    pub fn row(user: u128, secs: i64) -> TokenUsageRow {
        TokenUsageRow {
            id: Uuid::new_v4(),
            user_id: Uuid::from_u128(user),
            agent_id: user.is_multiple_of(2).then(|| Uuid::from_u128(99)),
            operation_type: "chat".into(),
            request_id: Some("req".into()),
            session_id: None,
            provider: "openai".into(),
            model: "gpt-4o-mini".into(),
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: 15,
            cache_creation_input_tokens: Some(0),
            cache_read_input_tokens: None,
            cached_tokens: Some(2),
            audio_tokens: None,
            reasoning_tokens: Some(1),
            accepted_prediction_tokens: None,
            rejected_prediction_tokens: None,
            completion_tokens_details: Some(serde_json::json!({"reasoning_tokens": 1})),
            prompt_tokens_details: None,
            cost_usd: Some(Decimal::new(123_456_789, 8)),
            latency_ms: Some(300),
            ttft_ms: None,
            streaming: Some(true),
            finish_reason: Some("stop".into()),
            metadata: Some(serde_json::json!({})),
            created_at: Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap(),
        }
    }

    #[test]
    fn round_trips_every_column() {
        let rows = vec![row(1, 0), row(2, 5)];
        let b = to_record_batch(&rows, Uuid::nil()).unwrap();
        assert_eq!(b.num_rows(), 2);
        assert_eq!(from_record_batch(&b).unwrap(), rows);
    }

    #[test]
    fn tags_batch_id() {
        let bid = Uuid::new_v4();
        let b = to_record_batch(&[row(1, 0)], bid).unwrap();
        let col = b.column_by_name("batch_id").unwrap();
        let col = col.as_any().downcast_ref::<StringArray>().unwrap();
        assert_eq!(col.value(0), bid.to_string());
    }

    #[test]
    fn sorts_by_user_then_time() {
        let mut rows = vec![row(2, 0), row(1, 9), row(1, 3)];
        sort_rows(&mut rows);
        let keys: Vec<_> = rows
            .iter()
            .map(|r| (r.user_id.as_u128(), r.created_at.timestamp()))
            .collect();
        assert_eq!(
            keys,
            [(1, 1_700_000_003), (1, 1_700_000_009), (2, 1_700_000_000)]
        );
    }
}
