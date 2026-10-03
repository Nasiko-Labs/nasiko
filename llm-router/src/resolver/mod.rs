//! Config resolution: `(agent_id, owner_id)` → [`ResolvedConfig`].
//!
//! Reads the agent's resolved LLM config (TTL-cached) and the owner's provider key, applying
//! the backward-compat defaults and the platform-key fallback.
//!
//! Configs live in the per-user `llm_configs` library (see `server::llm_configs`). The config
//! for an agent is resolved as: the **attached** config (`agents.llm_config_id`) → else the
//! **agent owner's default** config (`llm_configs.is_default`) → else none. Ownership is
//! per-user, so the resolved config's owner always matches the agent owner whose secret store
//! the API key is read from.
//!
//! **When the agent resolves to a config it is authoritative** — the incoming request's
//! `model`/provider are ignored (a configured agent is routed where its config says).
//! **When the agent has no `llm_config`** we honor what the request itself asked for — the
//! provider implied by the inbound SDK surface and the request body's `model` — falling
//! back to the platform defaults (`DEFAULT_PROVIDER`/`DEFAULT_MODEL`) only as the last-resort
//! safety net. Both hints ride in via [`RequestHint`].
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
use crate::providers::ProviderDialect;

mod cache;
pub use cache::ConfigCache;

/// Per-agent routing config, as stored in `agents.llm_config` (JSONB).
#[derive(Debug, Clone, Deserialize)]
pub struct LLMConfig {
    pub provider: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub fallback_models: Vec<String>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<i64>,
    #[serde(default)]
    pub api_key_secret_name: Option<String>,
    /// Compliance lock (Level 1): when `true`, the model router never re-selects — the
    /// pinned model is used and fallbacks are disabled. Lives inside the `llm_config`
    /// JSONB, so no schema migration is needed (see migration 016 for the blob shape).
    #[serde(default)]
    pub pinned: bool,
    /// The model to pin to when `pinned`. `None` ⇒ pin to `model` (the configured model),
    /// i.e. "lock whatever is configured; don't let the router change it".
    #[serde(default)]
    pub pinned_model: Option<String>,
    /// Per-config tier→model overrides. When set, the smart router uses these instead of
    /// the global `model_registry` for tier-based routing. `None` = fall through to the
    /// global registry.
    #[serde(default)]
    pub tier1_model: Option<String>,
    #[serde(default)]
    pub tier2_model: Option<String>,
    #[serde(default)]
    pub tier3_model: Option<String>,
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
    /// Whether `model`/`provider` came from an explicit `llm_config` row (vs. the platform
    /// defaults). Used by the model router only to tag Level 4 (config) vs Level 5 (default).
    pub has_llm_config: bool,
    /// The compliance-locked model (Level 1), or `None` when the agent isn't pinned. When
    /// `Some`, the router returns it directly and the chat handler disables fallbacks.
    pub pinned_model: Option<String>,
    /// Per-config tier→model overrides from the user's `llm_configs` row. When set, the
    /// smart router checks these before the global `model_registry`.
    pub tier1_model: Option<String>,
    pub tier2_model: Option<String>,
    pub tier3_model: Option<String>,
    /// Whether the call is paid with the platform's key (vs. the owner's own
    /// `user_secrets` key). Recorded on usage rows so platform-paid spend can be
    /// metered separately from bring-your-own-key spend.
    pub platform_paid: bool,
    /// The custom (DB-registered) endpoint this call goes to. `None` ⇒ a built-in
    /// provider at its env-configured base URL. Resolved from the `custom_providers`
    /// row so both the destination URL and the wire dialect follow the resolved
    /// config rather than only env config.
    pub custom_endpoint: Option<CustomEndpoint>,
    /// Whether this agent is a coding-agent CLI integration. See
    /// [`AgentConfigResult::is_coding_agent`] — the chat handler uses this to derive
    /// model-routing boundary signals from the transcript instead of the (permanently
    /// unreachable, for these agents) `flows`-table lookup.
    pub is_coding_agent: bool,
    /// Whether this agent opted into payload compression. Per-agent by design: compression
    /// changes what the model sees, so its blast radius is one agent. See `crate::compress`.
    pub compress_enabled: bool,
    /// Whether this agent opted into compact tool definitions (`agents.compact_tools_enabled`).
    /// The fleet flag `GatewayConfig::compact_tools_enabled` is the gate; this is the choice.
    pub compact_tools_enabled: bool,
}

/// What the incoming request itself asked for, used **only** when the agent has no
/// `llm_config` — the passthrough hint that's honored before the platform defaults.
///
/// `provider` is the label implied by the inbound SDK surface
/// ([`InboundFormat::provider_label`](crate::inbound::InboundFormat::provider_label)); `model`
/// is the request body's `model` field. Both come from the same SDK call, so honoring them
/// together yields a self-consistent `(provider, model)` pair. A configured agent
/// (`llm_config` present) ignores this entirely.
#[derive(Debug, Clone, Copy, Default)]
pub struct RequestHint<'a> {
    /// Destination provider implied by the caller's SDK surface. `None` ⇒ use the default.
    pub provider: Option<&'a str>,
    /// The `model` from the request body. `None` ⇒ use the default.
    pub model: Option<&'a str>,
}

/// Result of resolving an agent's config from the DB.
#[derive(Debug, Clone)]
pub struct AgentConfigResult {
    /// The resolved `llm_config` row (None = no config, use platform defaults).
    pub config: Option<LLMConfig>,
    /// Agent-level model pin (`agents.pinned_model`). Overrides config-level pinning.
    pub agent_pinned_model: Option<String>,
    /// Whether this agent is a coding-agent CLI integration (`agents.metadata` carries an
    /// `integration_id`, set by `nasiko connect` — see `cli::commands::coding_agent_router`).
    /// These agents are never dispatched through the orchestrator, so they never have a
    /// `flows` row; the chat handler uses this to derive boundary signals from the
    /// transcript instead ([`crate::routing::BoundarySignals::for_coding_agent`]).
    pub is_coding_agent: bool,
    /// Per-agent opt-in for payload compression (`agents.compress_enabled`).
    pub compress_enabled: bool,
    /// Per-agent opt-in for compact tool definitions (`agents.compact_tools_enabled`).
    pub compact_tools_enabled: bool,
}

