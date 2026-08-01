use std::collections::HashMap;

use tracing::{info, warn};
use uuid::Uuid;

use crate::catalog::models::Agent;
use crate::state::AppState;
use nasiko_mcp_gateway::catalog;
use nasiko_runtime::{ContainerId, RuntimeState};

const AGENT_PORT: u16 = 8000;

/// Ensure seed agents are deployed and running.
///
/// Reads `SEED_AGENTS` env var (space-separated image refs, e.g.
/// "nasiko/echo-agent nasiko/nutrition:v2"). For each image:
///
/// 1. Upsert the DB record (insert if new, update image if changed)
/// 2. Check runtime status
/// 3. Deploy if not running or image changed
/// 4. Fetch agent card once healthy
///
/// Designed to run as a background task — does not block server startup.
pub async fn seed_agents_if_configured(state: &AppState) {
    let images = match std::env::var("SEED_AGENTS") {
        Ok(val) if !val.trim().is_empty() => {
            info!(images = %val, "SEED_AGENTS configured, checking deployments");
            val
        }
        _ => {
            info!("SEED_AGENTS not set, skipping agent seeding");
            return;
        }
    };

    let owner_id: Uuid = match sqlx::query_scalar(
        "SELECT id FROM users WHERE is_superuser = true AND deleted_at IS NULL ORDER BY created_at LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(id)) => id,
        _ => {
            warn!("no admin user found, cannot seed agents (run bootstrap first)");
            return;
        }
    };

    let openai_key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
    let openai_base = std::env::var("OPENAI_BASE_URL").unwrap_or_default();
    // Without the model name, agents fall back to their compiled default
    // (e.g. gpt-4o-mini), which non-OpenAI providers reject with a 400.
    let openai_model = std::env::var("OPENAI_MODEL").unwrap_or_default();
    let otel_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").unwrap_or_default();

    for image in images.split_whitespace() {
        let agent_name = extract_name(image);

        let existing = sqlx::query_as::<_, Agent>("SELECT * FROM agents WHERE name = $1")
            .bind(&agent_name)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);

        let force_pull = std::env::var("SEED_FORCE_PULL").is_ok();
        let needs_deploy = match &existing {
            None => true,
            Some(agent) => {
                let image_changed = agent.image.as_deref() != Some(image);
                if image_changed || force_pull {
                    true
                } else {
                    // UUID-keyed (see agents::build_agent_spec / RUN-2c) — the deploy a
                    // few lines below keys on the same UUID, so the liveness probe must
                    // too or it always reports "not found" and redeploys every run.
                    let container_id = ContainerId::from_uuid(agent.id);
                    match state.runtime.status(&container_id).await {
                        Ok(status) => status.state != RuntimeState::Running,
                        Err(_) => true,
                    }
                }
            }
        };

        if !needs_deploy {
            info!(agent = %agent_name, "seed agent already running, skipping");
            continue;
        }

        info!(agent = %agent_name, image = %image, "seeding agent");

        let agent = match &existing {
            Some(a) => {
                // Update image if it changed
                if a.image.as_deref() != Some(image) {
                    let _ = sqlx::query(
                        "UPDATE agents SET image = $2, status = 'deploying', updated_at = now() WHERE id = $1",
                    )
                    .bind(a.id)
                    .bind(image)
                    .execute(&state.db)
                    .await;
                } else {
                    let _ = sqlx::query(
                        "UPDATE agents SET status = 'deploying', updated_at = now() WHERE id = $1",
                    )
                    .bind(a.id)
                    .execute(&state.db)
                    .await;
                }
                a.clone()
            }
            None => match register_agent(&state.db, &agent_name, image, owner_id).await {
                Ok(a) => a,
                Err(e) => {
                    warn!(agent = %agent_name, error = %e, "failed to register seed agent");
                    continue;
                }
            },
        };

        let mut env = HashMap::new();
        env.insert("PORT".into(), AGENT_PORT.to_string());
        if !openai_key.is_empty() {
            env.insert("OPENAI_API_KEY".into(), openai_key.clone());
        }
        if !openai_base.is_empty() {
            env.insert("OPENAI_BASE_URL".into(), openai_base.clone());
        }
        if !openai_model.is_empty() {
            env.insert("OPENAI_MODEL".into(), openai_model.clone());
        }
        if !otel_endpoint.is_empty() {
            env.insert("OTEL_EXPORTER_OTLP_ENDPOINT".into(), otel_endpoint.clone());
        }
        let discovery_url = std::env::var("A2A_DISCOVERY_URL")
            .unwrap_or_else(|_| "http://host.docker.internal:8080".into());
        env.insert("A2A_DISCOVERY_URL".into(), discovery_url);

        // Route the seed agent's LLM SDK through the configured LLM router when set;
        // otherwise the direct OPENAI_API_KEY/BASE_URL set above remain (best-effort).
        crate::llm_router::wiring::inject_agent_llm_env(
            &state.db,
            &mut env,
            agent.id,
            Some(owner_id),
        )
        .await;

        // UUID-keyed (see agents::build_agent_spec) so a re-seed re-targets the same
        // workload rather than leaving a name-keyed orphan.
        let mut spec = crate::agents::build_agent_spec(
            agent.id,
            &agent_name,
            image.to_string(),
            vec![AGENT_PORT],
            env,
            None,
            state.config.agent_max_replicas,
        );
        crate::agents::attach_pull_credential(
            &state.db,
            &state.config.agent_runtime,
            &state.config.agent_image_registry,
            &mut spec,
            agent.id,
        )
        .await;

        match state.runtime.deploy(&spec).await {
            Ok(status) => {
                info!(agent = %agent_name, ?status, "seed agent deployed");
                let agent_url =
                    crate::agents::resolve_agent_url(&state.runtime, &status, &spec.container_id)
                        .await;
                let _ = sqlx::query(
                    "UPDATE agents SET status = 'running', url = $2, updated_at = now() WHERE id = $1",
                )
                .bind(agent.id)
                .bind(&agent_url)
                .execute(&state.db)
                .await;

                // SEED_AGENTS deploys straight via runtime.deploy() with no
                // agent_builds row behind it — without this, the crash-loop
                // guardian (EE) never sees this agent (docs/CRASH_GUARDIAN_REPORT.md §5.1).
                crate::agents::utils::ensure_deployment_tracked(
                    &state.db,
                    agent.id,
                    Some(owner_id),
                    image,
                )
                .await;

                // Wait for container to become healthy, then fetch agent card
                crate::agents::utils::fetch_agent_card_with_retry(
                    state.db.clone(),
                    state.http_client.clone(),
                    agent.id,
                    agent_url.clone(),
                )
                .await;
            }
            Err(e) => {
                warn!(agent = %agent_name, error = %e, "failed to deploy seed agent");
                let _ = sqlx::query(
                    "UPDATE agents SET status = 'failed', updated_at = now() WHERE id = $1",
                )
                .bind(agent.id)
                .execute(&state.db)
                .await;
            }
        }
    }
}

