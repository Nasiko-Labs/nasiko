//! Data layer — every `sqlx` query against the v2 `mcp_*` tables.
//!
//! Functions take a `&PgPool`, return typed rows or `Result`, and never
//! encrypt/decrypt, call HTTP, or touch Redis. Credential columns are opaque
//! strings (already encrypted by the caller). Row structs deliberately do NOT
//! derive `Serialize`, so a route can never accidentally serialize a secret.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::Result;
use crate::types::PUBLIC_GRANTEE;

// ─── Row types ──────────────────────────────────────────────────────────────

/// Where a `mcp_server`-provider connector's `url` came from. Backed by the
/// Postgres enum `mcp_connector_source_kind` (026_mcp_connector_uploads.sql) —
/// a real enum type, unlike `provider_type`/`auth_type` (plain TEXT + CHECK),
/// so it needs `sqlx::Type` for `SELECT *` to decode it automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, sqlx::Type)]
#[sqlx(type_name = "mcp_connector_source_kind", rename_all = "snake_case")]
pub enum SourceKind {
    /// A user typed in the URL of a server already running somewhere else.
    /// The default for every pre-existing row and every Composio row.
    #[default]
    ExternalUrl,
    /// The platform built this connector's container from uploaded source; its
    /// `url` was resolved via `ContainerRuntime::endpoint()`, never user-typed.
    UploadedBuild,
}

/// One connector — either a Composio toolkit or a custom MCP server.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct McpConnector {
    pub id: Uuid,
    pub provider_type: String,
    pub owner_id: Option<Uuid>,
    pub name: String,
    pub display_name: Option<String>,
    pub logo_url: Option<String>,
    pub description: Option<String>,
    // composio-only
    pub auth_config_id: Option<String>,
    pub auth_scheme: Option<String>,
    pub use_composio_managed: Option<bool>,
    // mcp_server-only
    pub url: Option<String>,
    pub transport: Option<String>,
    pub auth_type: Option<String>,
    pub url_param_name: Option<String>,
    pub credential_header_name: Option<String>,
    pub headers: Option<Value>,
    pub is_active: Option<bool>,
    pub oauth_authorization_endpoint: Option<String>,
    pub oauth_token_endpoint: Option<String>,
    pub oauth_client_id: Option<String>,
    pub oauth_client_secret: Option<String>,
    pub source_kind: SourceKind,
    // uploaded_build-only
    pub build_status: Option<String>,
    pub container_image_tag: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl McpConnector {
    pub fn is_composio(&self) -> bool {
        self.provider_type == "composio"
    }
    pub fn is_mcp_server(&self) -> bool {
        self.provider_type == "mcp_server"
    }
    /// NULL is_active (composio rows) is treated as active.
    pub fn active(&self) -> bool {
        self.is_active.unwrap_or(true)
    }
    /// True once OAuth endpoints are discovered and a client is registered.
    pub fn oauth_configured(&self) -> bool {
        self.oauth_authorization_endpoint.is_some()
            && self.oauth_token_endpoint.is_some()
            && self.oauth_client_id.is_some()
    }
    /// True only for a platform-built-and-deployed MCP server — see
    /// `SourceKind::UploadedBuild`'s doc comment. Drives the SSRF-guard
    /// `trusted` split (credentials.rs) and the delete/destroy-container fix.
    pub fn is_uploaded_build(&self) -> bool {
        self.source_kind == SourceKind::UploadedBuild
    }
}

