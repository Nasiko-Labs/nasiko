//! `token_savings` ↔ Arrow. Pure: no IO, unit-tested below.
//!
//! Rolled up into `token_savings_daily` (cost frozen at tiering time) before the
//! delete; shares the 90-day hot window with `trace_usage`.

use std::sync::Arc;

use anyhow::{Context, Result};
use arrow_array::{
    Array, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray,
    TimestampMicrosecondArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub const TABLE: &str = "token_savings";
pub const INDEXES: [&str; 3] = ["id", "user_id", "flow_id"];
pub const COLUMNS: &str = "id, user_id, agent_id, flow_id, session_id, provider, model, layer, \
     program, bytes_before, bytes_after, saved_input_tokens, saved_output_tokens, saved_cost_usd, \
     method, token_estimated, context_tier, created_at";

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct TokenSavingsRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub agent_id: Option<Uuid>,
    pub flow_id: Option<String>,
    pub session_id: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub layer: String,
    pub program: String,
    pub bytes_before: Option<i64>,
    pub bytes_after: Option<i64>,
    pub saved_input_tokens: i64,
    pub saved_output_tokens: i64,
    pub saved_cost_usd: f64,
    pub method: String,
    pub token_estimated: bool,
    pub context_tier: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub fn sort_rows(rows: &mut [TokenSavingsRow]) {
    rows.sort_by_key(|r| (r.user_id, r.created_at, r.id));
}

const STR: [(&str, bool); 11] = [
    ("id", false),
    ("user_id", false),
    ("agent_id", true),
    ("flow_id", true),
    ("session_id", true),
    ("provider", true),
    ("model", true),
    ("layer", false),
    ("program", false),
    ("method", false),
    ("context_tier", true),
];

pub fn schema() -> SchemaRef {
    let mut f: Vec<Field> = STR
        .iter()
        .map(|(n, null)| Field::new(*n, DataType::Utf8, *null))
        .collect();
    f.push(Field::new("bytes_before", DataType::Int64, true));
    f.push(Field::new("bytes_after", DataType::Int64, true));
    f.push(Field::new("saved_input_tokens", DataType::Int64, false));
    f.push(Field::new("saved_output_tokens", DataType::Int64, false));
    f.push(Field::new("saved_cost_usd", DataType::Float64, false));
    f.push(Field::new("token_estimated", DataType::Boolean, false));
    f.push(Field::new(
        "created_at",
        DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
        false,
    ));
    f.push(Field::new("batch_id", DataType::Utf8, false));
    Arc::new(Schema::new(f))
}

fn strs(r: &TokenSavingsRow) -> [Option<String>; 11] {
    [
        Some(r.id.to_string()),
        Some(r.user_id.to_string()),
        r.agent_id.map(|a| a.to_string()),
        r.flow_id.clone(),
        r.session_id.clone(),
        r.provider.clone(),
        r.model.clone(),
        Some(r.layer.clone()),
        Some(r.program.clone()),
        Some(r.method.clone()),
        r.context_tier.clone(),
    ]
}

pub fn to_record_batch(rows: &[TokenSavingsRow], batch_id: Uuid) -> Result<RecordBatch> {
    let s: Vec<[Option<String>; 11]> = rows.iter().map(strs).collect();
    let mut cols: Vec<Arc<dyn Array>> = (0..STR.len())
        .map(|i| {
            Arc::new(StringArray::from(
                s.iter().map(|r| r[i].clone()).collect::<Vec<_>>(),
            )) as Arc<dyn Array>
        })
        .collect();
    let i = |f: &dyn Fn(&TokenSavingsRow) -> Option<i64>| -> Arc<dyn Array> {
        Arc::new(Int64Array::from(rows.iter().map(f).collect::<Vec<_>>()))
    };
    cols.push(i(&|r| r.bytes_before));
    cols.push(i(&|r| r.bytes_after));
    cols.push(i(&|r| Some(r.saved_input_tokens)));
    cols.push(i(&|r| Some(r.saved_output_tokens)));
    cols.push(Arc::new(Float64Array::from(
        rows.iter().map(|r| r.saved_cost_usd).collect::<Vec<_>>(),
    )));
    cols.push(Arc::new(BooleanArray::from(
        rows.iter().map(|r| r.token_estimated).collect::<Vec<_>>(),
    )));
    cols.push(Arc::new(
        TimestampMicrosecondArray::from(
            rows.iter()
                .map(|r| r.created_at.timestamp_micros())
                .collect::<Vec<_>>(),
        )
        .with_timezone("UTC"),
    ));
    let bid = batch_id.to_string();
    cols.push(Arc::new(StringArray::from(vec![bid; rows.len()])));
    Ok(RecordBatch::try_new(schema(), cols)?)
}

pub fn from_record_batch(b: &RecordBatch) -> Result<Vec<TokenSavingsRow>> {
    fn get<'a, T: 'static>(b: &'a RecordBatch, n: &str) -> Result<&'a T> {
        b.column_by_name(n)
            .with_context(|| format!("missing {n}"))?
            .as_any()
            .downcast_ref::<T>()
            .with_context(|| format!("{n} wrong type"))
    }
    let s = |n: &str| get::<StringArray>(b, n);
    let opt = |a: &StringArray, i: usize| (!a.is_null(i)).then(|| a.value(i).to_string());
    let (id, user, agent, flow, sess, prov, model, layer, prog, method, tier) = (
        s("id")?,
        s("user_id")?,
        s("agent_id")?,
        s("flow_id")?,
        s("session_id")?,
        s("provider")?,
        s("model")?,
        s("layer")?,
        s("program")?,
        s("method")?,
        s("context_tier")?,
    );
    let i = |n: &str| get::<Int64Array>(b, n);
    let (bb, ba, si, so) = (
        i("bytes_before")?,
        i("bytes_after")?,
        i("saved_input_tokens")?,
        i("saved_output_tokens")?,
    );
    let cost = get::<Float64Array>(b, "saved_cost_usd")?;
    let est = get::<BooleanArray>(b, "token_estimated")?;
    let at = get::<TimestampMicrosecondArray>(b, "created_at")?;
    let oi = |a: &Int64Array, k: usize| (!a.is_null(k)).then(|| a.value(k));
    (0..b.num_rows())
        .map(|k| {
            Ok(TokenSavingsRow {
                id: Uuid::parse_str(id.value(k))?,
                user_id: Uuid::parse_str(user.value(k))?,
                agent_id: opt(agent, k).map(|a| Uuid::parse_str(&a)).transpose()?,
                flow_id: opt(flow, k),
                session_id: opt(sess, k),
                provider: opt(prov, k),
                model: opt(model, k),
                layer: layer.value(k).to_string(),
                program: prog.value(k).to_string(),
                bytes_before: oi(bb, k),
                bytes_after: oi(ba, k),
                saved_input_tokens: si.value(k),
                saved_output_tokens: so.value(k),
                saved_cost_usd: cost.value(k),
                method: method.value(k).to_string(),
                token_estimated: est.value(k),
                context_tier: opt(tier, k),
                created_at: DateTime::from_timestamp_micros(at.value(k))
                    .context("created_at out of range")?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(user: u128, micros: i64, model: Option<&str>) -> TokenSavingsRow {
        TokenSavingsRow {
            id: Uuid::new_v4(),
            user_id: Uuid::from_u128(user),
            agent_id: None,
            flow_id: Some("f1".into()),
            session_id: None,
            provider: model.map(|_| "openai".into()),
            model: model.map(Into::into),
            layer: "compress_payload".into(),
            program: "caveman".into(),
            bytes_before: Some(100),
            bytes_after: None,
            saved_input_tokens: 10,
            saved_output_tokens: 0,
            saved_cost_usd: 0.5,
            method: "measured_bytes".into(),
            token_estimated: true,
            context_tier: None,
            created_at: DateTime::from_timestamp_micros(micros).unwrap(),
        }
    }

    #[test]
    fn round_trips_through_arrow() {
        let mut rows = vec![row(2, 2_000_000, Some("gpt-4o")), row(1, 1_000_000, None)];
        sort_rows(&mut rows);
        assert_eq!(rows[0].user_id, Uuid::from_u128(1));
        let b = to_record_batch(&rows, Uuid::new_v4()).unwrap();
        assert_eq!(from_record_batch(&b).unwrap(), rows);
    }
}