/// Extract agent name from image ref: "nasiko/echo-agent:v1" -> "echo-agent"
fn extract_name(image: &str) -> String {
    let without_tag = image.split(':').next().unwrap_or(image);
    without_tag
        .rsplit('/')
        .next()
        .unwrap_or(without_tag)
        .to_string()
}

async fn register_agent(
    db: &sqlx::PgPool,
    name: &str,
    image: &str,
    owner_id: Uuid,
) -> Result<Agent, sqlx::Error> {
    sqlx::query_as::<_, Agent>(
        r#"INSERT INTO agents (name, owner_id, image, status, is_public, metadata)
           VALUES ($1, $2, $3, 'deploying', true, '{"seed": true}')
           RETURNING *"#,
    )
    .bind(name)
    .bind(owner_id)
    .bind(image)
    .fetch_one(db)
    .await
}

fn toolkit_description(toolkit: &str) -> Option<&'static str> {
    Some(match toolkit {
        "slack" => "Send messages, read channels, and manage threads in Slack.",
        "gmail" => "Read, send, and manage emails in Gmail.",
        "github" => "Manage repositories, issues, pull requests, and code search on GitHub.",
        "notion" => "Create, read, and update pages and databases in Notion.",
        "jira" => "Create, update, and search Jira issues and sprints.",
        "linear" => "Manage issues, projects, and cycles in Linear.",
        "asana" => "Create and manage tasks, projects, and workspaces in Asana.",
        "trello" => "Manage boards, lists, and cards in Trello.",
        "discord" => "Send messages, manage channels, and interact with Discord servers.",
        "dropbox" => "Upload, download, and manage files in Dropbox.",
        "salesforce" => "Query, create, and update records in Salesforce CRM.",
        "hubspot" => "Manage contacts, deals, and marketing in HubSpot.",
        "zendesk" => "Create and manage support tickets in Zendesk.",
        "intercom" => "Manage conversations, contacts, and messages in Intercom.",
        "stripe" => "Manage payments, customers, and subscriptions in Stripe.",
        "airtable" => "Read and write records in Airtable bases.",
        "clickup" => "Manage tasks, spaces, and goals in ClickUp.",
        "monday" => "Manage boards, items, and workflows in monday.com.",
        "gitlab" => "Manage repositories, merge requests, and CI/CD pipelines on GitLab.",
        "pagerduty" => "Manage incidents, schedules, and escalation policies in PagerDuty.",
        "sentry" => "Monitor errors, manage issues, and track releases in Sentry.",
        "confluence" => "Create and manage pages, spaces, and content in Confluence.",
        "bitbucket" => "Manage repositories, pull requests, and pipelines on Bitbucket.",
        "googlecalendar" => "List, create, and manage events in Google Calendar.",
        "googledrive" => "Upload, download, and manage files in Google Drive.",
        "googlesheets" => "Read, write, and manage spreadsheets in Google Sheets.",
        _ => return None,
    })
}

