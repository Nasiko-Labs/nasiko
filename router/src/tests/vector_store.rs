use super::*;

#[test]
fn cosine_identical_vectors() {
    let v = vec![1.0, 0.0, 0.0];
    assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
}

#[test]
fn cosine_orthogonal_vectors() {
    let a = vec![1.0, 0.0];
    let b = vec![0.0, 1.0];
    assert!((cosine_similarity(&a, &b)).abs() < 1e-6);
}

#[test]
fn cosine_zero_vector_returns_zero() {
    let a = vec![0.0, 0.0];
    let b = vec![1.0, 1.0];
    assert_eq!(cosine_similarity(&a, &b), 0.0);
}

#[test]
fn cosine_length_mismatch_returns_zero() {
    let a = vec![1.0, 2.0];
    let b = vec![1.0];
    assert_eq!(cosine_similarity(&a, &b), 0.0);
}

#[tokio::test]
async fn disabled_store_shortlist_returns_all() {
    use uuid::Uuid;
    let store = VectorStore::disabled_from(vec![
        AgentCard { id: Uuid::new_v4(), name: "a".into(), description: "".into(), skills: vec![], tags: vec![], url: None },
        AgentCard { id: Uuid::new_v4(), name: "b".into(), description: "".into(), skills: vec![], tags: vec![], url: None },
    ]);
    let result = store.shortlist("query", 1, 15).await;
    assert_eq!(result.len(), 2);
}

#[tokio::test]
async fn below_threshold_returns_all() {
    use uuid::Uuid;
    // 2 agents, threshold 15 — should skip Stage 1 and return all
    let store = VectorStore::disabled_from(vec![
        AgentCard { id: Uuid::new_v4(), name: "a".into(), description: "".into(), skills: vec![], tags: vec![], url: None },
        AgentCard { id: Uuid::new_v4(), name: "b".into(), description: "".into(), skills: vec![], tags: vec![], url: None },
    ]);
    let result = store.shortlist("anything", 10, 15).await;
    assert_eq!(result.len(), 2);
}
