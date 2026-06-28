//! Config resolution: `(agent_id, owner_id)` → [`ResolvedConfig`].
//!
//! Reads the agent's `llm_config` (TTL-cached) and the owner's provider key, applying
//! the backward-compat defaults and the platform-key fallback. **C4:** the incoming
//! request's `model` is never consulted here — the registry/default model is
//! authoritative on every path (chat, stream, embeddings).
//!
//! The registry/secret reads sit behind [`RegistryStore`] so the resolver is unit
//! testable without a database; [`PgRegistry`] is the Postgres implementation.

use async_trait::async_trait;
use nasiko_secrets::SecretsCrypto;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::config::GatewayConfig;
use crate::error::GatewayError;

mod cache;
pub use cache::ConfigCache;

/// Per-agent routing config, as stored in `agents.llm_config` (JSONB).
#[derive(Debug, Clone, Deserialize)]
pub struct LLMConfig {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub fallback_models: Vec<String>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<i64>,
    #[serde(default)]
    pub api_key_secret_name: Option<String>,
}

/// The resolved call configuration handed to the provider client.
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub provider: String,
    /// Bare provider-native model id (no prefix) — reported in the response + usage.
    pub model: String,
    /// Provider-prefixed id `"{provider}/{model}"` — selects the spoke.
    pub litellm_model: String,
    pub api_key: String,
    pub fallback_models: Vec<String>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<i64>,
}

/// Storage seam for the resolver — mockable in tests.
#[async_trait]
pub trait RegistryStore: Send + Sync {
    /// `Ok(None)` = no agent row; `Ok(Some(None))` = row exists with NULL `llm_config`;
    /// `Ok(Some(Some(cfg)))` = `llm_config` present.
    async fn fetch_llm_config(
        &self,
        agent_id: Uuid,
    ) -> Result<Option<Option<LLMConfig>>, sqlx::Error>;

    /// Encrypted secret value for `(owner_id, name)`, or `None` if absent.
    async fn fetch_user_secret(
        &self,
        owner_id: Uuid,
        name: &str,
    ) -> Result<Option<String>, sqlx::Error>;
}

/// Postgres-backed [`RegistryStore`].
pub struct PgRegistry {
    db: PgPool,
}

impl PgRegistry {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }
}

#[async_trait]
impl RegistryStore for PgRegistry {
    async fn fetch_llm_config(
        &self,
        agent_id: Uuid,
    ) -> Result<Option<Option<LLMConfig>>, sqlx::Error> {
        let row: Option<(Option<sqlx::types::Json<LLMConfig>>,)> =
            sqlx::query_as("SELECT llm_config FROM agents WHERE id = $1")
                .bind(agent_id)
                .fetch_optional(&self.db)
                .await?;
        Ok(row.map(|(col,)| col.map(|j| j.0)))
    }

    async fn fetch_user_secret(
        &self,
        owner_id: Uuid,
        name: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT encrypted_value FROM user_secrets WHERE user_id = $1 AND name = $2")
                .bind(owner_id)
                .bind(name)
                .fetch_optional(&self.db)
                .await?;
        Ok(row.map(|(v,)| v))
    }
}

/// Resolve `(agent_id, owner_id)` into a [`ResolvedConfig`].
pub async fn resolve(
    store: &dyn RegistryStore,
    cache: &ConfigCache,
    cfg: &GatewayConfig,
    agent_id: &str,
    owner_id: &str,
) -> Result<ResolvedConfig, GatewayError> {
    // A non-UUID agent_id can't match any row → treat as "no registry entry".
    let agent_uuid = Uuid::parse_str(agent_id)
        .map_err(|_| GatewayError::NoRegistryEntry(agent_id.to_string()))?;

    let llm_config = load_llm_config(store, cache, agent_uuid, agent_id).await?;
    let plan = plan_config(llm_config, cfg);
    let api_key = resolve_api_key(store, cfg, owner_id, plan.api_key_secret_name.as_deref()).await?;

    Ok(ResolvedConfig {
        litellm_model: format!("{}/{}", plan.provider, plan.model),
        provider: plan.provider,
        model: plan.model,
        api_key,
        fallback_models: plan.fallback_models,
        temperature: plan.temperature,
        max_tokens: plan.max_tokens,
    })
}

/// Load `llm_config` via the cache, falling back to the store. A missing agent row is
/// an error (not cached); a present row (with or without config) is cached.
async fn load_llm_config(
    store: &dyn RegistryStore,
    cache: &ConfigCache,
    agent_uuid: Uuid,
    agent_id: &str,
) -> Result<Option<LLMConfig>, GatewayError> {
    if let Some(hit) = cache.get(agent_uuid) {
        return Ok(hit);
    }
    match store
        .fetch_llm_config(agent_uuid)
        .await
        .map_err(|e| GatewayError::Internal(format!("registry read failed: {e}")))?
    {
        None => Err(GatewayError::NoRegistryEntry(agent_id.to_string())),
        Some(config) => {
            cache.put(agent_uuid, config.clone());
            Ok(config)
        }
    }
}

