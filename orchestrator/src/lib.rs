mod agent_registry;
pub mod context_selection;
pub mod engine;
pub mod error;
pub mod maf;
pub mod models;
pub mod pacms_selector;
pub mod policy;
pub mod providers;
pub mod reranker;
pub mod selector;
pub mod session_history;
pub mod types;
pub mod vector_store;

pub use context_selection::{
    ContextPreview, ContextSelectionStrategy, ContextTiers, PacmsBudgetLevel, ReportAttribution,
    TierCounts, agent_compression_enabled, fetch_for_user, preview, record_context_report,
    selection_enabled, selection_enabled_checked,
};
pub use engine::{OssRoutingEngine, RouterConfig, RoutingEngine};
pub use error::RouterError;
pub use models::AgentCardSummary;
pub use policy::{RoutingPolicy, SelectionSchemaExtra};
pub use reranker::Reranker;
pub use selector::AgentSelector;
pub use selector::ConversationMessage;
pub use session_history::ContextReport;
pub use session_history::SessionHistory;
pub use types::{AgentCard, FilePart, RouteRequest, RouteResult, RouterLogEntry};
pub use vector_store::{TextEmbeddingCache, VectorStore, embed_and_store_agent};
