use axum::{
    Json, Router, extract::State, http::StatusCode, middleware, response::IntoResponse,
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::auth::Claims;
use crate::auth::rbac::require_superuser;
use crate::secrets::crypto::SecretsCrypto;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    let write_settings = Router::new()
        .route("/settings", axum::routing::put(update_settings))
        .layer(middleware::from_fn(require_superuser));

    Router::new()
        .route("/settings", get(get_settings))
        .merge(write_settings)
}

/// Response shape — `oidc_client_secret_configured` is derived (never the
/// secret itself); there is no way to read the secret back out once set.
#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct Settings {
    pub router_model: Option<String>,
    pub default_provider: Option<String>,
    pub max_flow_depth: Option<i32>,
    pub max_flow_fan_out: Option<i32>,
    pub max_flow_tokens: Option<i64>,
    pub flow_timeout_secs: Option<i32>,
    pub registry_url: Option<String>,
    pub oidc_issuer_url: Option<String>,
    pub oidc_client_id: Option<String>,
    pub oidc_redirect_uri: Option<String>,
    pub oidc_scopes: Option<String>,
    pub oidc_provider_label: Option<String>,
    pub oidc_client_secret_configured: bool,
}

/// Request shape — `oidc_client_secret` is write-only plaintext. Sending
/// `None`/omitting it leaves whatever secret is already stored untouched
/// (it can never be round-tripped from `GET /settings`, so a form that only
/// re-submits what it was shown must not accidentally clear it). Sending an
/// empty string clears it.
#[derive(Debug, Deserialize)]
pub struct SettingsUpdate {
    pub router_model: Option<String>,
    pub default_provider: Option<String>,
    pub max_flow_depth: Option<i32>,
    pub max_flow_fan_out: Option<i32>,
    pub max_flow_tokens: Option<i64>,
    pub flow_timeout_secs: Option<i32>,
    pub registry_url: Option<String>,
    pub oidc_issuer_url: Option<String>,
    pub oidc_client_id: Option<String>,
    pub oidc_redirect_uri: Option<String>,
    pub oidc_scopes: Option<String>,
    pub oidc_provider_label: Option<String>,
    #[serde(default)]
    pub oidc_client_secret: Option<String>,
}

async fn get_settings(State(state): State<AppState>, _claims: Claims) -> impl IntoResponse {
    let row = sqlx::query_as::<_, Settings>(
        r#"SELECT
            router_model, default_provider, max_flow_depth,
            max_flow_fan_out, max_flow_tokens, flow_timeout_secs,
            registry_url, oidc_issuer_url, oidc_client_id, oidc_redirect_uri,
            oidc_scopes, oidc_provider_label,
            (oidc_client_secret_encrypted IS NOT NULL) AS oidc_client_secret_configured
        FROM settings LIMIT 1"#,
    )
    .fetch_optional(&state.db)
    .await;

    match row {
        Ok(Some(s)) => Json(s).into_response(),
        Ok(None) => Json(Settings {
            router_model: Some("deepseek-v4-pro".into()),
            default_provider: Some("openai".into()),
            max_flow_depth: Some(5),
            max_flow_fan_out: Some(20),
            max_flow_tokens: Some(100000),
            flow_timeout_secs: Some(120),
            registry_url: None,
            oidc_issuer_url: None,
            oidc_client_id: None,
            oidc_redirect_uri: None,
            oidc_scopes: None,
            oidc_provider_label: None,
            oidc_client_secret_configured: false,
        })
        .into_response(),
        Err(e) => {
            tracing::error!(%e, "get_settings: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

async fn update_settings(
    State(state): State<AppState>,
    _claims: Claims,
    Json(body): Json<SettingsUpdate>,
) -> impl IntoResponse {
    // Three cases: None → leave the stored secret untouched (COALESCE below);
    // Some("") → clear it (forced NULL via the `clear_secret` flag, since SQL
    // ignores an empty string vs NULL distinction we'd otherwise need); non-empty
    // Some(secret) → encrypt and store it.
    let clear_secret = matches!(body.oidc_client_secret.as_deref(), Some(""));
    let new_secret_encrypted: Option<String> = body
        .oidc_client_secret
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|secret| SecretsCrypto::for_platform_settings().encrypt(secret));

    let result = sqlx::query_as::<_, Settings>(
        r#"INSERT INTO settings (
               id, router_model, default_provider, max_flow_depth, max_flow_fan_out,
               max_flow_tokens, flow_timeout_secs, registry_url,
               oidc_issuer_url, oidc_client_id, oidc_redirect_uri, oidc_scopes,
               oidc_provider_label, oidc_client_secret_encrypted
           )
           VALUES (1, $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                   CASE WHEN $13 THEN NULL ELSE $14 END)
           ON CONFLICT (id) DO UPDATE SET
             router_model = EXCLUDED.router_model,
             default_provider = EXCLUDED.default_provider,
             max_flow_depth = EXCLUDED.max_flow_depth,
             max_flow_fan_out = EXCLUDED.max_flow_fan_out,
             max_flow_tokens = EXCLUDED.max_flow_tokens,
             flow_timeout_secs = EXCLUDED.flow_timeout_secs,
             registry_url = EXCLUDED.registry_url,
             oidc_issuer_url = EXCLUDED.oidc_issuer_url,
             oidc_client_id = EXCLUDED.oidc_client_id,
             oidc_redirect_uri = EXCLUDED.oidc_redirect_uri,
             oidc_scopes = EXCLUDED.oidc_scopes,
             oidc_provider_label = EXCLUDED.oidc_provider_label,
             oidc_client_secret_encrypted = CASE
                 WHEN $13 THEN NULL
                 ELSE COALESCE(EXCLUDED.oidc_client_secret_encrypted, settings.oidc_client_secret_encrypted)
             END
           RETURNING
             router_model, default_provider, max_flow_depth, max_flow_fan_out,
             max_flow_tokens, flow_timeout_secs, registry_url,
             oidc_issuer_url, oidc_client_id, oidc_redirect_uri, oidc_scopes,
             oidc_provider_label,
             (oidc_client_secret_encrypted IS NOT NULL) AS oidc_client_secret_configured"#,
    )
    .bind(&body.router_model)
    .bind(&body.default_provider)
    .bind(body.max_flow_depth)
    .bind(body.max_flow_fan_out)
    .bind(body.max_flow_tokens)
    .bind(body.flow_timeout_secs)
    .bind(&body.registry_url)
    .bind(&body.oidc_issuer_url)
    .bind(&body.oidc_client_id)
    .bind(&body.oidc_redirect_uri)
    .bind(&body.oidc_scopes)
    .bind(&body.oidc_provider_label)
    .bind(clear_secret)
    .bind(&new_secret_encrypted)
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
