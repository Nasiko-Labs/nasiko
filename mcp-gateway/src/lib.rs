//! # nasiko-mcp-gateway
//!
//! The MCP (Model Context Protocol) Gateway — the tool layer for deployed
//! agents. It gives every agent a single, permanent URL that transparently
//! exposes all of a user's connected tools (Composio toolkits + generic MCP
//! servers) as one merged, permission-filtered tool list.
//!
//! ## Crate boundary
//!
//! This crate holds **only pure logic** — the JSON-RPC protocol handlers,
//! backend providers, tool aggregation, routing, the permission engine, the
//! session resolver, OAuth 2.1, the Composio HTTP client, credential
//! encryption, and the `sqlx` data layer. The **Axum route handlers live in
//! `oss/server/src/mcp/`** so they can use `AppState`, `Claims`, `acl`, and
//! `UsageTracker` without this crate ever depending on `nasiko-server` (which
//! would be a dependency cycle: `server -> mcp-gateway`, never the reverse).
//!
//! The server module constructs an [`McpState`] from its shared `PgPool`,
//! `redis::Client`, and pooled `reqwest::Client`, then calls into the pure
//! functions exposed here.

pub mod aggregator;
pub mod cache;
pub mod catalog;
pub mod config;
pub mod connect;
pub mod credentials;
pub mod error;
pub mod injector;
pub mod net;
pub mod oauth;
pub mod permissions;
pub mod protocol;
pub mod provider;
pub mod repo;
pub mod router;
pub mod servers;
pub mod session;
pub mod state;
pub mod types;
pub mod webhooks;

pub use permissions::PermissionContext;
pub use session::ResolvedSession;

pub use config::McpConfig;
pub use error::{McpError, Result};
pub use injector::McpInjector;
pub use provider::{Providers, ToolProvider};
pub use state::McpState;
