//! `flows` ↔ Arrow. Pure: no IO, unit-tested below.
//!
//! Hot rows live in Postgres until status is final (`completed` or `failed`) and
//! `completed_at` is older than the hot window (`trace_min_hot_days`).
//! Participants tier in the same batch.

use std::sync::Arc;

use anyhow::{Context, Result};
use arrow_array::{
    Array, Float64Array, Int32Array, Int64Array, RecordBatch, StringArray,
    TimestampMicrosecondArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub const TABLE: &str = "flows";
pub const INDEXES: [&str; 4] = ["flow_id", "session_id", "user_id", "root_agent_id"];
pub const COLUMNS: &str = "id, flow_id, user_id, session_id, parent_flow_id, root_agent_id, \
    root_agent_name, title, status, max_depth_reached, total_invocations, total_tokens_used, \
    total_cost_usd::float8 AS total_cost_usd, duration_ms, error_message, metadata::text AS metadata, \
    created_at, completed_at";

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct FlowRow {
    pub id: Uuid,
    pub flow_id: String,
    pub user_id: Uuid,
    pub session_id: String,
    pub parent_flow_id: Option<String>,
    pub root_agent_id: Option<Uuid>,
    pub root_agent_name: Option<String>,
    pub title: Option<String>,
    pub status: String,
    pub max_depth_reached: i32,
    pub total_invocations: i32,
    pub total_tokens_used: i64,
    pub total_cost_usd: Option<f64>,
    pub duration_ms: Option<i64>,
    pub error_message: Option<String>,
    pub metadata: String,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

pub fn sort_rows(rows: &mut [FlowRow]) {
    rows.sort_by(|a, b| {
        (a.user_id, a.created_at, &a.flow_id).cmp(&(b.user_id, b.created_at, &b.flow_id))
    });
}

fn ts() -> DataType {
    DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()))
}

pub fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("flow_id", DataType::Utf8, false),
        Field::new("user_id", DataType::Utf8, false),
        Field::new("session_id", DataType::Utf8, false),
        Field::new("parent_flow_id", DataType::Utf8, true),
        Field::new("root_agent_id", DataType::Utf8, true),
        Field::new("root_agent_name", DataType::Utf8, true),
        Field::new("title", DataType::Utf8, true),
        Field::new("status", DataType::Utf8, false),
        Field::new("max_depth_reached", DataType::Int32, false),
        Field::new("total_invocations", DataType::Int32, false),
        Field::new("total_tokens_used", DataType::Int64, false),
        Field::new("total_cost_usd", DataType::Float64, true),
        Field::new("duration_ms", DataType::Int64, true),
        Field::new("error_message", DataType::Utf8, true),
        Field::new("metadata", DataType::Utf8, false),
        Field::new("created_at", ts(), false),
        Field::new("completed_at", ts(), true),
        Field::new("batch_id", DataType::Utf8, false),
    ]))
}

