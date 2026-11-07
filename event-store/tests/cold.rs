//! Lance round trip on a temp dir: append, indexed read, crash-check by batch_id,
//! erase, purge. No Postgres needed.

use chrono::{TimeZone, Utc};
use nasiko_event_store::{cold, token_usage};
use uuid::Uuid;

fn row(user: u128, secs: i64) -> token_usage::TokenUsageRow {
    token_usage::TokenUsageRow {
        id: Uuid::new_v4(),
        user_id: Uuid::from_u128(user),
        agent_id: None,
        operation_type: "chat".into(),
        request_id: None,
        session_id: Some("trace".into()),
        provider: "openai".into(),
        model: "gpt-4o-mini".into(),
        input_tokens: 1,
        output_tokens: 2,
        total_tokens: 3,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
        cached_tokens: None,
        audio_tokens: None,
        reasoning_tokens: None,
        accepted_prediction_tokens: None,
        rejected_prediction_tokens: None,
        completion_tokens_details: None,
        prompt_tokens_details: None,
        cost_usd: None,
        latency_ms: None,
        ttft_ms: None,
        streaming: None,
        finish_reason: None,
        metadata: None,
        created_at: Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn append_read_erase_purge() {
    let dir = tempfile::tempdir().unwrap();
    let uri = cold::dataset_uri(dir.path().to_str().unwrap(), token_usage::TABLE);
    let idx = ["user_id", "agent_id", "session_id"];

    let b1 = Uuid::new_v4();
    let mut rows: Vec<_> = (0..100).map(|i| row(i % 4, i as i64)).collect();
    token_usage::sort_rows(&mut rows);
    cold::append(&uri, token_usage::to_record_batch(&rows, b1).unwrap(), &idx)
        .await
        .unwrap();
    let b2 = Uuid::new_v4();
    cold::append(
        &uri,
        token_usage::to_record_batch(&[row(1, 500)], b2).unwrap(),
        &idx,
    )
    .await
    .unwrap();

    let u1 = cold::lit(&Uuid::from_u128(1).to_string());
    assert_eq!(
        cold::count(&uri, &format!("user_id = {u1}")).await.unwrap(),
        26
    );
    let ids = cold::strings(
        &uri,
        "id",
        &format!("batch_id = {}", cold::lit(&b2.to_string())),
    )
    .await
    .unwrap();
    assert_eq!(ids.len(), 1);

    let back: Vec<_> = cold::scan(&uri, &format!("user_id = {u1}"))
        .await
        .unwrap()
        .iter()
        .flat_map(|b| token_usage::from_record_batch(b).unwrap())
        .collect();
    assert_eq!(back.len(), 26);

    cold::delete(&uri, &format!("user_id = {u1}"))
        .await
        .unwrap();
    assert_eq!(
        cold::count(&uri, &format!("user_id = {u1}")).await.unwrap(),
        0
    );
    cold::purge(&uri, chrono::TimeDelta::zero()).await.unwrap();
    assert_eq!(cold::count(&uri, "user_id IS NOT NULL").await.unwrap(), 75);
}

/// TRD §10: a batch with a new nullable column appends to an older dataset;
/// old rows read NULL for it.
#[tokio::test(flavor = "multi_thread")]
async fn append_adds_new_column() {
    use arrow_array::{Int32Array, RecordBatch, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let uri = cold::dataset_uri(dir.path().to_str().unwrap(), "evo");
    let s1 = Arc::new(Schema::new(vec![Field::new(
        "user_id",
        DataType::Utf8,
        false,
    )]));
    let b1 = RecordBatch::try_new(s1, vec![Arc::new(StringArray::from(vec!["a", "b"]))]).unwrap();
    cold::append(&uri, b1, &["user_id"]).await.unwrap();

    let s2 = Arc::new(Schema::new(vec![
        Field::new("user_id", DataType::Utf8, false),
        Field::new("extra", DataType::Int32, true),
    ]));
    let b2 = RecordBatch::try_new(
        s2,
        vec![
            Arc::new(StringArray::from(vec!["c"])),
            Arc::new(Int32Array::from(vec![7])),
        ],
    )
    .unwrap();
    cold::append(&uri, b2, &["user_id"]).await.unwrap();

    assert_eq!(cold::count(&uri, "extra IS NULL").await.unwrap(), 2);
    assert_eq!(cold::count(&uri, "extra = 7").await.unwrap(), 1);
}

#[tokio::test]
async fn missing_dataset_reads_empty() {
    let dir = tempfile::tempdir().unwrap();
    let uri = cold::dataset_uri(dir.path().to_str().unwrap(), "nope");
    assert_eq!(cold::count(&uri, "true").await.unwrap(), 0);
    assert!(cold::scan(&uri, "true").await.unwrap().is_empty());
}

/// NAS-1164: the same append/scan/delete path against S3 (rustfs/MinIO).
/// Runs only when `TEST_S3_ENDPOINT` is set, e.g.
/// `TEST_S3_ENDPOINT=http://localhost:9000 TEST_S3_KEY=nasiko TEST_S3_SECRET=nasiko123 TEST_S3_BUCKET=nasiko`.
#[tokio::test(flavor = "multi_thread")]
async fn s3_round_trip() {
    let Ok(endpoint) = std::env::var("TEST_S3_ENDPOINT") else {
        eprintln!("TEST_S3_ENDPOINT unset; skipping");
        return;
    };
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.into());
    let opts: std::collections::HashMap<String, String> = [
        ("aws_access_key_id", env("TEST_S3_KEY", "nasiko")),
        ("aws_secret_access_key", env("TEST_S3_SECRET", "nasiko123")),
        ("aws_region", "us-east-1".into()),
        ("aws_endpoint", endpoint.clone()),
        ("allow_http", endpoint.starts_with("http://").to_string()),
        ("aws_virtual_hosted_style_request", "false".into()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let root = format!(
        "s3://{}/cold-test-{}",
        env("TEST_S3_BUCKET", "nasiko"),
        Uuid::new_v4().simple()
    );
    let loc = cold::Loc {
        uri: cold::dataset_uri(&root, token_usage::TABLE),
        opts,
    };

    let rows: Vec<_> = (0..20u128).map(|i| row(i % 4 + 1, i as i64)).collect();
    let b = token_usage::to_record_batch(&rows, Uuid::new_v4()).unwrap();
    cold::append(&loc, b, &["user_id"]).await.unwrap();
    let u1 = cold::lit(&Uuid::from_u128(1).to_string());
    assert_eq!(
        cold::count(&loc, &format!("user_id = {u1}")).await.unwrap(),
        5
    );
    cold::delete(&loc, &format!("user_id = {u1}"))
        .await
        .unwrap();
    assert_eq!(cold::count(&loc, "user_id IS NOT NULL").await.unwrap(), 15);
}
