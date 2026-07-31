//! OpenAPI spec + Swagger UI for the control plane's `/api/*` routes.
//!
//! Only routes annotated with `#[utoipa::path(...)]` (and DTOs annotated with
//! `#[derive(ToSchema)]`) show up here — this is an opt-in, incremental
//! rollout, not a scan of every handler in the app. `secrets` is the first
//! module done end-to-end; use it as the template for the rest (see
//! `secrets::routes` for the annotation pattern). EE route modules add their
//! own `utoipa::OpenApi` derive that nests this one, mirroring how
//! `build_ee_app()` wraps `build_app_with_user_router()`.
use axum::Router;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::state::AppState;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Nasiko Control Plane API",
        description = "A2A orchestration control plane: agent lifecycle, routing, observability, and secrets.",
        version = "0.1.0"
    ),
    paths(
        crate::secrets::routes::list_secrets,
        crate::secrets::routes::create_secret,
        crate::secrets::routes::get_secret,
        crate::secrets::routes::update_secret,
        crate::secrets::routes::delete_secret,
        crate::catalog::routes::by_skill,
        crate::catalog::routes::create,
        crate::catalog::routes::list,
        crate::catalog::routes::get_one,
        crate::catalog::routes::update,
        crate::catalog::routes::delete,
        crate::catalog::routes::list_versions,
        crate::catalog::routes::delete_version,
        crate::catalog::routes::search,
        crate::catalog::routes::search_users,
        crate::catalog::routes::registry_user_agents,
        crate::catalog::routes::get_by_registry_id,
        crate::catalog::agent_secrets::list_secrets,
        crate::catalog::agent_secrets::set_secret,
        crate::catalog::agent_secrets::delete_secret,
        crate::catalog::agent_secrets::import_secrets,
        crate::catalog::import::import_upload,
        crate::catalog::import::import_github,
        crate::catalog::import::import_registry,
        crate::agents::deployments::dev_env,
        crate::agents::deployments::list_deployments,
        crate::agents::deployments::get_agent_deployment,
        crate::agents::deployments::restart_deployment,
        crate::agents::llm_config::get_llm_config,
        crate::agents::llm_config::update_llm_config,
        crate::agents::llm_config::delete_llm_config,
        crate::agents::update::update_agent,
        crate::agents::update::rollback_agent,
        crate::agents::upload::upload_and_deploy,
        crate::agents::upload::deploy_status_sse,
        crate::agents::upload::get_upload_status,
        crate::agents::upload::list_upload_status,
        crate::agents::upload::list_upload_agents,
        crate::router::a2a_dispatch::a2a_dispatch_handler,
        crate::router::a2a_dispatch::a2a_upload_handler,
        crate::router::a2a_dispatch::router_stats_handler,
        crate::users::routes::list_users,
        crate::users::routes::get_user,
        crate::users::routes::create_user,
        crate::users::routes::update_user,
        crate::users::routes::delete_user,
        crate::users::routes::deactivate,
        crate::users::routes::reinstate,
        crate::users::routes::regenerate_credentials,
        crate::users::routes::change_role,
        crate::users::routes::list_admins,
        crate::users::routes::accessible_agents_for_user,
        crate::users::routes::my_accessible_agents,
        crate::users::routes::get_me,
        crate::usage::routes::summary,
        crate::usage::routes::history,
        crate::usage::routes::by_agent,
        crate::usage::routes::by_model,
        crate::observability::handler::get_all_sessions,
        crate::observability::handler::get_session_details,
        crate::observability::handler::get_trace_details,
        crate::observability::handler::get_span_details,
        crate::observability::handler::get_agent_stats,
        crate::observability::handler::get_finops_dashboard,
        crate::observability::handler::get_finops_insights,
        crate::observability::handler::get_agent_hours,
        crate::observability::routes::agent_logs,
        crate::observability::routes::agent_logs_stream,
    ),
    components(schemas(
        crate::secrets::routes::SecretEntry,
        crate::secrets::routes::CreateSecret,
        crate::secrets::routes::UpdateSecret,
        crate::secrets::routes::SecretValue,
        crate::catalog::models::Agent,
        crate::catalog::models::Skill,
        crate::catalog::models::AgentSummary,
        crate::catalog::models::CreateAgent,
        crate::catalog::models::UpdateAgent,
        crate::catalog::models::AgentVersion,
        crate::catalog::routes::SingleResponse,
        crate::catalog::routes::AgentDetailResponse,
        crate::catalog::routes::DeletedAgent,
        crate::catalog::routes::AgentSearchResponse,
        crate::catalog::routes::AgentSearchResult,
        crate::catalog::routes::UserSearchResponse,
        crate::catalog::routes::UserSearchResult,
        crate::catalog::routes::RegistryUserAgentsResponse,
        crate::catalog::routes::RegistryUserAgentSummary,
        crate::catalog::agent_secrets::SecretListEntry,
        crate::catalog::agent_secrets::SetSecretRequest,
        crate::catalog::agent_secrets::ImportSecretsRequest,
        crate::catalog::agent_secrets::ImportSecretsResponse,
        crate::catalog::import::ImportResult,
        crate::catalog::import::ImportUploadForm,
        crate::catalog::import::GithubImportRequest,
        crate::catalog::import::RegistryImportRequest,
        crate::agents::deployments::DevEnvResponse,
        crate::agents::deployments::DeploymentRow,
        crate::agents::deployments::RestartDeploymentResponse,
        crate::agents::llm_config::LlmConfigResponse,
        crate::agents::llm_config::LlmConfigEnvelope,
        crate::agents::llm_config::AttachLlmConfigRequest,
        crate::agents::llm_config::LlmConfigUpdateResponse,
        crate::agents::llm_config::LlmConfigUpdateEnvelope,
        crate::agents::update::UpdateAgentResponse,
        crate::agents::update::UpdateAgentForm,
        crate::agents::update::RollbackRequest,
        crate::agents::update::RollbackResponse,
        crate::agents::upload::UploadAndDeployForm,
        crate::agents::upload::UploadAndDeployResponse,
        crate::agents::upload::UploadQueuedData,
        crate::agents::upload::UploadStatusItem,
        crate::agents::upload::SourceInfoJson,
        crate::agents::upload::UploadStatusListResponse,
        crate::agents::upload::UploadAgentsListResponse,
        crate::agents::upload::UploadAgentResponse,
        crate::agents::upload::UploadInfoResponse,
        crate::router::a2a_dispatch::A2aJsonRpcRequest,
        crate::router::a2a_dispatch::A2aUploadForm,
        crate::router::a2a_dispatch::JsonRpcErrorBody,
        crate::router::a2a_dispatch::JsonRpcErrorResponse,
        crate::router::a2a_dispatch::RouterStatsRow,
        crate::router::a2a_dispatch::RouterStatsResponse,
        crate::users::routes::UserRow,
        crate::users::routes::UserListResponse,
        crate::users::routes::CreateUser,
        crate::users::routes::CreateUserResponse,
        crate::users::routes::UpdateUser,
        crate::users::routes::ChangeRoleRequest,
        crate::users::routes::RegenerateCredentialsResponse,
        crate::users::routes::AdminUser,
        crate::users::routes::AdminListResponse,
        crate::users::routes::AccessibleAgent,
        crate::users::routes::AccessibleAgentsResponse,
        crate::usage::routes::UsageSummaryResponse,
        crate::usage::routes::DailyUsage,
        crate::usage::routes::AgentUsage,
        crate::usage::routes::AgentUsagePage,
        crate::usage::routes::ModelUsage,
        crate::usage::routes::ModelUsagePage,
        crate::observability::service::SessionListResponse,
        crate::observability::service::SessionListData,
        crate::observability::service::SessionSummary,
        crate::observability::service::TokenUsageSummary,
        crate::observability::service::SimpleCostSummary,
        crate::observability::service::CostEntry,
        crate::observability::service::Pagination,
        crate::observability::service::SessionDetailResponse,
        crate::observability::service::SessionDetailData,
        crate::observability::service::SessionDetail,
        crate::observability::service::FullCostSummary,
        crate::observability::service::CostWithTokens,
        crate::observability::service::TraceEntry,
        crate::observability::service::RootSpanEntry,
        crate::observability::service::ProjectRef,
        crate::observability::service::ContentField,
        crate::observability::service::TraceRef,
        crate::observability::service::TraceDetailResponse,
        crate::observability::service::TraceDetailData,
        crate::observability::service::TraceDetail,
        crate::observability::service::NestedCostSummary,
        crate::observability::service::CostOnly,
        crate::observability::service::RootSpansWrapper,
        crate::observability::service::RootSpanEdge,
        crate::observability::service::RootSpanRef,
        crate::observability::service::SpanNode,
        crate::observability::service::SpanDetailResponse,
        crate::observability::service::SpanDetailData,
        crate::observability::service::SpanTraceRef,
        crate::observability::service::SpanProjectRef,
        crate::observability::service::SpanDetail,
        crate::observability::service::AgentStatsResponse,
        crate::observability::service::AgentStatsData,
        crate::observability::service::AgentProjectStats,
        crate::observability::service::FinopsDashboardResponse,
        crate::observability::service::FinopsDashboardData,
        crate::observability::service::FinopsSummary,
        crate::observability::service::AgentFinopsRow,
        crate::observability::service::FinopsTokenUsage,
        crate::observability::service::InsightsRequest,
        crate::observability::service::InsightsResponse,
        crate::observability::service::AgentHoursResponse,
        crate::observability::service::AgentHoursData,
        crate::observability::service::AgentHoursWindow,
        crate::observability::service::AgentHoursRow,
        crate::observability::service::AgentHoursBucket,
        crate::observability::service::AgentHoursBucketAgent,
        crate::observability::logs::LogLine,
        EmptyEnvelope,
    )),
    tags(
        (name = "secrets", description = "Encrypted per-user agent secrets"),
        (name = "catalog", description = "Agent catalog: registration, discovery, versions, per-agent secrets, and source import"),
        (name = "agents", description = "Agent lifecycle: deployments, LLM routing config, update/rollback, upload-and-deploy"),
        (name = "orchestrator", description = "A2A dispatch: routing-engine/ReAct orchestrator and direct agent chat, plus routing stats"),
        (name = "users", description = "User management: CRUD, roles, credentials, accessible agents (superuser-only)"),
        (name = "usage", description = "Per-user token usage and cost reporting"),
        (name = "observability", description = "Sessions, traces, spans, agent logs, and FinOps reporting"),
    ),
)]
pub struct ApiDoc;

