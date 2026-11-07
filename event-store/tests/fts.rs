//! Cold full-text index: build, detect, search with user prefilter.

use std::sync::Arc;

use arrow_array::{RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use nasiko_event_store::cold;

fn batch(rows: &[(&str, &str, &str)]) -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("user_id", DataType::Utf8, false),
        Field::new("content", DataType::Utf8, false),
    ]));
    let col = |i: usize| -> Arc<StringArray> {
        Arc::new(StringArray::from(
            rows.iter()
                .map(|r| [r.0, r.1, r.2][i])
                .collect::<Vec<&str>>(),
        ))
    };
    RecordBatch::try_new(schema, vec![col(0), col(1), col(2)]).unwrap()
}

fn ids(batches: &[RecordBatch]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for b in batches {
        let a = b
            .column_by_name("id")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        out.extend(a.iter().flatten().map(str::to_owned));
    }
    out.sort();
    out
}

#[tokio::test]
async fn fts_index_finds_all_terms_for_user() {
    let dir = tempfile::tempdir().unwrap();
    let uri: String = format!("{}/chat_messages.lance", dir.path().display());
    cold::append(
        &uri,
        batch(&[
            ("1", "u1", "Deploy the Rust server today"),
            ("2", "u1", "rust only"),
            ("3", "u2", "deploy rust server"),
        ]),
        &[],
    )
    .await
    .unwrap();
    assert!(!cold::has_fts_index(&uri, "content").await.unwrap());
    cold::create_fts_index(&uri, "content").await.unwrap();
    assert!(cold::has_fts_index(&uri, "content").await.unwrap());

    let terms: Vec<String> = vec!["rust".into(), "deploy".into()];
    let hits: Vec<RecordBatch> = cold::fts_search(&uri, "content", &terms, "user_id = 'u1'")
        .await
        .unwrap();
    assert_eq!(ids(&hits), vec!["1"]);

    // New fragment, then index optimize (as append does) keeps it searchable.
    cold::append(&uri, batch(&[("4", "u1", "rust deploy again")]), &[])
        .await
        .unwrap();
    let hits: Vec<RecordBatch> = cold::fts_search(&uri, "content", &terms, "user_id = 'u1'")
        .await
        .unwrap();
    assert_eq!(ids(&hits), vec!["1", "4"]);
}

#[tokio::test]
async fn missing_dataset_has_no_index() {
    let dir = tempfile::tempdir().unwrap();
    let uri: String = format!("{}/none.lance", dir.path().display());
    assert!(!cold::has_fts_index(&uri, "content").await.unwrap());
}