/// Where a custom endpoint lives and how it wants to be addressed. Carried on
/// [`ResolvedConfig`] so the provider client can build the right URL and credential
/// header without re-reading the DB.
#[derive(Debug, Clone)]
pub struct CustomEndpoint {
    /// The endpoint's base URL (used in place of the built-in one).
    pub base_url: String,
    /// Wire dialect: plain OpenAI-compatible, Azure OpenAI, …
    pub dialect: ProviderDialect,
}

/// An admin-registered custom provider (`custom_providers` table), resolved by its
/// `label`. Read per request through [`RegistryStore`] — deliberately uncached so a
/// key rotation or delete takes effect immediately on every replica (the api key is
/// decrypted in the impl, so mocks can supply plaintext).
#[derive(Debug, Clone)]
pub struct CustomProvider {
    /// The endpoint's base URL (used in place of the built-in one).
    pub base_url: String,
    /// Wire dialect this endpoint speaks, from the row's `kind`/`api_version`.
    pub dialect: ProviderDialect,
    /// The decrypted platform-owned API key for this endpoint.
    pub api_key: String,
    /// Last-resort model, used in place of the global `DEFAULT_MODEL`. See §4.3.
    pub default_model: Option<String>,
}

/// Storage seam for the resolver — mockable in tests.
#[async_trait]
pub trait RegistryStore: Send + Sync {
    /// Resolve the agent's config: attached (`agents.llm_config_id`) → agent owner's default
    /// (`llm_configs.is_default`) → none. `Ok(None)` = no agent row; `Ok(Some(..))` = agent
    /// exists (config may or may not be present).
    async fn fetch_llm_config(
        &self,
        agent_id: Uuid,
    ) -> Result<Option<AgentConfigResult>, sqlx::Error>;

    /// Encrypted secret value for `(owner_id, name)`, or `None` if absent.
    async fn fetch_user_secret(
        &self,
        owner_id: Uuid,
        name: &str,
    ) -> Result<Option<String>, sqlx::Error>;

    /// The live flow named by a forwarded traceparent's trace id, together
    /// with whether `agent_id` is a recorded `flow_participants` member of it.
    /// "Live" = `status = 'running'` and younger than `window_secs` (a row
    /// whose completion marking never ran ages out of attribution).
    /// `Ok(None)` = no live flow ⇒ the caller must deny attribution.
    async fn fetch_live_flow(
        &self,
        flow_id: &str,
        agent_id: Uuid,
        window_secs: i64,
    ) -> Result<Option<crate::routing::attribution::LiveFlow>, sqlx::Error>;

    /// The active custom provider registered under `label`, or `None`. Decryption
    /// of the stored api key happens in the impl, so mocks can supply a plaintext
    /// key. Read per request (never cached) so key rotation and delete take effect
    /// immediately — see §2.4 of the custom-provider plan.
    async fn fetch_custom_provider(
        &self,
        label: &str,
    ) -> Result<Option<CustomProvider>, sqlx::Error>;
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

/// A row of the `llm_configs` library, in column order — mapped into [`LLMConfig`].
type ConfigRow = (
    String,                         // provider
    Option<String>,                 // model
    sqlx::types::Json<Vec<String>>, // fallback_models (JSONB)
    Option<f64>,                    // temperature
    Option<i64>,                    // max_tokens
    Option<String>,                 // api_key_secret_name
    bool,                           // pinned
    Option<String>,                 // pinned_model
    Option<String>,                 // tier1_model
    Option<String>,                 // tier2_model
    Option<String>,                 // tier3_model
);

/// The `llm_configs` columns the resolver reads, in [`ConfigRow`] order.
const CONFIG_COLS: &str = "provider, model, fallback_models, temperature, max_tokens, \
     api_key_secret_name, pinned, pinned_model, tier1_model, tier2_model, tier3_model";

fn row_to_config(r: ConfigRow) -> LLMConfig {
    LLMConfig {
        provider: r.0,
        model: r.1,
        fallback_models: r.2.0,
        temperature: r.3,
        max_tokens: r.4,
        api_key_secret_name: r.5,
        pinned: r.6,
        pinned_model: r.7,
        tier1_model: r.8,
        tier2_model: r.9,
        tier3_model: r.10,
    }
}

impl PgRegistry {
    async fn load_config_by_id(&self, id: Uuid) -> Result<Option<LLMConfig>, sqlx::Error> {
        let row: Option<ConfigRow> = sqlx::query_as(&format!(
            "SELECT {CONFIG_COLS} FROM llm_configs WHERE id = $1 AND deleted_at IS NULL"
        ))
        .bind(id)
        .fetch_optional(&self.db)
        .await?;
        Ok(row.map(row_to_config))
    }

