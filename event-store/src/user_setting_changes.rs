//! `user_setting_changes` ↔ Arrow. Pure: no IO, unit-tested below.
//!
//! No rollup: the only reader is one user's changes over a window (≤ 90 days by
//! range preset), so the table shares the 90-day `trace_usage` hot window and
//! older rows are read back per user from Lance.

use std::sync::Arc;

use anyhow::{Context, Result};
use arrow_array::{Array, RecordBatch, StringArray, TimestampMicrosecondArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub const TABLE: &str = "user_setting_changes";
pub const INDEXES: [&str; 2] = ["id", "user_id"];
pub const COLUMNS: &str = "id, user_id, field, from_value, to_value, changed_at";

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct SettingChangeRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub field: String,
    pub from_value: String,
    pub to_value: String,
    pub changed_at: DateTime<Utc>,
}

pub fn sort_rows(rows: &mut [SettingChangeRow]) {
    rows.sort_by_key(|r| (r.user_id, r.changed_at, r.id));
}

pub fn schema() -> SchemaRef {
    let mut f: Vec<Field> = ["id", "user_id", "field", "from_value", "to_value"]
        .into_iter()
        .map(|n| Field::new(n, DataType::Utf8, false))
        .collect();
    f.push(Field::new(
        "changed_at",
        DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
        false,
    ));
    f.push(Field::new("batch_id", DataType::Utf8, false));
    Arc::new(Schema::new(f))
}

pub fn to_record_batch(rows: &[SettingChangeRow], batch_id: Uuid) -> Result<RecordBatch> {
    let s = |f: &dyn Fn(&SettingChangeRow) -> String| -> Arc<dyn Array> {
        Arc::new(StringArray::from(rows.iter().map(f).collect::<Vec<_>>()))
    };
    let bid = batch_id.to_string();
    let cols: Vec<Arc<dyn Array>> = vec![
        s(&|r| r.id.to_string()),
        s(&|r| r.user_id.to_string()),
        s(&|r| r.field.clone()),
        s(&|r| r.from_value.clone()),
        s(&|r| r.to_value.clone()),
        Arc::new(
            TimestampMicrosecondArray::from(
                rows.iter()
                    .map(|r| r.changed_at.timestamp_micros())
                    .collect::<Vec<_>>(),
            )
            .with_timezone("UTC"),
        ),
        s(&|_| bid.clone()),
    ];
    Ok(RecordBatch::try_new(schema(), cols)?)
}

pub fn from_record_batch(b: &RecordBatch) -> Result<Vec<SettingChangeRow>> {
    let col = |n: &str| -> Result<&StringArray> {
        b.column_by_name(n)
            .with_context(|| format!("missing {n}"))?
            .as_any()
            .downcast_ref::<StringArray>()
            .with_context(|| format!("{n} not utf8"))
    };
    let (id, user, field, from, to) = (
        col("id")?,
        col("user_id")?,
        col("field")?,
        col("from_value")?,
        col("to_value")?,
    );
    let at = b
        .column_by_name("changed_at")
        .context("missing changed_at")?
        .as_any()
        .downcast_ref::<TimestampMicrosecondArray>()
        .context("changed_at not timestamp")?;
    (0..b.num_rows())
        .map(|i| {
            Ok(SettingChangeRow {
                id: Uuid::parse_str(id.value(i))?,
                user_id: Uuid::parse_str(user.value(i))?,
                field: field.value(i).to_string(),
                from_value: from.value(i).to_string(),
                to_value: to.value(i).to_string(),
                changed_at: DateTime::from_timestamp_micros(at.value(i))
                    .context("changed_at out of range")?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_arrow() {
        let mut rows = vec![
            SettingChangeRow {
                id: Uuid::new_v4(),
                user_id: Uuid::from_u128(2),
                field: "strategy".into(),
                from_value: "a".into(),
                to_value: "b".into(),
                changed_at: DateTime::from_timestamp_micros(2_000_000).unwrap(),
            },
            SettingChangeRow {
                id: Uuid::new_v4(),
                user_id: Uuid::from_u128(1),
                field: "level".into(),
                from_value: "1".into(),
                to_value: "2".into(),
                changed_at: DateTime::from_timestamp_micros(1_000_000).unwrap(),
            },
        ];
        sort_rows(&mut rows);
        assert_eq!(rows[0].user_id, Uuid::from_u128(1));
        let b = to_record_batch(&rows, Uuid::new_v4()).unwrap();
        assert_eq!(from_record_batch(&b).unwrap(), rows);
    }
}
