use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use serde::{Deserialize, Serialize};

use crate::auth::Claims;
use crate::embedding_change::{self, EmbeddingTarget};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    // Write route requires admin role — the middleware is applied here so the
    // state is available when the router is merged into the app.
    let write_settings = Router::new().route("/settings", axum::routing::put(update_settings));

    Router::new()
        .route("/settings", get(get_settings))
        .merge(write_settings)
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct Settings {
    pub router_model: Option<String>,
    pub default_provider: Option<String>,
    pub max_flow_depth: Option<i32>,
    pub max_flow_fan_out: Option<i32>,
    pub max_flow_tokens: Option<i64>,
    pub flow_timeout_secs: Option<i32>,
    pub registry_url: Option<String>,
    /// Comma-separated tag names pinning the agent-catalog tab list.
    /// Unset/empty → the UI derives tabs from the most common agent tags.
    pub catalog_tabs: Option<String>,
    /// Organization-wide embedding destination — the last level of the embedding
    /// priority order, used when neither the request nor the agent's LLM config names an
    /// embedding model.
    pub default_embedding_provider: Option<String>,
    pub default_embedding_model: Option<String>,
    /// Whether a key is stored, never the key itself. Same rule as the OIDC client
    /// secret: a configured-or-not boolean is all any caller needs, and returning the
    /// value would put a platform credential in every authenticated user's browser.
    pub default_embedding_api_key_configured: bool,
}

impl Settings {
    /// What a deployment that has never saved settings behaves as if it had.
    ///
    /// One definition, used for BOTH "no row at all" and "a row whose column is
    /// still NULL". Those two used to disagree — the no-row case returned these
    /// values and the NULL case returned nulls — which was invisible only
    /// because nothing else created the row. `PUT /api/orchestrator/policy`
    /// (enterprise) now does: it names just its own two columns, so an operator
    /// who opens Settings → Orchestrator first materialises `id = 1` with every
    /// column here left NULL, and this endpoint started reporting blanks where
    /// it had reported defaults.
    fn defaults() -> Self {
        Self {
            router_model: Some("deepseek-v4-pro".into()),
            default_provider: Some("openai".into()),
            max_flow_depth: Some(5),
            max_flow_fan_out: Some(20),
            max_flow_tokens: Some(100000),
            flow_timeout_secs: Some(120),
            registry_url: None,
            catalog_tabs: None,
            default_embedding_provider: None,
            default_embedding_model: None,
            default_embedding_api_key_configured: false,
        }
    }