    async fn load_owner_default(&self, owner_id: Uuid) -> Result<Option<LLMConfig>, sqlx::Error> {
        let row: Option<ConfigRow> = sqlx::query_as(&format!(
            "SELECT {CONFIG_COLS} FROM llm_configs \
             WHERE created_by = $1 AND is_default AND deleted_at IS NULL"
        ))
        .bind(owner_id)
        .fetch_optional(&self.db)
        .await?;
        Ok(row.map(row_to_config))
    }
}

#[async_trait]
impl RegistryStore for PgRegistry {
    async fn fetch_llm_config(
        &self,
        agent_id: Uuid,
    ) -> Result<Option<AgentConfigResult>, sqlx::Error> {
        // The agent's attached config id, owner, agent-level pin, and server-managed
        // coding-agent identity. Generic agent metadata must never grant this exemption.
        // A missing row → NoRegistryEntry upstream.
        let agent: Option<(Option<Uuid>, Uuid, Option<String>, bool, bool, bool)> = sqlx::query_as(
            "SELECT llm_config_id, owner_id, pinned_model, \
                        coding_agent_integration_id IS NOT NULL, \
                        compress_enabled, compact_tools_enabled \
                 FROM agents WHERE id = $1",
        )
        .bind(agent_id)
        .fetch_optional(&self.db)
        .await?;
        let Some((
            config_id,
            owner_id,
            agent_pinned_model,
            is_coding_agent,
            compress_enabled,
            compact_tools_enabled,
        )) = agent
        else {
            return Ok(None);
        };

        // Attached config wins; else the owner's default; else none (platform defaults).
        let attached = match config_id {
            Some(cid) => self.load_config_by_id(cid).await?,
            None => None,
        };
        let config = match attached {
            Some(cfg) => Some(cfg),
            None => self.load_owner_default(owner_id).await?,
        };
        Ok(Some(AgentConfigResult {
            config,
            agent_pinned_model,
            is_coding_agent,
            compress_enabled,
            compact_tools_enabled,
        }))
    }

    async fn fetch_user_secret(
        &self,
        owner_id: Uuid,
        name: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT encrypted_value FROM user_secrets WHERE user_id = $1 AND name = $2",
        )
        .bind(owner_id)
        .bind(name)
        .fetch_optional(&self.db)
        .await?;
        Ok(row.map(|(v,)| v))
    }

    async fn fetch_live_flow(
        &self,
        flow_id: &str,
        agent_id: Uuid,
        window_secs: i64,
    ) -> Result<Option<crate::routing::attribution::LiveFlow>, sqlx::Error> {
        let row: Option<(Option<Uuid>, Option<String>, Option<String>, bool)> = sqlx::query_as(
            "SELECT f.user_id, f.metadata->>'context_id', f.metadata->>'mode', \
                    EXISTS(SELECT 1 FROM flow_participants fp \
                           WHERE fp.flow_id = f.flow_id AND fp.agent_id = $2) \
             FROM flows f \
             WHERE f.flow_id = $1 \
               AND f.status = 'running' \
               AND f.created_at > now() - make_interval(secs => $3)",
        )
        .bind(flow_id)
        .bind(agent_id)
        .bind(window_secs as f64)
        .fetch_optional(&self.db)
        .await?;
        Ok(
            row.map(|(user_id, context_id, mode, agent_is_participant)| {
                crate::routing::attribution::LiveFlow {
                    user_id,
                    context_id,
                    mode,
                    agent_is_participant,
                }
            }),
        )
    }

    async fn fetch_custom_provider(
        &self,
        label: &str,
    ) -> Result<Option<CustomProvider>, sqlx::Error> {
        let row: Option<(String, String, Option<String>, String, Option<String>)> = sqlx::query_as(
            "SELECT base_url, kind, api_version, encrypted_api_key, default_model \
             FROM custom_providers WHERE label = $1 AND deleted_at IS NULL",
        )
        .bind(label)
        .fetch_optional(&self.db)
        .await?;
        let Some((base_url, kind, api_version, encrypted_api_key, default_model)) = row else {
            return Ok(None);
        };
        // Shared platform credential (not a per-user secret) → platform-settings scope.
        let api_key = SecretsCrypto::for_platform_settings()
            .decrypt(&encrypted_api_key)
            .map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
        Ok(Some(CustomProvider {
            base_url,
            dialect: ProviderDialect::from_kind(&kind, api_version.as_deref()),
            api_key,
            default_model,
        }))
    }
}

