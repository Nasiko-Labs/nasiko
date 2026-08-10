//! Resource usage (CPU / memory / disk), at two different scopes.
//!
//! - `GET /resources` — the whole box. **Admin-only**: it necessarily reveals the
//!   deployment's shape (which services run) and the host's size.
//! - `GET /agent/{agent_ref}/resources` — one agent. **Authenticated + agent
//!   ACL**, so an agent's owner can see their own agent's usage.
//!
//! The split is the point. An owner has a legitimate need to know whether their
//! agent is starved or leaking, but no need to learn what else shares the host —
//! so the narrow endpoint returns one container's row and nothing else, rather
//! than filtering the admin payload down after the fact.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use nasiko_config::Config;
use nasiko_runtime::{
    AgentNameResolver, ContainerStats, DiskSource, HostStats, ResourceStatsProvider, StatsGroup,
};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::Claims;
use crate::state::AppState;

/// Resolves agent UUIDs to their registered names.
///
/// Without this the UI lists `nasiko-agent-6e05532a-b1ed-…`, which tells an
/// admin nothing about which agent is eating the box.
struct DbAgentNames {
    db: PgPool,
}

#[async_trait]
impl AgentNameResolver for DbAgentNames {
    async fn resolve(&self, agent_ids: &[String]) -> HashMap<String, String> {
        // Container names carry the UUID, but a container can outlive its row
        // (or predate a rename), so parse defensively and skip what will not
        // parse rather than failing the whole lookup.
        let uuids: Vec<Uuid> = agent_ids.iter().filter_map(|s| s.parse().ok()).collect();
        if uuids.is_empty() {
            return HashMap::new();
        }
        let rows = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, name FROM agents WHERE id = ANY($1) AND deleted_at IS NULL",
        )
        .bind(&uuids)
        .fetch_all(&self.db)
        .await
        .unwrap_or_default();

        rows.into_iter()
            .map(|(id, name)| (id.to_string(), name))
            .collect()
    }
}

/// Builds the provider for the configured runtime.
///
/// Docker is the default for the Compose topology (the `_` arm mirrors the
/// runtime selection in the composition roots). `kubernetes` and `simulated` get
/// a provider that reports honestly that it cannot read usage — EE replaces the
/// Kubernetes case with its own.
pub fn build_provider(config: &Config, db: PgPool) -> Arc<dyn ResourceStatsProvider> {
    match config.agent_runtime.as_str() {
        "kubernetes" | "k8s" | "simulated" => Arc::new(nasiko_runtime::UnsupportedStatsProvider {
            runtime: config.agent_runtime.clone(),
        }),
        other => match nasiko_runtime::DockerStatsProvider::connect() {
            Ok(p) => Arc::new(p.with_agent_names(Arc::new(DbAgentNames { db }))),
            Err(e) => {
                tracing::warn!(error = %e, "resource stats unavailable: cannot reach Docker");
                Arc::new(nasiko_runtime::UnsupportedStatsProvider {
                    runtime: other.to_owned(),
                })
            }
        },
    }
}

/// Containers split into the three groups the UI renders as separate cards.
#[derive(Debug, Serialize)]
pub struct ResourceGroups {
    pub control_plane: Vec<ContainerStats>,
    pub agent_runtime: Vec<ContainerStats>,
    pub infra: Vec<ContainerStats>,
}

#[derive(Debug, Serialize)]
pub struct ResourcesPayload {
    pub host: HostStats,
    pub groups: ResourceGroups,
    pub disk_source: DiskSource,
    pub collected_at: String,
}

#[derive(Debug, Serialize)]
pub struct ResourcesEnvelope {
    pub data: ResourcesPayload,
}

#[derive(Debug, Serialize)]
pub struct AgentResourcesPayload {
    pub agent_id: String,
    pub agent_name: String,
    /// `None` when the agent has no container right now (scaled to zero, or never
    /// deployed) — a normal state, not an error.
    pub usage: Option<ContainerStats>,
    pub collected_at: String,
}

#[derive(Debug, Serialize)]
pub struct AgentResourcesEnvelope {
    pub data: AgentResourcesPayload,
}

/// `GET /api/observability/agent/{agent_ref}/resources`
///
/// Owner-scoped counterpart to [`get_resources`]. Authenticated rather than
/// admin-gated, then narrowed by the same agent ACL every other agent-scoped
/// route uses, so an owner sees their own agent's usage and nothing else: no host
/// totals, no other containers, no hint of what else runs on the box.
///
/// `agent_ref` accepts a UUID or an agent name, matching the other observability
/// routes.
pub async fn get_agent_resources(
    State(state): State<AppState>,
    claims: Claims,
    axum::extract::Path(agent_ref): axum::extract::Path<String>,
) -> Response {
    let Some((agent_id, agent_name)) = super::routes::resolve_agent(&state.db, &agent_ref).await
    else {
        return (StatusCode::NOT_FOUND, "agent not found").into_response();
    };

    // `can_access_agent` (not the OSS-only free function) so EE team/department
    // grants are honoured and the superuser short-circuit applies.
    if !crate::acl::can_access_agent(&state, &claims, agent_id).await {
        return (StatusCode::FORBIDDEN, "no access to this agent").into_response();
    }

    match state
        .resource_stats
        .agent_stats(&agent_id.to_string())
        .await
    {
        Ok(usage) => Json(AgentResourcesEnvelope {
            data: AgentResourcesPayload {
                agent_id: agent_id.to_string(),
                agent_name,
                usage,
                collected_at: chrono::Utc::now().to_rfc3339(),
            },
        })
        .into_response(),
        Err(e) => (StatusCode::SERVICE_UNAVAILABLE, e.to_string()).into_response(),
    }
}

/// `GET /api/observability/resources`
pub async fn get_resources(State(state): State<AppState>, _claims: Claims) -> Response {
    let stats = match state.resource_stats.platform_stats().await {
        Ok(s) => s,
        // The provider distinguishes "cannot read here" from "read failed", but
        // both leave the admin with no data, so both surface as 503 with the
        // reason attached rather than an empty 200 that looks like an idle box.
        Err(e) => {
            return (StatusCode::SERVICE_UNAVAILABLE, e.to_string()).into_response();
        }
    };

    let mut groups = ResourceGroups {
        control_plane: Vec::new(),
        agent_runtime: Vec::new(),
        infra: Vec::new(),
    };
    for c in stats.containers {
        match c.group {
            StatsGroup::ControlPlane => groups.control_plane.push(c),
            StatsGroup::AgentRuntime => groups.agent_runtime.push(c),
            StatsGroup::Infra => groups.infra.push(c),
        }
    }
    // Heaviest first: the reason to open this page is to find what is consuming
    // the box, and an unsorted list buries it. `total_cmp` because these are
    // f64s that may be NaN-free but are not `Ord`.
    for list in [
        &mut groups.control_plane,
        &mut groups.agent_runtime,
        &mut groups.infra,
    ] {
        list.sort_by(|a, b| {
            b.cpu_percent
                .unwrap_or(0.0)
                .total_cmp(&a.cpu_percent.unwrap_or(0.0))
        });
    }

    Json(ResourcesEnvelope {
        data: ResourcesPayload {
            host: stats.host,
            groups,
            disk_source: stats.disk_source,
            collected_at: chrono::Utc::now().to_rfc3339(),
        },
    })
    .into_response()
}