/// Pure: pick provider/model/params from `llm_config`, or the backward-compat defaults.
struct ConfigPlan {
    provider: String,
    model: String,
    fallback_models: Vec<String>,
    temperature: Option<f64>,
    max_tokens: Option<i64>,
    api_key_secret_name: Option<String>,
}

fn plan_config(llm_config: Option<LLMConfig>, cfg: &GatewayConfig) -> ConfigPlan {
    match llm_config {
        Some(c) => ConfigPlan {
            provider: c.provider,
            model: c.model,
            fallback_models: c.fallback_models,
            temperature: c.temperature,
            max_tokens: c.max_tokens,
            api_key_secret_name: c.api_key_secret_name,
        },
        None => ConfigPlan {
            provider: cfg.default_provider.clone(),
            model: cfg.default_model.clone(),
            fallback_models: Vec::new(),
            temperature: None,
            max_tokens: None,
            api_key_secret_name: None,
        },
    }
}

/// Resolve the provider API key. Per-user secret lookup requires a secret name **and**
/// a non-empty owner; an empty owner_id means "no per-user lookup" → platform key.
async fn resolve_api_key(
    store: &dyn RegistryStore,
    cfg: &GatewayConfig,
    owner_id: &str,
    secret_name: Option<&str>,
) -> Result<String, GatewayError> {
    if let Some(name) = secret_name
        && !owner_id.is_empty()
    {
        // Non-empty-but-invalid owner_id is a token-minting bug → server error.
        let owner = Uuid::parse_str(owner_id)
            .map_err(|_| GatewayError::Internal(format!("invalid owner_id in token: {owner_id}")))?;
        let encrypted = store
            .fetch_user_secret(owner, name)
            .await
            .map_err(|e| GatewayError::Internal(format!("secret read failed: {e}")))?
            .ok_or_else(|| GatewayError::SecretNotFound(name.to_string(), owner_id.to_string()))?;
        let crypto = SecretsCrypto::try_for_user(owner)
            .map_err(|e| GatewayError::Internal(format!("secret cipher init failed: {e}")))?;
        return crypto
            .decrypt(&encrypted)
            .map_err(|e| GatewayError::Internal(format!("secret decryption failed: {e}")));
    }
    platform_key(cfg)
}