    /// Fill any column still NULL from [`Self::defaults`].
    ///
    /// Only the columns that HAVE a default are filled: `registry_url` and
    /// `catalog_tabs` default to `None`, so "unset" stays unset and keeps
    /// meaning what it means to their consumers (no registry configured; derive
    /// the catalog tabs from agent tags).
    fn with_defaults(self) -> Self {
        let d = Self::defaults();
        Self {
            router_model: self.router_model.or(d.router_model),
            default_provider: self.default_provider.or(d.default_provider),
            max_flow_depth: self.max_flow_depth.or(d.max_flow_depth),
            max_flow_fan_out: self.max_flow_fan_out.or(d.max_flow_fan_out),
            max_flow_tokens: self.max_flow_tokens.or(d.max_flow_tokens),
            flow_timeout_secs: self.flow_timeout_secs.or(d.flow_timeout_secs),
            registry_url: self.registry_url,
            catalog_tabs: self.catalog_tabs,
            // No default: "unset" means the embedding path falls through to a 400 when
            // nothing else named a model, which is the honest answer. Inventing a model
            // here would pick a vector space on the operator's behalf.
            default_embedding_provider: self.default_embedding_provider,
            default_embedding_model: self.default_embedding_model,
            default_embedding_api_key_configured: self.default_embedding_api_key_configured,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SettingsUpdate {
    pub router_model: Option<String>,
    pub default_provider: Option<String>,
    pub max_flow_depth: Option<i32>,
    pub max_flow_fan_out: Option<i32>,
    pub max_flow_tokens: Option<i64>,
    pub flow_timeout_secs: Option<i32>,
    pub registry_url: Option<String>,
    pub catalog_tabs: Option<String>,
    pub default_embedding_provider: Option<String>,
    pub default_embedding_model: Option<String>,
    /// Plaintext key, encrypted here before storage. Omit it to keep the stored one —
    /// this endpoint is a full-row replace, so without that rule every save that did not
    /// retype the key would erase it.
    pub default_embedding_api_key: Option<String>,
    /// Explicitly remove the stored key (omitting the field means "keep").
    #[serde(default)]
    pub clear_default_embedding_api_key: bool,
    /// Acknowledge that changing an already-set embedding provider/model invalidates
    /// vectors indexed with the old one. Without it such a change is rejected with 428.
    #[serde(default)]
    pub confirm_embedding_change: bool,
}

async fn get_settings(State(state): State<AppState>, _claims: Claims) -> impl IntoResponse {
    let row = sqlx::query_as::<_, Settings>(
        r#"SELECT
            router_model, default_provider, max_flow_depth,
            max_flow_fan_out, max_flow_tokens, flow_timeout_secs,
            registry_url, catalog_tabs,
            default_embedding_provider, default_embedding_model,
            (default_embedding_api_key_encrypted IS NOT NULL)
                AS default_embedding_api_key_configured
        FROM settings LIMIT 1"#,
    )
    .fetch_optional(&state.db)
    .await;

    match row {
        Ok(Some(s)) => Json(s.with_defaults()).into_response(),
        Ok(None) => Json(Settings::defaults()).into_response(),
        Err(e) => {
            tracing::error!(%e, "get_settings: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

async fn update_settings(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<SettingsUpdate>,
) -> impl IntoResponse {
    let identity: nasiko_auth::Identity = claims.into();
    if !state.auth.can_manage_users(&identity).await {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "requires admin role"})),
        )
            .into_response();
    }

    // §2.6 — compare against what is stored right now. This endpoint is a full-row
    // replace, so the page resends every column on every save; gating on presence rather
    // than on an actual change would prompt on unrelated edits. Checked before the write,
    // so a rejected change stores nothing.
    let current: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT default_embedding_provider, default_embedding_model FROM settings LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);
    let (current_provider, current_model) = current.unwrap_or((None, None));
    let current_embedding = EmbeddingTarget::new(current_provider, current_model);
    // `EmbeddingTarget::new` already blanks `""` to `None`; reuse its result for the write
    // too, so a cleared form field stores NULL rather than an empty-but-present value the
    // resolver would treat as a real destination.
    let incoming_embedding = EmbeddingTarget::new(
        body.default_embedding_provider.clone(),
        body.default_embedding_model.clone(),
    );
    // The pair is all-or-nothing, and the provider has to be one this deployment can
    // actually route to. Both are caught here rather than at call time: this is one
    // organization-wide row, so a typo would otherwise sit unnoticed until some agent's
    // next embedding call, on a request that had nothing to do with setting it.
    if incoming_embedding.provider.is_some() != incoming_embedding.model.is_some() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "default_embedding_provider and default_embedding_model must be set \
                          together (an embedding model with no provider cannot be routed)",
                "code": "invalid_embedding_default",
            })),
        )
            .into_response();
    }
    if let Some(provider) = &incoming_embedding.provider
        && let Err(message) = crate::llm_configs::require_known_provider(&state.db, provider).await
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": message, "code": "invalid_embedding_default" })),
        )
            .into_response();
    }
    match embedding_change::needs_confirmation(&current_embedding, &incoming_embedding) {
        Some(change) if !body.confirm_embedding_change => {
            return embedding_change::confirmation_required(&change);
        }
        Some(change) => embedding_change::log_confirmed(
            "settings",
            "organization-default",
            &identity.user_id,
            &change,
        ),
        None => {}
    }

    // Shared platform credential with no owning user (settings is one global row) →
    // platform-settings scope, the same one `custom_providers.encrypted_api_key` uses.
    let new_key_encrypted = body
        .default_embedding_api_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(|k| nasiko_secrets::SecretsCrypto::for_platform_settings().encrypt(k));

    let result = sqlx::query_as::<_, Settings>(
        r#"INSERT INTO settings (
               id, router_model, default_provider, max_flow_depth, max_flow_fan_out,
               max_flow_tokens, flow_timeout_secs, registry_url, catalog_tabs,
               default_embedding_provider, default_embedding_model,
               default_embedding_api_key_encrypted
           )
           VALUES (1, $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                   CASE WHEN $11 THEN NULL ELSE $12 END)
           ON CONFLICT (id) DO UPDATE SET
             router_model = EXCLUDED.router_model,
             default_provider = EXCLUDED.default_provider,
             max_flow_depth = EXCLUDED.max_flow_depth,
             max_flow_fan_out = EXCLUDED.max_flow_fan_out,
             max_flow_tokens = EXCLUDED.max_flow_tokens,
             flow_timeout_secs = EXCLUDED.flow_timeout_secs,
             registry_url = EXCLUDED.registry_url,
             catalog_tabs = EXCLUDED.catalog_tabs,
             default_embedding_provider = EXCLUDED.default_embedding_provider,
             default_embedding_model = EXCLUDED.default_embedding_model,
             -- The key is the one column this full-row replace must NOT overwrite with a
             -- null: it is never returned by GET, so a client cannot resend what it was
             -- never given. Absent means keep; the explicit flag is the only way to clear.
             default_embedding_api_key_encrypted = CASE
                 WHEN $11 THEN NULL
                 ELSE COALESCE(EXCLUDED.default_embedding_api_key_encrypted,
                               settings.default_embedding_api_key_encrypted)
             END
           RETURNING
             router_model, default_provider, max_flow_depth, max_flow_fan_out,
             max_flow_tokens, flow_timeout_secs, registry_url, catalog_tabs,
             default_embedding_provider, default_embedding_model,
             (default_embedding_api_key_encrypted IS NOT NULL)
                 AS default_embedding_api_key_configured"#,
    )
    .bind(&body.router_model)
    .bind(&body.default_provider)
    .bind(body.max_flow_depth)
    .bind(body.max_flow_fan_out)
    .bind(body.max_flow_tokens)
    .bind(body.flow_timeout_secs)
    .bind(&body.registry_url)
    .bind(&body.catalog_tabs)
    .bind(&incoming_embedding.provider)
    .bind(&incoming_embedding.model)
    .bind(body.clear_default_embedding_api_key)
    .bind(&new_key_encrypted)
    .fetch_one(&state.db)
    .await;

    match result {
        Ok(s) => Json(s).into_response(),
        Err(e) => {
            tracing::error!(%e, "update_settings: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: a row created by another endpoint — `PUT
    /// /api/orchestrator/policy` names only its own two columns — leaves every
    /// column here NULL, and `GET /api/settings` reported those nulls instead
    /// of the defaults it returns when there is no row at all. Same deployment,
    /// same configuration, two different answers depending on which settings
    /// page the operator happened to open first.
    #[test]
    fn a_row_of_nulls_reads_the_same_as_no_row_at_all() {
        let materialised = Settings {
            router_model: None,
            default_provider: None,
            max_flow_depth: None,
            max_flow_fan_out: None,
            max_flow_tokens: None,
            flow_timeout_secs: None,
            registry_url: None,
            catalog_tabs: None,
            default_embedding_provider: None,
            default_embedding_model: None,
            default_embedding_api_key_configured: false,
        }
        .with_defaults();

        let no_row = Settings::defaults();
        assert_eq!(materialised.router_model, no_row.router_model);
        assert_eq!(materialised.default_provider, no_row.default_provider);
        assert_eq!(materialised.max_flow_depth, no_row.max_flow_depth);
        assert_eq!(materialised.max_flow_fan_out, no_row.max_flow_fan_out);
        assert_eq!(materialised.max_flow_tokens, no_row.max_flow_tokens);
        assert_eq!(materialised.flow_timeout_secs, no_row.flow_timeout_secs);
    }

    /// Defaults fill gaps; they never overwrite what the operator stored.
    #[test]
    fn a_stored_value_survives_the_defaults() {
        let stored = Settings {
            router_model: Some("gpt-4o".into()),
            default_provider: None,
            max_flow_depth: Some(1),
            max_flow_fan_out: None,
            max_flow_tokens: None,
            flow_timeout_secs: None,
            registry_url: Some("https://registry.example.com".into()),
            catalog_tabs: Some("a,b".into()),
            default_embedding_provider: Some("openai".into()),
            default_embedding_model: Some("text-embedding-3-small".into()),
            default_embedding_api_key_configured: true,
        }
        .with_defaults();

        assert_eq!(stored.router_model.as_deref(), Some("gpt-4o"));
        assert_eq!(stored.max_flow_depth, Some(1));
        assert_eq!(
            stored.registry_url.as_deref(),
            Some("https://registry.example.com")
        );
        assert_eq!(stored.catalog_tabs.as_deref(), Some("a,b"));
        assert_eq!(stored.default_embedding_provider.as_deref(), Some("openai"));
        assert_eq!(
            stored.default_embedding_model.as_deref(),
            Some("text-embedding-3-small")
        );
        // …and the gaps are filled.
        assert_eq!(stored.default_provider.as_deref(), Some("openai"));
        assert_eq!(stored.max_flow_fan_out, Some(20));
    }

    /// These two genuinely mean "unset" to their consumers — no registry
    /// configured, and derive the catalog tabs from agent tags — so they must
    /// not acquire a value they never had.
    #[test]
    fn unset_optional_columns_are_not_invented() {
        let filled = Settings::defaults();
        assert!(filled.registry_url.is_none());
        assert!(filled.catalog_tabs.is_none());
        // An invented embedding default would pick a vector space for the operator; the
        // honest answer for "nothing configured" is a 400 at call time.
        assert!(filled.default_embedding_provider.is_none());
        assert!(filled.default_embedding_model.is_none());
        assert!(!filled.default_embedding_api_key_configured);
    }

    /// The key is a platform credential and `GET /api/settings` is readable by every
    /// authenticated user, so the response type must not be able to carry it at all.
    #[test]
    fn the_response_type_exposes_only_whether_a_key_is_configured() {
        let json = serde_json::to_value(Settings {
            default_embedding_api_key_configured: true,
            ..Settings::defaults()
        })
        .unwrap();
        assert_eq!(json["default_embedding_api_key_configured"], true);
        assert!(json.get("default_embedding_api_key").is_none());
        assert!(json.get("default_embedding_api_key_encrypted").is_none());
    }
}