/// Resolve `(agent_id, owner_id)` into a [`ResolvedConfig`].
pub async fn resolve(
    store: &dyn RegistryStore,
    cache: &ConfigCache,
    cfg: &GatewayConfig,
    agent_id: &str,
    owner_id: &str,
    hint: RequestHint<'_>,
) -> Result<ResolvedConfig, GatewayError> {
    // A non-UUID agent_id can't match any row → treat as "no registry entry".
    let agent_uuid = Uuid::parse_str(agent_id)
        .map_err(|_| GatewayError::NoRegistryEntry(agent_id.to_string()))?;

    let agent_result = load_llm_config(store, cache, agent_uuid, agent_id).await?;
    let llm_config = agent_result.config;
    let agent_pinned_model = agent_result.agent_pinned_model;
    let is_coding_agent = agent_result.is_coding_agent;
    let compress_enabled = agent_result.compress_enabled;
    let compact_tools_enabled = agent_result.compact_tools_enabled;
    let has_llm_config = llm_config.is_some();
    let secret_name = plan_secret_name(&llm_config);

    // Decide the destination provider up front (same rule `plan_config` uses) so a
    // custom (DB-registered) provider row can be loaded before the pure planning
    // step. A non-built-in name that resolves to no active row is a client error —
    // never a silent fallthrough to OpenAI (that would send the platform key to an
    // unknown endpoint; see the `platform_key_for` fail-closed change).
    let provider_name = destination_provider(&llm_config, cfg, hint);
    let custom = if is_builtin_provider(&provider_name) {
        None
    } else {
        match store
            .fetch_custom_provider(&provider_name)
            .await
            .map_err(|e| GatewayError::Internal(format!("custom provider read failed: {e}")))?
        {
            Some(cp) => Some(cp),
            None => {
                return Err(GatewayError::BadRequest(format!(
                    "provider '{provider_name}' is not a registered custom provider"
                )));
            }
        }
    };

    let plan = plan_config(
        llm_config,
        cfg,
        hint,
        agent_pinned_model.as_deref(),
        custom.as_ref(),
    );
    let api_key = resolve_api_key(
        store,
        cfg,
        owner_id,
        &plan.provider,
        plan.api_key_secret_name.as_deref(),
        custom.as_ref(),
    )
    .await?;

    // Mirrors resolve_api_key: a secret name + non-empty owner uses the owner's key
    // (or errors) — everything else is a platform-owned key (the built-in platform
    // key or the custom provider's stored key).
    let platform_paid = secret_name.is_none() || owner_id.is_empty();
    let resolved = ResolvedConfig {
        litellm_model: format!("{}/{}", plan.provider, plan.model),
        provider: plan.provider,
        model: plan.model,
        api_key,
        fallback_models: plan.fallback_models,
        temperature: plan.temperature,
        max_tokens: plan.max_tokens,
        has_llm_config,
        pinned_model: plan.pinned_model,
        tier1_model: plan.tier1_model,
        tier2_model: plan.tier2_model,
        tier3_model: plan.tier3_model,
        platform_paid,
        custom_endpoint: custom.as_ref().map(|c| CustomEndpoint {
            base_url: c.base_url.clone(),
            dialect: c.dialect.clone(),
        }),
        is_coding_agent,
        compress_enabled,
        compact_tools_enabled,
    };
    tracing::info!(
        target: "nasiko::llm_router::resolver",
        %agent_id,
        has_llm_config,
        provider = %resolved.provider,
        model = %resolved.model,
        litellm_model = %resolved.litellm_model,
        pinned_model = ?resolved.pinned_model,
        temperature = ?resolved.temperature,
        max_tokens = ?resolved.max_tokens,
        fallback_models = ?resolved.fallback_models,
        api_key_secret_name = ?secret_name,
        api_key_source = if secret_name.is_some() && !owner_id.is_empty() { "per-user-secret" } else { "platform-key" },
        api_key_present = !resolved.api_key.is_empty(),
        "resolver: resolved agent config (source: {})",
        if has_llm_config { "agents.llm_config row" } else { "platform defaults (no llm_config)" }
    );
    Ok(resolved)
}

/// The `api_key_secret_name` from an optional `llm_config` — used only for logging which
/// key source the resolver will pick (never the secret value itself).
fn plan_secret_name(llm_config: &Option<LLMConfig>) -> Option<String> {
    llm_config
        .as_ref()
        .and_then(|c| c.api_key_secret_name.clone())
}

