//! OpenAPI spec for the `/api/mcp/*` route group.
//!
//! Kept as its own `OpenApi` derive (rather than adding these paths to the
//! main `crate::openapi::ApiDoc`) so the MCP surface stays self-contained;
//! `crate::openapi` merges [`McpApiDoc`] into the served spec the same way
//! `ee/server/src/openapi.rs` merges via `spec.merge(...)`.

use utoipa::OpenApi;

use super::handlers;

#[derive(OpenApi)]
#[openapi(
    paths(
        handlers::gateway::mcp_gateway,
        handlers::upload::upload_zip,
        handlers::upload::upload_github,
        handlers::upload::build_status,
        handlers::upload::build_logs,
        handlers::upload::list_my_uploads,
        handlers::catalog::get_catalog,
        handlers::catalog::list_toolkits,
        handlers::catalog::create_auth_config,
        handlers::catalog::list_auth_configs,
        handlers::catalog::update_auth_config,
        handlers::catalog::delete_auth_config,
        handlers::connect::connect_service,
        handlers::connect::list_connections,
        handlers::connect::disconnect,
        handlers::connectors::create,
        handlers::connectors::list,
        handlers::connectors::get,
        handlers::connectors::update,
        handlers::connectors::delete,
        handlers::connectors::probe,
        handlers::connectors::pin,
        handlers::connectors::unpin,
        handlers::connectors::pinned,
        handlers::connectors::recent,
        handlers::sharing::list,
        handlers::sharing::grant_public,
        handlers::sharing::revoke_public,
        handlers::sharing::grant_user,
        handlers::sharing::revoke_user,
        handlers::sharing::grant_agent,
        handlers::sharing::revoke_agent,
        handlers::sharing::search_targets,
        handlers::sharing::resolve_target,
        handlers::sharing::consumers,
        handlers::credentials::register,
        handlers::credentials::status,
        handlers::credentials::delete,
        handlers::oauth::authorize,
        handlers::oauth::callback,
        handlers::oauth::status,
        handlers::oauth::revoke,
        handlers::permissions::list_connectors,
        handlers::permissions::set_connector_access,
        handlers::permissions::list_connector_tools,
        handlers::permissions::list_tool_rules,
        handlers::permissions::bulk_update_tools,
        handlers::permissions::reset,
    ),
    components(schemas(
        McpEnvelope,
        handlers::upload::UploadZipForm,
        handlers::upload::UploadFromGithub,
        handlers::upload::UploadedConnectorsListResponse,
        handlers::upload::UploadedConnectorResponse,
        handlers::upload::UploadInfoMcp,
        handlers::catalog::CreateAuthConfig,
        handlers::catalog::UpdateAuthConfig,
        handlers::connect::ConnectRequest,
        handlers::connect::Credentials,
        handlers::connectors::CreateConnector,
        handlers::connectors::UpdateConnector,
        handlers::connectors::ProbeRequest,
        handlers::credentials::RegisterCredential,
        handlers::oauth::AuthorizeRequest,
        handlers::permissions::SetConnectorAccess,
        handlers::permissions::ToolRule,
        handlers::permissions::BulkToolUpdate,
    )),
    tags(
        (name = "mcp", description = "MCP gateway: agent-facing JSON-RPC tool calls, connector registration/upload/sharing, credentials & OAuth, and per-agent tool permissions"),
    ),
)]
#[allow(dead_code)] // constructed only via `OpenApi::openapi()` in `crate::openapi::merged_spec`
pub struct McpApiDoc;

/// Doc-only schema for the standard MCP `ApiResponse` envelope
/// (`{"data": …, "status_code": N, "message": "…"}`). Most `/api/mcp/*`
/// payloads are service-layer `serde_json::Value` views, so `data` is
/// documented as a free-form object; each route's response description says
/// what it carries.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[allow(dead_code)]
pub struct McpEnvelope {
    #[schema(value_type = Object, nullable)]
    pub data: Option<serde_json::Value>,
    #[schema(example = 200)]
    pub status_code: u16,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_api_doc_builds_with_paths_and_schemas() {
        let spec = McpApiDoc::openapi();
        assert!(spec.paths.paths.contains_key("/api/mcp"));
        assert!(spec.paths.paths.contains_key("/api/mcp/connectors/{id}"));
        assert!(
            spec.components
                .as_ref()
                .is_some_and(|c| c.schemas.contains_key("McpEnvelope"))
        );
    }
}
