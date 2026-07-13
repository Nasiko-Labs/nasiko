//! Shared state for the MCP gateway.
//!
//! Constructed once from the server's `AppState` primitives (see
//! `oss/server/src/mcp/`) and threaded through the pure gateway logic. It holds
//! only cheaply-cloneable handles — the same `PgPool`, `redis::Client`, and
//! pooled `reqwest::Client` the rest of the server already shares — so there is
//! no duplicated infrastructure.

use nasiko_config::Config;
use sqlx::PgPool;

use crate::config::McpConfig;
use crate::provider::Providers;

#[derive(Clone)]
pub struct McpState {
    pub db: PgPool,
    pub redis: redis::Client,
    pub http_client: reqwest::Client,
    /// SSRF/DNS-rebinding-guarded client for outbound calls to user-controlled
    /// URLs (OAuth discovery/exchange/refresh against dynamically-discovered
    /// endpoints). Distinct from `http_client`, which may reach internal hosts.
    pub guarded_http_client: reqwest::Client,
    pub config: McpConfig,
    /// Tool backends: the Composio Tool Router client (when configured) + the
    /// shared generic MCP transport.
    pub providers: Providers,
}

impl McpState {
    /// Build gateway state from the server's shared handles and platform config.
    pub fn new(
        db: PgPool,
        redis: redis::Client,
        http_client: reqwest::Client,
        config: &Config,
    ) -> Self {
        let mcp_config = McpConfig::from_config(config);
        let providers = Providers::new(http_client.clone(), &mcp_config);
        Self {
            db,
            redis,
            http_client,
            guarded_http_client: crate::net::guarded_http_client(),
            config: mcp_config,
            providers,
        }
    }
}