/// Load `llm_config` via the cache (for the config part), falling back to the store.
/// A missing agent row is an error (not cached); a present row is cached. The agent-level
/// pin and the coding-agent flag are always read from the store (not cached) so changes
/// (a re-pin, a fresh `nasiko connect`) take effect immediately.
async fn load_llm_config(
    store: &dyn RegistryStore,
    cache: &ConfigCache,
    agent_uuid: Uuid,
    agent_id: &str,
) -> Result<AgentConfigResult, GatewayError> {
    if let Some(hit) = cache.get(agent_uuid) {
        tracing::debug!(
            target: "nasiko::llm_router::resolver",
            %agent_id,
            source = "config-cache",
            has_llm_config = hit.is_some(),
            provider = ?hit.as_ref().map(|c| &c.provider),
            model = ?hit.as_ref().map(|c| &c.model),
            pinned = ?hit.as_ref().map(|c| c.pinned),
            pinned_model = ?hit.as_ref().and_then(|c| c.pinned_model.clone()),
            "resolver: llm_config cache HIT (agent pin resolved from store)"
        );
        // Config is cached, but we still need the agent-level pin and coding-agent flag
        // from the store. Re-fetch just the agent row; fall back to safe defaults on error.
        let agent_row = store.fetch_llm_config(agent_uuid).await.ok().flatten();
        let agent_pin = agent_row
            .as_ref()
            .and_then(|r| r.agent_pinned_model.clone());
        let compress_enabled = agent_row.as_ref().is_some_and(|r| r.compress_enabled);
        let compact_tools_enabled = agent_row.as_ref().is_some_and(|r| r.compact_tools_enabled);
        let is_coding_agent = agent_row.is_some_and(|r| r.is_coding_agent);
        return Ok(AgentConfigResult {
            config: hit,
            agent_pinned_model: agent_pin,
            is_coding_agent,
            compress_enabled,
            compact_tools_enabled,
        });
    }
    tracing::debug!(
        target: "nasiko::llm_router::resolver",
        %agent_id, "resolver: llm_config cache MISS — resolving config from DB"
    );
    match store
        .fetch_llm_config(agent_uuid)
        .await
        .map_err(|e| GatewayError::Internal(format!("registry read failed: {e}")))?
    {
        None => {
            tracing::warn!(
                target: "nasiko::llm_router::resolver",
                %agent_id, "resolver: no agents row for agent_id — NoRegistryEntry"
            );
            Err(GatewayError::NoRegistryEntry(agent_id.to_string()))
        }
        Some(result) => {
            tracing::info!(
                target: "nasiko::llm_router::resolver",
                %agent_id,
                source = "database",
                has_llm_config = result.config.is_some(),
                agent_pinned_model = ?result.agent_pinned_model,
                provider = ?result.config.as_ref().map(|c| &c.provider),
                model = ?result.config.as_ref().map(|c| &c.model),
                fallback_models = ?result.config.as_ref().map(|c| &c.fallback_models),
                temperature = ?result.config.as_ref().and_then(|c| c.temperature),
                max_tokens = ?result.config.as_ref().and_then(|c| c.max_tokens),
                pinned = ?result.config.as_ref().map(|c| c.pinned),
                pinned_model = ?result.config.as_ref().and_then(|c| c.pinned_model.clone()),
                api_key_secret_name = ?result.config.as_ref().and_then(|c| c.api_key_secret_name.clone()),
                "resolver: loaded llm_config from DB (now caching config)"
            );
            cache.put(agent_uuid, result.config.clone());
            Ok(result)
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
    pinned_model: Option<String>,
    tier1_model: Option<String>,
    tier2_model: Option<String>,
    tier3_model: Option<String>,
}

/// Whether `provider` is one of the built-in providers the router speaks natively.
/// Everything else is a candidate custom (DB-registered) provider.
fn is_builtin_provider(provider: &str) -> bool {
    matches!(provider, "openai" | "anthropic" | "gemini")
}

/// The destination provider name, decided exactly as [`plan_config`] decides it:
/// the config's provider when the agent is configured, else the request hint's
/// provider, else the platform default. Computed before `plan_config` so a custom
/// provider row can be loaded for its key + default model.
fn destination_provider(
    llm_config: &Option<LLMConfig>,
    cfg: &GatewayConfig,
    hint: RequestHint<'_>,
) -> String {
    match llm_config {
        Some(c) => c.provider.clone(),
        None => hint
            .provider
            .map(str::to_string)
            .unwrap_or_else(|| cfg.default_provider.clone()),
    }
}

fn plan_config(
    llm_config: Option<LLMConfig>,
    cfg: &GatewayConfig,
    hint: RequestHint<'_>,
    agent_pinned_model: Option<&str>,
    custom: Option<&CustomProvider>,
) -> ConfigPlan {
    // Last-resort model: a custom provider almost never serves the global
    // DEFAULT_MODEL (gpt-4o-mini), so its own registered default is used instead.
    let default_model = custom
        .and_then(|c| c.default_model.clone())
        .unwrap_or_else(|| cfg.default_model.clone());
    match llm_config {
        Some(c) => {
            // Configured agent: the config is authoritative — the request hint is ignored.
            // When `model` is None (user relies on tier routing), pick the first available
            // tier model as the Level 4/5 fallback so the router always has something.
            let fallback = c
                .model
                .clone()
                .or_else(|| c.tier2_model.clone())
                .or_else(|| c.tier1_model.clone())
                .or_else(|| c.tier3_model.clone())
                .unwrap_or_else(|| default_model.clone());
            // Agent-level pin overrides config-level pin.
            let config_pin = c
                .pinned
                .then(|| c.pinned_model.clone().unwrap_or_else(|| fallback.clone()));
            let pinned_model = agent_pinned_model.map(str::to_string).or(config_pin);
            ConfigPlan {
                provider: c.provider,
                model: fallback,
                fallback_models: c.fallback_models,
                temperature: c.temperature,
                max_tokens: c.max_tokens,
                api_key_secret_name: c.api_key_secret_name,
                pinned_model,
                tier1_model: c.tier1_model,
                tier2_model: c.tier2_model,
                tier3_model: c.tier3_model,
            }
        }
        // No config: honor what the request asked for (provider from the SDK surface, model
        // from the request body), falling back to the platform defaults only per-field when
        // the request didn't supply one — DEFAULT_PROVIDER/DEFAULT_MODEL are the last-resort
        // safety net, not the first choice.
        None => ConfigPlan {
            provider: hint
                .provider
                .map(str::to_string)
                .unwrap_or_else(|| cfg.default_provider.clone()),
            model: hint
                .model
                .map(str::to_string)
                .unwrap_or_else(|| default_model.clone()),
            fallback_models: Vec::new(),
            temperature: None,
            max_tokens: None,
            api_key_secret_name: None,
            pinned_model: agent_pinned_model.map(str::to_string),
            tier1_model: None,
            tier2_model: None,
            tier3_model: None,
        },
    }
}

/// Resolve the provider API key. Per-user secret lookup requires a secret name **and**
/// a non-empty owner; an empty owner_id means "no per-user lookup" → platform key.
async fn resolve_api_key(
    store: &dyn RegistryStore,
    cfg: &GatewayConfig,
    owner_id: &str,
    provider: &str,
    secret_name: Option<&str>,
    custom: Option<&CustomProvider>,
) -> Result<String, GatewayError> {
    if let Some(name) = secret_name
        && !owner_id.is_empty()
    {
        // Non-empty-but-invalid owner_id is a token-minting bug → server error.
        let owner = Uuid::parse_str(owner_id).map_err(|_| {
            GatewayError::Internal(format!("invalid owner_id in token: {owner_id}"))
        })?;
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
    // No per-user secret: a custom provider pays with its own stored (platform-owned)
    // key; a built-in provider pays with the platform key for that provider.
    if let Some(cp) = custom {
        return Ok(cp.api_key.clone());
    }
    platform_key(cfg, provider)
}

/// Pure: the platform-owned fallback key for `provider`, or the 400 when none is configured.
fn platform_key(cfg: &GatewayConfig, provider: &str) -> Result<String, GatewayError> {
    let key = cfg.platform_key_for(provider);
    if key.is_empty() {
        Err(GatewayError::NoApiKey)
    } else {
        Ok(key.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const AGENT: &str = "11111111-1111-1111-1111-111111111111";
    const OWNER: &str = "22222222-2222-2222-2222-222222222222";

    #[derive(Default)]
    struct MockRegistry {
        config: Option<Option<LLMConfig>>,
        secret: Option<String>,
        agent_pinned_model: Option<String>,
        custom_provider: Option<CustomProvider>,
        compact_tools_enabled: bool,
    }

    #[async_trait]
    impl RegistryStore for MockRegistry {
        async fn fetch_llm_config(
            &self,
            _: Uuid,
        ) -> Result<Option<AgentConfigResult>, sqlx::Error> {
            Ok(self.config.as_ref().map(|c| AgentConfigResult {
                config: c.clone(),
                agent_pinned_model: self.agent_pinned_model.clone(),
                // Coding-agent detection and compression opt-in are exercised at the
                // handler level (handlers::chat), where they're actually consumed.
                is_coding_agent: false,
                compress_enabled: false,
                compact_tools_enabled: self.compact_tools_enabled,
            }))
        }
        async fn fetch_user_secret(&self, _: Uuid, _: &str) -> Result<Option<String>, sqlx::Error> {
            Ok(self.secret.clone())
        }
        async fn fetch_live_flow(
            &self,
            _: &str,
            _: Uuid,
            _: i64,
        ) -> Result<Option<crate::routing::attribution::LiveFlow>, sqlx::Error> {
            unreachable!("resolver tests never attribute flows")
        }
        async fn fetch_custom_provider(
            &self,
            _: &str,
        ) -> Result<Option<CustomProvider>, sqlx::Error> {
            Ok(self.custom_provider.clone())
        }
    }

    fn cache() -> ConfigCache {
        ConfigCache::new(Duration::from_secs(30))
    }

    fn cfg(provider: &str, model: &str, platform_key: &str) -> GatewayConfig {
        // Populate every provider's platform key with the same value: these tests
        // exercise resolution independent of which provider the key belongs to.
        // Provider-specific key selection is covered by the dedicated tests below.
        GatewayConfig {
            default_provider: provider.into(),
            default_model: model.into(),
            platform_openai_api_key: platform_key.into(),
            platform_anthropic_api_key: platform_key.into(),
            platform_gemini_api_key: platform_key.into(),
            ..Default::default()
        }
    }

    fn llm_config(provider: &str, model: &str, secret_name: Option<&str>) -> LLMConfig {
        LLMConfig {
            provider: provider.into(),
            model: Some(model.into()),
            fallback_models: vec!["openai/gpt-4o-mini".into()],
            temperature: Some(0.5),
            max_tokens: Some(1024),
            api_key_secret_name: secret_name.map(str::to_string),
            pinned: false,
            pinned_model: None,
            tier1_model: None,
            tier2_model: None,
            tier3_model: None,
        }
    }

    #[tokio::test]
    async fn defaults_when_no_llm_config() {
        // Empty hint + no config → the platform-default safety net.
        let store = MockRegistry {
            config: Some(None),
            secret: None,
            agent_pinned_model: None,
            custom_provider: None,
            compact_tools_enabled: false,
        };
        let r = resolve(
            &store,
            &cache(),
            &cfg("openai", "gpt-4o-mini", "platform-key"),
            AGENT,
            OWNER,
            RequestHint::default(),
        )
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
        let store = MockRegistry {
            config: None,
            secret: None,
            agent_pinned_model: None,
            custom_provider: None,
            compact_tools_enabled: false,
        };
        let err = resolve(
            &store,
            &cache(),
            &cfg("openai", "gpt-4o-mini", "k"),
            AGENT,
            OWNER,
            RequestHint::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, GatewayError::NoRegistryEntry(_)));
        assert_eq!(
            err.to_string(),
            format!("No registry entry for agent_id={AGENT}")
        );
    }

    #[tokio::test]
    async fn no_secret_and_no_platform_key_is_bad_request() {
        let store = MockRegistry {
            config: Some(None),
            secret: None,
            agent_pinned_model: None,
            custom_provider: None,
            compact_tools_enabled: false,
        };
        let err = resolve(
            &store,
            &cache(),
            &cfg("openai", "gpt-4o-mini", ""),
            AGENT,
            OWNER,
            RequestHint::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, GatewayError::NoApiKey));
    }

    #[tokio::test]
    async fn platform_key_is_selected_by_resolved_provider() {
        // Agent resolves to the anthropic default; only the anthropic platform key
        // should be handed to the provider — never the OpenAI key.
        let config = GatewayConfig {
            default_provider: "anthropic".into(),
            default_model: "claude-haiku-4-5".into(),
            platform_openai_api_key: "sk-openai".into(),
            platform_anthropic_api_key: "sk-ant".into(),
            ..Default::default()
        };
        let store = MockRegistry {
            config: Some(None),
            secret: None,
            agent_pinned_model: None,
            custom_provider: None,
            compact_tools_enabled: false,
        };
        let r = resolve(
            &store,
            &cache(),
            &config,
            AGENT,
            OWNER,
            RequestHint::default(),
        )
        .await
        .unwrap();
        assert_eq!(r.provider, "anthropic");
        assert_eq!(r.api_key, "sk-ant");
    }

    #[tokio::test]
    async fn provider_without_its_platform_key_is_bad_request() {
        // Only the OpenAI platform key is set, but the agent resolves to anthropic —
        // the mismatch that caused the "invalid x-api-key" 401 must now fail closed.
        let config = GatewayConfig {
            default_provider: "anthropic".into(),
            default_model: "claude-haiku-4-5".into(),
            platform_openai_api_key: "sk-openai".into(),
            ..Default::default()
        };
        let store = MockRegistry {
            config: Some(None),
            secret: None,
            agent_pinned_model: None,
            custom_provider: None,
            compact_tools_enabled: false,
        };
        let err = resolve(
            &store,
            &cache(),
            &config,
            AGENT,
            OWNER,
            RequestHint::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, GatewayError::NoApiKey));
    }

    #[tokio::test]
    async fn config_wins_over_request_hint() {
        // A configured agent ignores the request hint entirely — even a fully-populated
        // hint (openai/gpt-4o) can't override the anthropic llm_config.
        let store = MockRegistry {
            config: Some(Some(llm_config(
                "anthropic",
                "claude-3-5-sonnet-20241022",
                None,
            ))),
            secret: None,
            agent_pinned_model: None,
            custom_provider: None,
            compact_tools_enabled: false,
        };
        let hint = RequestHint {
            provider: Some("openai"),
            model: Some("gpt-4o"),
        };
        let r = resolve(
            &store,
            &cache(),
            &cfg("openai", "gpt-4o-mini", "platform-key"),
            AGENT,
            OWNER,
            hint,
        )
        .await
        .unwrap();
        assert_eq!(r.provider, "anthropic");
        assert_eq!(r.model, "claude-3-5-sonnet-20241022");
        assert_eq!(r.litellm_model, "anthropic/claude-3-5-sonnet-20241022");
        assert_eq!(r.fallback_models, vec!["openai/gpt-4o-mini".to_string()]);
        assert_eq!(r.temperature, Some(0.5));
        assert_eq!(r.max_tokens, Some(1024));
        assert_eq!(r.api_key, "platform-key"); // no secret_name → platform key
    }

    #[tokio::test]
    async fn no_config_honors_request_hint_over_defaults() {
        // No llm_config: the request's own provider (from the SDK surface) + model are
        // honored, not the platform defaults. Anthropic platform key follows the provider.
        let config = GatewayConfig {
            default_provider: "openai".into(),
            default_model: "gpt-4o-mini".into(),
            platform_openai_api_key: "sk-openai".into(),
            platform_anthropic_api_key: "sk-ant".into(),
            ..Default::default()
        };
        let store = MockRegistry {
            config: Some(None),
            secret: None,
            agent_pinned_model: None,
            custom_provider: None,
            compact_tools_enabled: false,
        };
        let hint = RequestHint {
            provider: Some("anthropic"),
            model: Some("claude-3-5-sonnet-20241022"),
        };
        let r = resolve(&store, &cache(), &config, AGENT, OWNER, hint)
            .await
            .unwrap();
        assert_eq!(r.provider, "anthropic");
        assert_eq!(r.model, "claude-3-5-sonnet-20241022");
        assert_eq!(r.litellm_model, "anthropic/claude-3-5-sonnet-20241022");
        assert_eq!(r.api_key, "sk-ant"); // key follows the request's provider, not the default
        assert!(!r.has_llm_config);
    }

    #[tokio::test]
    async fn no_config_falls_back_per_field_when_hint_absent() {
        // Safety net: a hint field that's None falls back to the platform default for that
        // field independently (here: provider from hint, model from default).
        let store = MockRegistry {
            config: Some(None),
            secret: None,
            agent_pinned_model: None,
            custom_provider: None,
            compact_tools_enabled: false,
        };
        let hint = RequestHint {
            provider: Some("openai"),
            model: None,
        };
        let r = resolve(
            &store,
            &cache(),
            &cfg("openai", "gpt-4o-mini", "platform-key"),
            AGENT,
            OWNER,
            hint,
        )
        .await
        .unwrap();
        assert_eq!(r.provider, "openai");
        assert_eq!(r.model, "gpt-4o-mini"); // model absent in hint → default safety net
    }

    #[tokio::test]
    async fn missing_secret_row_is_secret_not_found() {
        let store = MockRegistry {
            config: Some(Some(llm_config(
                "anthropic",
                "claude-x",
                Some("ANTHROPIC_API_KEY"),
            ))),
            secret: None,
            agent_pinned_model: None,
            custom_provider: None,
            compact_tools_enabled: false,
        };
        let err = resolve(
            &store,
            &cache(),
            &cfg("openai", "gpt-4o-mini", ""),
            AGENT,
            OWNER,
            RequestHint::default(),
        )
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
            config: Some(Some(llm_config(
                "anthropic",
                "claude-x",
                Some("ANTHROPIC_API_KEY"),
            ))),
            secret: Some(ciphertext),
            agent_pinned_model: None,
            custom_provider: None,
            compact_tools_enabled: false,
        };
        let r = resolve(
            &store,
            &cache(),
            &cfg("openai", "gpt-4o-mini", ""),
            AGENT,
            OWNER,
            RequestHint::default(),
        )
        .await
        .unwrap();
        assert_eq!(r.api_key, "sk-real-anthropic-key");
        assert_eq!(r.provider, "anthropic");
    }

    #[tokio::test]
    async fn pinned_with_explicit_model_exposes_that_model() {
        let mut c = llm_config("anthropic", "claude-x", None);
        c.pinned = true;
        c.pinned_model = Some("claude-locked".into());
        let store = MockRegistry {
            config: Some(Some(c)),
            secret: None,
            agent_pinned_model: None,
            custom_provider: None,
            compact_tools_enabled: false,
        };
        let r = resolve(
            &store,
            &cache(),
            &cfg("openai", "gpt-4o-mini", "platform-key"),
            AGENT,
            OWNER,
            RequestHint::default(),
        )
        .await
        .unwrap();
        // The pin surfaces on ResolvedConfig; the router (Level 1) applies it. The
        // configured model is untouched here.
        assert_eq!(r.pinned_model.as_deref(), Some("claude-locked"));
        assert_eq!(r.model, "claude-x");
    }

    #[tokio::test]
    async fn pinned_without_model_pins_to_configured_model() {
        let mut c = llm_config("anthropic", "claude-x", None);
        c.pinned = true; // no explicit pinned_model
        let store = MockRegistry {
            config: Some(Some(c)),
            secret: None,
            agent_pinned_model: None,
            custom_provider: None,
            compact_tools_enabled: false,
        };
        let r = resolve(
            &store,
            &cache(),
            &cfg("openai", "gpt-4o-mini", "platform-key"),
            AGENT,
            OWNER,
            RequestHint::default(),
        )
        .await
        .unwrap();
        assert_eq!(r.pinned_model.as_deref(), Some("claude-x"));
    }

    #[tokio::test]
    async fn not_pinned_has_no_pinned_model() {
        let store = MockRegistry {
            config: Some(Some(llm_config("anthropic", "claude-x", None))),
            secret: None,
            agent_pinned_model: None,
            custom_provider: None,
            compact_tools_enabled: false,
        };
        let r = resolve(
            &store,
            &cache(),
            &cfg("openai", "gpt-4o-mini", "platform-key"),
            AGENT,
            OWNER,
            RequestHint::default(),
        )
        .await
        .unwrap();
        assert!(r.pinned_model.is_none());
    }

    #[tokio::test]
    async fn second_resolve_is_cache_hit() {
        // The config is cached after the 1st fetch. The 2nd resolve still calls
        // fetch_llm_config to read the agent-level pin (not cached), but the config
        // itself comes from the cache. We verify both resolves produce the same model.
        let store = MockRegistry {
            config: Some(None),
            secret: None,
            agent_pinned_model: None,
            custom_provider: None,
            compact_tools_enabled: false,
        };
        let cache = cache();
        let cfg = cfg("openai", "gpt-4o-mini", "platform-key");
        let a = resolve(&store, &cache, &cfg, AGENT, OWNER, RequestHint::default())
            .await
            .unwrap();
        let b = resolve(&store, &cache, &cfg, AGENT, OWNER, RequestHint::default())
            .await
            .unwrap();
        assert_eq!(a.model, b.model);
        // Config is cached, so the cache should have an entry.
        assert!(cache.get(Uuid::parse_str(AGENT).unwrap()).is_some());
    }

    #[tokio::test]
    async fn compact_tools_opt_in_is_read_on_the_cache_miss_and_hit_paths() {
        // The config row is cached; the agent's own flags are re-read from the store on every
        // resolve. A hit that forgot to copy the flag would make the Settings switch look dead
        // for the cache TTL.
        let store = MockRegistry {
            config: Some(Some(llm_config("openai", "gpt-4o-mini", None))),
            compact_tools_enabled: true,
            ..Default::default()
        };
        let cache = cache();
        let cfg = cfg("openai", "gpt-4o-mini", "platform-key");
        let miss = resolve(&store, &cache, &cfg, AGENT, OWNER, RequestHint::default())
            .await
            .unwrap();
        assert!(miss.compact_tools_enabled);
        assert!(cache.get(Uuid::parse_str(AGENT).unwrap()).is_some());
        let hit = resolve(&store, &cache, &cfg, AGENT, OWNER, RequestHint::default())
            .await
            .unwrap();
        assert!(hit.compact_tools_enabled);
        let opted_out = MockRegistry {
            config: Some(Some(llm_config("openai", "gpt-4o-mini", None))),
            ..Default::default()
        };
        let r = resolve(
            &opted_out,
            &cache,
            &cfg,
            AGENT,
            OWNER,
            RequestHint::default(),
        )
        .await
        .unwrap();
        assert!(!r.compact_tools_enabled);
    }

    fn custom(base_url: &str, api_key: &str, default_model: &str) -> CustomProvider {
        CustomProvider {
            base_url: base_url.into(),
            dialect: ProviderDialect::OpenAi,
            api_key: api_key.into(),
            default_model: Some(default_model.into()),
        }
    }

    #[tokio::test]
    async fn custom_provider_resolves_base_url_and_own_key() {
        // A config naming a DB-registered custom provider resolves to that endpoint's
        // base URL and its own stored key — never the platform OpenAI key.
        let store = MockRegistry {
            config: Some(Some(llm_config("my-gateway", "llama-3.1-70b", None))),
            secret: None,
            agent_pinned_model: None,
            custom_provider: Some(custom(
                "https://gw.internal/v1",
                "sk-gateway",
                "llama-3.1-8b",
            )),
            compact_tools_enabled: false,
        };
        let r = resolve(
            &store,
            &cache(),
            &cfg("openai", "gpt-4o-mini", "sk-platform-openai"),
            AGENT,
            OWNER,
            RequestHint::default(),
        )
        .await
        .unwrap();
        assert_eq!(r.provider, "my-gateway");
        assert_eq!(r.model, "llama-3.1-70b");
        assert_eq!(r.litellm_model, "my-gateway/llama-3.1-70b");
        assert_eq!(r.api_key, "sk-gateway");
        let endpoint = r
            .custom_endpoint
            .expect("custom provider resolves an endpoint");
        assert_eq!(endpoint.base_url, "https://gw.internal/v1");
        assert_eq!(endpoint.dialect, ProviderDialect::OpenAi);
    }

    #[tokio::test]
    async fn custom_provider_default_model_beats_global_default() {
        // Config has no model and no tier models: the last-resort fallback must be the
        // custom provider's own default_model, not the global DEFAULT_MODEL (gpt-4o-mini).
        let mut c = llm_config("my-gateway", "unused", None);
        c.model = None;
        let store = MockRegistry {
            config: Some(Some(c)),
            secret: None,
            agent_pinned_model: None,
            custom_provider: Some(custom(
                "https://gw.internal/v1",
                "sk-gateway",
                "llama-3.1-8b",
            )),
            compact_tools_enabled: false,
        };
        let r = resolve(
            &store,
            &cache(),
            &cfg("openai", "gpt-4o-mini", "sk-platform-openai"),
            AGENT,
            OWNER,
            RequestHint::default(),
        )
        .await
        .unwrap();
        assert_eq!(r.model, "llama-3.1-8b");
        assert_eq!(r.litellm_model, "my-gateway/llama-3.1-8b");
    }

    #[tokio::test]
    async fn unregistered_custom_provider_is_bad_request() {
        // A non-built-in provider name with no active custom_providers row is a client
        // error — it must not fall through to the OpenAI platform key + base URL.
        let store = MockRegistry {
            config: Some(Some(llm_config("ghost-gateway", "some-model", None))),
            secret: None,
            agent_pinned_model: None,
            custom_provider: None,
            compact_tools_enabled: false,
        };
        let err = resolve(
            &store,
            &cache(),
            &cfg("openai", "gpt-4o-mini", "sk-platform-openai"),
            AGENT,
            OWNER,
            RequestHint::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, GatewayError::BadRequest(_)));
    }
}
