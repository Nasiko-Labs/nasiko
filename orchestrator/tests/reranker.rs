use nasiko_orchestrator::session_history::ChatMessage;
use nasiko_orchestrator::{AgentCard, Reranker, SessionHistory, VectorStore};
use std::sync::Arc;
use uuid::Uuid;

// ── Helpers ───────────────────────────────────────────────────────────────────

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

// ── empty history — no embed call ────────────────────────────────────────────

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
async fn k_zero_returns_empty() {
    let agents = make_agents(&["a", "b", "c"]);
    let store = make_store_disabled(agents.clone());
    let reranker = Reranker::new(store);
    let history = SessionHistory::default();

    let result = reranker.rerank(agents, &history, "query", 0).await;
    assert!(result.is_empty());
}

// ── non-empty history with disabled store ─────────────────────────────────────
// Disabled store always returns Err from embed() → graceful fallback to first-k.

#[tokio::test]
async fn with_history_disabled_store_returns_first_k() {
    let agents = make_agents(&["x", "y", "z"]);
    let store = make_store_disabled(agents.clone());
    let reranker = Reranker::new(store);
    let history = SessionHistory {
        messages: vec![ChatMessage {
            role: "user".into(),
            content: "hello".into(),
        }],
    };
    let result = reranker.rerank(agents, &history, "query", 2).await;
    assert_eq!(result.len(), 2);
}

#[tokio::test]
async fn with_history_disabled_store_k_larger_than_list() {
    let agents = make_agents(&["p", "q"]);
    let store = make_store_disabled(agents.clone());
    let reranker = Reranker::new(store);
    let history = SessionHistory {
        messages: vec![ChatMessage {
            role: "assistant".into(),
            content: "previous answer".into(),
        }],
    };
    let result = reranker.rerank(agents, &history, "follow-up", 100).await;
    assert_eq!(result.len(), 2);
}

// ── empty agent list ──────────────────────────────────────────────────────────

#[tokio::test]
async fn empty_agent_list_returns_empty() {
    let store = Arc::new(VectorStore::disabled());
    let reranker = Reranker::new(store);
    let history = SessionHistory::default();

    let result = reranker.rerank(vec![], &history, "query", 5).await;
    assert!(result.is_empty());
}

#[tokio::test]
async fn empty_agent_list_with_history_returns_empty() {
    let store = Arc::new(VectorStore::disabled());
    let reranker = Reranker::new(store);
    let history = SessionHistory {
        messages: vec![ChatMessage {
            role: "user".into(),
            content: "some message".into(),
        }],
    };
    let result = reranker.rerank(vec![], &history, "query", 5).await;
    assert!(result.is_empty());
}

// ── ordering preservation (empty history) ─────────────────────────────────────

#[tokio::test]
async fn empty_history_preserves_input_order() {
    let agents = make_agents(&["first", "second", "third"]);
    let store = make_store_disabled(agents.clone());
    let reranker = Reranker::new(store);

    let result = reranker
        .rerank(agents, &SessionHistory::default(), "q", 3)
        .await;
    assert_eq!(result[0].name, "first");
    assert_eq!(result[1].name, "second");
    assert_eq!(result[2].name, "third");
}

// ── live LLM/embeddings tests ─────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires live OpenAI-compatible embeddings API"]
async fn with_history_and_live_store_returns_scored_results() {
    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY required");
    let base_url =
        std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com".into());
    let model = "text-embedding-3-small".to_string();

    let agents: Vec<AgentCard> = vec![
        AgentCard {
            id: Uuid::new_v4(),
            name: "coding-agent".into(),
            description: "Writes and reviews Rust code".into(),
            skills: vec!["rust".into()],
            tags: vec!["engineering".into()],
            url: None,
        },
        AgentCard {
            id: Uuid::new_v4(),
            name: "finance-agent".into(),
            description: "Analyzes stock prices and crypto markets".into(),
            skills: vec!["trading".into()],
            tags: vec!["finance".into()],
            url: None,
        },
    ];

    let cache = Default::default();
    let store =
        Arc::new(VectorStore::build(agents.clone(), api_key, base_url, model, &cache).await);
    let reranker = Reranker::new(store);
    let history = SessionHistory {
        messages: vec![ChatMessage {
            role: "user".into(),
            content: "I've been looking at BTC prices".into(),
        }],
    };

    let result = reranker
        .rerank(agents, &history, "what about ETH?", 2)
        .await;
    assert!(!result.is_empty());
}
