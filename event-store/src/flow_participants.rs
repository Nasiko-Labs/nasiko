//! `flow_participants` ↔ Arrow. Pure: no IO, unit-tested below.
//!
//! Cold key for one row: `row_key` is `flow_id/agent_id`.
//! Participants move in the same tiering batch as their flow.
//! `user_id` is denormalized from the flow so user erasure can filter directly.

use std::sync::Arc;

use anyhow::{Context, Result};
use arrow_array::{Array, RecordBatch, StringArray, TimestampMicrosecondArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub const TABLE: &str = "flow_participants";
pub const INDEXES: [&str; 4] = ["flow_id", "agent_id", "user_id", "row_key"];
pub const COLUMNS: &str = "fp.flow_id, fp.agent_id, fp.created_at, f.user_id";

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct FlowParticipantRow {
    pub flow_id: String,
    pub agent_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub user_id: Option<Uuid>,
}

pub fn row_key(flow_id: &str, agent_id: Uuid) -> String {
    format!("{flow_id}/{agent_id}")
}

pub fn split_key(key: &str) -> Option<(&str, &str)> {
    key.split_once('/')
}

pub fn sort_rows(rows: &mut [FlowParticipantRow]) {
    rows.sort_by(|a, b| (&a.flow_id, a.agent_id).cmp(&(&b.flow_id, b.agent_id)));
}

pub fn schema() -> SchemaRef {
    let ts = || DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()));
    Arc::new(Schema::new(vec![
        Field::new("row_key", DataType::Utf8, false),
        Field::new("flow_id", DataType::Utf8, false),
        Field::new("agent_id", DataType::Utf8, false),
        Field::new("user_id", DataType::Utf8, true),
        Field::new("created_at", ts(), false),
        Field::new("batch_id", DataType::Utf8, false),
    ]))
}

pub fn to_record_batch(rows: &[FlowParticipantRow], batch_id: Uuid) -> Result<RecordBatch> {
    let bid = batch_id.to_string();
    let row_keys: Vec<String> = rows
        .iter()
        .map(|r| row_key(&r.flow_id, r.agent_id))
        .collect();
    let flow_ids: Vec<&str> = rows.iter().map(|r| r.flow_id.as_str()).collect();
    let agent_ids: Vec<String> = rows.iter().map(|r| r.agent_id.to_string()).collect();
    let user_ids: Vec<Option<String>> = rows
        .iter()
        .map(|r| r.user_id.map(|u| u.to_string()))
        .collect();
    let created_ats: Vec<i64> = rows
        .iter()
        .map(|r| r.created_at.timestamp_micros())
        .collect();
    let batch_ids: Vec<&str> = (0..rows.len()).map(|_| bid.as_str()).collect();

    let cols: Vec<Arc<dyn Array>> = vec![
        Arc::new(StringArray::from(row_keys)),
        Arc::new(StringArray::from(flow_ids)),
        Arc::new(StringArray::from(agent_ids)),
        Arc::new(StringArray::from(user_ids)),
        Arc::new(TimestampMicrosecondArray::from(created_ats).with_timezone("UTC")),
        Arc::new(StringArray::from(batch_ids)),
    ];
    RecordBatch::try_new(schema(), cols).context("build flow_participants RecordBatch")
}

pub fn from_record_batch(b: &RecordBatch) -> Result<Vec<FlowParticipantRow>> {
    let get_str = |name: &str| -> Result<&StringArray> {
        b.column_by_name(name)
            .with_context(|| format!("column {name} missing"))?
            .as_any()
            .downcast_ref::<StringArray>()
            .with_context(|| format!("column {name} not string"))
    };
    let flow_id = get_str("flow_id")?;
    let agent_id = get_str("agent_id")?;
    let user_id = get_str("user_id")?;
    let created_at = b
        .column_by_name("created_at")
        .context("column created_at missing")?
        .as_any()
        .downcast_ref::<TimestampMicrosecondArray>()
        .context("column created_at not ts")?;

    let mut out = Vec::with_capacity(b.num_rows());
    for i in 0..b.num_rows() {
        let aid = Uuid::parse_str(agent_id.value(i))
            .with_context(|| format!("invalid agent_id at row {i}"))?;
        let uid = if user_id.is_null(i) {
            None
        } else {
            Some(
                Uuid::parse_str(user_id.value(i))
                    .with_context(|| format!("invalid user_id at {i}"))?,
            )
        };
        let cat = DateTime::from_timestamp_micros(created_at.value(i))
            .context("invalid created_at timestamp")?;
        out.push(FlowParticipantRow {
            flow_id: flow_id.value(i).to_string(),
            agent_id: aid,
            user_id: uid,
            created_at: cat,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_arrow() {
        let f1 = FlowParticipantRow {
            flow_id: "flow-123".into(),
            agent_id: Uuid::new_v4(),
            created_at: Utc::now(),
            user_id: Some(Uuid::new_v4()),
        };
        let f2 = FlowParticipantRow {
            flow_id: "flow-456".into(),
            agent_id: Uuid::new_v4(),
            created_at: Utc::now(),
            user_id: None,
        };
        let rows = vec![f1.clone(), f2.clone()];
        let batch = to_record_batch(&rows, Uuid::new_v4()).unwrap();
        let decoded = from_record_batch(&batch).unwrap();
        assert_eq!(rows.len(), decoded.len());
        assert_eq!(rows[0].flow_id, decoded[0].flow_id);
        assert_eq!(rows[0].agent_id, decoded[0].agent_id);
        assert_eq!(rows[0].user_id, decoded[0].user_id);
        assert_eq!(rows[1].flow_id, decoded[1].flow_id);
        assert_eq!(rows[1].user_id, None);
    }
}
