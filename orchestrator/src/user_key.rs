//! The OpenAI key the server itself spends on a user's behalf (orchestrator turns, routing,
//! titles, history embeddings), as opposed to the key an agent's calls are billed to.
//!
//! A key the user saved in the UI (the setup guide, or Settings → Secrets, via an `openai`
//! LLM config) wins; the server's `OPENAI_API_KEY` is the fallback, so an install that only
//! sets the environment behaves as before.

use nasiko_secrets::SecretsCrypto;
use reqwest::Client;
use sqlx::PgPool;
use uuid::Uuid;

use crate::providers::LLMProvider;

/// The user's own OpenAI key: the secret named by their default `openai` config, else by their
/// most recently updated one. `None` when they have none, or it cannot be read.
pub async fn saved_openai_key(pool: &PgPool, user_id: Uuid) -> Option<String> {
    let encrypted: Option<String> = sqlx::query_scalar(
        "SELECT s.encrypted_value \
         FROM llm_configs c \
         JOIN user_secrets s ON s.user_id = c.created_by AND s.name = c.api_key_secret_name \
         WHERE c.created_by = $1 AND c.provider = 'openai' AND c.deleted_at IS NULL \
         ORDER BY c.is_default DESC, c.updated_at DESC \
         LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(%e, %user_id, "saved OpenAI key lookup failed; using the server key");
        None
    });
    let encrypted = encrypted?;
    let key = SecretsCrypto::try_for_user(user_id)
        .and_then(|crypto| crypto.decrypt(&encrypted))
        .inspect_err(|e| tracing::warn!(%e, %user_id, "saved OpenAI key unreadable"))
        .ok()?;
    Some(key.trim().to_owned()).filter(|k| !k.is_empty())
}

/// An environment-configured provider that spends `user_id`'s saved key when they have one.
pub async fn llm_provider(pool: &PgPool, client: Client, user_id: Uuid) -> LLMProvider {
    let provider = LLMProvider::from_env(client);
    match saved_openai_key(pool, user_id).await {
        Some(key) => provider.with_api_key(key),
        None => provider,
    }
}

/// Like [`llm_provider`], but with no saved user key the calls go through `backend` (the
/// in-process LLM router), billed to `user_id` under `component`. Without a backend it falls
/// back to the environment's OpenAI endpoint.
pub async fn routed_llm_provider(
    pool: &PgPool,
    client: Client,
    user_id: Uuid,
    backend: Option<&crate::llm_backend::SharedLlmBackend>,
    component: &str,
) -> LLMProvider {
    let provider = LLMProvider::from_env(client);
    match (saved_openai_key(pool, user_id).await, backend) {
        (Some(key), _) => provider.with_api_key(key),
        (None, Some(backend)) => provider.with_backend(
            std::sync::Arc::clone(backend),
            crate::llm_backend::LlmCallContext {
                user_id,
                component: component.to_owned(),
                session_id: None,
            },
        ),
        (None, None) => provider,
    }
}

/// The key to call OpenAI with for `user_id`: their saved one, else `server_key`.
pub async fn openai_key(pool: &PgPool, user_id: Uuid, server_key: &str) -> String {
    saved_openai_key(pool, user_id)
        .await
        .unwrap_or_else(|| server_key.to_owned())
}