/// Pure: the platform-owned fallback key, or the 400 when none is configured.
fn platform_key(cfg: &GatewayConfig) -> Result<String, GatewayError> {
    if cfg.platform_openai_api_key.is_empty() {
        Err(GatewayError::NoApiKey)
    } else {
        Ok(cfg.platform_openai_api_key.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const AGENT: &str = "11111111-1111-1111-1111-111111111111";
    const OWNER: &str = "22222222-2222-2222-2222-222222222222";

    struct MockRegistry {
        config: Option<Option<LLMConfig>>,
        secret: Option<String>,
    }

    #[async_trait]
    impl RegistryStore for MockRegistry {
        async fn fetch_llm_config(
            &self,
            _: Uuid,
        ) -> Result<Option<Option<LLMConfig>>, sqlx::Error> {
            Ok(self.config.clone())
        }
        async fn fetch_user_secret(&self, _: Uuid, _: &str) -> Result<Option<String>, sqlx::Error> {
            Ok(self.secret.clone())
        }
    }

    fn cache() -> ConfigCache {
        ConfigCache::new(Duration::from_secs(30))
    }

    fn cfg(provider: &str, model: &str, platform_key: &str) -> GatewayConfig {
        GatewayConfig {
            default_provider: provider.into(),
            default_model: model.into(),
            platform_openai_api_key: platform_key.into(),
            ..Default::default()
        }
    }

    fn llm_config(provider: &str, model: &str, secret_name: Option<&str>) -> LLMConfig {
        LLMConfig {
            provider: provider.into(),
            model: model.into(),
            fallback_models: vec!["openai/gpt-4o-mini".into()],
            temperature: Some(0.5),
            max_tokens: Some(1024),
            api_key_secret_name: secret_name.map(str::to_string),
        }
    }

    #[tokio::test]
    async fn defaults_when_no_llm_config() {
        let store = MockRegistry { config: Some(None), secret: None };
        let r = resolve(&store, &cache(), &cfg("openai", "gpt-4o-mini", "platform-key"), AGENT, OWNER)
            .await
            .unwrap();
        assert_eq!(r.provider, "openai");
        assert_eq!(r.model, "gpt-4o-mini");
        assert_eq!(r.litellm_model, "openai/gpt-4o-mini");
        assert_eq!(r.api_key, "platform-key");
        assert!(r.fallback_models.is_empty());
        assert_eq!(r.temperature, None);
    }

    #[tokio::test]
    async fn missing_agent_is_no_registry_entry() {
        let store = MockRegistry { config: None, secret: None };
        let err = resolve(&store, &cache(), &cfg("openai", "gpt-4o-mini", "k"), AGENT, OWNER)
            .await
            .unwrap_err();
        assert!(matches!(err, GatewayError::NoRegistryEntry(_)));
        assert_eq!(err.to_string(), format!("No registry entry for agent_id={AGENT}"));
    }

    #[tokio::test]
    async fn no_secret_and_no_platform_key_is_bad_request() {
        let store = MockRegistry { config: Some(None), secret: None };
        let err = resolve(&store, &cache(), &cfg("openai", "gpt-4o-mini", ""), AGENT, OWNER)
            .await
            .unwrap_err();
        assert!(matches!(err, GatewayError::NoApiKey));
    }

    #[tokio::test]
    async fn registry_model_wins_request_model_never_seen() {
        // resolve() takes no request model at all — the registry model is authoritative (C4).
        let store = MockRegistry {
            config: Some(Some(llm_config("anthropic", "claude-3-5-sonnet-20241022", None))),
            secret: None,
        };
        let r = resolve(&store, &cache(), &cfg("openai", "gpt-4o-mini", "platform-key"), AGENT, OWNER)
            .await
            .unwrap();
        assert_eq!(r.model, "claude-3-5-sonnet-20241022");
        assert_eq!(r.litellm_model, "anthropic/claude-3-5-sonnet-20241022");
        assert_eq!(r.fallback_models, vec!["openai/gpt-4o-mini".to_string()]);
        assert_eq!(r.temperature, Some(0.5));
        assert_eq!(r.max_tokens, Some(1024));
        assert_eq!(r.api_key, "platform-key"); // no secret_name → platform key
    }

    #[tokio::test]
    async fn missing_secret_row_is_secret_not_found() {
        let store = MockRegistry {
            config: Some(Some(llm_config("anthropic", "claude-x", Some("ANTHROPIC_API_KEY")))),
            secret: None,
        };
        let err = resolve(&store, &cache(), &cfg("openai", "gpt-4o-mini", ""), AGENT, OWNER)
            .await
            .unwrap_err();
        assert!(matches!(err, GatewayError::SecretNotFound(_, _)));
        assert_eq!(
            err.to_string(),
            format!("Secret 'ANTHROPIC_API_KEY' not found for owner_id={OWNER}")
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn secret_name_set_decrypts_owner_key() {
        use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
        // Encrypt a known value with the per-user key under a known master key, then
        // assert resolve() decrypts it back via the same env master key.
        let owner = Uuid::parse_str(OWNER).unwrap();
        let master = [7u8; 32];
        // SAFETY: serialized via #[serial]; no other test reads the env concurrently.
        unsafe {
            std::env::set_var("SECRETS_ENCRYPTION_KEY", BASE64.encode(master));
        }
        let ciphertext = SecretsCrypto::for_user(owner).encrypt("sk-real-anthropic-key");

        let store = MockRegistry {
            config: Some(Some(llm_config("anthropic", "claude-x", Some("ANTHROPIC_API_KEY")))),
            secret: Some(ciphertext),
        };
        let r = resolve(&store, &cache(), &cfg("openai", "gpt-4o-mini", ""), AGENT, OWNER)
            .await
            .unwrap();
        assert_eq!(r.api_key, "sk-real-anthropic-key");
        assert_eq!(r.provider, "anthropic");
    }

    #[tokio::test]
    async fn second_resolve_is_cache_hit() {
        // A store that errors on the 2nd fetch proves the 1st result was cached.
        struct OnceStore {
            calls: std::sync::atomic::AtomicUsize,
        }
        #[async_trait]
        impl RegistryStore for OnceStore {
            async fn fetch_llm_config(
                &self,
                _: Uuid,
            ) -> Result<Option<Option<LLMConfig>>, sqlx::Error> {
                let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n == 0 {
                    Ok(Some(None))
                } else {
                    Err(sqlx::Error::PoolClosed) // must not be reached on a cache hit
                }
            }
            async fn fetch_user_secret(
                &self,
                _: Uuid,
                _: &str,
            ) -> Result<Option<String>, sqlx::Error> {
                Ok(None)
            }
        }
        let store = OnceStore { calls: Default::default() };
        let cache = cache();
        let cfg = cfg("openai", "gpt-4o-mini", "platform-key");
        let a = resolve(&store, &cache, &cfg, AGENT, OWNER).await.unwrap();
        let b = resolve(&store, &cache, &cfg, AGENT, OWNER).await.unwrap();
        assert_eq!(a.model, b.model);
        assert_eq!(store.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
