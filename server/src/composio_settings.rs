//! `GET/PUT /api/settings/composio`: the Composio API key and which toolkits are switched on.
//!
//! The key used to come only from `COMPOSIO_API_KEY` and the toolkits only from
//! `SEED_TOOLKITS`, both read at boot. A key saved here wins over the env value and takes
//! effect at once on the replica that saved it (the gateway's Composio backend is a shared,
//! swappable slot) and within [`KEY_REFRESH_INTERVAL`] on the others; clearing it falls back
//! to the env value. A key from a different Composio project moves the registered toolkits
//! into that project, and their users reconnect. Switching a toolkit off hides it rather
//! than deleting it, so users' connected accounts and agents' permission rules survive
//! switching it back on; the gateway refuses its tools while it is hidden.

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
    /// A key was saved here but can no longer be decrypted, so it is being ignored.
    pub stored_key_unreadable: bool,
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

/// The key saved from this page, as the settings row holds it.
#[derive(Debug, PartialEq, Eq)]
enum StoredKey {
    Absent,
    Readable(String),
    /// Saved, but it no longer decrypts (e.g. `SECRETS_ENCRYPTION_KEY` was rotated), so it
    /// is ignored and the env key, if any, applies.
    Unreadable,
}

impl StoredKey {
    fn from_encrypted(encrypted: Option<&str>) -> Self {
        let Some(encrypted) = encrypted else {
            return Self::Absent;
        };
        match nasiko_secrets::SecretsCrypto::for_platform_settings().decrypt(encrypted) {
            Ok(key) => Self::Readable(key),
            Err(_) => Self::Unreadable,
        }
    }
}

/// The key the gateway should use: the one saved here, else `COMPOSIO_API_KEY`.
fn effective_key(stored: &StoredKey, env: Option<&str>) -> Option<String> {
    match stored {
        StoredKey::Readable(key) => Some(key.clone()),
        StoredKey::Absent | StoredKey::Unreadable => env.map(str::to_string),
    }
}

/// Where the key in effect comes from — the same rule as [`effective_key`].
fn key_source(stored: &StoredKey, env: Option<&str>) -> KeySource {
    match (stored, env) {
        (StoredKey::Readable(_), _) => KeySource::Ui,
        (_, Some(_)) => KeySource::Env,
        (_, None) => KeySource::None,
    }
}

/// How often each process re-reads the saved key. Only the replica that served the PUT
/// switches at once; every other one catches up within this window.
const KEY_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

/// Point this process's gateway at the key in effect, if it isn't already.
async fn refresh_key(state: &AppState) {
    let stored = match stored_key(&state.db).await {
        Ok(stored) => StoredKey::from_encrypted(stored.as_deref()),
        Err(e) => {
            tracing::warn!(%e, "could not read the stored Composio API key");
            return;
        }
    };
    let key = effective_key(&stored, state.config.composio_api_key.as_deref());
    if key != state.mcp.providers.composio.api_key() {
        if stored == StoredKey::Unreadable {
            tracing::warn!("stored Composio API key could not be decrypted; ignoring it");
        }
        tracing::info!(
            configured = key.is_some(),
            "Composio API key changed; switching"
        );
        state.mcp.set_composio_key(key);
    }
}

/// Point the gateway at the key saved in Settings, if there is one. Called once at boot,
/// before toolkit seeding, so a UI-only key works with `SEED_TOOLKITS`.
pub async fn apply_stored_key(state: &AppState) {
    refresh_key(state).await;
}

