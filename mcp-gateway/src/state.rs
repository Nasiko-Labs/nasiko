//! Shared state for the MCP gateway.
//!
//! Constructed once from the server's `AppState` primitives (see
//! `oss/server/src/mcp/`) and threaded through the pure gateway logic. It holds
//! only cheaply-cloneable handles — the same `PgPool`, `SharedRedis`, and
//! pooled `reqwest::Client` the rest of the server already shares — so there is
//! no duplicated infrastructure.

use std::sync::Arc;

use nasiko_config::Config;
use sqlx::PgPool;

use nasiko_orchestrator::llm_backend::{LlmCallContext, SharedLlmBackend};
use nasiko_orchestrator::providers::LLMProvider;

use crate::authorizer::{ConnectorAuthorizer, OssConnectorAuthorizer};
use crate::config::{McpConfig, ToolSearchMode};
use crate::endpoint_refresh::{EndpointRefresher, NoopEndpointRefresher};
use crate::entitlements::{OssToolEntitlements, ToolEntitlements};
use crate::provider::{ComposioBackend, ComposioProvider, Providers};
use crate::search::ToolSearchIndex;

/// `trace_usage.agent_name` for description-backfill chat calls.
pub const DESCRIPTION_COMPONENT: &str = "mcp_tool_description";

#[derive(Clone)]
pub struct McpState {
    pub db: PgPool,
    pub redis: nasiko_redis::SharedRedis,
    pub http_client: reqwest::Client,
    /// SSRF/DNS-rebinding-guarded client for outbound calls to user-controlled
    /// URLs (OAuth discovery/exchange/refresh against dynamically-discovered
    /// endpoints). Distinct from `http_client`, which may reach internal hosts.
    pub guarded_http_client: reqwest::Client,
    pub config: McpConfig,
    /// Tool backends: the Composio Tool Router client (when configured) + the
    /// shared generic MCP transport.
    pub providers: Providers,
    /// Layer-1 connector reachability. Default = owner ∪ user/public grant; an
    /// edition can swap a richer impl at startup.
    pub authorizer: Arc<dyn ConnectorAuthorizer>,
    /// Per-user tool entitlements. Default = unrestricted; an edition can swap
    /// a restricting impl at startup, same pattern as `authorizer`.
    pub entitlements: Arc<dyn ToolEntitlements>,
    /// Self-heal for `uploaded_build` connectors whose live container address
    /// drifted (restart/redeploy/reboot) — see `endpoint_refresh`'s module
    /// doc. Default is a no-op; `oss/server` swaps in a real,
    /// `ContainerRuntime`-backed impl at `AppState` construction, same swap
    /// pattern as `authorizer` above.
    pub endpoint_refresher: Arc<dyn EndpointRefresher>,
    /// Best-effort LLM fallback for connector/tool descriptions the native
    /// source didn't provide — see `description_backfill`. Never used when a
    /// native description is already present.
    pub llm: LLMProvider,
    /// Flat tool search index — semantic (default) or BM25 (fallback).
    pub search_index: Arc<dyn ToolSearchIndex>,
}

impl McpState {
    /// Point the Composio backend at `api_key`, or switch it off with `None`. Every clone
    /// of this state shares the one slot, so the change is live at once.
    pub fn set_composio_key(&self, api_key: Option<String>) {
        let backend = api_key.map(|key| ComposioBackend {
            provider: Arc::new(ComposioProvider::new(
                self.http_client.clone(),
                key.clone(),
                self.config.composio_base_url.clone(),
            )),
            api_key: Some(key),
        });
        self.providers.composio.replace(backend);
    }

    /// Build gateway state from the server's shared handles and platform config.
    ///
    /// `llm_backend` is the in-process LLM router. When present, tool embeddings and
    /// description backfill go through it (any configured provider, usage in
    /// `trace_usage`); when absent, semantic search degrades to keyword search and
    /// descriptions use the env OpenAI key.
    pub fn new(
        db: PgPool,
        redis: nasiko_redis::SharedRedis,
        http_client: reqwest::Client,
        config: &Config,
        llm_backend: Option<SharedLlmBackend>,
    ) -> Self {
        let mcp_config = McpConfig::from_config(config);
        let providers = Providers::new(http_client.clone(), &mcp_config);
        let mut llm = LLMProvider::from_env(http_client.clone());
        if let Some(backend) = &llm_backend {
            llm = llm.with_backend(
                Arc::clone(backend),
                LlmCallContext {
                    user_id: uuid::Uuid::nil(),
                    component: DESCRIPTION_COMPONENT.to_string(),
                    session_id: None,
                },
            );
        }
        let search_index: Arc<dyn ToolSearchIndex> = match mcp_config.tool_search_mode {
            ToolSearchMode::Semantic => match &llm_backend {
                Some(backend) => Arc::new(crate::search::SemanticSearchIndex::new(
                    redis.clone(),
                    Arc::clone(backend),
                    mcp_config.embedding_model.clone(),
                )),
                None => {
                    tracing::warn!(
                        "MCP_TOOL_SEARCH_MODE=semantic but no LLM router backend, \
                         falling back to keyword"
                    );
                    Arc::new(crate::search::Bm25SearchIndex::new())
                }
            },
            ToolSearchMode::Keyword => Arc::new(crate::search::Bm25SearchIndex::new()),
            ToolSearchMode::None => Arc::new(crate::search::NoopSearchIndex),
        };
        Self {
            db,
            redis,
            http_client,
            guarded_http_client: crate::net::guarded_http_client(),
            config: mcp_config,
            providers,
            authorizer: Arc::new(OssConnectorAuthorizer),
            entitlements: Arc::new(OssToolEntitlements),
            endpoint_refresher: Arc::new(NoopEndpointRefresher),
            llm,
            search_index,
        }
    }
}
