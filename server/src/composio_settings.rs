//! `GET/PUT /api/settings/composio`: the Composio API key and which toolkits are switched on.
//!
//! The key used to come only from `COMPOSIO_API_KEY` and the toolkits only from
//! `SEED_TOOLKITS`, both read at boot. A key saved here wins over the env value and takes
//! effect at once (the gateway's Composio backend is a shared, swappable slot); clearing it
//! falls back to the env value. Switching a toolkit off hides it rather than deleting it,
//! so users' connected accounts and agents' permission rules survive switching it back on.

use std::collections::{BTreeSet, HashMap};

use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use nasiko_mcp_gateway::repo::{self, McpConnector};
use serde::{Deserialize, Serialize};

use crate::auth::Claims;
use crate::seed;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/settings/composio", get(get_composio).put(update_composio))
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KeySource {
    /// Saved from this page.
    Ui,
    /// `COMPOSIO_API_KEY`.
    Env,
    None,
}

#[derive(Debug, Serialize)]
pub struct ToolkitState {
    pub toolkit: String,
    pub display_name: String,
    pub description: Option<String>,
    pub logo_url: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct ToolkitFailure {
    pub toolkit: String,
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct ComposioSettings {
    /// Whether a key is in effect, never the key itself.
    pub api_key_configured: bool,
    pub key_source: KeySource,
    pub toolkits: Vec<ToolkitState>,
    /// Toolkits that were ticked but could not be registered with Composio.
    pub failed: Vec<ToolkitFailure>,
}

#[derive(Debug, Deserialize)]
pub struct ComposioSettingsUpdate {
    /// Plaintext key, encrypted before storage. Omit or leave blank to keep the stored one.
    pub api_key: Option<String>,
    /// Remove the key saved here (the env value, if any, applies again).
    #[serde(default)]
    pub clear_api_key: bool,
    /// The full set of toolkits to offer. Omit to leave the selection as it is.
    pub toolkits: Option<Vec<String>>,
}

fn require_superuser(claims: &Claims) -> Result<(), axum::response::Response> {
    if claims.is_superuser {
        Ok(())
    } else {
        Err((StatusCode::FORBIDDEN, "requires superuser").into_response())
    }
}

fn internal(context: &str, e: impl std::fmt::Display) -> axum::response::Response {
    tracing::error!(%e, "{context}");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
}

fn bad_request(code: &str, message: impl Into<String>) -> axum::response::Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": message.into(), "code": code })),
    )
        .into_response()
}

async fn stored_key(db: &sqlx::PgPool) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT composio_api_key_encrypted FROM settings WHERE id = 1")
        .fetch_optional(db)
        .await
        .map(Option::flatten)
}

fn decrypt(encrypted: &str) -> Option<String> {
    match nasiko_secrets::SecretsCrypto::for_platform_settings().decrypt(encrypted) {
        Ok(key) => Some(key),
        Err(e) => {
            tracing::warn!(%e, "stored Composio API key could not be decrypted");
            None
        }
    }
}

/// The key the gateway should use: the one saved here, else `COMPOSIO_API_KEY`.
fn effective_key(stored: Option<&str>, env: Option<&str>) -> Option<String> {
    stored.or(env).map(str::to_string)
}

/// Point the gateway at the key saved in Settings, if there is one. Called once at boot,
/// before toolkit seeding, so a UI-only key works with `SEED_TOOLKITS`.
pub async fn apply_stored_key(state: &AppState) {
    let stored = match stored_key(&state.db).await {
        Ok(stored) => stored.as_deref().and_then(decrypt),
        Err(e) => {
            tracing::warn!(%e, "could not read the stored Composio API key");
            return;
        }
    };
    if let Some(key) = effective_key(stored.as_deref(), state.config.composio_api_key.as_deref()) {
        state.mcp.set_composio_key(Some(key));
    }
}

