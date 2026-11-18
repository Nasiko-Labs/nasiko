//! Registration of A2A agents the CLI found running on a user's machine.
//!
//! `nasiko agents discover` reports every agent it finds, with its card, and `POST` makes the
//! catalog match: new agents are added, known ones get their card refreshed, and agents the
//! machine no longer reports are marked `stopped`. `GET` reads that back for `nasiko status`,
//! which shows what the last discovery found without probing anything itself. Nasiko never
//! deploys or calls these agents (their status is never `running`); they appear in the
//! catalog and may route their LLM calls through the router, which reports when each last did.

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::agents::{image_name_slug, utils::apply_agent_card};
use crate::auth::Claims;
use crate::state::AppState;

/// Longest machine name or card name accepted; both end up in indexed columns.
const MAX_NAME_CHARS: usize = 255;
/// Longest copy id accepted. The CLI sends a 16-character hash.
const MAX_INSTANCE_CHARS: usize = 64;
/// Most agents one machine's report may carry; each becomes a catalog row.
const MAX_AGENTS_PER_REPORT: usize = 100;

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/agents/discovered",
        get(list_discovered_agents).post(register_discovered_agents),
    )
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct RegisterDiscoveredRequest {
    /// The machine the agents run on, as the CLI names it (its hostname).
    machine: String,
    /// Every agent currently running there. Agents of this machine left out are marked stopped.
    agents: Vec<DiscoveredAgentInput>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct DiscoveredAgentInput {
    /// The agent card as the agent serves it.
    card: serde_json::Value,
    /// Which copy of the agent this is, when the machine runs it more than once: an opaque id
    /// that stays the same across restarts (the CLI hashes the folder or container serving
    /// it). Two copies with one card name are two agents; without it, the card name alone
    /// identifies the agent on its machine.
    #[serde(default)]
    instance: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct RegisterDiscoveredResponse {
    agents: Vec<RegisteredAgent>,
    /// How many of this machine's agents were marked stopped by this call.
    stopped: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct RegisteredAgent {
    /// The card's `name`, as sent.
    card_name: String,
    /// The copy id, as sent.
    instance: Option<String>,
    id: Uuid,
    /// The catalog name: the card name's slug, or the slug and machine when the slug is taken.
    /// A second copy on one machine also carries the start of its copy id (`planner-3fa2`).
    name: String,
    created: bool,
    /// When the router last handled an LLM call made with this agent's key; `null` if never.
    last_llm_call_at: Option<DateTime<Utc>>,
}

#[utoipa::path(
    post,
    path = "/api/agents/discovered",
    tag = "catalog",
    request_body = RegisterDiscoveredRequest,
    responses(
        (status = 200, description = "Catalog updated to the machine's running agents", body = RegisterDiscoveredResponse),
        (status = 400, description = "Missing machine, more than 100 agents, or a card without a usable name"),
        (status = 409, description = "No free catalog name for an agent"),
    ),
)]
pub(crate) async fn register_discovered_agents(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<RegisterDiscoveredRequest>,
) -> impl IntoResponse {
    let owner_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let machine = body.machine.trim();
    if machine.is_empty() || machine.chars().count() > MAX_NAME_CHARS {
        return (StatusCode::BAD_REQUEST, "machine must be 1-255 characters").into_response();
    }
    if body.agents.len() > MAX_AGENTS_PER_REPORT {
        return (
            StatusCode::BAD_REQUEST,
            format!("a report may carry at most {MAX_AGENTS_PER_REPORT} agents"),
        )
            .into_response();
    }
    let mut cards = Vec::with_capacity(body.agents.len());
    for (index, agent) in body.agents.into_iter().enumerate() {
        let Some(name) = card_name(&agent.card) else {
            return (
                StatusCode::BAD_REQUEST,
                format!("agents[{index}].card.name must be a 1-255 character string"),
            )
                .into_response();
        };
        let Ok(instance) = instance(agent.instance) else {
            return (
                StatusCode::BAD_REQUEST,
                format!("agents[{index}].instance must be 1-64 letters, digits, '-' or '_'"),
            )
                .into_response();
        };
        cards.push(ReportedAgent {
            card_name: name,
            instance,
            card: agent.card,
        });
    }

    match register(&state.db, owner_id, machine, &cards).await {
        Ok(Ok(response)) => (StatusCode::OK, Json(response)).into_response(),
        Ok(Err(card_name)) => (
            StatusCode::CONFLICT,
            format!("no free catalog name for agent '{card_name}'"),
        )
            .into_response(),
        Err(error) => {
            tracing::error!(%error, %owner_id, %machine, "discovered-agent registration failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub(crate) struct ListDiscoveredQuery {
    /// The machine whose agents to list, as the CLI names it (its hostname).
    machine: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ListDiscoveredResponse {
    agents: Vec<DiscoveredAgentEntry>,
}

#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
pub(crate) struct DiscoveredAgentEntry {
    /// The card's `name`, as last reported.
    card_name: String,
    /// The copy id the CLI reported it with; `null` when it reported none.
    instance: Option<String>,
    id: Uuid,
    /// The catalog name.
    name: String,
    /// `registered` while the machine's last report included it, `stopped` once one did not.
    status: String,
    /// When a report last changed it: its last registration, or when it was marked stopped.
    updated_at: DateTime<Utc>,
    /// When the router last handled an LLM call made with this agent's key; `null` if never.
    last_llm_call_at: Option<DateTime<Utc>>,
}

#[utoipa::path(
    get,
    path = "/api/agents/discovered",
    tag = "catalog",
    params(ListDiscoveredQuery),
    responses(
        (status = 200, description = "The caller's agents last reported from this machine", body = ListDiscoveredResponse),
        (status = 400, description = "Missing machine"),
    ),
)]
pub(crate) async fn list_discovered_agents(
    State(state): State<AppState>,
    claims: Claims,
    Query(query): Query<ListDiscoveredQuery>,
) -> impl IntoResponse {
    let owner_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let machine = query.machine.trim();
    if machine.is_empty() || machine.chars().count() > MAX_NAME_CHARS {
        return (StatusCode::BAD_REQUEST, "machine must be 1-255 characters").into_response();
    }
    let agents = sqlx::query_as::<_, DiscoveredAgentEntry>(
        "SELECT a.metadata->>'card_name' AS card_name, a.metadata->>'instance' AS instance, a.id, a.name, a.status, a.updated_at, \
                (SELECT max(t.started_at) FROM trace_usage t WHERE t.agent_id = a.id) \
                    AS last_llm_call_at \
         FROM agents a \
         WHERE a.owner_id = $1 AND a.discovered_on = $2 AND a.deleted_at IS NULL \
         ORDER BY a.name",
    )
    .bind(owner_id)
    .bind(machine)
    .fetch_all(&state.db)
    .await;
    match agents {
        Ok(agents) => (StatusCode::OK, Json(ListDiscoveredResponse { agents })).into_response(),
        Err(error) => {
            tracing::error!(%error, %owner_id, %machine, "discovered-agent listing failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// The card's trimmed `name`, when it is a usable one.
fn card_name(card: &serde_json::Value) -> Option<String> {
    let name = card.get("name")?.as_str()?.trim();
    let usable = !name.is_empty() && name.chars().count() <= MAX_NAME_CHARS;
    usable.then(|| name.to_string())
}

/// A reported copy id: absent, or 1-64 characters a catalog name can carry.
fn instance(reported: Option<String>) -> Result<Option<String>, ()> {
    let Some(instance) = reported else {
        return Ok(None);
    };
    let usable = (1..=MAX_INSTANCE_CHARS).contains(&instance.len())
        && instance
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if usable { Ok(Some(instance)) } else { Err(()) }
}

/// One agent of a report.
struct ReportedAgent {
    card_name: String,
    instance: Option<String>,
    card: serde_json::Value,
}

/// Catalog names to try for an agent, in order: the card name's slug, then the slug with the
/// machine, so the same agent on a second machine still gets a name. A second copy on one
/// machine (`sibling`) skips the plain slug and carries the start of its copy id, so the two
/// can be told apart by name.
fn candidate_names(
    card_name: &str,
    machine: &str,
    instance: Option<&str>,
    sibling: bool,
) -> Vec<String> {
    let slug = image_name_slug(card_name);
    let with_machine = format!("{slug}-{}", image_name_slug(machine));
    match instance.filter(|_| sibling) {
        None => vec![slug, with_machine],
        Some(instance) => {
            let tag = image_name_slug(instance);
            let short: String = tag.chars().take(4).collect();
            vec![
                format!("{slug}-{short}"),
                format!("{slug}-{tag}"),
                format!("{with_machine}-{tag}"),
            ]
        }
    }
}

/// Upsert every reported agent and mark the machine's others stopped, in one transaction.
/// `Ok(Err(card_name))` when an agent could not be given any catalog name.
async fn register(
    db: &sqlx::PgPool,
    owner_id: Uuid,
    machine: &str,
    cards: &[ReportedAgent],
) -> Result<Result<RegisterDiscoveredResponse, String>, sqlx::Error> {
    let mut tx = db.begin().await?;
    let mut registered = Vec::with_capacity(cards.len());
    for agent in cards {
        match upsert(&mut tx, owner_id, machine, agent).await? {
            Some((id, name, created)) => registered.push((agent, id, name, created)),
            None => return Ok(Err(agent.card_name.clone())),
        }
    }
    let ids: Vec<Uuid> = registered.iter().map(|(_, id, _, _)| *id).collect();
    let stopped = sqlx::query(
        "UPDATE agents SET status = 'stopped', updated_at = now() \
         WHERE owner_id = $1 AND discovered_on = $2 AND deleted_at IS NULL \
           AND NOT (id = ANY($3)) AND status <> 'stopped'",
    )
    .bind(owner_id)
    .bind(machine)
    .bind(&ids)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;

    // Card fields are written through the same path as a deployed agent's card. A failure
    // there leaves the row registered with its previous card, which the next report retries.
    for (agent, id) in cards.iter().zip(&ids) {
        apply_agent_card(db, *id, &agent.card).await;
    }

    let last_calls = last_llm_calls(db, &ids).await?;
    let agents = registered
        .into_iter()
        .map(|(agent, id, name, created)| RegisteredAgent {
            card_name: agent.card_name.clone(),
            instance: agent.instance.clone(),
            id,
            name,
            created,
            last_llm_call_at: last_calls
                .iter()
                .find(|(agent_id, _)| *agent_id == id)
                .map(|(_, at)| *at),
        })
        .collect();
    Ok(Ok(RegisterDiscoveredResponse { agents, stopped }))
}

/// The agent's id and catalog name, inserting it when this machine has not reported this copy
/// before (`created` = true). `None` when every candidate name is taken.
///
/// A copy is found by its card name and copy id. A copy reported with an id for the first
/// time claims the row registered without one, if any, so it keeps its id, key and history.
/// One reported without an id (its folder could not be read this time) is the row without one
/// or, failing that, the only copy registered.
async fn upsert(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    owner_id: Uuid,
    machine: &str,
    agent: &ReportedAgent,
) -> Result<Option<(Uuid, String, bool)>, sqlx::Error> {
    if let Some(found) = known_copy(tx, owner_id, machine, agent).await? {
        let (id, name): (Uuid, String) = sqlx::query_as(
            "UPDATE agents SET status = 'registered', updated_at = now(), \
                    metadata = CASE WHEN $2::text IS NULL THEN metadata \
                        ELSE metadata || jsonb_build_object('instance', $2::text) END \
             WHERE id = $1 RETURNING id, name",
        )
        .bind(found)
        .bind(&agent.instance)
        .fetch_one(&mut **tx)
        .await?;
        return Ok(Some((id, name, false)));
    }

    // Another copy of this agent already registered from this machine, so this one's name
    // must tell them apart.
    let (sibling,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM agents \
         WHERE owner_id = $1 AND discovered_on = $2 AND metadata->>'card_name' = $3 \
           AND deleted_at IS NULL)",
    )
    .bind(owner_id)
    .bind(machine)
    .bind(&agent.card_name)
    .fetch_one(&mut **tx)
    .await?;
    let mut metadata = serde_json::json!({
        "source": "nasiko-cli-discovery",
        "card_name": agent.card_name,
        "machine": machine,
    });
    if let Some(instance) = &agent.instance {
        metadata["instance"] = serde_json::Value::from(instance.as_str());
    }
    for name in candidate_names(
        &agent.card_name,
        machine,
        agent.instance.as_deref(),
        sibling,
    ) {
        let inserted: Option<(Uuid, String)> = sqlx::query_as(
            "INSERT INTO agents (name, display_name, owner_id, tags, metadata, discovered_on) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT DO NOTHING \
             RETURNING id, name",
        )
        .bind(&name)
        .bind(&agent.card_name)
        .bind(owner_id)
        .bind(vec!["local", "a2a"])
        .bind(&metadata)
        .bind(machine)
        .fetch_optional(&mut **tx)
        .await?;
        if let Some((id, name)) = inserted {
            return Ok(Some((id, name, true)));
        }
    }
    Ok(None)
}

/// The registered row `agent` is, per [`upsert`]'s rules.
async fn known_copy(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    owner_id: Uuid,
    machine: &str,
    agent: &ReportedAgent,
) -> Result<Option<Uuid>, sqlx::Error> {
    let copies: Vec<(Uuid, Option<String>)> = sqlx::query_as(
        "SELECT id, metadata->>'instance' FROM agents \
         WHERE owner_id = $1 AND discovered_on = $2 AND metadata->>'card_name' = $3 \
           AND deleted_at IS NULL",
    )
    .bind(owner_id)
    .bind(machine)
    .bind(&agent.card_name)
    .fetch_all(&mut **tx)
    .await?;
    Ok(pick_copy(&copies, agent.instance.as_deref()))
}

/// Of the copies registered under one card name, the one reported as `instance`.
fn pick_copy(copies: &[(Uuid, Option<String>)], instance: Option<&str>) -> Option<Uuid> {
    let with = |wanted: Option<&str>| {
        copies
            .iter()
            .find(|(_, copy)| copy.as_deref() == wanted)
            .map(|(id, _)| *id)
    };
    match instance {
        Some(_) => with(instance).or_else(|| with(None)),
        None => with(None).or(match copies {
            [(id, _)] => Some(*id),
            _ => None,
        }),
    }
}

/// When the router last logged an LLM call for each of `ids` that made one. Tool calls through
/// the MCP gateway are kept in `tool_calls`, so every `trace_usage` row is an LLM call.
async fn last_llm_calls(
    db: &sqlx::PgPool,
    ids: &[Uuid],
) -> Result<Vec<(Uuid, DateTime<Utc>)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT agent_id, max(started_at) FROM trace_usage \
         WHERE agent_id = ANY($1) \
         GROUP BY agent_id",
    )
    .bind(ids)
    .fetch_all(db)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_needs_a_name_within_the_column_limit() {
        assert_eq!(
            card_name(&serde_json::json!({"name": "  crew-researcher "})),
            Some("crew-researcher".into())
        );
        assert_eq!(card_name(&serde_json::json!({"name": " "})), None);
        assert_eq!(card_name(&serde_json::json!({"name": 7})), None);
        assert_eq!(card_name(&serde_json::json!({})), None);
        assert_eq!(
            card_name(&serde_json::json!({"name": "x".repeat(MAX_NAME_CHARS + 1)})),
            None
        );
    }

    #[test]
    fn a_taken_name_falls_back_to_one_carrying_the_machine() {
        assert_eq!(
            candidate_names(
                "Crew Researcher",
                "Arvinds-MacBook.local",
                Some("3fa2b1c4"),
                false
            ),
            [
                "crew-researcher".to_string(),
                "crew-researcher-arvinds-macbooklocal".to_string()
            ]
        );
    }

    #[test]
    fn a_second_copy_on_one_machine_is_named_after_its_copy_id() {
        assert_eq!(
            candidate_names("planner", "mac", Some("3fa2b1c4d5e6f708"), true),
            [
                "planner-3fa2".to_string(),
                "planner-3fa2b1c4d5e6f708".to_string(),
                "planner-mac-3fa2b1c4d5e6f708".to_string()
            ]
        );
    }

    #[test]
    fn a_reported_copy_is_found_by_its_id_then_by_the_rows_without_one() {
        let (a, b, legacy) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let copies = [(a, Some("aa".to_string())), (b, Some("bb".to_string()))];
        assert_eq!(pick_copy(&copies, Some("bb")), Some(b));
        assert_eq!(pick_copy(&copies, Some("cc")), None);
        // Its folder unread this time: two copies, so it can't be told which.
        assert_eq!(pick_copy(&copies, None), None);
        assert_eq!(pick_copy(&copies[..1], None), Some(a));

        let with_legacy = [(a, Some("aa".to_string())), (legacy, None)];
        assert_eq!(pick_copy(&with_legacy, Some("cc")), Some(legacy));
        assert_eq!(pick_copy(&with_legacy, None), Some(legacy));
        assert_eq!(pick_copy(&[], Some("aa")), None);
    }

    #[test]
    fn a_copy_id_is_short_and_safe_to_name_with() {
        assert_eq!(instance(None), Ok(None));
        assert_eq!(
            instance(Some("3fa2_b-1".into())),
            Ok(Some("3fa2_b-1".into()))
        );
        assert_eq!(instance(Some(String::new())), Err(()));
        assert_eq!(instance(Some("a/b".into())), Err(()));
        assert_eq!(instance(Some("a".repeat(MAX_INSTANCE_CHARS + 1))), Err(()));
    }
}
