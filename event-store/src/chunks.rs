//! Cold `memory_l1b_chunks` vectors (TRD §3.4 step 2) ↔ Arrow, and the hot/cold
//! merge for vector search. Pure: no IO, unit-tested below.
//!
//! Distances are cosine distance (`embedding <=> q` in pgvector, `MetricType::Cosine`
//! in Lance), so both stores rank on the same scale; lower is closer.

use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result, ensure};
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, Int32Array, RecordBatch, StringArray,
    TimestampMicrosecondArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub const TABLE: &str = "chunks";
pub const INDEXES: [&str; 2] = ["row_key", "user_id"];
pub const VECTOR_COLUMN: &str = "embedding";
pub const DIM: i32 = 1536;

#[derive(Debug, Clone, PartialEq)]
pub struct ChunkRow {
    pub source_kind: String,
    pub source_id: String,
    pub chunk_idx: i32,
    pub user_id: Uuid,
    pub agent_domain: Option<String>,
    pub session_id: String,
    pub ts: DateTime<Utc>,
    pub text: String,
    pub embedding: Vec<f32>,
    pub embedding_model: String,
}

/// Postgres primary key minus `ts`: `source_kind/source_id/chunk_idx`.
pub fn row_key(r: &ChunkRow) -> String {
    format!("{}/{}/{}", r.source_kind, r.source_id, r.chunk_idx)
}

fn ts() -> DataType {
    DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()))
}

fn vec_type() -> DataType {
    DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), DIM)
}

pub fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("row_key", DataType::Utf8, false),
        Field::new("source_kind", DataType::Utf8, false),
        Field::new("source_id", DataType::Utf8, false),
        Field::new("chunk_idx", DataType::Int32, false),
        Field::new("user_id", DataType::Utf8, false),
        Field::new("agent_domain", DataType::Utf8, true),
        Field::new("session_id", DataType::Utf8, false),
        Field::new("ts", ts(), false),
        Field::new("text", DataType::Utf8, false),
        Field::new(VECTOR_COLUMN, vec_type(), false),
        Field::new("embedding_model", DataType::Utf8, false),
        Field::new("batch_id", DataType::Utf8, false),
    ]))
}

pub fn to_record_batch(rows: &[ChunkRow], batch_id: Uuid) -> Result<RecordBatch> {
    for r in rows {
        ensure!(
            r.embedding.len() == DIM as usize,
            "chunk {} has dim {}",
            row_key(r),
            r.embedding.len()
        );
    }
    let s = |f: &dyn Fn(&ChunkRow) -> Option<String>| -> Arc<dyn Array> {
        Arc::new(StringArray::from(rows.iter().map(f).collect::<Vec<_>>()))
    };
    let flat: Float32Array = rows
        .iter()
        .flat_map(|r| r.embedding.iter().copied())
        .collect();
    let item = Arc::new(Field::new("item", DataType::Float32, true));
    let vectors = FixedSizeListArray::try_new(item, DIM, Arc::new(flat), None)?;
    let bid = batch_id.to_string();
    let cols: Vec<Arc<dyn Array>> = vec![
        s(&|r| Some(row_key(r))),
        s(&|r| Some(r.source_kind.clone())),
        s(&|r| Some(r.source_id.clone())),
        Arc::new(Int32Array::from(
            rows.iter().map(|r| r.chunk_idx).collect::<Vec<_>>(),
        )),
        s(&|r| Some(r.user_id.to_string())),
        s(&|r| r.agent_domain.clone()),
        s(&|r| Some(r.session_id.clone())),
        Arc::new(
            TimestampMicrosecondArray::from(
                rows.iter()
                    .map(|r| r.ts.timestamp_micros())
                    .collect::<Vec<_>>(),
            )
            .with_timezone("UTC"),
        ),
        s(&|r| Some(r.text.clone())),
        Arc::new(vectors),
        s(&|r| Some(r.embedding_model.clone())),
        s(&|_| Some(bid.clone())),
    ];
    Ok(RecordBatch::try_new(schema(), cols)?)
}

/// One search hit from either store.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkHit {
    pub row_key: String,
    pub session_id: String,
    pub ts: DateTime<Utc>,
    pub text: String,
    /// Agent attribution, when the source batch carries it.
    pub agent_domain: Option<String>,
    /// Cosine distance; lower is closer.
    pub distance: f32,
}