pub fn to_record_batch(rows: &[FlowRow], batch_id: Uuid) -> Result<RecordBatch> {
    let bid = batch_id.to_string();
    let s = |f: &dyn Fn(&FlowRow) -> Option<String>| -> Arc<dyn Array> {
        Arc::new(StringArray::from(rows.iter().map(f).collect::<Vec<_>>()))
    };
    let cols: Vec<Arc<dyn Array>> = vec![
        s(&|r| Some(r.id.to_string())),
        s(&|r| Some(r.flow_id.clone())),
        s(&|r| Some(r.user_id.to_string())),
        s(&|r| Some(r.session_id.clone())),
        s(&|r| r.parent_flow_id.clone()),
        s(&|r| r.root_agent_id.map(|v| v.to_string())),
        s(&|r| r.root_agent_name.clone()),
        s(&|r| r.title.clone()),
        s(&|r| Some(r.status.clone())),
        Arc::new(Int32Array::from(
            rows.iter().map(|r| r.max_depth_reached).collect::<Vec<_>>(),
        )),
        Arc::new(Int32Array::from(
            rows.iter().map(|r| r.total_invocations).collect::<Vec<_>>(),
        )),
        Arc::new(Int64Array::from(
            rows.iter().map(|r| r.total_tokens_used).collect::<Vec<_>>(),
        )),
        Arc::new(Float64Array::from(
            rows.iter().map(|r| r.total_cost_usd).collect::<Vec<_>>(),
        )),
        Arc::new(Int64Array::from(
            rows.iter().map(|r| r.duration_ms).collect::<Vec<_>>(),
        )),
        s(&|r| r.error_message.clone()),
        s(&|r| Some(r.metadata.clone())),
        Arc::new(
            TimestampMicrosecondArray::from(
                rows.iter()
                    .map(|r| r.created_at.timestamp_micros())
                    .collect::<Vec<_>>(),
            )
            .with_timezone("UTC"),
        ),
        Arc::new(
            TimestampMicrosecondArray::from(
                rows.iter()
                    .map(|r| r.completed_at.map(|t| t.timestamp_micros()))
                    .collect::<Vec<_>>(),
            )
            .with_timezone("UTC"),
        ),
        Arc::new(StringArray::from(vec![bid.as_str(); rows.len()])),
    ];
    RecordBatch::try_new(schema(), cols).context("build flows RecordBatch")
}

pub fn from_record_batch(b: &RecordBatch) -> Result<Vec<FlowRow>> {
    let get_str = |name: &str| -> Result<&StringArray> {
        b.column_by_name(name)
            .with_context(|| format!("column {name} missing"))?
            .as_any()
            .downcast_ref::<StringArray>()
            .with_context(|| format!("column {name} not string"))
    };
    let id_col = get_str("id")?;
    let flow_id_col = get_str("flow_id")?;
    let user_id_col = get_str("user_id")?;
    let session_id_col = get_str("session_id")?;
    let parent_flow_id_col = get_str("parent_flow_id")?;
    let root_agent_id_col = get_str("root_agent_id")?;
    let root_agent_name_col = get_str("root_agent_name")?;
    let title_col = get_str("title")?;
    let status_col = get_str("status")?;
    let error_message_col = get_str("error_message")?;
    let metadata_col = get_str("metadata")?;

    let max_depth = b
        .column_by_name("max_depth_reached")
        .context("column max_depth_reached missing")?
        .as_any()
        .downcast_ref::<Int32Array>()
        .context("column max_depth_reached not int32")?;
    let total_inv = b
        .column_by_name("total_invocations")
        .context("column total_invocations missing")?
        .as_any()
        .downcast_ref::<Int32Array>()
        .context("column total_invocations not int32")?;
    let total_tokens = b
        .column_by_name("total_tokens_used")
        .context("column total_tokens_used missing")?
        .as_any()
        .downcast_ref::<Int64Array>()
        .context("column total_tokens_used not int64")?;
    let total_cost = b
        .column_by_name("total_cost_usd")
        .context("column total_cost_usd missing")?
        .as_any()
        .downcast_ref::<Float64Array>()
        .context("column total_cost_usd not float64")?;
    let duration = b
        .column_by_name("duration_ms")
        .context("column duration_ms missing")?
        .as_any()
        .downcast_ref::<Int64Array>()
        .context("column duration_ms not int64")?;
    let created_at = b
        .column_by_name("created_at")
        .context("column created_at missing")?
        .as_any()
        .downcast_ref::<TimestampMicrosecondArray>()
        .context("column created_at not ts")?;
    let completed_at = b
        .column_by_name("completed_at")
        .context("column completed_at missing")?
        .as_any()
        .downcast_ref::<TimestampMicrosecondArray>()
        .context("column completed_at not ts")?;

    let mut out = Vec::with_capacity(b.num_rows());
    for i in 0..b.num_rows() {
        let id = Uuid::parse_str(id_col.value(i)).with_context(|| format!("invalid id at {i}"))?;
        let user_id = Uuid::parse_str(user_id_col.value(i))
            .with_context(|| format!("invalid user_id at {i}"))?;
        let root_agent_id = if root_agent_id_col.is_null(i) {
            None
        } else {
            Some(
                Uuid::parse_str(root_agent_id_col.value(i))
                    .with_context(|| format!("invalid root_agent_id at {i}"))?,
            )
        };
        let cat = DateTime::from_timestamp_micros(created_at.value(i))
            .context("invalid created_at timestamp")?;
        let comp = if completed_at.is_null(i) {
            None
        } else {
            Some(
                DateTime::from_timestamp_micros(completed_at.value(i))
                    .context("invalid completed_at timestamp")?,
            )
        };

        out.push(FlowRow {
            id,
            flow_id: flow_id_col.value(i).to_string(),
            user_id,
            session_id: session_id_col.value(i).to_string(),
            parent_flow_id: if parent_flow_id_col.is_null(i) {
                None
            } else {
                Some(parent_flow_id_col.value(i).to_string())
            },
            root_agent_id,
            root_agent_name: if root_agent_name_col.is_null(i) {
                None
            } else {
                Some(root_agent_name_col.value(i).to_string())
            },
            title: if title_col.is_null(i) {
                None
            } else {
                Some(title_col.value(i).to_string())
            },
            status: status_col.value(i).to_string(),
            max_depth_reached: max_depth.value(i),
            total_invocations: total_inv.value(i),
            total_tokens_used: total_tokens.value(i),
            total_cost_usd: if total_cost.is_null(i) {
                None
            } else {
                Some(total_cost.value(i))
            },
            duration_ms: if duration.is_null(i) {
                None
            } else {
                Some(duration.value(i))
            },
            error_message: if error_message_col.is_null(i) {
                None
            } else {
                Some(error_message_col.value(i).to_string())
            },
            metadata: metadata_col.value(i).to_string(),
            created_at: cat,
            completed_at: comp,
        });
    }
    Ok(out)
}

