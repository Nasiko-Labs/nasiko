use super::*;
use uuid::Uuid;

fn make_agents(names: &[&str]) -> Vec<AgentCard> {
    names
        .iter()
        .map(|n| AgentCard {
            id: Uuid::new_v4(),
            name: n.to_string(),
            description: String::new(),
            skills: vec![],
            tags: vec![],
            url: None,
        })
        .collect()
}

fn make_store_disabled(agents: Vec<AgentCard>) -> Arc<VectorStore> {
    Arc::new(VectorStore::disabled_from_public(agents))
}

#[tokio::test]
async fn empty_history_returns_first_k_unchanged() {
    let agents = make_agents(&["a", "b", "c", "d"]);
    let store = make_store_disabled(agents.clone());
    let reranker = Reranker::new(store);
    let history = SessionHistory::default();

    let result = reranker.rerank(agents, &history, "query", 2).await;
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].name, "a");
    assert_eq!(result[1].name, "b");
}

#[tokio::test]
async fn k_larger_than_list_returns_all() {
    let agents = make_agents(&["a", "b"]);
    let store = make_store_disabled(agents.clone());
    let reranker = Reranker::new(store);
    let history = SessionHistory::default();

    let result = reranker.rerank(agents, &history, "query", 100).await;
    assert_eq!(result.len(), 2);
}

#[tokio::test]
async fn with_history_disabled_store_returns_first_k() {
    use crate::session_history::ChatMessage;
    let agents = make_agents(&["x", "y", "z"]);
    let store = make_store_disabled(agents.clone());
    let reranker = Reranker::new(store);
    let history = SessionHistory {
        messages: vec![ChatMessage {
            role: "user".into(),
            content: "hello".into(),
        }],
    };
    // disabled store → embed() returns Err → fallback to first k
    let result = reranker.rerank(agents, &history, "query", 2).await;
    assert_eq!(result.len(), 2);
}