fn capitalize(name: &str) -> String {
    let mut chars = name.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

/// The toolkits an admin can pick: the known ones plus any already registered (an
/// admin-created one, or a `SEED_TOOLKITS` name outside the known list), each with whether
/// it is currently offered.
fn toolkit_states(registered: &[McpConnector]) -> Vec<ToolkitState> {
    let by_name: HashMap<&str, &McpConnector> =
        registered.iter().map(|c| (c.name.as_str(), c)).collect();
    let names: BTreeSet<&str> = seed::KNOWN_TOOLKITS
        .iter()
        .copied()
        .chain(by_name.keys().copied())
        .collect();
    names
        .into_iter()
        .map(|name| {
            let row = by_name.get(name);
            ToolkitState {
                toolkit: name.to_string(),
                display_name: row
                    .and_then(|c| c.display_name.clone())
                    .unwrap_or_else(|| capitalize(name)),
                description: row
                    .and_then(|c| c.description.clone())
                    .or_else(|| seed::toolkit_description(name).map(str::to_string)),
                logo_url: row
                    .and_then(|c| c.logo_url.clone())
                    .or_else(|| seed::toolkit_logo_url(name).map(str::to_string)),
                enabled: row.is_some_and(|c| c.active()),
            }
        })
        .collect()
}

async fn current(
    state: &AppState,
    failed: Vec<ToolkitFailure>,
) -> Result<ComposioSettings, axum::response::Response> {
    let stored = stored_key(&state.db)
        .await
        .map_err(|e| internal("composio settings: read key", e))?;
    let registered = repo::list_composio_connectors(&state.db)
        .await
        .map_err(|e| internal("composio settings: list toolkits", e))?;
    let key_source = if stored.is_some() {
        KeySource::Ui
    } else if state.config.composio_api_key.is_some() {
        KeySource::Env
    } else {
        KeySource::None
    };
    Ok(ComposioSettings {
        api_key_configured: key_source != KeySource::None,
        key_source,
        toolkits: toolkit_states(&registered),
        failed,
    })
}

async fn get_composio(State(state): State<AppState>, claims: Claims) -> impl IntoResponse {
    if let Err(denied) = require_superuser(&claims) {
        return denied;
    }
    match current(&state, Vec::new()).await {
        Ok(settings) => Json(settings).into_response(),
        Err(e) => e,
    }
}

/// Lowercase, dedupe, and check each name is one this page offers.
fn normalize_selection(
    requested: &[String],
    registered: &[McpConnector],
) -> Result<BTreeSet<String>, String> {
    let offered: BTreeSet<&str> = seed::KNOWN_TOOLKITS
        .iter()
        .copied()
        .chain(registered.iter().map(|c| c.name.as_str()))
        .collect();
    let mut wanted = BTreeSet::new();
    for raw in requested {
        let name = raw.trim().to_lowercase();
        if !offered.contains(name.as_str()) {
            return Err(format!("unknown toolkit '{raw}'"));
        }
        wanted.insert(name);
    }
    Ok(wanted)
}

/// Make the offered toolkits exactly `wanted`: show or hide the registered ones, register
/// the rest. Returns the ones Composio refused.
async fn apply_selection(
    state: &AppState,
    registered: &[McpConnector],
    wanted: &BTreeSet<String>,
) -> Result<Vec<ToolkitFailure>, nasiko_mcp_gateway::error::McpError> {
    for connector in registered {
        let want = wanted.contains(&connector.name);
        if want != connector.active() {
            repo::set_composio_active(&state.db, connector.id, want).await?;
        }
    }
    let missing: Vec<String> = wanted
        .iter()
        .filter(|name| !registered.iter().any(|c| &c.name == *name))
        .cloned()
        .collect();
    if missing.is_empty() {
        return Ok(Vec::new());
    }
    Ok(seed::register_toolkits(state, &missing)
        .await
        .into_iter()
        .map(|(toolkit, error)| ToolkitFailure { toolkit, error })
        .collect())
}

async fn update_composio(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<ComposioSettingsUpdate>,
) -> impl IntoResponse {
    if let Err(denied) = require_superuser(&claims) {
        return denied;
    }
    let new_key = body
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty());
    if new_key.is_some() && body.clear_api_key {
        return bad_request(
            "conflicting_key_update",
            "send either api_key or clear_api_key, not both",
        );
    }

    let registered = match repo::list_composio_connectors(&state.db).await {
        Ok(rows) => rows,
        Err(e) => return internal("composio settings: list toolkits", e),
    };
    let wanted = match body.toolkits.as_deref() {
        Some(requested) => match normalize_selection(requested, &registered) {
            Ok(set) => Some(set),
            Err(message) => return bad_request("unknown_toolkit", message),
        },
        None => None,
    };

    let stored = match stored_key(&state.db).await {
        Ok(stored) => stored,
        Err(e) => return internal("composio settings: read key", e),
    };
    let stored_after = if body.clear_api_key {
        None
    } else if let Some(key) = new_key {
        Some(nasiko_secrets::SecretsCrypto::for_platform_settings().encrypt(key))
    } else {
        stored.clone()
    };
    let key_after = effective_key(
        stored_after.as_deref().and_then(decrypt).as_deref(),
        state.config.composio_api_key.as_deref(),
    );
    if wanted.as_ref().is_some_and(|w| !w.is_empty()) && key_after.is_none() {
        return bad_request(
            "composio_not_configured",
            "add a Composio API key before switching toolkits on",
        );
    }

    if stored_after != stored {
        let saved = sqlx::query(
            "INSERT INTO settings (id, composio_api_key_encrypted) VALUES (1, $1) \
             ON CONFLICT (id) DO UPDATE SET composio_api_key_encrypted = EXCLUDED.composio_api_key_encrypted",
        )
        .bind(&stored_after)
        .execute(&state.db)
        .await;
        if let Err(e) = saved {
            return internal("composio settings: save key", e);
        }
    }
    if key_after != state.mcp.providers.composio.api_key() {
        state.mcp.set_composio_key(key_after);
        nasiko_mcp_gateway::session::clear_session_cache(&state.mcp.redis).await;
    }

    // Registering a toolkit is a round trip to Composio each; run it to completion even if
    // the browser gives up waiting, so a dropped request cannot leave the set half-applied.
    let failed = match wanted {
        Some(wanted) => {
            let task_state = state.clone();
            let task =
                tokio::spawn(
                    async move { apply_selection(&task_state, &registered, &wanted).await },
                );
            match task.await {
                Ok(Ok(failed)) => failed,
                Ok(Err(e)) => return internal("composio settings: apply toolkits", e),
                Err(e) => return internal("composio settings: apply toolkits", e),
            }
        }
        None => Vec::new(),
    };

    match current(&state, failed).await {
        Ok(settings) => Json(settings).into_response(),
        Err(e) => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_key_beats_the_env_key() {
        assert_eq!(
            effective_key(Some("saved"), Some("env")).as_deref(),
            Some("saved")
        );
        assert_eq!(effective_key(None, Some("env")).as_deref(), Some("env"));
        assert_eq!(effective_key(None, None), None);
    }

    #[test]
    fn the_known_toolkits_are_offered_even_before_any_is_registered() {
        let states = toolkit_states(&[]);
        assert_eq!(states.len(), seed::KNOWN_TOOLKITS.len());
        assert!(states.iter().all(|t| !t.enabled));
        assert!(
            states
                .iter()
                .all(|t| t.description.is_some() && t.logo_url.is_some())
        );
    }

    #[test]
    fn a_selection_is_lowercased_deduped_and_limited_to_offered_toolkits() {
        let picked =
            normalize_selection(&["Slack".into(), "slack".into(), " gmail ".into()], &[]).unwrap();
        assert_eq!(
            picked.into_iter().collect::<Vec<_>>(),
            vec!["gmail".to_string(), "slack".to_string()]
        );
        assert!(normalize_selection(&["not-a-toolkit".into()], &[]).is_err());
    }

    #[test]
    fn the_response_never_carries_a_key() {
        let json = serde_json::to_value(ComposioSettings {
            api_key_configured: true,
            key_source: KeySource::Ui,
            toolkits: vec![],
            failed: vec![],
        })
        .unwrap();
        assert!(json.get("api_key").is_none());
        assert_eq!(json["key_source"], "ui");
    }
}
