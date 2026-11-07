//! Lance dataset operations. Postgres-free so they are testable on a temp dir.
//! Runs on the caller's tokio runtime (the server's single shared one).

use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use arrow_array::{RecordBatch, RecordBatchIterator, StringArray};
use futures::TryStreamExt;
use lance::dataset::builder::DatasetBuilder;
use lance::{
    Dataset,
    dataset::{
        NewColumnTransform, WriteMode, WriteParams,
        optimize::{CompactionOptions, compact_files},
    },
    index::DatasetIndexExt,
};
use lance_index::{IndexType, scalar::ScalarIndexParams};
use lance_io::object_store::{ObjectStoreParams, StorageOptionsAccessor};

/// `<root>/<table>.lance`.
pub fn dataset_uri(root: &str, table: &str) -> String {
    format!("{}/{table}.lance", root.trim_end_matches('/'))
}

/// SQL string literal for a Lance filter. Values are UUIDs/ids we produce, but escape anyway.
pub fn lit(v: &str) -> String {
    format!("'{}'", v.replace('\'', "''"))
}

/// Dataset location: URI plus object-store options (S3 credentials, endpoint).
/// Local paths need no options, so `&str` converts directly.
#[derive(Debug, Clone, Default)]
pub struct Loc {
    pub uri: String,
    pub opts: HashMap<String, String>,
}

impl From<&str> for Loc {
    fn from(uri: &str) -> Self {
        Self {
            uri: uri.into(),
            opts: HashMap::new(),
        }
    }
}
impl From<&String> for Loc {
    fn from(uri: &String) -> Self {
        uri.as_str().into()
    }
}
impl From<&Loc> for Loc {
    fn from(l: &Loc) -> Self {
        l.clone()
    }
}