/// Reads hits from a Lance `nearest` result (carries `_distance`).
pub fn hits_from_record_batch(b: &RecordBatch) -> Result<Vec<ChunkHit>> {
    let col = |n: &str| -> Result<&StringArray> {
        b.column_by_name(n)
            .with_context(|| format!("missing {n}"))?
            .as_any()
            .downcast_ref::<StringArray>()
            .with_context(|| format!("{n} not utf8"))
    };
    let (key, sess, text) = (col("row_key")?, col("session_id")?, col("text")?);
    let at = b
        .column_by_name("ts")
        .context("missing ts")?
        .as_any()
        .downcast_ref::<TimestampMicrosecondArray>()
        .context("ts not timestamp")?;
    let dist = b
        .column_by_name("_distance")
        .context("missing _distance")?
        .as_any()
        .downcast_ref::<Float32Array>()
        .context("_distance not f32")?;
    let domain: Option<&StringArray> = b
        .column_by_name("agent_domain")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>());
    (0..b.num_rows())
        .map(|i| {
            Ok(ChunkHit {
                row_key: key.value(i).to_string(),
                session_id: sess.value(i).to_string(),
                ts: DateTime::from_timestamp_micros(at.value(i)).context("ts out of range")?,
                text: text.value(i).to_string(),
                agent_domain: domain
                    .filter(|d| d.is_valid(i))
                    .map(|d| d.value(i).to_string()),
                distance: dist.value(i),
            })
        })
        .collect()
}

/// Merges hot (pgvector) and cold (Lance ANN) top-k by distance. A chunk in both
/// stores (tiered, not yet deleted) keeps its hot copy. Ties break on `row_key`. Pure.
pub fn merge_by_distance(hot: Vec<ChunkHit>, cold: Vec<ChunkHit>, k: usize) -> Vec<ChunkHit> {
    let mut by_key: HashMap<String, ChunkHit> = HashMap::new();
    for h in cold {
        by_key.insert(h.row_key.clone(), h);
    }
    for h in hot {
        by_key.insert(h.row_key.clone(), h);
    }
    let mut out: Vec<ChunkHit> = by_key.into_values().collect();
    out.sort_by(|a, b| {
        a.distance
            .total_cmp(&b.distance)
            .then_with(|| a.row_key.cmp(&b.row_key))
    });
    out.truncate(k);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(key: &str, d: f32, text: &str) -> ChunkHit {
        ChunkHit {
            row_key: key.into(),
            session_id: "s".into(),
            ts: DateTime::from_timestamp_micros(0).unwrap(),
            text: text.into(),
            agent_domain: None,
            distance: d,
        }
    }

    #[test]
    fn merge_interleaves_by_distance_and_truncates() {
        let hot = vec![hit("h1", 0.1, ""), hit("h2", 0.5, "")];
        let cold = vec![hit("c1", 0.2, ""), hit("c2", 0.9, "")];
        let keys: Vec<String> = merge_by_distance(hot, cold, 3)
            .into_iter()
            .map(|h| h.row_key)
            .collect();
        assert_eq!(keys, ["h1", "c1", "h2"]);
    }

    #[test]
    fn merge_prefers_hot_copy() {
        let out = merge_by_distance(vec![hit("x", 0.3, "hot")], vec![hit("x", 0.3, "cold")], 10);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "hot");
    }

    #[test]
    fn batch_has_fixed_size_vectors() {
        let r = ChunkRow {
            source_kind: "chat_message".into(),
            source_id: "m1".into(),
            chunk_idx: 0,
            user_id: Uuid::from_u128(1),
            agent_domain: None,
            session_id: "s1".into(),
            ts: DateTime::from_timestamp_micros(1).unwrap(),
            text: "hello".into(),
            embedding: vec![0.5; DIM as usize],
            embedding_model: "text-embedding-3-small".into(),
        };
        let b = to_record_batch(std::slice::from_ref(&r), Uuid::nil()).unwrap();
        assert_eq!(b.num_rows(), 1);
        assert_eq!(
            b.column_by_name("row_key")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap()
                .value(0),
            "chat_message/m1/0"
        );
        let bad = ChunkRow {
            embedding: vec![0.0; 3],
            ..r
        };
        assert!(to_record_batch(&[bad], Uuid::nil()).is_err());
    }
}