/// Doc-only schema for the standard `ApiResponse` envelope when a handler has
/// no payload to return (`ApiResponse::ok(json!(null), …)`): `data` is always
/// JSON null, so only `status_code` and `message` carry information.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[allow(dead_code)]
pub struct EmptyEnvelope {
    #[schema(value_type = Object, nullable)]
    pub data: Option<serde_json::Value>,
    #[schema(example = 200)]
    pub status_code: u16,
    pub message: String,
}

/// Swagger UI at `/api/docs`, raw spec at `/api/openapi.json`. Unauthenticated
/// like `/health` — it only describes route shapes and schemas, no live data.
pub fn router() -> Router<AppState> {
    Router::new().merge(SwaggerUi::new("/api/docs").url("/api/openapi.json", merged_spec()))
}

/// The full OSS spec: the incrementally-annotated `ApiDoc` plus the MCP
/// group's self-contained `McpApiDoc` (kept separate so the mcp module owns
/// its own registration).
pub fn merged_spec() -> utoipa::openapi::OpenApi {
    let mut spec = ApiDoc::openapi();
    spec.merge(crate::mcp::openapi::McpApiDoc::openapi());
    spec
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Building the spec walks every registered schema; a self-referential
    /// schema without `#[schema(no_recursion)]` overflows the stack here
    /// instead of at server startup.
    #[test]
    fn full_spec_builds() {
        let spec = merged_spec();
        assert!(!spec.paths.paths.is_empty());
        let json = serde_json::to_string(&spec).expect("spec serializes");
        assert!(json.contains("/api/observability/session/list"));
        assert!(json.contains("/api/mcp/connectors"));
        assert!(json.contains("/api/usage/summary"));
    }
}