pub async fn open(loc: impl Into<Loc>) -> Result<Option<Dataset>> {
    let loc = loc.into();
    match DatasetBuilder::from_uri(&loc.uri)
        .with_storage_options(loc.opts)
        .load()
        .await
    {
        Ok(ds) => Ok(Some(ds)),
        Err(lance::Error::DatasetNotFound { .. }) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Appends one batch (creating the dataset and its BTREE indexes on first write).
/// Returns the new Lance version.
pub async fn append(
    loc: impl Into<Loc>,
    batch: RecordBatch,
    index_columns: &[&str],
) -> Result<u64> {
    let loc = loc.into();
    let mut existing = open(&loc).await?;
    if let Some(ds) = existing.as_mut() {
        add_missing_columns(ds, &batch.schema()).await?;
    }
    let mode = if existing.is_some() {
        WriteMode::Append
    } else {
        WriteMode::Create
    };
    let schema = batch.schema();
    let reader = RecordBatchIterator::new([Ok(batch)], schema);
    let params = WriteParams {
        mode,
        store_params: (!loc.opts.is_empty()).then(|| ObjectStoreParams {
            storage_options_accessor: Some(Arc::new(StorageOptionsAccessor::with_static_options(
                loc.opts.clone(),
            ))),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut ds = Dataset::write(reader, &loc.uri, Some(params)).await?;
    if existing.is_none() {
        for col in index_columns {
            ds.create_index(
                &[*col],
                IndexType::BTree,
                None,
                &ScalarIndexParams::default(),
                true,
            )
            .await?;
        }
    } else {
        // New fragments are scanned, not indexed, until this runs (TRD §8).
        ds.optimize_indices(&Default::default()).await?;
    }
    Ok(ds.version().version)
}

/// Columns in `incoming` that the dataset lacks. Pure, unit-tested.
pub fn missing_fields(
    existing: &arrow_schema::Schema,
    incoming: &arrow_schema::Schema,
) -> Vec<arrow_schema::Field> {
    incoming
        .fields()
        .iter()
        .filter(|f| existing.field_with_name(f.name()).is_err())
        .map(|f| f.as_ref().clone().with_nullable(true))
        .collect()
}

/// TRD §10: a new Postgres column becomes a metadata-only all-null Lance column;
/// old cold rows read `NULL`.
async fn add_missing_columns(ds: &mut Dataset, incoming: &arrow_schema::SchemaRef) -> Result<()> {
    let current = arrow_schema::Schema::from(ds.schema());
    let new = missing_fields(&current, incoming);
    if new.is_empty() {
        return Ok(());
    }
    tracing::info!(cols = ?new.iter().map(|f| f.name()).collect::<Vec<_>>(), "cold schema: adding columns");
    let schema = std::sync::Arc::new(arrow_schema::Schema::new(new));
    ds.add_columns(NewColumnTransform::AllNulls(schema), None, None)
        .await?;
    Ok(())
}

pub async fn count(uri: impl Into<Loc>, filter: &str) -> Result<usize> {
    match open(uri).await? {
        Some(ds) => Ok(ds.count_rows(Some(filter.into())).await?),
        None => Ok(0),
    }
}

/// Table name from a dataset URI (`<root>/<table>.lance` -> `<table>`), for metric labels.
pub fn table_of(uri: &str) -> &str {
    let last: &str = uri.trim_end_matches('/').rsplit('/').next().unwrap_or(uri);
    last.strip_suffix(".lance").unwrap_or(last)
}

/// Filtered scan; records `nasiko.tiering.cold_read.duration{op="scan"}`.
pub async fn scan(uri: impl Into<Loc>, filter: &str) -> Result<Vec<RecordBatch>> {
    let loc: Loc = uri.into();
    let started = std::time::Instant::now();
    let res: Result<Vec<RecordBatch>> = scan_inner(&loc, filter).await;
    crate::metrics::metrics().cold_read(table_of(&loc.uri), "scan", started.elapsed());
    res
}

async fn scan_inner(loc: &Loc, filter: &str) -> Result<Vec<RecordBatch>> {
    let Some(ds) = open(loc).await? else {
        return Ok(vec![]);
    };
    let mut s = ds.scan();
    s.filter(filter)?;
    Ok(s.try_into_stream().await?.try_collect().await?)
}

/// Values of one string column for rows matching `filter`.
pub async fn strings(uri: impl Into<Loc>, column: &str, filter: &str) -> Result<Vec<String>> {
    let loc: Loc = uri.into();
    let started = std::time::Instant::now();
    let res: Result<Vec<String>> = strings_inner(&loc, column, filter).await;
    crate::metrics::metrics().cold_read(table_of(&loc.uri), "strings", started.elapsed());
    res
}

async fn strings_inner(loc: &Loc, column: &str, filter: &str) -> Result<Vec<String>> {
    let Some(ds) = open(loc).await? else {
        return Ok(vec![]);
    };
    let mut s = ds.scan();
    s.project(&[column])?.filter(filter)?;
    let batches: Vec<RecordBatch> = s.try_into_stream().await?.try_collect().await?;
    let mut out = Vec::new();
    for b in &batches {
        if let Some(a) = b.column(0).as_any().downcast_ref::<StringArray>() {
            out.extend(a.iter().flatten().map(str::to_owned));
        }
    }
    Ok(out)
}

/// Builds (or rebuilds) an `IVF_PQ` cosine index on `column` (TRD §3.4). Needs at
/// least `partitions` rows to train; callers run it once the dataset is large enough.
pub async fn create_vector_index(
    uri: impl Into<Loc>,
    column: &str,
    partitions: usize,
    sub_vectors: usize,
) -> Result<()> {
    let Some(mut ds) = open(uri).await? else {
        return Ok(());
    };
    let params = lance::index::vector::VectorIndexParams::ivf_pq(
        partitions,
        8,
        sub_vectors,
        lance_linalg::distance::MetricType::Cosine,
        50,
    );
    ds.create_index(&[column], IndexType::Vector, None, &params, true)
        .await?;
    Ok(())
}

/// Cold ANN: top-`k` rows nearest `query` (cosine) matching `filter`.
/// Result batches carry `_distance`.
pub async fn nearest(
    uri: impl Into<Loc>,
    column: &str,
    query: &[f32],
    k: usize,
    filter: &str,
    nprobes: usize,
) -> Result<Vec<RecordBatch>> {
    let Some(ds) = open(uri).await? else {
        return Ok(vec![]);
    };
    let q = arrow_array::Float32Array::from(query.to_vec());
    let mut s = ds.scan();
    s.nearest(column, &q, k)?
        .distance_metric(lance_linalg::distance::MetricType::Cosine)
        .nprobes(nprobes)
        .filter(filter)?;
    Ok(s.try_into_stream().await?.try_collect().await?)
}

/// Logical delete; rows vanish from reads at once. Physical purge is `purge`.
pub async fn delete(uri: impl Into<Loc>, filter: &str) -> Result<()> {
    if let Some(mut ds) = open(uri).await? {
        ds.delete(filter).await?;
    }
    Ok(())
}

/// Weekly maintenance / erasure purge: compact (materializing deletions),
/// drop old versions older than `grace`, re-optimize indexes (TRD §8, §9).
pub async fn purge(uri: impl Into<Loc>, grace: chrono::TimeDelta) -> Result<()> {
    let Some(mut ds) = open(uri).await? else {
        return Ok(());
    };
    let opts = CompactionOptions {
        materialize_deletions_threshold: 0.0,
        ..Default::default()
    };
    compact_files(&mut ds, opts, None).await?;
    ds.cleanup_old_versions(grace, Some(true), None).await?;
    ds.optimize_indices(&Default::default()).await?;
    Ok(())
}

/// Name of the `INVERTED` full-text index built on a text column.
pub fn fts_index_name(column: &str) -> String {
    format!("{column}_fts")
}

/// True when the dataset has the full-text index for `column`.
pub async fn has_fts_index(uri: impl Into<Loc>, column: &str) -> Result<bool> {
    let Some(ds) = open(uri).await? else {
        return Ok(false);
    };
    let name: String = fts_index_name(column);
    let indices = ds.load_indices().await?;
    Ok(indices.iter().any(|i| i.name == name))
}

/// Builds (or rebuilds) an `INVERTED` index on `column`: simple tokenizer,
/// lowercase, no stemming or stop words, so tokens match Postgres `'simple'` FTS.
pub async fn create_fts_index(uri: impl Into<Loc>, column: &str) -> Result<()> {
    let Some(mut ds) = open(uri).await? else {
        return Ok(());
    };
    if ds.count_rows(None).await? == 0 {
        return Ok(());
    }
    let params = lance_index::scalar::InvertedIndexParams::default()
        .stem(false)
        .remove_stop_words(false)
        .lower_case(true);
    ds.create_index(
        &[column],
        IndexType::Inverted,
        Some(fts_index_name(column)),
        &params,
        true,
    )
    .await?;
    Ok(())
}

/// Full-text search: rows whose `column` contains every term (AND), prefiltered
/// by `filter`. Caller must check `has_fts_index` first.
pub async fn fts_search(
    uri: impl Into<Loc>,
    column: &str,
    terms: &[String],
    filter: &str,
) -> Result<Vec<RecordBatch>> {
    let loc: Loc = uri.into();
    let started = std::time::Instant::now();
    let res: Result<Vec<RecordBatch>> = fts_inner(&loc, column, terms, filter).await;
    crate::metrics::metrics().cold_read(table_of(&loc.uri), "fts", started.elapsed());
    res
}

async fn fts_inner(
    loc: &Loc,
    column: &str,
    terms: &[String],
    filter: &str,
) -> Result<Vec<RecordBatch>> {
    use lance_index::scalar::{
        FullTextSearchQuery,
        inverted::query::{MatchQuery, Operator},
    };
    let Some(ds) = open(loc).await? else {
        return Ok(vec![]);
    };
    let m: MatchQuery = MatchQuery::new(terms.join(" "))
        .with_column(Some(column.to_owned()))
        .with_operator(Operator::And);
    let q: FullTextSearchQuery = FullTextSearchQuery::new_query(m.into());
    let mut s = ds.scan();
    s.full_text_search(q)?.filter(filter)?.prefilter(true);
    Ok(s.try_into_stream().await?.try_collect().await?)
}

#[cfg(test)]
mod tests {
    use super::table_of;

    #[test]
    fn table_of_strips_root_and_suffix() {
        assert_eq!(table_of("s3://b/root/chat_messages.lance"), "chat_messages");
        assert_eq!(table_of("/tmp/x/token_usage.lance/"), "token_usage");
        assert_eq!(table_of("plain"), "plain");
    }
}
