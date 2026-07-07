//! Data layer — every `sqlx` query against the `mcp_*` tables.
//!
//! Thin and side-effect-free beyond the DB: functions take a `&PgPool`, return
//! typed rows or `Result`, and never encrypt/decrypt, call HTTP, or touch Redis.
//! Credential/token columns are stored and returned as opaque strings (already
//! encrypted by the caller) — the row structs deliberately do **not** derive
//! `Serialize`, so a route can never accidentally serialize a secret; the server
//! module maps rows to explicit response DTOs.
//!
//! `agent_id` is a `Uuid` (FK to `agents.id`), not the PoC's free-form string —
//! it comes from the delegation token's `act` claim / a management path param.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::Result;

// ─── Row types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct McpAuthConfig {
    pub id: Uuid,
    pub auth_config_id: String,
    pub user_id: Option<Uuid>,
    pub toolkit: String,
    pub auth_scheme: String,
    pub use_composio_managed: bool,
    pub is_platform: bool,
    pub display_name: Option<String>,
    pub logo_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct McpServer {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub transport: String,
    pub auth_type: String,
    pub url_param_name: Option<String>,
    pub credential_header_name: Option<String>,
    pub headers: Option<Value>,
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub logo_url: Option<String>,
    pub is_platform: bool,
    pub user_id: Option<Uuid>,
    pub is_active: bool,
    pub oauth_authorization_endpoint: Option<String>,
    pub oauth_token_endpoint: Option<String>,
    pub oauth_client_id: Option<String>,
    pub oauth_client_secret: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl McpServer {
    /// True once OAuth endpoints are discovered and a client is registered.
    pub fn oauth_configured(&self) -> bool {
        self.oauth_authorization_endpoint.is_some()
            && self.oauth_token_endpoint.is_some()
            && self.oauth_client_id.is_some()
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct McpConnection {
    pub id: Uuid,
    pub user_id: Uuid,
    pub auth_config_id: String,
    pub toolkit: String,
    pub status: String,
    pub connected_account_id: Option<String>,
    pub redirect_url: Option<String>,
    pub oauth_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct McpOAuthToken {
    pub id: Uuid,
    pub mcp_server_id: Uuid,
    pub user_id: Uuid,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub scope: Option<String>,
    pub token_type: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct McpUserCredential {
    pub id: Uuid,
    pub mcp_server_id: Uuid,
    pub user_id: Uuid,
    pub credential_type: String,
    pub credential_value: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct McpAgentServerAccess {
    pub id: Uuid,
    pub user_id: Uuid,
    pub agent_id: Uuid,
    pub server_name: String,
    pub server_type: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct McpAgentToolPermission {
    pub id: Uuid,
    pub user_id: Uuid,
    pub agent_id: Uuid,
    pub server_name: String,
    pub tool_pattern: String,
    pub stance: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct McpComposioSession {
    pub id: Uuid,
    pub user_id: Uuid,
    pub session_id: String,
    pub connected_account_ids: Option<Value>,
    pub connected_toolkits: Option<Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Input for [`create_mcp_server`] — groups the many server columns so call
/// sites don't pass a dozen positional args.
#[derive(Debug, Clone)]
pub struct NewMcpServer {
    pub name: String,
    pub url: String,
    pub transport: String,
    pub auth_type: String,
    pub url_param_name: Option<String>,
    pub credential_header_name: Option<String>,
    pub headers: Option<Value>,
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub logo_url: Option<String>,
    pub is_platform: bool,
    pub user_id: Option<Uuid>,
    pub is_active: bool,
}

// ─── Auth configs ───────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub async fn create_auth_config(
    db: &PgPool,
    auth_config_id: &str,
    user_id: Option<Uuid>,
    toolkit: &str,
    use_composio_managed: bool,
    is_platform: bool,
    display_name: Option<&str>,
    logo_url: Option<&str>,
) -> Result<McpAuthConfig> {
    let row = sqlx::query_as::<_, McpAuthConfig>(
        r#"INSERT INTO mcp_auth_configs
             (auth_config_id, user_id, toolkit, use_composio_managed, is_platform, display_name, logo_url)
           VALUES ($1, $2, $3, $4, $5, $6, $7)
           RETURNING *"#,
    )
    .bind(auth_config_id)
    .bind(user_id)
    .bind(toolkit)
    .bind(use_composio_managed)
    .bind(is_platform)
    .bind(display_name)
    .bind(logo_url)
    .fetch_one(db)
    .await?;
    Ok(row)
}

pub async fn get_auth_config(db: &PgPool, auth_config_id: &str) -> Result<Option<McpAuthConfig>> {
    let row = sqlx::query_as::<_, McpAuthConfig>(
        "SELECT * FROM mcp_auth_configs WHERE auth_config_id = $1",
    )
    .bind(auth_config_id)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

pub async fn get_platform_auth_config_by_toolkit(
    db: &PgPool,
    toolkit: &str,
) -> Result<Option<McpAuthConfig>> {
    let row = sqlx::query_as::<_, McpAuthConfig>(
        "SELECT * FROM mcp_auth_configs WHERE is_platform = true AND toolkit = $1",
    )
    .bind(toolkit)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

pub async fn get_auth_config_by_user_and_toolkit(
    db: &PgPool,
    user_id: Uuid,
    toolkit: &str,
) -> Result<Option<McpAuthConfig>> {
    let row = sqlx::query_as::<_, McpAuthConfig>(
        "SELECT * FROM mcp_auth_configs WHERE user_id = $1 AND toolkit = $2",
    )
    .bind(user_id)
    .bind(toolkit)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

pub async fn list_platform_auth_configs(db: &PgPool) -> Result<Vec<McpAuthConfig>> {
    let rows = sqlx::query_as::<_, McpAuthConfig>(
        "SELECT * FROM mcp_auth_configs WHERE is_platform = true ORDER BY toolkit",
    )
    .fetch_all(db)
    .await?;
    Ok(rows)
}

pub async fn list_auth_configs_by_user(db: &PgPool, user_id: Uuid) -> Result<Vec<McpAuthConfig>> {
    let rows = sqlx::query_as::<_, McpAuthConfig>(
        "SELECT * FROM mcp_auth_configs WHERE user_id = $1 ORDER BY toolkit",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

pub async fn delete_auth_config(db: &PgPool, auth_config_id: &str) -> Result<bool> {
    let res = sqlx::query("DELETE FROM mcp_auth_configs WHERE auth_config_id = $1")
        .bind(auth_config_id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn update_auth_config_catalog(
    db: &PgPool,
    auth_config_id: &str,
    display_name: Option<&str>,
    logo_url: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE mcp_auth_configs SET display_name = $2, logo_url = $3 WHERE auth_config_id = $1",
    )
    .bind(auth_config_id)
    .bind(display_name)
    .bind(logo_url)
    .execute(db)
    .await?;
    Ok(())
}

// ─── Servers ────────────────────────────────────────────────────────────────

pub async fn create_mcp_server(db: &PgPool, s: &NewMcpServer) -> Result<McpServer> {
    let row = sqlx::query_as::<_, McpServer>(
        r#"INSERT INTO mcp_servers
             (name, url, transport, auth_type, url_param_name, credential_header_name,
              headers, description, display_name, logo_url, is_platform, user_id, is_active)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
           RETURNING *"#,
    )
    .bind(&s.name)
    .bind(&s.url)
    .bind(&s.transport)
    .bind(&s.auth_type)
    .bind(&s.url_param_name)
    .bind(&s.credential_header_name)
    .bind(&s.headers)
    .bind(&s.description)
    .bind(&s.display_name)
    .bind(&s.logo_url)
    .bind(s.is_platform)
    .bind(s.user_id)
    .bind(s.is_active)
    .fetch_one(db)
    .await?;
    Ok(row)
}

pub async fn get_mcp_server_by_id(db: &PgPool, id: Uuid) -> Result<Option<McpServer>> {
    let row = sqlx::query_as::<_, McpServer>("SELECT * FROM mcp_servers WHERE id = $1")
        .bind(id)
        .fetch_optional(db)
        .await?;
    Ok(row)
}

pub async fn get_platform_mcp_server_by_name(
    db: &PgPool,
    name: &str,
) -> Result<Option<McpServer>> {
    let row = sqlx::query_as::<_, McpServer>(
        "SELECT * FROM mcp_servers WHERE is_platform = true AND name = $1",
    )
    .bind(name)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

pub async fn get_user_mcp_server_by_name(
    db: &PgPool,
    user_id: Uuid,
    name: &str,
) -> Result<Option<McpServer>> {
    let row = sqlx::query_as::<_, McpServer>(
        "SELECT * FROM mcp_servers WHERE is_platform = false AND user_id = $1 AND name = $2",
    )
    .bind(user_id)
    .bind(name)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

/// All active servers visible to a user: platform servers ∪ their own.
pub async fn list_mcp_servers_for_user(db: &PgPool, user_id: Uuid) -> Result<Vec<McpServer>> {
    let rows = sqlx::query_as::<_, McpServer>(
        r#"SELECT * FROM mcp_servers
           WHERE is_active = true
             AND (is_platform = true OR (is_platform = false AND user_id = $1))
           ORDER BY is_platform DESC, name"#,
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

pub async fn list_platform_mcp_servers(db: &PgPool) -> Result<Vec<McpServer>> {
    let rows = sqlx::query_as::<_, McpServer>(
        "SELECT * FROM mcp_servers WHERE is_platform = true ORDER BY name",
    )
    .fetch_all(db)
    .await?;
    Ok(rows)
}

pub async fn list_user_mcp_servers(db: &PgPool, user_id: Uuid) -> Result<Vec<McpServer>> {
    let rows = sqlx::query_as::<_, McpServer>(
        "SELECT * FROM mcp_servers WHERE is_platform = false AND user_id = $1 ORDER BY name",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

pub async fn delete_mcp_server(db: &PgPool, id: Uuid) -> Result<bool> {
    let res = sqlx::query("DELETE FROM mcp_servers WHERE id = $1")
        .bind(id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn update_mcp_server_oauth_config(
    db: &PgPool,
    id: Uuid,
    authorization_endpoint: &str,
    token_endpoint: &str,
    client_id: Option<&str>,
    client_secret: Option<&str>,
) -> Result<()> {
    sqlx::query(
        r#"UPDATE mcp_servers
           SET oauth_authorization_endpoint = $2,
               oauth_token_endpoint = $3,
               oauth_client_id = $4,
               oauth_client_secret = $5
           WHERE id = $1"#,
    )
    .bind(id)
    .bind(authorization_endpoint)
    .bind(token_endpoint)
    .bind(client_id)
    .bind(client_secret)
    .execute(db)
    .await?;
    Ok(())
}

// ─── OAuth tokens ───────────────────────────────────────────────────────────

pub async fn get_mcp_oauth_tokens_for_user(
    db: &PgPool,
    user_id: Uuid,
) -> Result<Vec<McpOAuthToken>> {
    let rows = sqlx::query_as::<_, McpOAuthToken>(
        "SELECT * FROM mcp_oauth_tokens WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

pub async fn get_mcp_oauth_token(
    db: &PgPool,
    mcp_server_id: Uuid,
    user_id: Uuid,
) -> Result<Option<McpOAuthToken>> {
    let row = sqlx::query_as::<_, McpOAuthToken>(
        "SELECT * FROM mcp_oauth_tokens WHERE mcp_server_id = $1 AND user_id = $2",
    )
    .bind(mcp_server_id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

#[allow(clippy::too_many_arguments)]
pub async fn upsert_mcp_oauth_token(
    db: &PgPool,
    mcp_server_id: Uuid,
    user_id: Uuid,
    access_token: &str,
    refresh_token: Option<&str>,
    expires_at: Option<DateTime<Utc>>,
    scope: Option<&str>,
) -> Result<McpOAuthToken> {
    let row = sqlx::query_as::<_, McpOAuthToken>(
        r#"INSERT INTO mcp_oauth_tokens
             (mcp_server_id, user_id, access_token, refresh_token, expires_at, scope)
           VALUES ($1, $2, $3, $4, $5, $6)
           ON CONFLICT (mcp_server_id, user_id) DO UPDATE SET
             access_token = EXCLUDED.access_token,
             refresh_token = EXCLUDED.refresh_token,
             expires_at = EXCLUDED.expires_at,
             scope = EXCLUDED.scope
           RETURNING *"#,
    )
    .bind(mcp_server_id)
    .bind(user_id)
    .bind(access_token)
    .bind(refresh_token)
    .bind(expires_at)
    .bind(scope)
    .fetch_one(db)
    .await?;
    Ok(row)
}

pub async fn delete_mcp_oauth_token(
    db: &PgPool,
    mcp_server_id: Uuid,
    user_id: Uuid,
) -> Result<bool> {
    let res = sqlx::query("DELETE FROM mcp_oauth_tokens WHERE mcp_server_id = $1 AND user_id = $2")
        .bind(mcp_server_id)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}

// ─── User credentials ───────────────────────────────────────────────────────

pub async fn upsert_user_credential(
    db: &PgPool,
    mcp_server_id: Uuid,
    user_id: Uuid,
    credential_type: &str,
    credential_value: &str,
) -> Result<McpUserCredential> {
    let row = sqlx::query_as::<_, McpUserCredential>(
        r#"INSERT INTO mcp_user_credentials
             (mcp_server_id, user_id, credential_type, credential_value)
           VALUES ($1, $2, $3, $4)
           ON CONFLICT (mcp_server_id, user_id) DO UPDATE SET
             credential_type = EXCLUDED.credential_type,
             credential_value = EXCLUDED.credential_value
           RETURNING *"#,
    )
    .bind(mcp_server_id)
    .bind(user_id)
    .bind(credential_type)
    .bind(credential_value)
    .fetch_one(db)
    .await?;
    Ok(row)
}

pub async fn get_user_credential(
    db: &PgPool,
    mcp_server_id: Uuid,
    user_id: Uuid,
) -> Result<Option<McpUserCredential>> {
    let row = sqlx::query_as::<_, McpUserCredential>(
        "SELECT * FROM mcp_user_credentials WHERE mcp_server_id = $1 AND user_id = $2",
    )
    .bind(mcp_server_id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

pub async fn get_user_credentials_for_user(
    db: &PgPool,
    user_id: Uuid,
) -> Result<Vec<McpUserCredential>> {
    let rows = sqlx::query_as::<_, McpUserCredential>(
        "SELECT * FROM mcp_user_credentials WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

pub async fn delete_user_credential(
    db: &PgPool,
    mcp_server_id: Uuid,
    user_id: Uuid,
) -> Result<bool> {
    let res =
        sqlx::query("DELETE FROM mcp_user_credentials WHERE mcp_server_id = $1 AND user_id = $2")
            .bind(mcp_server_id)
            .bind(user_id)
            .execute(db)
            .await?;
    Ok(res.rows_affected() > 0)
}

// ─── Connections ────────────────────────────────────────────────────────────

pub async fn create_connection(
    db: &PgPool,
    user_id: Uuid,
    auth_config_id: &str,
    toolkit: &str,
    oauth_url: Option<&str>,
    redirect_url: Option<&str>,
    connected_account_id: Option<&str>,
) -> Result<McpConnection> {
    let row = sqlx::query_as::<_, McpConnection>(
        r#"INSERT INTO mcp_connections
             (user_id, auth_config_id, toolkit, oauth_url, redirect_url, connected_account_id, status)
           VALUES ($1, $2, $3, $4, $5, $6, 'INITIATED')
           RETURNING *"#,
    )
    .bind(user_id)
    .bind(auth_config_id)
    .bind(toolkit)
    .bind(oauth_url)
    .bind(redirect_url)
    .bind(connected_account_id)
    .fetch_one(db)
    .await?;
    Ok(row)
}

pub async fn get_connection(db: &PgPool, id: Uuid) -> Result<Option<McpConnection>> {
    let row = sqlx::query_as::<_, McpConnection>("SELECT * FROM mcp_connections WHERE id = $1")
        .bind(id)
        .fetch_optional(db)
        .await?;
    Ok(row)
}

/// List a user's connections, optionally filtered by status.
pub async fn list_connections_by_user(
    db: &PgPool,
    user_id: Uuid,
    status: Option<&str>,
) -> Result<Vec<McpConnection>> {
    let rows = sqlx::query_as::<_, McpConnection>(
        r#"SELECT * FROM mcp_connections
           WHERE user_id = $1 AND ($2::text IS NULL OR status = $2)
           ORDER BY created_at DESC"#,
    )
    .bind(user_id)
    .bind(status)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

pub async fn get_connection_by_user_and_toolkit(
    db: &PgPool,
    user_id: Uuid,
    toolkit: &str,
    status: Option<&str>,
) -> Result<Option<McpConnection>> {
    let row = sqlx::query_as::<_, McpConnection>(
        r#"SELECT * FROM mcp_connections
           WHERE user_id = $1 AND toolkit = $2 AND ($3::text IS NULL OR status = $3)
           ORDER BY created_at DESC
           LIMIT 1"#,
    )
    .bind(user_id)
    .bind(toolkit)
    .bind(status)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

/// The single most-relevant non-EXPIRED connection for (user, toolkit):
/// ACTIVE first, else the most-recent INITIATED.
pub async fn get_active_or_pending_connection(
    db: &PgPool,
    user_id: Uuid,
    toolkit: &str,
) -> Result<Option<McpConnection>> {
    let row = sqlx::query_as::<_, McpConnection>(
        r#"SELECT * FROM mcp_connections
           WHERE user_id = $1 AND toolkit = $2 AND status <> 'EXPIRED'
           ORDER BY (status = 'ACTIVE') DESC, created_at DESC
           LIMIT 1"#,
    )
    .bind(user_id)
    .bind(toolkit)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

/// Look up a connection by its Composio account id (ca_…); newest first.
pub async fn get_connection_by_account_id(
    db: &PgPool,
    account_id: &str,
) -> Result<Option<McpConnection>> {
    let row = sqlx::query_as::<_, McpConnection>(
        r#"SELECT * FROM mcp_connections
           WHERE connected_account_id = $1
           ORDER BY created_at DESC
           LIMIT 1"#,
    )
    .bind(account_id)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

pub async fn update_connection_status(
    db: &PgPool,
    id: Uuid,
    status: &str,
) -> Result<Option<McpConnection>> {
    let row = sqlx::query_as::<_, McpConnection>(
        "UPDATE mcp_connections SET status = $2 WHERE id = $1 RETURNING *",
    )
    .bind(id)
    .bind(status)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

pub async fn update_connection_account_id(
    db: &PgPool,
    id: Uuid,
    account_id: &str,
) -> Result<()> {
    sqlx::query("UPDATE mcp_connections SET connected_account_id = $2 WHERE id = $1")
        .bind(id)
        .bind(account_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Delete EXPIRED connection rows that never completed OAuth (no account id).
pub async fn delete_orphan_expired_connections(
    db: &PgPool,
    user_id: Uuid,
    toolkit: &str,
) -> Result<u64> {
    let res = sqlx::query(
        r#"DELETE FROM mcp_connections
           WHERE user_id = $1 AND toolkit = $2 AND status = 'EXPIRED'
             AND connected_account_id IS NULL"#,
    )
    .bind(user_id)
    .bind(toolkit)
    .execute(db)
    .await?;
    Ok(res.rows_affected())
}

// ─── Agent server access ────────────────────────────────────────────────────

pub async fn get_agent_server_access(
    db: &PgPool,
    user_id: Uuid,
    agent_id: Uuid,
) -> Result<Vec<McpAgentServerAccess>> {
    let rows = sqlx::query_as::<_, McpAgentServerAccess>(
        "SELECT * FROM mcp_agent_server_access WHERE user_id = $1 AND agent_id = $2",
    )
    .bind(user_id)
    .bind(agent_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

pub async fn get_agent_server_access_row(
    db: &PgPool,
    user_id: Uuid,
    agent_id: Uuid,
    server_name: &str,
) -> Result<Option<McpAgentServerAccess>> {
    let row = sqlx::query_as::<_, McpAgentServerAccess>(
        "SELECT * FROM mcp_agent_server_access WHERE user_id = $1 AND agent_id = $2 AND server_name = $3",
    )
    .bind(user_id)
    .bind(agent_id)
    .bind(server_name)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

pub async fn upsert_agent_server_access(
    db: &PgPool,
    user_id: Uuid,
    agent_id: Uuid,
    server_name: &str,
    server_type: &str,
    enabled: bool,
) -> Result<McpAgentServerAccess> {
    let row = sqlx::query_as::<_, McpAgentServerAccess>(
        r#"INSERT INTO mcp_agent_server_access
             (user_id, agent_id, server_name, server_type, enabled)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (user_id, agent_id, server_name) DO UPDATE SET
             enabled = EXCLUDED.enabled,
             server_type = EXCLUDED.server_type
           RETURNING *"#,
    )
    .bind(user_id)
    .bind(agent_id)
    .bind(server_name)
    .bind(server_type)
    .bind(enabled)
    .fetch_one(db)
    .await?;
    Ok(row)
}

pub async fn delete_agent_server_access(
    db: &PgPool,
    user_id: Uuid,
    agent_id: Uuid,
    server_name: &str,
) -> Result<bool> {
    let res = sqlx::query(
        "DELETE FROM mcp_agent_server_access WHERE user_id = $1 AND agent_id = $2 AND server_name = $3",
    )
    .bind(user_id)
    .bind(agent_id)
    .bind(server_name)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

// ─── Agent tool permissions ─────────────────────────────────────────────────

pub async fn get_agent_tool_permissions(
    db: &PgPool,
    user_id: Uuid,
    agent_id: Uuid,
) -> Result<Vec<McpAgentToolPermission>> {
    let rows = sqlx::query_as::<_, McpAgentToolPermission>(
        "SELECT * FROM mcp_agent_tool_permissions WHERE user_id = $1 AND agent_id = $2",
    )
    .bind(user_id)
    .bind(agent_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

pub async fn get_agent_tool_permissions_for_server(
    db: &PgPool,
    user_id: Uuid,
    agent_id: Uuid,
    server_name: &str,
) -> Result<Vec<McpAgentToolPermission>> {
    let rows = sqlx::query_as::<_, McpAgentToolPermission>(
        r#"SELECT * FROM mcp_agent_tool_permissions
           WHERE user_id = $1 AND agent_id = $2 AND server_name = $3"#,
    )
    .bind(user_id)
    .bind(agent_id)
    .bind(server_name)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

pub async fn upsert_agent_tool_permission(
    db: &PgPool,
    user_id: Uuid,
    agent_id: Uuid,
    server_name: &str,
    tool_pattern: &str,
    stance: &str,
) -> Result<McpAgentToolPermission> {
    let row = sqlx::query_as::<_, McpAgentToolPermission>(
        r#"INSERT INTO mcp_agent_tool_permissions
             (user_id, agent_id, server_name, tool_pattern, stance)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (user_id, agent_id, server_name, tool_pattern) DO UPDATE SET
             stance = EXCLUDED.stance
           RETURNING *"#,
    )
    .bind(user_id)
    .bind(agent_id)
    .bind(server_name)
    .bind(tool_pattern)
    .bind(stance)
    .fetch_one(db)
    .await?;
    Ok(row)
}

pub async fn delete_agent_tool_permission(
    db: &PgPool,
    user_id: Uuid,
    agent_id: Uuid,
    server_name: &str,
    tool_pattern: &str,
) -> Result<bool> {
    let res = sqlx::query(
        r#"DELETE FROM mcp_agent_tool_permissions
           WHERE user_id = $1 AND agent_id = $2 AND server_name = $3 AND tool_pattern = $4"#,
    )
    .bind(user_id)
    .bind(agent_id)
    .bind(server_name)
    .bind(tool_pattern)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Reset an agent to default (all-allowed) by deleting all its permission rows.
/// Returns the total number of rows deleted across both tables.
pub async fn delete_all_agent_permissions(
    db: &PgPool,
    user_id: Uuid,
    agent_id: Uuid,
) -> Result<u64> {
    let r1 = sqlx::query(
        "DELETE FROM mcp_agent_server_access WHERE user_id = $1 AND agent_id = $2",
    )
    .bind(user_id)
    .bind(agent_id)
    .execute(db)
    .await?;
    let r2 = sqlx::query(
        "DELETE FROM mcp_agent_tool_permissions WHERE user_id = $1 AND agent_id = $2",
    )
    .bind(user_id)
    .bind(agent_id)
    .execute(db)
    .await?;
    Ok(r1.rows_affected() + r2.rows_affected())
}

/// Distinct agent ids that have any permission rows for this user.
pub async fn list_configured_agents(db: &PgPool, user_id: Uuid) -> Result<Vec<Uuid>> {
    let ids = sqlx::query_scalar::<_, Uuid>(
        r#"SELECT agent_id FROM mcp_agent_server_access WHERE user_id = $1
           UNION
           SELECT agent_id FROM mcp_agent_tool_permissions WHERE user_id = $1"#,
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    Ok(ids)
}

/// Distinct (user_id, agent_id) pairs referencing a server_name across both
/// permission tables — used to invalidate the permission cache before deleting
/// a server's rows. Pass `user_id = None` for a platform server (all users).
pub async fn get_agent_pairs_for_server(
    db: &PgPool,
    server_name: &str,
    user_id: Option<Uuid>,
) -> Result<Vec<(Uuid, Uuid)>> {
    let pairs = sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"SELECT user_id, agent_id FROM mcp_agent_server_access
             WHERE server_name = $1 AND ($2::uuid IS NULL OR user_id = $2)
           UNION
           SELECT user_id, agent_id FROM mcp_agent_tool_permissions
             WHERE server_name = $1 AND ($2::uuid IS NULL OR user_id = $2)"#,
    )
    .bind(server_name)
    .bind(user_id)
    .fetch_all(db)
    .await?;
    Ok(pairs)
}

/// Delete permission rows referencing a server_name across both tables (called
/// when the underlying server is removed). `user_id = None` → all users
/// (platform server); `Some` → just that user's rows. Returns rows deleted.
pub async fn delete_agent_permissions_for_server(
    db: &PgPool,
    server_name: &str,
    user_id: Option<Uuid>,
) -> Result<u64> {
    let r1 = sqlx::query(
        r#"DELETE FROM mcp_agent_server_access
           WHERE server_name = $1 AND ($2::uuid IS NULL OR user_id = $2)"#,
    )
    .bind(server_name)
    .bind(user_id)
    .execute(db)
    .await?;
    let r2 = sqlx::query(
        r#"DELETE FROM mcp_agent_tool_permissions
           WHERE server_name = $1 AND ($2::uuid IS NULL OR user_id = $2)"#,
    )
    .bind(server_name)
    .bind(user_id)
    .execute(db)
    .await?;
    Ok(r1.rows_affected() + r2.rows_affected())
}

// ─── Composio sessions ──────────────────────────────────────────────────────

pub async fn get_composio_session(
    db: &PgPool,
    user_id: Uuid,
) -> Result<Option<McpComposioSession>> {
    let row = sqlx::query_as::<_, McpComposioSession>(
        "SELECT * FROM mcp_composio_sessions WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

pub async fn upsert_composio_session(
    db: &PgPool,
    user_id: Uuid,
    session_id: &str,
    connected_account_ids: Option<&Value>,
    connected_toolkits: Option<&Value>,
) -> Result<McpComposioSession> {
    let row = sqlx::query_as::<_, McpComposioSession>(
        r#"INSERT INTO mcp_composio_sessions
             (user_id, session_id, connected_account_ids, connected_toolkits)
           VALUES ($1, $2, $3, $4)
           ON CONFLICT (user_id) DO UPDATE SET
             session_id = EXCLUDED.session_id,
             connected_account_ids = EXCLUDED.connected_account_ids,
             connected_toolkits = EXCLUDED.connected_toolkits
           RETURNING *"#,
    )
    .bind(user_id)
    .bind(session_id)
    .bind(connected_account_ids)
    .bind(connected_toolkits)
    .fetch_one(db)
    .await?;
    Ok(row)
}

pub async fn delete_composio_session(db: &PgPool, user_id: Uuid) -> Result<bool> {
    let res = sqlx::query("DELETE FROM mcp_composio_sessions WHERE user_id = $1")
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}