fn toolkit_logo_url(toolkit: &str) -> Option<&'static str> {
    // simple-icons on jsdelivr CDN — monochrome SVGs, always available.
    Some(match toolkit {
        "slack" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/slack.svg",
        "gmail" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/gmail.svg",
        "github" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/github.svg",
        "notion" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/notion.svg",
        "jira" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/jira.svg",
        "linear" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/linear.svg",
        "asana" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/asana.svg",
        "trello" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/trello.svg",
        "discord" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/discord.svg",
        "dropbox" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/dropbox.svg",
        "salesforce" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/salesforce.svg",
        "hubspot" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/hubspot.svg",
        "zendesk" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/zendesk.svg",
        "intercom" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/intercom.svg",
        "stripe" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/stripe.svg",
        "airtable" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/airtable.svg",
        "clickup" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/clickup.svg",
        "monday" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/monday.svg",
        "gitlab" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/gitlab.svg",
        "pagerduty" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/pagerduty.svg",
        "sentry" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/sentry.svg",
        "confluence" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/confluence.svg",
        "bitbucket" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/bitbucket.svg",
        "googlecalendar" => {
            "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/googlecalendar.svg"
        }
        "googledrive" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/googledrive.svg",
        "googlesheets" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/googlesheets.svg",
        _ => return None,
    })
}

/// Auto-register Composio toolkits from `SEED_TOOLKITS` config.
/// Runs once at boot — skips toolkits that already exist in the DB.
/// Requires `COMPOSIO_API_KEY` to be set; silently skips otherwise.
pub async fn seed_toolkits_if_configured(state: &AppState) {
    if state.config.seed_toolkits.is_empty() {
        info!("SEED_TOOLKITS not set, skipping toolkit seeding");
        return;
    }
    if state.config.composio_api_key.is_none() {
        warn!(
            "SEED_TOOLKITS is set but COMPOSIO_API_KEY is not — \
             cannot register Composio toolkits without an API key"
        );
        return;
    }

    let toolkits = &state.config.seed_toolkits;
    info!(
        count = toolkits.len(),
        "SEED_TOOLKITS configured, registering"
    );

    // Phase 1: register each toolkit connector (fast, one API call each).
    let mut newly_seeded: HashMap<String, Uuid> = HashMap::new();
    for toolkit in toolkits {
        let toolkit = toolkit.to_lowercase();
        if nasiko_mcp_gateway::repo::get_composio_connector_by_name(&state.db, &toolkit)
            .await
            .ok()
            .flatten()
            .is_some()
        {
            info!(toolkit = %toolkit, "toolkit already registered, skipping");
            continue;
        }
        let logo = toolkit_logo_url(&toolkit);
        let desc = toolkit_description(&toolkit);
        match catalog::create_composio_connector(
            &state.mcp,
            catalog::CreateComposioInput {
                toolkit: &toolkit,
                use_composio_managed: true,
                client_id: None,
                client_secret: None,
                scopes: None,
                display_name: None,
                description: desc,
                logo_url: logo,
            },
        )
        .await
        {
            Ok(view) => {
                info!(toolkit = %toolkit, "seeded composio toolkit");
                if let Some(id) = view.get("connector_id").and_then(|v| v.as_str())
                    && let Ok(cid) = uuid::Uuid::parse_str(id)
                {
                    newly_seeded.insert(toolkit, cid);
                }
            }
            Err(e) => warn!(toolkit = %toolkit, %e, "failed to seed composio toolkit"),
        }
    }

    // Phase 2: bulk-sync tools for newly seeded toolkits in a single pass
    // through the Composio catalog (~48 pages, not 48 × N).
    if newly_seeded.is_empty() {
        return;
    }
    let Some(provider) = &state.mcp.providers.composio else {
        return;
    };
    // Downcast to ComposioProvider to access the bulk method.
    let composio = provider
        .as_any()
        .downcast_ref::<nasiko_mcp_gateway::provider::ComposioProvider>();
    let Some(composio) = composio else {
        warn!("composio provider is not ComposioProvider, skipping bulk tool sync");
        return;
    };
    let toolkit_names: Vec<String> = newly_seeded.keys().cloned().collect();
    info!(
        count = toolkit_names.len(),
        "bulk-syncing tools for newly seeded toolkits"
    );
    let tools_by_toolkit = composio.list_tools_for_toolkits(&toolkit_names).await;
    for (toolkit, tools) in &tools_by_toolkit {
        let Some(cid) = newly_seeded.get(toolkit) else {
            continue;
        };
        if tools.is_empty() {
            continue;
        }
        let parsed: Vec<(String, Option<String>)> = tools
            .iter()
            .map(|t| (t.name.clone(), t.description.clone()))
            .collect();
        match nasiko_mcp_gateway::repo::upsert_connector_tools(&state.db, *cid, &parsed).await {
            Ok(()) => info!(toolkit = %toolkit, count = parsed.len(), "synced toolkit tools to DB"),
            Err(e) => warn!(toolkit = %toolkit, %e, "failed to sync tools"),
        }
    }
}