/// Insert input for [`create_connector`].
#[derive(Debug, Clone, Default)]
pub struct NewConnector {
    pub provider_type: String,
    pub owner_id: Option<Uuid>,
    pub name: String,
    pub display_name: Option<String>,
    pub logo_url: Option<String>,
    pub description: Option<String>,
    pub auth_config_id: Option<String>,
    pub auth_scheme: Option<String>,
    pub use_composio_managed: Option<bool>,
    pub url: Option<String>,
    pub transport: Option<String>,
    pub auth_type: Option<String>,
    pub url_param_name: Option<String>,
    pub credential_header_name: Option<String>,
    pub headers: Option<Value>,
    pub is_active: Option<bool>,
    /// Defaults to `ExternalUrl` (matches the column's own DB default) — every
    /// pre-existing caller (`register_connector`) sets this explicitly rather
    /// than relying on the derived `Default`, so it's never ambiguous at a
    /// call site which kind of row is being created.
    pub source_kind: SourceKind,
    /// `Some("pending")` for a freshly queued `uploaded_build` connector;
    /// `None` for every `external_url`/composio row (column is nullable).
    pub build_status: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct McpConnectorGrant {
    pub id: Uuid,
    pub connector_id: Uuid,
    pub grant_type: String,
    pub grantee_id: String,
    pub granted_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct McpConnectorTool {
    pub id: Uuid,
    pub connector_id: Uuid,
    pub tool_name: String,
    pub description: Option<String>,
    pub default_stance: String,
    pub last_synced_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct McpUserConnection {
    pub id: Uuid,
    pub user_id: Uuid,
    pub connector_id: Uuid,
    pub status: String,
    pub connected_account_id: Option<String>,
    pub redirect_url: Option<String>,
    pub oauth_url: Option<String>,
    pub encrypted_credential: Option<String>,
    pub encrypted_refresh_token: Option<String>,
    pub token_expires_at: Option<DateTime<Utc>>,
    pub scope: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct McpComposioSession {
    pub id: Uuid,
    pub user_id: Uuid,
    pub composio_session_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct McpAgentConnectorAccess {
    pub id: Uuid,
    pub user_id: Uuid,
    pub agent_id: Uuid,
    pub connector_id: Uuid,
    pub enabled: bool,
    pub tool_rules: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// An active Composio connection joined to its toolkit name (connector name).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ComposioActiveConn {
    pub connector_id: Uuid,
    pub toolkit: String,
    pub connected_account_id: String,
}

// ─── Connectors ───────────────────────────────────────────────────────────────

pub async fn create_connector(db: &PgPool, c: &NewConnector) -> Result<McpConnector> {
    let row = sqlx::query_as::<_, McpConnector>(
        r#"INSERT INTO mcp_connectors
             (provider_type, owner_id, name, display_name, logo_url, description,
              auth_config_id, auth_scheme, use_composio_managed,
              url, transport, auth_type, url_param_name, credential_header_name,
              headers, is_active, source_kind, build_status)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)
           RETURNING *"#,
    )
    .bind(&c.provider_type)
    .bind(c.owner_id)
    .bind(&c.name)
    .bind(&c.display_name)
    .bind(&c.logo_url)
    .bind(&c.description)
    .bind(&c.auth_config_id)
    .bind(&c.auth_scheme)
    .bind(c.use_composio_managed)
    .bind(&c.url)
    .bind(&c.transport)
    .bind(&c.auth_type)
    .bind(&c.url_param_name)
    .bind(&c.credential_header_name)
    .bind(&c.headers)
    .bind(c.is_active)
    .bind(c.source_kind)
    .bind(&c.build_status)
    .fetch_one(db)
    .await?;
    Ok(row)
}

pub async fn get_connector_by_id(db: &PgPool, id: Uuid) -> Result<Option<McpConnector>> {
    let row = sqlx::query_as::<_, McpConnector>("SELECT * FROM mcp_connectors WHERE id = $1")
        .bind(id)
        .fetch_optional(db)
        .await?;
    Ok(row)
}

/// A Composio connector by toolkit slug (its `name`).
pub async fn get_composio_connector_by_name(db: &PgPool, name: &str) -> Result<Option<McpConnector>> {
    let row = sqlx::query_as::<_, McpConnector>(
        "SELECT * FROM mcp_connectors WHERE provider_type = 'composio' AND name = $1",
    )
    .bind(name)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

pub async fn list_composio_connectors(db: &PgPool) -> Result<Vec<McpConnector>> {
    let rows = sqlx::query_as::<_, McpConnector>(
        "SELECT * FROM mcp_connectors WHERE provider_type = 'composio' ORDER BY name",
    )
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// A user-owned connector by name (used for collision checks / auto-register).
pub async fn get_owned_connector_by_name(
    db: &PgPool,
    owner_id: Uuid,
    name: &str,
) -> Result<Option<McpConnector>> {
    let row = sqlx::query_as::<_, McpConnector>(
        "SELECT * FROM mcp_connectors WHERE owner_id = $1 AND name = $2",
    )
    .bind(owner_id)
    .bind(name)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

/// Every connector the user can reach (Layer 1): composio ∪ owned ∪ granted.
pub async fn list_accessible_connectors(db: &PgPool, user_id: Uuid) -> Result<Vec<McpConnector>> {
    let rows = sqlx::query_as::<_, McpConnector>(
        r#"SELECT * FROM mcp_connectors c
           WHERE c.provider_type = 'composio'
              OR c.owner_id = $1
              OR EXISTS (
                   SELECT 1 FROM mcp_connector_grants g
                   WHERE g.connector_id = c.id
                     AND ( (g.grant_type = 'user'   AND g.grantee_id = $1::text)
                        OR (g.grant_type = 'public' AND g.grantee_id = '*') )
                 )
           ORDER BY c.provider_type, c.name"#,
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// Accessible custom (mcp_server) connectors only — for building generic backends.
pub async fn list_accessible_mcp_connectors(db: &PgPool, user_id: Uuid) -> Result<Vec<McpConnector>> {
    let rows = sqlx::query_as::<_, McpConnector>(
        r#"SELECT * FROM mcp_connectors c
           WHERE c.provider_type = 'mcp_server' AND c.is_active = true
             AND ( c.owner_id = $1
                OR EXISTS (
                     SELECT 1 FROM mcp_connector_grants g
                     WHERE g.connector_id = c.id
                       AND ( (g.grant_type = 'user'   AND g.grantee_id = $1::text)
                          OR (g.grant_type = 'public' AND g.grantee_id = '*') )
                   ) )
           ORDER BY c.name"#,
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// Connectors owned by the user (custom servers they registered).
pub async fn list_owned_connectors(db: &PgPool, owner_id: Uuid) -> Result<Vec<McpConnector>> {
    let rows = sqlx::query_as::<_, McpConnector>(
        "SELECT * FROM mcp_connectors WHERE owner_id = $1 ORDER BY name",
    )
    .bind(owner_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// Layer 1 check for a single connector.
pub async fn can_access_connector(db: &PgPool, user_id: Uuid, connector_id: Uuid) -> Result<bool> {
    let ok = sqlx::query_scalar::<_, bool>(
        r#"SELECT EXISTS (
             SELECT 1 FROM mcp_connectors c
             WHERE c.id = $2 AND (
                 c.provider_type = 'composio' OR c.owner_id = $1
                 OR EXISTS (
                      SELECT 1 FROM mcp_connector_grants g
                      WHERE g.connector_id = c.id
                        AND ( (g.grant_type = 'user'   AND g.grantee_id = $1::text)
                           OR (g.grant_type = 'public' AND g.grantee_id = '*') )
                    )
             )
           )"#,
    )
    .bind(user_id)
    .bind(connector_id)
    .fetch_one(db)
    .await?;
    Ok(ok)
}

/// Partial-update input for [`update_connector`]. `None` fields are left unchanged.
#[derive(Debug, Clone, Default)]
pub struct UpdateConnector {
    pub name: Option<String>,
    pub url: Option<String>,
    pub transport: Option<String>,
    pub auth_type: Option<String>,
    pub url_param_name: Option<String>,
    pub credential_header_name: Option<String>,
    pub headers: Option<Value>,
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub logo_url: Option<String>,
    pub is_active: Option<bool>,
}

/// Partial-update a connector. Uses COALESCE so omitted (`None`) fields keep
/// their current value.
pub async fn update_connector(db: &PgPool, id: Uuid, u: &UpdateConnector) -> Result<McpConnector> {
    let row = sqlx::query_as::<_, McpConnector>(
        r#"UPDATE mcp_connectors SET
             name = COALESCE($2, name),
             url = COALESCE($3, url),
             transport = COALESCE($4, transport),
             auth_type = COALESCE($5, auth_type),
             url_param_name = COALESCE($6, url_param_name),
             credential_header_name = COALESCE($7, credential_header_name),
             headers = COALESCE($8, headers),
             description = COALESCE($9, description),
             display_name = COALESCE($10, display_name),
             logo_url = COALESCE($11, logo_url),
             is_active = COALESCE($12, is_active)
           WHERE id = $1
           RETURNING *"#,
    )
    .bind(id)
    .bind(&u.name)
    .bind(&u.url)
    .bind(&u.transport)
    .bind(&u.auth_type)
    .bind(&u.url_param_name)
    .bind(&u.credential_header_name)
    .bind(&u.headers)
    .bind(&u.description)
    .bind(&u.display_name)
    .bind(&u.logo_url)
    .bind(u.is_active)
    .fetch_one(db)
    .await?;
    Ok(row)
}

pub async fn update_connector_oauth_config(
    db: &PgPool,
    id: Uuid,
    authorization_endpoint: &str,
    token_endpoint: &str,
    client_id: Option<&str>,
    client_secret: Option<&str>,
) -> Result<()> {
    sqlx::query(
        r#"UPDATE mcp_connectors
           SET oauth_authorization_endpoint = $2, oauth_token_endpoint = $3,
               oauth_client_id = $4, oauth_client_secret = $5
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

pub async fn delete_connector(db: &PgPool, id: Uuid) -> Result<bool> {
    let res = sqlx::query("DELETE FROM mcp_connectors WHERE id = $1")
        .bind(id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}

// ─── Grants ─────────────────────────────────────────────────────────────────

pub async fn create_grant(
    db: &PgPool,
    connector_id: Uuid,
    grant_type: &str,
    grantee_id: &str,
    granted_by: Uuid,
) -> Result<McpConnectorGrant> {
    let row = sqlx::query_as::<_, McpConnectorGrant>(
        r#"INSERT INTO mcp_connector_grants (connector_id, grant_type, grantee_id, granted_by)
           VALUES ($1, $2, $3, $4)
           ON CONFLICT (connector_id, grant_type, grantee_id) DO UPDATE SET granted_by = EXCLUDED.granted_by
           RETURNING *"#,
    )
    .bind(connector_id)
    .bind(grant_type)
    .bind(grantee_id)
    .bind(granted_by)
    .fetch_one(db)
    .await?;
    Ok(row)
}

pub async fn list_grants_for_connector(db: &PgPool, connector_id: Uuid) -> Result<Vec<McpConnectorGrant>> {
    let rows = sqlx::query_as::<_, McpConnectorGrant>(
        "SELECT * FROM mcp_connector_grants WHERE connector_id = $1 ORDER BY created_at",
    )
    .bind(connector_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

pub async fn resolve_username_to_user_id(db: &PgPool, username: &str) -> Result<Option<Uuid>> {
    let id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM users WHERE username = $1 AND deleted_at IS NULL",
    )
    .bind(username)
    .fetch_optional(db)
    .await?;
    Ok(id)
}

/// Revoke a grant AND delete the grantee's connection row for the connector, in
/// one transaction (audited fix #2). Returns true if a grant row was removed.
pub async fn revoke_grant_and_connection(
    db: &PgPool,
    connector_id: Uuid,
    grant_type: &str,
    grantee_id: &str,
) -> Result<bool> {
    let mut tx = db.begin().await?;
    let res = sqlx::query(
        "DELETE FROM mcp_connector_grants WHERE connector_id = $1 AND grant_type = $2 AND grantee_id = $3",
    )
    .bind(connector_id)
    .bind(grant_type)
    .bind(grantee_id)
    .execute(&mut *tx)
    .await?;

    // Only a specific user's connection is removed; a public revoke leaves other
    // users' own connections intact.
    if grant_type == "user" && grantee_id != PUBLIC_GRANTEE
        && let Ok(uid) = Uuid::parse_str(grantee_id)
    {
        sqlx::query("DELETE FROM mcp_user_connections WHERE user_id = $1 AND connector_id = $2")
            .bind(uid)
            .bind(connector_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(res.rows_affected() > 0)
}

// ─── User connections ─────────────────────────────────────────────────────────

pub async fn get_user_connection(
    db: &PgPool,
    user_id: Uuid,
    connector_id: Uuid,
) -> Result<Option<McpUserConnection>> {
    let row = sqlx::query_as::<_, McpUserConnection>(
        "SELECT * FROM mcp_user_connections WHERE user_id = $1 AND connector_id = $2",
    )
    .bind(user_id)
    .bind(connector_id)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

pub async fn list_user_connections(
    db: &PgPool,
    user_id: Uuid,
    status: Option<&str>,
) -> Result<Vec<McpUserConnection>> {
    let rows = sqlx::query_as::<_, McpUserConnection>(
        r#"SELECT * FROM mcp_user_connections
           WHERE user_id = $1 AND ($2::text IS NULL OR status = $2)
           ORDER BY created_at DESC"#,
    )
    .bind(user_id)
    .bind(status)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// Active Composio connections joined to their toolkit (connector name).
pub async fn list_active_composio_connections(
    db: &PgPool,
    user_id: Uuid,
) -> Result<Vec<ComposioActiveConn>> {
    let rows = sqlx::query_as::<_, ComposioActiveConn>(
        r#"SELECT uc.connector_id, c.name AS toolkit, uc.connected_account_id
           FROM mcp_user_connections uc
           JOIN mcp_connectors c ON c.id = uc.connector_id
           WHERE uc.user_id = $1 AND c.provider_type = 'composio'
             AND uc.status = 'ACTIVE' AND uc.connected_account_id IS NOT NULL"#,
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// Store a bearer/basic/url_param credential (status → ACTIVE).
pub async fn upsert_connection_credential(
    db: &PgPool,
    user_id: Uuid,
    connector_id: Uuid,
    encrypted_credential: &str,
) -> Result<McpUserConnection> {
    let row = sqlx::query_as::<_, McpUserConnection>(
        r#"INSERT INTO mcp_user_connections (user_id, connector_id, status, encrypted_credential)
           VALUES ($1, $2, 'ACTIVE', $3)
           ON CONFLICT (user_id, connector_id) DO UPDATE SET
             status = 'ACTIVE', encrypted_credential = EXCLUDED.encrypted_credential
           RETURNING *"#,
    )
    .bind(user_id)
    .bind(connector_id)
    .bind(encrypted_credential)
    .fetch_one(db)
    .await?;
    Ok(row)
}

/// Store an OAuth2 token set (status → ACTIVE).
pub async fn upsert_connection_oauth_token(
    db: &PgPool,
    user_id: Uuid,
    connector_id: Uuid,
    encrypted_access: &str,
    encrypted_refresh: Option<&str>,
    expires_at: Option<DateTime<Utc>>,
    scope: Option<&str>,
) -> Result<McpUserConnection> {
    let row = sqlx::query_as::<_, McpUserConnection>(
        r#"INSERT INTO mcp_user_connections
             (user_id, connector_id, status, encrypted_credential, encrypted_refresh_token, token_expires_at, scope)
           VALUES ($1, $2, 'ACTIVE', $3, $4, $5, $6)
           ON CONFLICT (user_id, connector_id) DO UPDATE SET
             status = 'ACTIVE',
             encrypted_credential = EXCLUDED.encrypted_credential,
             encrypted_refresh_token = EXCLUDED.encrypted_refresh_token,
             token_expires_at = EXCLUDED.token_expires_at,
             scope = EXCLUDED.scope
           RETURNING *"#,
    )
    .bind(user_id)
    .bind(connector_id)
    .bind(encrypted_access)
    .bind(encrypted_refresh)
    .bind(expires_at)
    .bind(scope)
    .fetch_one(db)
    .await?;
    Ok(row)
}

/// Create/refresh a Composio connection row (status INITIATED, with oauth url).
pub async fn upsert_composio_connection(
    db: &PgPool,
    user_id: Uuid,
    connector_id: Uuid,
    oauth_url: Option<&str>,
    redirect_url: Option<&str>,
) -> Result<McpUserConnection> {
    let row = sqlx::query_as::<_, McpUserConnection>(
        r#"INSERT INTO mcp_user_connections
             (user_id, connector_id, status, oauth_url, redirect_url)
           VALUES ($1, $2, 'INITIATED', $3, $4)
           ON CONFLICT (user_id, connector_id) DO UPDATE SET
             status = 'INITIATED', oauth_url = EXCLUDED.oauth_url, redirect_url = EXCLUDED.redirect_url
           RETURNING *"#,
    )
    .bind(user_id)
    .bind(connector_id)
    .bind(oauth_url)
    .bind(redirect_url)
    .fetch_one(db)
    .await?;
    Ok(row)
}

pub async fn get_connection_by_account_id(
    db: &PgPool,
    account_id: &str,
) -> Result<Option<McpUserConnection>> {
    let row = sqlx::query_as::<_, McpUserConnection>(
        "SELECT * FROM mcp_user_connections WHERE connected_account_id = $1 ORDER BY created_at DESC LIMIT 1",
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
) -> Result<Option<McpUserConnection>> {
    let row = sqlx::query_as::<_, McpUserConnection>(
        "UPDATE mcp_user_connections SET status = $2 WHERE id = $1 RETURNING *",
    )
    .bind(id)
    .bind(status)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

pub async fn update_connection_account_id(db: &PgPool, id: Uuid, account_id: &str) -> Result<()> {
    sqlx::query("UPDATE mcp_user_connections SET connected_account_id = $2 WHERE id = $1")
        .bind(id)
        .bind(account_id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn delete_user_connection(db: &PgPool, user_id: Uuid, connector_id: Uuid) -> Result<bool> {
    let res = sqlx::query("DELETE FROM mcp_user_connections WHERE user_id = $1 AND connector_id = $2")
        .bind(user_id)
        .bind(connector_id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}

// ─── Tool catalog ─────────────────────────────────────────────────────────────

/// Replace a connector's synced tool catalog with `tools` (name, description).
pub async fn upsert_connector_tools(
    db: &PgPool,
    connector_id: Uuid,
    tools: &[(String, Option<String>)],
) -> Result<()> {
    let mut tx = db.begin().await?;
    for (name, desc) in tools {
        sqlx::query(
            r#"INSERT INTO mcp_connector_tools (connector_id, tool_name, description, last_synced_at)
               VALUES ($1, $2, $3, now())
               ON CONFLICT (connector_id, tool_name) DO UPDATE SET
                 description = EXCLUDED.description, last_synced_at = now()"#,
        )
        .bind(connector_id)
        .bind(name)
        .bind(desc)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn list_connector_tools(db: &PgPool, connector_id: Uuid) -> Result<Vec<McpConnectorTool>> {
    let rows = sqlx::query_as::<_, McpConnectorTool>(
        "SELECT * FROM mcp_connector_tools WHERE connector_id = $1 ORDER BY tool_name",
    )
    .bind(connector_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

// ─── Composio sessions ──────────────────────────────────────────────────────

pub async fn get_composio_session(db: &PgPool, user_id: Uuid) -> Result<Option<McpComposioSession>> {
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
) -> Result<McpComposioSession> {
    let row = sqlx::query_as::<_, McpComposioSession>(
        r#"INSERT INTO mcp_composio_sessions (user_id, composio_session_id)
           VALUES ($1, $2)
           ON CONFLICT (user_id) DO UPDATE SET composio_session_id = EXCLUDED.composio_session_id
           RETURNING *"#,
    )
    .bind(user_id)
    .bind(session_id)
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

// ─── Per-agent connector access ─────────────────────────────────────────────

pub async fn get_agent_connector_access(
    db: &PgPool,
    user_id: Uuid,
    agent_id: Uuid,
) -> Result<Vec<McpAgentConnectorAccess>> {
    let rows = sqlx::query_as::<_, McpAgentConnectorAccess>(
        "SELECT * FROM mcp_agent_connector_access WHERE user_id = $1 AND agent_id = $2",
    )
    .bind(user_id)
    .bind(agent_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

pub async fn get_agent_connector_access_row(
    db: &PgPool,
    user_id: Uuid,
    agent_id: Uuid,
    connector_id: Uuid,
) -> Result<Option<McpAgentConnectorAccess>> {
    let row = sqlx::query_as::<_, McpAgentConnectorAccess>(
        "SELECT * FROM mcp_agent_connector_access WHERE user_id = $1 AND agent_id = $2 AND connector_id = $3",
    )
    .bind(user_id)
    .bind(agent_id)
    .bind(connector_id)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

pub async fn upsert_agent_connector_access(
    db: &PgPool,
    user_id: Uuid,
    agent_id: Uuid,
    connector_id: Uuid,
    enabled: bool,
    tool_rules: &Value,
) -> Result<McpAgentConnectorAccess> {
    let row = sqlx::query_as::<_, McpAgentConnectorAccess>(
        r#"INSERT INTO mcp_agent_connector_access (user_id, agent_id, connector_id, enabled, tool_rules)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (user_id, agent_id, connector_id) DO UPDATE SET
             enabled = EXCLUDED.enabled, tool_rules = EXCLUDED.tool_rules
           RETURNING *"#,
    )
    .bind(user_id)
    .bind(agent_id)
    .bind(connector_id)
    .bind(enabled)
    .bind(tool_rules)
    .fetch_one(db)
    .await?;
    Ok(row)
}

/// Reset an agent to default (all-allowed) — delete all its access rows.
pub async fn delete_all_agent_access(db: &PgPool, user_id: Uuid, agent_id: Uuid) -> Result<u64> {
    let res = sqlx::query("DELETE FROM mcp_agent_connector_access WHERE user_id = $1 AND agent_id = $2")
        .bind(user_id)
        .bind(agent_id)
        .execute(db)
        .await?;
    Ok(res.rows_affected())
}

/// Distinct (user_id, agent_id) pairs with access rows for a connector — used to
/// invalidate permission caches before the connector is deleted.
pub async fn get_agent_pairs_for_connector(
    db: &PgPool,
    connector_id: Uuid,
) -> Result<Vec<(Uuid, Uuid)>> {
    let pairs = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT user_id, agent_id FROM mcp_agent_connector_access WHERE connector_id = $1",
    )
    .bind(connector_id)
    .fetch_all(db)
    .await?;
    Ok(pairs)
}