/// Merge hot and cold flows, hot taking precedence if duplicated by flow_id.
pub fn merge(mut hot: Vec<FlowRow>, cold: Vec<FlowRow>) -> Vec<FlowRow> {
    let hot_ids: std::collections::HashSet<String> =
        hot.iter().map(|r| r.flow_id.clone()).collect();
    for c in cold {
        if !hot_ids.contains(&c.flow_id) {
            hot.push(c);
        }
    }
    hot.sort_by_key(|a| std::cmp::Reverse(a.created_at));
    hot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_arrow() {
        let f1 = FlowRow {
            id: Uuid::new_v4(),
            flow_id: "flow-1".into(),
            user_id: Uuid::new_v4(),
            session_id: "session-1".into(),
            parent_flow_id: Some("parent-1".into()),
            root_agent_id: Some(Uuid::new_v4()),
            root_agent_name: Some("test-agent".into()),
            title: Some("Test Flow".into()),
            status: "completed".into(),
            max_depth_reached: 2,
            total_invocations: 5,
            total_tokens_used: 1000,
            total_cost_usd: Some(0.02),
            duration_ms: Some(150),
            error_message: None,
            metadata: r#"{"foo":"bar"}"#.into(),
            created_at: Utc::now(),
            completed_at: Some(Utc::now()),
        };
        let rows = vec![f1.clone()];
        let batch = to_record_batch(&rows, Uuid::new_v4()).unwrap();
        let decoded = from_record_batch(&batch).unwrap();
        assert_eq!(rows.len(), decoded.len());
        assert_eq!(rows[0].flow_id, decoded[0].flow_id);
        assert_eq!(rows[0].session_id, decoded[0].session_id);
        assert_eq!(rows[0].parent_flow_id, decoded[0].parent_flow_id);
        assert_eq!(rows[0].total_tokens_used, decoded[0].total_tokens_used);
    }
}