/// Keep following the saved key, so a change made through another replica reaches this
/// one without a restart.
pub async fn watch_stored_key(state: AppState) {
    let mut interval = tokio::time::interval(KEY_REFRESH_INTERVAL);
    interval.tick().await; // the first tick fires at once; boot already applied the key
    loop {
        interval.tick().await;
        refresh_key(&state).await;
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
    let stored = StoredKey::from_encrypted(stored.as_deref());
    let key_source = key_source(&stored, state.config.composio_api_key.as_deref());
    Ok(ComposioSettings {
        api_key_configured: key_source != KeySource::None,
        key_source,
        stored_key_unreadable: stored == StoredKey::Unreadable,
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

/// After the key changes, give every registered toolkit an auth config in the project the
/// new key belongs to. One the project already knows (the same project, a rotated key) is
/// left alone. Any other is re-created there and its users' connections are expired, since
/// their accounts live in the old project, so they reconnect. Returns the toolkits that
/// could not be moved.
async fn move_toolkits_to_current_project(
    state: &AppState,
    registered: &[McpConnector],
) -> Vec<ToolkitFailure> {
    let Some(provider) = state.mcp.providers.composio.get() else {
        return Vec::new();
    };
    let mut failed = Vec::new();
    for connector in registered {
        let fail = |error: String| ToolkitFailure {
            toolkit: connector.name.clone(),
            error,
        };
        let known = match connector.auth_config_id.as_deref() {
            Some(id) => provider.auth_config_exists(id).await,
            None => Ok(false),
        };
        match known {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => {
                failed.push(fail(format!("could not check it under the new key: {e}")));
                continue;
            }
        }
        if !connector.use_composio_managed.unwrap_or(true) {
            failed.push(fail(
                "it uses its own OAuth app, which can't be re-created from here; \
                 register it again with that app"
                    .to_string(),
            ));
            continue;
        }
        let created = match provider
            .create_auth_config(&connector.name, true, None, None, None)
            .await
        {
            Ok(created) => created,
            Err(e) => {
                failed.push(fail(e.to_string()));
                continue;
            }
        };
        if let Err(e) =
            repo::rebind_composio_connector(&state.db, connector.id, &created.auth_config_id).await
        {
            failed.push(fail(e.to_string()));
            continue;
        }
        tracing::info!(toolkit = %connector.name, "moved composio toolkit to the new key's project");
    }
    failed
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
    let env_key = state.config.composio_api_key.as_deref();
    let key_before = effective_key(&StoredKey::from_encrypted(stored.as_deref()), env_key);
    let stored_after = if body.clear_api_key {
        None
    } else if let Some(key) = new_key {
        Some(nasiko_secrets::SecretsCrypto::for_platform_settings().encrypt(key))
    } else {
        stored.clone()
    };
    let key_after = effective_key(&StoredKey::from_encrypted(stored_after.as_deref()), env_key);
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
        state.mcp.set_composio_key(key_after.clone());
        nasiko_mcp_gateway::session::clear_session_cache(state.mcp.redis.client()).await;
    }
    let key_changed = key_after.is_some() && key_after != key_before;

    // Each toolkit is a round trip to Composio; run them to completion even if the browser
    // gives up waiting, so a dropped request cannot leave the change half-applied.
    let task_state = state.clone();
    let task = tokio::spawn(async move {
        let mut failed = Vec::new();
        if key_changed {
            failed.extend(move_toolkits_to_current_project(&task_state, &registered).await);
        }
        if let Some(wanted) = wanted {
            failed.extend(apply_selection(&task_state, &registered, &wanted).await?);
        }
        Ok::<_, nasiko_mcp_gateway::error::McpError>(failed)
    });
    let failed = match task.await {
        Ok(Ok(failed)) => failed,
        // The gateway's own redaction decides what is safe to name (a rejected Composio key is;
        // database text is not), so the admin learns why the toolkits did not change.
        Ok(Err(e)) => {
            tracing::error!(%e, "composio settings: apply toolkits");
            let status =
                StatusCode::from_u16(e.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            return (status, e.client_message()).into_response();
        }
        Err(e) => return internal("composio settings: apply toolkits", e),
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
        let saved = StoredKey::Readable("saved".into());
        assert_eq!(effective_key(&saved, Some("env")).as_deref(), Some("saved"));
        assert_eq!(key_source(&saved, Some("env")), KeySource::Ui);
        assert_eq!(
            effective_key(&StoredKey::Absent, Some("env")).as_deref(),
            Some("env")
        );
        assert_eq!(effective_key(&StoredKey::Absent, None), None);
        assert_eq!(key_source(&StoredKey::Absent, None), KeySource::None);
    }

    #[test]
    fn a_saved_key_that_no_longer_decrypts_is_reported_as_not_in_effect() {
        let unreadable = StoredKey::Unreadable;
        assert_eq!(
            effective_key(&unreadable, Some("env")).as_deref(),
            Some("env")
        );
        assert_eq!(key_source(&unreadable, Some("env")), KeySource::Env);
        assert_eq!(effective_key(&unreadable, None), None);
        assert_eq!(key_source(&unreadable, None), KeySource::None);
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
            stored_key_unreadable: false,
            toolkits: vec![],
            failed: vec![],
        })
        .unwrap();
        assert!(json.get("api_key").is_none());
        assert_eq!(json["key_source"], "ui");
    }
}
