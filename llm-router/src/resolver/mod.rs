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
//! **A shared agent runs on the caller's config, not the owner's.** When the user a call is
//! attributed to (`flows.user_id`) is not the agent's owner — the agent was shared with them —
//! the call resolves against *that user's* config (the one they chose for this agent, else their
//! default) and secret store ([`ConfigSubject::SharedUser`]); the agent's attached config and the
//! owner's keys are not read. A shared user with no such config is refused
//! ([`GatewayError::NoSharedUserConfig`]), never silently put on the owner's key. The one way
//! onto the owner's config and keys is [`crate::payer::SharedCallPayer`] answering
//! [`crate::payer::Payer::Owner`], which selects [`ConfigSubject::Sponsored`] instead: the
//! owner's config and keys, and the platform's own key where the config stores none.
//! [`ConfigSubject::Platform`] is a call the deployment covers outright: the platform defaults
//! and the platform's own key, with no config, no agent pin and no requested model read.
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
use chrono::{DateTime, Utc};
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
    /// Embedding destination, read **only** by [`resolve_embeddings`]. Deliberately a
    /// separate provider/model/key triple rather than a reuse of the chat columns: an
    /// embedding model is a vector space, not a quality tier, so it can never be
    /// substituted for the chat model. The pair is all-or-nothing (DB CHECK); the key
    /// name is optional and empty means "the platform key for this provider".
    #[serde(default)]
    pub embedding_provider: Option<String>,
    #[serde(default)]
    pub embedding_model: Option<String>,
    #[serde(default)]
    pub embedding_api_key_secret_name: Option<String>,
    /// When `true`, [`embedding_model`](Self::embedding_model) beats the model named on
    /// the request. Off by default — the agent knows which vector space its stored data
    /// lives in and the platform does not, so the agent's own choice wins unless an
    /// operator deliberately takes it over.
    #[serde(default)]
    pub embedding_model_override: bool,
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
    /// Whether this agent is an A2A agent the CLI discovered on a user's machine. See
    /// [`AgentConfigResult::is_discovered_agent`].
    pub is_discovered_agent: bool,
    /// Whether this agent opted into payload compression. Per-agent by design: compression
    /// changes what the model sees, so its blast radius is one agent. See `crate::compress`.
    pub compress_enabled: bool,
    /// The agent the call is for. OpenAI groups its prompt cache by it (`prompt_cache_key`,
    /// see `providers::openai`). Empty when no agent is known, or once the endpoint has
    /// rejected the key (`providers::fallback::strip_param`).
    pub agent_id: String,
}

/// The resolved **embedding** call configuration.
///
/// A deliberately narrow type: the embedding path must not depend on the chat columns,
/// and "must not" is enforced here by the type system rather than by a comment. There is
/// no `fallback_models`, no `temperature`, no `pinned_model` and no tier field, so no
/// future edit can quietly reintroduce chat semantics — or chaining — on this path.
#[derive(Debug, Clone)]
pub struct ResolvedEmbeddingConfig {
    pub provider: String,
    /// Bare provider-native embedding model id — sent upstream, echoed in the response,
    /// and recorded on the usage row.
    pub model: String,
    pub api_key: String,
    /// The custom (DB-registered) endpoint this call goes to. `None` ⇒ a built-in
    /// provider at its env-configured base URL.
    pub custom_endpoint: Option<CustomEndpoint>,
    /// Whether the call is paid with a platform-owned key. Never derived from the chat
    /// `api_key_secret_name` — an agent with a chat key but no embedding key spends the
    /// platform's key and must be metered as such.
    pub platform_paid: bool,
}

impl ResolvedEmbeddingConfig {
    /// A minimal built-in-provider destination, for tests that exercise a provider's
    /// embeddings translation rather than resolution. Test-only so production code has
    /// to go through [`resolve_embeddings`] and cannot hand-assemble a destination that
    /// skipped the priority order.
    #[cfg(test)]
    pub(crate) fn for_test(provider: &str, model: &str, api_key: &str) -> Self {
        Self {
            provider: provider.to_string(),
            model: model.to_string(),
            api_key: api_key.to_string(),
            custom_endpoint: None,
            platform_paid: true,
        }
    }
}

/// Which level of [`resolve_embeddings`]'s priority order supplied the model. Logged so
/// an operator can see *why* a given model ran without re-deriving the rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingSource {
    /// The config's `embedding_model`, taken ahead of the request's because
    /// `embedding_model_override` is set.
    ConfigOverride,
    /// The `model` field of the request itself.
    Request,
    /// The config's `embedding_model`.
    Config,
    /// The organization default in `settings`.
    OrgDefault,
}

impl EmbeddingSource {
    /// Stable label for logs.
    pub fn label(self) -> &'static str {
        match self {
            Self::ConfigOverride => "config-override",
            Self::Request => "request",
            Self::Config => "config",
            Self::OrgDefault => "org-default",
        }
    }
}

/// The organization-wide embedding default (`settings`), already decrypted.
///
/// `api_key` is `None` when the operator set a provider and model but no key — that
/// means "use the platform key for this provider", the same as an empty secret name on
/// a per-config embedding entry.
#[derive(Debug, Clone)]
pub struct OrgEmbeddingDefault {
    pub provider: String,
    pub model: String,
    pub api_key: Option<String>,
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
    /// Whether this agent is an A2A agent the CLI discovered running on a user's machine
    /// (`agents.discovered_on` set). Like a coding agent it is never dispatched through the
    /// orchestrator, so its calls carry no flow to attribute and are billed to its owner; unlike
    /// one it is an ordinary agent in every other respect (brevity, tool compaction).
    pub is_discovered_agent: bool,
    /// Per-agent opt-in for payload compression (`agents.compress_enabled`).
    pub compress_enabled: bool,
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
/// What the router needs to know about a live agent to accept its identity token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentIdentity {
    pub owner_id: Uuid,
    /// When the owner last revoked the agent's LLM routing tokens; a token issued before it is
    /// refused. `None` = never revoked.
    pub llm_tokens_revoked_at: Option<DateTime<Utc>>,
}

#[async_trait]
pub trait RegistryStore: Send + Sync {
    /// Resolve the agent's config: attached (`agents.llm_config_id`) → agent owner's default
    /// (`llm_configs.is_default`) → none. `Ok(None)` = no agent row; `Ok(Some(..))` = agent
    /// exists (config may or may not be present).
    async fn fetch_llm_config(
        &self,
        agent_id: Uuid,
    ) -> Result<Option<AgentConfigResult>, sqlx::Error>;

    /// The current owner of a live (not deleted) agent, or `None` when there is
    /// no such agent. Read per request, never cached: it decides whose keys pay,
    /// and an ownership transfer must take effect at once.
    async fn fetch_agent_owner(&self, agent_id: Uuid) -> Result<Option<Uuid>, sqlx::Error>;

    /// [`fetch_agent_owner`](Self::fetch_agent_owner) plus when the owner last revoked the
    /// agent's LLM routing tokens, in one read. Read per request for the same reason. The
    /// default reports no revocation, for stores that do not track it.
    async fn fetch_agent_identity(
        &self,
        agent_id: Uuid,
    ) -> Result<Option<AgentIdentity>, sqlx::Error> {
        Ok(self
            .fetch_agent_owner(agent_id)
            .await?
            .map(|owner_id| AgentIdentity {
                owner_id,
                llm_tokens_revoked_at: None,
            }))
    }

    /// [`fetch_llm_config`](Self::fetch_llm_config) without the config: the agent-level pin and
    /// flags only, for a caller whose config came from the cache. What it returns in `config` is
    /// unspecified. The default reads everything, so stores that do not care need not implement it.
    async fn fetch_agent_flags(
        &self,
        agent_id: Uuid,
    ) -> Result<Option<AgentConfigResult>, sqlx::Error> {
        self.fetch_llm_config(agent_id).await
    }

    /// The user's own default config (`llm_configs.is_default`, created by them), or `None` when
    /// they have none — what a shared agent runs on when this user chats with it. The default
    /// answers `None`, so a store that does not implement it refuses shared callers.
    async fn fetch_user_default_config(
        &self,
        _user_id: Uuid,
    ) -> Result<Option<LLMConfig>, sqlx::Error> {
        Ok(None)
    }

    /// Whether `user` chose one of their own live configs for `agent` (`agent_user_llm_configs`).
    /// The default answers `false`, so a store that does not implement it never reports a choice.
    async fn has_user_agent_config(
        &self,
        _user_id: Uuid,
        _agent_id: Uuid,
    ) -> Result<bool, sqlx::Error> {
        Ok(false)
    }

    /// The config `user` runs `agent` on: the one they chose for it, else their default, or `None`
    /// when they have neither. The default skips the choice and answers their default.
    async fn fetch_user_agent_config(
        &self,
        user_id: Uuid,
        _agent_id: Uuid,
    ) -> Result<Option<LLMConfig>, sqlx::Error> {
        self.fetch_user_default_config(user_id).await
    }

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

    /// The flow a session-token call is attributed to (the traceparent's flow, else the
    /// session's newest flow, else the claim). Infallible: a lookup failure keeps the claim.
    /// The default keeps the claim, so mocks that do not care need not implement it.
    async fn attribute_session_flow(
        &self,
        _trace_flow: Option<&str>,
        session: &nasiko_session_token::SessionClaims,
    ) -> nasiko_session_token::attribution::AttributedFlow {
        nasiko_session_token::attribution::AttributedFlow {
            flow_id: session.flow_id.clone(),
            source: nasiko_session_token::attribution::FlowSource::Claim,
        }
    }

    /// The active custom provider registered under `label`, or `None`. Decryption
    /// of the stored api key happens in the impl, so mocks can supply a plaintext
    /// key. Read per request (never cached) so key rotation and delete take effect
    /// immediately — see §2.4 of the custom-provider plan.
    async fn fetch_custom_provider(
        &self,
        label: &str,
    ) -> Result<Option<CustomProvider>, sqlx::Error>;

    /// The organization-wide embedding default from `settings`, or `None` when the
    /// operator has not set one. The stored key is decrypted in the impl, so mocks can
    /// supply plaintext. Read behind the config cache's TTL — this is the last level of
    /// the embedding priority order, so it is off the hot path for nearly every call.
    async fn fetch_org_embedding_default(&self)
    -> Result<Option<OrgEmbeddingDefault>, sqlx::Error>;
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
    Option<String>,                 // embedding_provider
    Option<String>,                 // embedding_model
    Option<String>,                 // embedding_api_key_secret_name
    bool,                           // embedding_model_override
);

/// The `llm_configs` columns the resolver reads, in [`ConfigRow`] order.
const CONFIG_COLS: &str = "provider, model, fallback_models, temperature, max_tokens, \
     api_key_secret_name, pinned, pinned_model, tier1_model, tier2_model, tier3_model, \
     embedding_provider, embedding_model, embedding_api_key_secret_name, \
     embedding_model_override";

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
        embedding_provider: r.11,
        embedding_model: r.12,
        embedding_api_key_secret_name: r.13,
        embedding_model_override: r.14,
    }
}

/// `agents` columns the resolver reads: attached config id, owner, pin, coding-agent flag,
/// discovered-agent flag, compression flag.
type AgentColumns = (Option<Uuid>, Uuid, Option<String>, bool, bool, bool);

impl PgRegistry {
    async fn load_agent(&self, agent_id: Uuid) -> Result<Option<AgentColumns>, sqlx::Error> {
        sqlx::query_as(
            "SELECT llm_config_id, owner_id, pinned_model, \
                    coding_agent_integration_id IS NOT NULL, \
                    discovered_on IS NOT NULL, \
                    compress_enabled \
             FROM agents WHERE id = $1",
        )
        .bind(agent_id)
        .fetch_optional(&self.db)
        .await
    }

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
        let Some((
            config_id,
            owner_id,
            agent_pinned_model,
            is_coding_agent,
            is_discovered_agent,
            compress_enabled,
        )) = self.load_agent(agent_id).await?
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
            is_discovered_agent,
            compress_enabled,
        }))
    }

    async fn fetch_agent_owner(&self, agent_id: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
        sqlx::query_scalar("SELECT owner_id FROM agents WHERE id = $1 AND deleted_at IS NULL")
            .bind(agent_id)
            .fetch_optional(&self.db)
            .await
    }

    async fn fetch_agent_identity(
        &self,
        agent_id: Uuid,
    ) -> Result<Option<AgentIdentity>, sqlx::Error> {
        let row: Option<(Uuid, Option<DateTime<Utc>>)> = sqlx::query_as(
            "SELECT owner_id, llm_tokens_revoked_at FROM agents \
             WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(agent_id)
        .fetch_optional(&self.db)
        .await?;
        Ok(row.map(|(owner_id, llm_tokens_revoked_at)| AgentIdentity {
            owner_id,
            llm_tokens_revoked_at,
        }))
    }

    async fn fetch_agent_flags(
        &self,
        agent_id: Uuid,
    ) -> Result<Option<AgentConfigResult>, sqlx::Error> {
        Ok(self.load_agent(agent_id).await?.map(
            |(_, _, agent_pinned_model, is_coding_agent, is_discovered_agent, compress_enabled)| {
                AgentConfigResult {
                    config: None,
                    agent_pinned_model,
                    is_coding_agent,
                    is_discovered_agent,
                    compress_enabled,
                }
            },
        ))
    }

    async fn fetch_user_default_config(
        &self,
        user_id: Uuid,
    ) -> Result<Option<LLMConfig>, sqlx::Error> {
        self.load_owner_default(user_id).await
    }

    async fn has_user_agent_config(
        &self,
        user_id: Uuid,
        agent_id: Uuid,
    ) -> Result<bool, sqlx::Error> {
        // The cascades only cover hard deletes: a soft-deleted config, or one row naming
        // another user's config, must read as no choice.
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM agent_user_llm_configs a \
             JOIN llm_configs c ON c.id = a.llm_config_id \
             WHERE a.user_id = $1 AND a.agent_id = $2 \
               AND c.created_by = $1 AND c.deleted_at IS NULL)",
        )
        .bind(user_id)
        .bind(agent_id)
        .fetch_one(&self.db)
        .await
    }

    async fn fetch_user_agent_config(
        &self,
        user_id: Uuid,
        agent_id: Uuid,
    ) -> Result<Option<LLMConfig>, sqlx::Error> {
        // One read for both cases: the chosen config sorts ahead of the default.
        let row: Option<ConfigRow> = sqlx::query_as(&format!(
            "SELECT {CONFIG_COLS} FROM llm_configs c \
             LEFT JOIN agent_user_llm_configs a \
                    ON a.llm_config_id = c.id AND a.user_id = $1 AND a.agent_id = $2 \
             WHERE c.created_by = $1 AND c.deleted_at IS NULL \
               AND (a.llm_config_id IS NOT NULL OR c.is_default) \
             ORDER BY a.llm_config_id IS NULL \
             LIMIT 1"
        ))
        .bind(user_id)
        .bind(agent_id)
        .fetch_optional(&self.db)
        .await?;
        Ok(row.map(row_to_config))
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

    async fn attribute_session_flow(
        &self,
        trace_flow: Option<&str>,
        session: &nasiko_session_token::SessionClaims,
    ) -> nasiko_session_token::attribution::AttributedFlow {
        nasiko_session_token::attribution::attributed_flow(&self.db, trace_flow, session).await
    }

    async fn fetch_live_flow(
        &self,
        flow_id: &str,
        agent_id: Uuid,
        window_secs: i64,
    ) -> Result<Option<crate::routing::attribution::LiveFlow>, sqlx::Error> {
        #[allow(clippy::type_complexity)]
        let row: Option<(
            Option<Uuid>,
            Option<String>,
            Option<String>,
            Option<String>,
            bool,
        )> = sqlx::query_as(
            "SELECT f.user_id, f.metadata->>'context_id', f.metadata->>'mode', \
                    f.metadata->>'user_message', \
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
        Ok(row.map(
            |(user_id, context_id, mode, user_message, agent_is_participant)| {
                crate::routing::attribution::LiveFlow {
                    user_id,
                    context_id,
                    mode,
                    user_message,
                    agent_is_participant,
                }
            },
        ))
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

    async fn fetch_org_embedding_default(
        &self,
    ) -> Result<Option<OrgEmbeddingDefault>, sqlx::Error> {
        let row: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT default_embedding_provider, default_embedding_model, \
                    default_embedding_api_key_encrypted \
             FROM settings LIMIT 1",
        )
        .fetch_optional(&self.db)
        .await?;
        // The DB CHECK keeps provider and model all-or-nothing, so one present and the
        // other absent cannot be stored; treat it as unset rather than half-routing. An
        // empty string is not a value either — the write path normalizes blanks to NULL,
        // and this is the read-side guard for anything stored before it did.
        let Some((provider, model, encrypted)) = row else {
            return Ok(None);
        };
        let blank_to_none = |v: Option<String>| v.filter(|s| !s.trim().is_empty());
        let (Some(provider), Some(model)) = (blank_to_none(provider), blank_to_none(model)) else {
            return Ok(None);
        };
        // Shared platform credential (not a per-user secret) → platform-settings scope,
        // the same one `custom_providers.encrypted_api_key` uses.
        let api_key = match encrypted.filter(|e| !e.is_empty()) {
            Some(e) => Some(
                SecretsCrypto::for_platform_settings()
                    .decrypt(&e)
                    .map_err(|err| sqlx::Error::Decode(Box::new(err)))?,
            ),
            None => None,
        };
        Ok(Some(OrgEmbeddingDefault {
            provider,
            model,
            api_key,
        }))
    }
}

/// Whose LLM config and provider keys an agent's call runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSubject<'a> {
    /// The agent's owner, as the agent token names them (empty for a token minted without an
    /// owner, which takes the platform-key path): the agent's attached config, else the owner's
    /// default, with the owner's keys.
    Owner(&'a str),
    /// The agent's owner, paying for a call someone else made because the payer said so
    /// ([`crate::payer::Payer::Owner`]): the owner's config and keys as for `Owner`, but a config
    /// with no stored key runs on the platform's own key ([`GatewayConfig::service_key_for`]), not
    /// the one agents may fall back to. The payer sponsors the call; the agent did not choose it.
    Sponsored(&'a str),
    /// A user the agent is shared with: their own chosen-or-default config and keys. The agent's
    /// attached config and its owner's keys are never read for them.
    SharedUser(Uuid),
    /// A call the deployment covers ([`crate::payer::Payer::Platform`]): the platform's default
    /// provider and model on the platform's own key. No config, no agent pin and no user secret
    /// is read, and the requested chat model is ignored: the agent is user code, and honouring
    /// its choice would let it pick a pricier model on the platform's key.
    Platform,
}

impl<'a> ConfigSubject<'a> {
    /// Pure: who pays for a call by an agent owned by `owner_id`, made on behalf of
    /// `acting_user` (the attributed flow's user, when there is one). Anyone other than the owner
    /// is a shared user. A token without an owner claim stays on the owner path — it pays with
    /// the platform key, never an owner's.
    pub fn for_call(owner_id: &'a str, acting_user: Option<Uuid>) -> Self {
        let owner = Uuid::parse_str(owner_id).ok();
        match (owner, acting_user) {
            (Some(owner), Some(user)) if owner != user => Self::SharedUser(user),
            _ => Self::Owner(owner_id),
        }
    }

    /// The user whose `user_secrets` hold the config's named key (empty = none).
    fn key_holder(&self) -> String {
        match self {
            Self::Owner(owner_id) | Self::Sponsored(owner_id) => (*owner_id).to_string(),
            Self::SharedUser(user) => user.to_string(),
            Self::Platform => String::new(),
        }
    }

    /// The platform-owned key a call with no stored key of its own runs on for `provider`:
    /// the platform's own key for a sponsored call, else the one agents may fall back to.
    fn platform_key<'c>(&self, cfg: &'c GatewayConfig, provider: &str) -> &'c str {
        match self {
            Self::Sponsored(_) | Self::Platform => cfg.service_key_for(provider),
            Self::Owner(_) | Self::SharedUser(_) => cfg.platform_key_for(provider),
        }
    }
}

/// Resolve `(agent_id, owner_id)` into a [`ResolvedConfig`] on the owner's config and keys.
pub async fn resolve(
    store: &dyn RegistryStore,
    cache: &ConfigCache,
    cfg: &GatewayConfig,
    agent_id: &str,
    owner_id: &str,
    hint: RequestHint<'_>,
) -> Result<ResolvedConfig, GatewayError> {
    let subject = ConfigSubject::Owner(owner_id);
    resolve_for(store, cache, cfg, agent_id, subject, hint).await
}

/// Resolve an agent's call into a [`ResolvedConfig`] on `subject`'s config and keys.
pub async fn resolve_for(
    store: &dyn RegistryStore,
    cache: &ConfigCache,
    cfg: &GatewayConfig,
    agent_id: &str,
    subject: ConfigSubject<'_>,
    hint: RequestHint<'_>,
) -> Result<ResolvedConfig, GatewayError> {
    // A non-UUID agent_id can't match any row → treat as "no registry entry".
    let agent_uuid = Uuid::parse_str(agent_id)
        .map_err(|_| GatewayError::NoRegistryEntry(agent_id.to_string()))?;

    let agent_result = load_subject_config(store, cache, agent_uuid, agent_id, subject).await?;
    let hint = match subject {
        ConfigSubject::Platform => RequestHint::default(),
        _ => hint,
    };
    let key_holder = subject.key_holder();
    let owner_id = key_holder.as_str();
    let llm_config = agent_result.config;
    let agent_pinned_model = agent_result.agent_pinned_model;
    let is_coding_agent = agent_result.is_coding_agent;
    let is_discovered_agent = agent_result.is_discovered_agent;
    let compress_enabled = agent_result.compress_enabled;
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
        subject,
        owner_id,
        Destination {
            provider: &plan.provider,
            model: &plan.model,
        },
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
        is_discovered_agent,
        compress_enabled,
        agent_id: agent_id.to_string(),
    };
    tracing::info!(
        target: "nasiko::llm_router::resolver",
        %agent_id,
        has_llm_config,
        shared_user = matches!(subject, ConfigSubject::SharedUser(_)),
        sponsored = matches!(subject, ConfigSubject::Sponsored(_)),
        platform = matches!(subject, ConfigSubject::Platform),
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

/// Resolve `(agent_id, owner_id)` into a [`ResolvedEmbeddingConfig`] for `POST /v1/embeddings`.
///
/// **Selection is by presence, and it happens exactly once.** We walk the priority order
/// below, take the first level that has a model, and return that level's provider, model
/// and key *as a set* — pieces are never mixed across levels. The caller then makes one
/// upstream call. If it fails, the request fails: the order answers "which model do we
/// pick?", never "what do we try next?".
///
/// | Priority | Model | Provider + key |
/// | --- | --- | --- |
/// | 1 | `embedding_model`, when `embedding_model_override` is set | the same config row |
/// | 2 | the request's own `model` | the inbound OpenAI surface + the platform OpenAI key |
/// | 3 | `embedding_model` | the same config row |
/// | 4 | `settings.default_embedding_model` | the `settings` row |
/// | — | nothing set anywhere | `400` |
///
/// There is deliberately no fallback. Embedding models differ in dimensionality and
/// vector space, so silently substituting one for another does not degrade a result —
/// it returns vectors that cannot be compared with anything already stored. That is also
/// why the request's own model outranks the config's by default: the agent knows which
/// space its corpus lives in and the platform does not. `embedding_model_override`
/// exists for operators who want to take that decision back, and only reorders levels
/// 1 and 2 — it adds no chaining.
///
/// The chat columns (`model`, `fallback_models`, `pinned_model`, the tier models) and
/// `agents.pinned_model` are not read here at all, and [`ResolvedEmbeddingConfig`] has
/// nowhere to put them.
pub async fn resolve_embeddings(
    store: &dyn RegistryStore,
    cache: &ConfigCache,
    cfg: &GatewayConfig,
    agent_id: &str,
    owner_id: &str,
    request_model: Option<&str>,
) -> Result<ResolvedEmbeddingConfig, GatewayError> {
    let subject = ConfigSubject::Owner(owner_id);
    resolve_embeddings_for(store, cache, cfg, agent_id, subject, request_model).await
}

/// [`resolve_embeddings`] on `subject`'s config and keys: a shared user's own config stands in
/// for the agent's at levels 1 and 3.
pub async fn resolve_embeddings_for(
    store: &dyn RegistryStore,
    cache: &ConfigCache,
    cfg: &GatewayConfig,
    agent_id: &str,
    subject: ConfigSubject<'_>,
    request_model: Option<&str>,
) -> Result<ResolvedEmbeddingConfig, GatewayError> {
    let agent_uuid = Uuid::parse_str(agent_id)
        .map_err(|_| GatewayError::NoRegistryEntry(agent_id.to_string()))?;
    let llm_config = load_subject_config(store, cache, agent_uuid, agent_id, subject)
        .await?
        .config;
    let key_holder = subject.key_holder();
    let owner_id = key_holder.as_str();

    let (source, pick) = match pick_embedding(llm_config.as_ref(), request_model) {
        Some(picked) => picked,
        // Neither the request, the config nor the org default named a model.
        None => match load_org_embedding_default(store, cache).await? {
            Some(org) => (
                EmbeddingSource::OrgDefault,
                EmbeddingPick {
                    provider: org.provider,
                    model: org.model,
                    // The org key is platform-owned however it is supplied: either the
                    // operator's stored key, or the platform key for that provider.
                    owner_key_secret_name: None,
                    org_api_key: org.api_key,
                },
            ),
            None => {
                return Err(GatewayError::BadRequest(
                    "no embedding model available: the request named no model, the agent's \
                     LLM config has no embedding_model, and no organization default is set"
                        .to_string(),
                ));
            }
        },
    };

    let custom = load_custom_provider(store, &pick.provider).await?;
    let api_key = match &pick.org_api_key {
        // Level 4 with a stored key: it is already decrypted and already scoped to this
        // provider, so it bypasses the per-user secret path entirely.
        Some(key) => key.clone(),
        None => {
            resolve_api_key(
                store,
                cfg,
                subject,
                owner_id,
                Destination {
                    provider: &pick.provider,
                    model: &pick.model,
                },
                pick.owner_key_secret_name.as_deref(),
                custom.as_ref(),
            )
            .await?
        }
    };
    // Mirrors resolve_api_key: only a named per-user secret with a real owner is
    // bring-your-own-key. Everything else — the platform key, a custom provider's own
    // stored key, the org key — is platform-owned. Never derived from the chat
    // `api_key_secret_name`, or an agent with a chat key but no embedding key would be
    // billed as BYOK while spending the platform's key.
    let platform_paid = pick.owner_key_secret_name.is_none() || owner_id.is_empty();

    let resolved = ResolvedEmbeddingConfig {
        provider: pick.provider,
        model: pick.model,
        api_key,
        custom_endpoint: custom.as_ref().map(|c| CustomEndpoint {
            base_url: c.base_url.clone(),
            dialect: c.dialect.clone(),
        }),
        platform_paid,
    };
    tracing::info!(
        target: "nasiko::llm_router::resolver",
        %agent_id,
        source = source.label(),
        provider = %resolved.provider,
        model = %resolved.model,
        platform_paid = resolved.platform_paid,
        custom_endpoint = ?resolved.custom_endpoint.as_ref().map(|e| &e.base_url),
        api_key_present = !resolved.api_key.is_empty(),
        "resolver: resolved embedding config (one attempt, no fallback)"
    );
    Ok(resolved)
}

/// One level's complete (provider, model, key source) set, before the key is fetched.
struct EmbeddingPick {
    provider: String,
    model: String,
    /// The agent owner's `user_secrets` entry holding the key, when this level names one.
    owner_key_secret_name: Option<String>,
    /// An already-decrypted platform-owned key supplied by the level itself (level 4).
    org_api_key: Option<String>,
}

/// Pure: the first of levels 1–3 that has a model, or `None` to fall through to the
/// organization default. Split out so the priority order is unit-testable without a DB.
fn pick_embedding(
    llm_config: Option<&LLMConfig>,
    request_model: Option<&str>,
) -> Option<(EmbeddingSource, EmbeddingPick)> {
    let configured = llm_config.and_then(config_embedding_pick);
    let requested = request_model
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(|model| EmbeddingPick {
            // The call arrived on the OpenAI surface, so the model it names is an OpenAI
            // model and the platform OpenAI key is the credential that can run it.
            provider: "openai".to_string(),
            model: model.to_string(),
            owner_key_secret_name: None,
            org_api_key: None,
        });
    let overriding = llm_config.is_some_and(|c| c.embedding_model_override);

    match (overriding, configured, requested) {
        (true, Some(pick), _) => Some((EmbeddingSource::ConfigOverride, pick)),
        (_, _, Some(pick)) => Some((EmbeddingSource::Request, pick)),
        (_, Some(pick), None) => Some((EmbeddingSource::Config, pick)),
        (_, None, None) => None,
    }
}

/// Pure: this config row's embedding level, or `None` when it has no embedding model.
/// Reads only the four embedding columns — never the chat ones.
fn config_embedding_pick(c: &LLMConfig) -> Option<EmbeddingPick> {
    let model = c
        .embedding_model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())?;
    // The DB CHECK makes the pair all-or-nothing, so a model without a provider cannot
    // be stored; a row that somehow has one is treated as unset rather than half-routed.
    let provider = c
        .embedding_provider
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())?;
    Some(EmbeddingPick {
        provider: provider.to_string(),
        model: model.to_string(),
        // An empty name means "the platform key for this provider", same as absent.
        owner_key_secret_name: c
            .embedding_api_key_secret_name
            .clone()
            .filter(|n| !n.is_empty()),
        org_api_key: None,
    })
}

/// The organization embedding default, through the config cache's TTL.
async fn load_org_embedding_default(
    store: &dyn RegistryStore,
    cache: &ConfigCache,
) -> Result<Option<OrgEmbeddingDefault>, GatewayError> {
    if let Some(hit) = cache.get_org_embedding_default() {
        return Ok(hit);
    }
    let fetched = store
        .fetch_org_embedding_default()
        .await
        .map_err(|e| GatewayError::Internal(format!("org embedding default read failed: {e}")))?;
    cache.put_org_embedding_default(fetched.clone());
    Ok(fetched)
}

/// The custom-provider row backing `provider`, or `None` for a built-in.
///
/// Same fail-closed rule the chat path uses: a non-built-in name with no active row is a
/// client error, never a silent fallthrough to OpenAI (which would send the platform
/// OpenAI key to an unknown endpoint).
async fn load_custom_provider(
    store: &dyn RegistryStore,
    provider: &str,
) -> Result<Option<CustomProvider>, GatewayError> {
    if is_builtin_provider(provider) {
        return Ok(None);
    }
    store
        .fetch_custom_provider(provider)
        .await
        .map_err(|e| GatewayError::Internal(format!("custom provider read failed: {e}")))?
        .map(Some)
        .ok_or_else(|| {
            GatewayError::BadRequest(format!(
                "provider '{provider}' is not a registered custom provider"
            ))
        })
}

/// The `api_key_secret_name` from an optional `llm_config` — used only for logging which
/// key source the resolver will pick (never the secret value itself).
fn plan_secret_name(llm_config: &Option<LLMConfig>) -> Option<String> {
    llm_config
        .as_ref()
        .and_then(|c| c.api_key_secret_name.clone())
}

/// The agent's flags plus the config `subject` runs on: the owner's through
/// [`load_llm_config`], a shared user's own, or none for a platform call.
async fn load_subject_config(
    store: &dyn RegistryStore,
    cache: &ConfigCache,
    agent_uuid: Uuid,
    agent_id: &str,
    subject: ConfigSubject<'_>,
) -> Result<AgentConfigResult, GatewayError> {
    match subject {
        ConfigSubject::Owner(_) | ConfigSubject::Sponsored(_) => {
            load_llm_config(store, cache, agent_uuid, agent_id).await
        }
        ConfigSubject::SharedUser(user) => {
            load_shared_user_config(store, agent_uuid, agent_id, user).await
        }
        ConfigSubject::Platform => load_agent_flags_only(store, agent_uuid, agent_id).await,
    }
}

/// The agent's flags alone, for a platform-covered call: the config and the agent's model pin are
/// cleared, since a platform call runs on the platform defaults.
async fn load_agent_flags_only(
    store: &dyn RegistryStore,
    agent_uuid: Uuid,
    agent_id: &str,
) -> Result<AgentConfigResult, GatewayError> {
    let flags = store
        .fetch_agent_flags(agent_uuid)
        .await
        .map_err(|e| GatewayError::Internal(format!("registry read failed: {e}")))?
        .ok_or_else(|| GatewayError::NoRegistryEntry(agent_id.to_string()))?;
    Ok(AgentConfigResult {
        config: None,
        agent_pinned_model: None,
        ..flags
    })
}

/// A shared user's own config (their choice for this agent, else their default), with the agent's
/// flags. Not cached: the agent-keyed cache holds the owner's config, and this read is one
/// indexed row.
///
/// The owner's agent-level model pin is dropped along with the owner's config: it names a
/// model on the owner's provider, which the shared user's config may not reach.
async fn load_shared_user_config(
    store: &dyn RegistryStore,
    agent_uuid: Uuid,
    agent_id: &str,
    user: Uuid,
) -> Result<AgentConfigResult, GatewayError> {
    let flags = store
        .fetch_agent_flags(agent_uuid)
        .await
        .map_err(|e| GatewayError::Internal(format!("registry read failed: {e}")))?
        .ok_or_else(|| GatewayError::NoRegistryEntry(agent_id.to_string()))?;
    let config = store
        .fetch_user_agent_config(user, agent_uuid)
        .await
        .map_err(|e| GatewayError::Internal(format!("shared user config read failed: {e}")))?
        .ok_or(GatewayError::NoSharedUserConfig)?;
    tracing::info!(
        target: "nasiko::llm_router::resolver",
        %agent_id, %user, provider = %config.provider,
        "resolver: shared agent, running on the calling user's own config"
    );
    Ok(AgentConfigResult {
        config: Some(config),
        agent_pinned_model: None,
        ..flags
    })
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
        let agent_row = store.fetch_agent_flags(agent_uuid).await.ok().flatten();
        let agent_pin = agent_row
            .as_ref()
            .and_then(|r| r.agent_pinned_model.clone());
        let compress_enabled = agent_row.as_ref().is_some_and(|r| r.compress_enabled);
        let is_coding_agent = agent_row.as_ref().is_some_and(|r| r.is_coding_agent);
        let is_discovered_agent = agent_row.is_some_and(|r| r.is_discovered_agent);
        return Ok(AgentConfigResult {
            config: hit,
            agent_pinned_model: agent_pin,
            is_coding_agent,
            is_discovered_agent,
            compress_enabled,
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

/// Picks the destination for an unconfigured agent. The inbound SDK surface only fixes the
/// wire format; the router translates any format to any provider, so the surface never picks
/// the provider.
///
/// 1. `provider/model` naming a keyed built-in provider → that provider, bare model.
/// 2. Otherwise the platform `DEFAULT_PROVIDER`. The requested model is kept only when the
///    SDK surface is that provider's own family (an OpenAI SDK on an OpenAI deployment);
///    any other pairing gets `DEFAULT_MODEL`, since a model named for another wire format
///    is not something the default provider serves. No model-name guessing.
fn usable_hint<'a>(cfg: &'a GatewayConfig, hint: RequestHint<'a>) -> RequestHint<'a> {
    let default_provider: &'a str = cfg.default_provider.as_str();
    if let Some((provider, model)) = hint.model.and_then(|m| explicit_provider(cfg, m)) {
        return RequestHint {
            provider: Some(provider),
            model: Some(model),
        };
    }
    // No SDK surface means no evidence the model belongs to the default provider, so the
    // family check fails and the model falls back to `DEFAULT_MODEL`.
    let surface: &'a str = hint.provider.unwrap_or("");
    // OpenRouter ids are `vendor/model`; an OpenRouter default serves them as-is.
    let openrouter_id: bool =
        default_provider == "openrouter" && hint.model.is_some_and(|m| m.contains('/'));
    // A prefix naming an unavailable built-in provider is not a model the default serves.
    let foreign_prefix: bool = hint.model.is_some_and(names_builtin_provider);
    let keep_model: bool = openrouter_id
        || (!foreign_prefix && !surface.is_empty() && same_family(surface, default_provider));
    let model: Option<&'a str> = if keep_model {
        hint.model
    } else {
        Some(cfg.default_model.as_str())
    };
    if surface != default_provider || model != hint.model {
        tracing::info!(
            surface,
            to = default_provider,
            requested_model = ?hint.model,
            model = ?model,
            "resolver: routing unconfigured agent to platform default provider"
        );
    }
    RequestHint {
        provider: Some(default_provider),
        model,
    }
}

/// `provider/model` where `provider` is a keyed built-in → `(provider, model)`.
fn explicit_provider<'a>(cfg: &'a GatewayConfig, model: &'a str) -> Option<(&'a str, &'a str)> {
    let (prefix, rest) = model.split_once('/')?;
    let provider: &'static str = ["openai", "anthropic", "gemini", "vertex", "openrouter"]
        .into_iter()
        .find(|p| p.eq_ignore_ascii_case(prefix.trim()))?;
    if rest.is_empty() || cfg.platform_key_for(provider).is_empty() {
        return None;
    }
    Some((provider, rest))
}

/// Whether `model` is `provider/...` with `provider` a recognized built-in.
fn names_builtin_provider(model: &str) -> bool {
    model.split_once('/').is_some_and(|(prefix, _)| {
        let prefix: String = prefix.trim().to_ascii_lowercase();
        is_builtin_provider(&prefix)
    })
}

/// Whether the SDK surface `surface` natively belongs to `provider` (Gemini and Vertex share
/// one model namespace).
fn same_family(surface: &str, provider: &str) -> bool {
    fn family(p: &str) -> String {
        let p: String = p.trim().to_ascii_lowercase();
        if p == "vertex" {
            "gemini".to_string()
        } else {
            p
        }
    }
    family(surface) == family(provider)
}

/// Whether `provider` is one of the built-in providers the router speaks natively.
/// Everything else is a candidate custom (DB-registered) provider.
///
/// Must stay in step with the two functions that answer the same question from the other
/// side — [`GatewayConfig::platform_key_for`] (which key pays) and
/// [`crate::providers::provider_for`] (which client runs it). `openrouter` was missing
/// here while present in both of those, so a config naming it resolved to
/// "not a registered custom provider" despite the rest of the stack supporting it.
fn is_builtin_provider(provider: &str) -> bool {
    matches!(
        provider,
        "openai" | "anthropic" | "gemini" | "vertex" | "openrouter"
    )
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
    let hint: RequestHint<'_> = usable_hint(cfg, hint);
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
    let hint: RequestHint<'_> = usable_hint(cfg, hint);
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

/// The provider and model a key is being resolved for; the model only names the call in a
/// refusal the user reads.
#[derive(Clone, Copy)]
struct Destination<'a> {
    provider: &'a str,
    model: &'a str,
}

/// Resolve the provider API key. Per-user secret lookup requires a secret name **and**
/// a non-empty owner; an empty owner_id means "no per-user lookup" → platform key, the one
/// `subject` runs on ([`ConfigSubject::platform_key`]).
async fn resolve_api_key(
    store: &dyn RegistryStore,
    cfg: &GatewayConfig,
    subject: ConfigSubject<'_>,
    owner_id: &str,
    destination: Destination<'_>,
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
            .ok_or_else(|| {
                tracing::warn!(%owner, secret = name, "LLM config names a secret the owner never stored");
                GatewayError::SecretNotFound(name.to_string())
            })?;
        let crypto = SecretsCrypto::try_for_user(owner)
            .map_err(|e| GatewayError::Internal(format!("secret cipher init failed: {e}")))?;
        return crypto.decrypt(&encrypted).map_err(|e| {
            tracing::warn!(%owner, secret = name, error = %e, "stored LLM secret can't be decrypted");
            GatewayError::SecretUnreadable(name.to_string())
        });
    }
    // No per-user secret: a custom provider pays with its own stored (platform-owned)
    // key; a built-in provider pays with the platform key for that provider.
    if let Some(cp) = custom {
        return Ok(cp.api_key.clone());
    }
    platform_key(cfg, subject, destination)
}

/// Pure: the platform-owned fallback key `subject` runs on for the destination's provider, or
/// the 400 when none is configured.
fn platform_key(
    cfg: &GatewayConfig,
    subject: ConfigSubject<'_>,
    destination: Destination<'_>,
) -> Result<String, GatewayError> {
    let key = subject.platform_key(cfg, destination.provider);
    if key.is_empty() {
        Err(GatewayError::NoApiKey {
            provider: destination.provider.to_string(),
            model: destination.model.to_string(),
        })
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
        org_embedding_default: Option<OrgEmbeddingDefault>,
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
                is_discovered_agent: false,
                compress_enabled: false,
            }))
        }
        async fn fetch_agent_owner(&self, _: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
            Ok(Uuid::parse_str(OWNER).ok())
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
        async fn fetch_org_embedding_default(
            &self,
        ) -> Result<Option<OrgEmbeddingDefault>, sqlx::Error> {
            Ok(self.org_embedding_default.clone())
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
            embedding_provider: None,
            embedding_model: None,
            embedding_api_key_secret_name: None,
            embedding_model_override: false,
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
            org_embedding_default: None,
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
        assert_eq!(r.agent_id, AGENT);
    }

    #[tokio::test]
    async fn missing_agent_is_no_registry_entry() {
        let store = MockRegistry {
            config: None,
            secret: None,
            agent_pinned_model: None,
            custom_provider: None,
            org_embedding_default: None,
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

    /// Agents' platform key off (the Free plane), the platform's own Gemini key set.
    fn free_plane_cfg() -> GatewayConfig {
        GatewayConfig {
            default_provider: "gemini".into(),
            default_model: "gemini-4-hydrogen".into(),
            generic_key_fallback: false,
            service_gemini_api_key: "sk-platform-gemini".into(),
            ..Default::default()
        }
    }

    fn unconfigured_agent() -> MockRegistry {
        MockRegistry {
            config: Some(None),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn an_owners_own_unconfigured_agent_never_gets_the_platforms_own_key() {
        let err = resolve_for(
            &unconfigured_agent(),
            &cache(),
            &free_plane_cfg(),
            AGENT,
            ConfigSubject::Owner(OWNER),
            RequestHint::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, GatewayError::NoApiKey { .. }));
    }

    #[tokio::test]
    async fn a_sponsored_unconfigured_agent_runs_on_the_platforms_own_key() {
        let r = resolve_for(
            &unconfigured_agent(),
            &cache(),
            &free_plane_cfg(),
            AGENT,
            ConfigSubject::Sponsored(OWNER),
            RequestHint::default(),
        )
        .await
        .unwrap();
        assert_eq!(r.provider, "gemini");
        assert_eq!(r.model, "gemini-4-hydrogen");
        assert_eq!(r.api_key, "sk-platform-gemini");
        assert!(r.platform_paid);
    }

    #[tokio::test]
    async fn a_sponsored_agent_asking_for_an_openai_model_runs_on_the_platform_default() {
        // The platform holds no OpenAI key for agents: the hint is dropped, not served.
        let hint = RequestHint {
            provider: Some("openai"),
            model: Some("gpt-4o-mini"),
        };
        let r = resolve_for(
            &unconfigured_agent(),
            &cache(),
            &free_plane_cfg(),
            AGENT,
            ConfigSubject::Sponsored(OWNER),
            hint,
        )
        .await
        .unwrap();
        assert_eq!(r.provider, "gemini");
        assert_eq!(r.api_key, "sk-platform-gemini");
    }

    #[tokio::test]
    async fn a_sponsored_agents_embeddings_run_on_the_platforms_own_key() {
        let cfg = GatewayConfig {
            service_openai_api_key: "sk-platform-openai".into(),
            ..free_plane_cfg()
        };
        let store = MockRegistry {
            config: Some(Some(embedding_config(
                "openai",
                "text-embedding-3-small",
                None,
            ))),
            ..Default::default()
        };
        let r = resolve_embeddings_for(
            &store,
            &cache(),
            &cfg,
            AGENT,
            ConfigSubject::Sponsored(OWNER),
            None,
        )
        .await
        .unwrap();
        assert_eq!(r.api_key, "sk-platform-openai");
        let err = resolve_embeddings_for(
            &store,
            &cache(),
            &cfg,
            AGENT,
            ConfigSubject::Owner(OWNER),
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, GatewayError::NoApiKey { .. }));
    }

    /// An agent whose owner attached a keyed config and pinned a model: a platform call must
    /// read neither.
    fn configured_and_pinned_agent() -> MockRegistry {
        MockRegistry {
            config: Some(Some(llm_config("gemini", "owner-model", Some("KEY")))),
            secret: Some("user-key".into()),
            agent_pinned_model: Some("pinned-pro".into()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_platform_call_runs_on_the_platform_defaults_and_its_own_key() {
        let r = resolve_for(
            &configured_and_pinned_agent(),
            &cache(),
            &free_plane_cfg(),
            AGENT,
            ConfigSubject::Platform,
            RequestHint::default(),
        )
        .await
        .unwrap();
        assert_eq!(r.provider, "gemini");
        assert_eq!(r.model, "gemini-4-hydrogen");
        assert_eq!(r.api_key, "sk-platform-gemini");
        assert!(r.platform_paid);
        assert!(!r.has_llm_config);
        assert!(r.pinned_model.is_none());
    }

    #[tokio::test]
    async fn a_platform_call_ignores_the_requested_model() {
        let hints = [
            RequestHint {
                provider: Some("gemini"),
                model: Some("gemini-pro-x"),
            },
            RequestHint {
                provider: None,
                model: Some("anthropic/claude-x"),
            },
        ];
        for hint in hints {
            let r = resolve_for(
                &configured_and_pinned_agent(),
                &cache(),
                &free_plane_cfg(),
                AGENT,
                ConfigSubject::Platform,
                hint,
            )
            .await
            .unwrap();
            assert_eq!(r.provider, "gemini");
            assert_eq!(r.model, "gemini-4-hydrogen");
        }
    }

    #[tokio::test]
    async fn a_platform_embedding_call_runs_on_the_platform_key() {
        let cfg = GatewayConfig {
            service_openai_api_key: "sk-platform-openai".into(),
            ..free_plane_cfg()
        };
        let r = resolve_embeddings_for(
            &configured_and_pinned_agent(),
            &cache(),
            &cfg,
            AGENT,
            ConfigSubject::Platform,
            Some("text-embedding-3-small"),
        )
        .await
        .unwrap();
        assert_eq!(r.provider, "openai");
        assert_eq!(r.model, "text-embedding-3-small");
        assert_eq!(r.api_key, "sk-platform-openai");
        assert!(r.platform_paid);
    }

    #[tokio::test]
    async fn no_secret_and_no_platform_key_is_bad_request() {
        let store = MockRegistry {
            config: Some(None),
            secret: None,
            agent_pinned_model: None,
            custom_provider: None,
            org_embedding_default: None,
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
        assert!(
            matches!(&err, GatewayError::NoApiKey { provider, model }
                if provider == "openai" && model == "gpt-4o-mini"),
            "{err}"
        );
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
            org_embedding_default: None,
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
            org_embedding_default: None,
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
        assert_eq!(
            err.to_string(),
            "No API key for provider 'anthropic' (model claude-haiku-4-5). Connect a model with \
             your own key in LLM router, or ask your admin to set PLATFORM_ANTHROPIC_API_KEY."
        );
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
            org_embedding_default: None,
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
    async fn no_config_routes_to_explicit_provider_prefix() {
        // No llm_config: the SDK surface never picks the provider; an explicit
        // `provider/model` naming a keyed provider does. Key follows that provider.
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
            org_embedding_default: None,
        };
        let hint = RequestHint {
            provider: Some("openai"),
            model: Some("anthropic/claude-3-5-sonnet-20241022"),
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
            org_embedding_default: None,
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
            org_embedding_default: None,
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
        assert!(matches!(err, GatewayError::SecretNotFound(_)));
        assert_eq!(
            err.to_string(),
            "Your LLM config uses the secret 'ANTHROPIC_API_KEY', which doesn't exist. Add it in \
             Settings → Secrets or edit the config."
        );
        assert!(!err.to_string().contains(OWNER));
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
            org_embedding_default: None,
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
            org_embedding_default: None,
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
            org_embedding_default: None,
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
            org_embedding_default: None,
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
            org_embedding_default: None,
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
            org_embedding_default: None,
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
            org_embedding_default: None,
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
            org_embedding_default: None,
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

    // ── Embedding resolution (resolve_embeddings) ────────────────────────────
    //
    // The order under test: config-with-override → request model → config model →
    // org default → 400. Each level brings its own provider, model and key; pieces are
    // never mixed across levels, and a chosen level is never abandoned for the next one.

    /// A config whose chat side is deliberately hostile to embeddings — anthropic has no
    /// embeddings API at all, and the chat model/fallbacks/tiers are all chat models. If
    /// any of them leaks into the embedding path the assertions below fail loudly.
    fn embedding_config(provider: &str, model: &str, secret: Option<&str>) -> LLMConfig {
        let mut c = llm_config("anthropic", "claude-3-5-sonnet-20241022", None);
        c.fallback_models = vec!["anthropic/claude-haiku-4-5".into()];
        c.pinned = true;
        c.pinned_model = Some("claude-locked".into());
        c.tier1_model = Some("claude-opus-4-8".into());
        c.tier2_model = Some("claude-sonnet-4-6".into());
        c.tier3_model = Some("claude-haiku-4-5".into());
        c.api_key_secret_name = Some("CHAT_KEY".into());
        c.embedding_provider = Some(provider.into());
        c.embedding_model = Some(model.into());
        c.embedding_api_key_secret_name = secret.map(str::to_string);
        c
    }

    fn org_default(provider: &str, model: &str, key: Option<&str>) -> OrgEmbeddingDefault {
        OrgEmbeddingDefault {
            provider: provider.into(),
            model: model.into(),
            api_key: key.map(str::to_string),
        }
    }

    async fn embeddings_for(
        store: &MockRegistry,
        cfg: &GatewayConfig,
        request_model: Option<&str>,
    ) -> Result<ResolvedEmbeddingConfig, GatewayError> {
        resolve_embeddings(store, &cache(), cfg, AGENT, OWNER, request_model).await
    }

    #[tokio::test]
    async fn embedding_request_model_wins_over_config_and_org_default() {
        // Priority 2 (the default top level): the agent knows which vector space its
        // corpus lives in, so its own model outranks anything the platform configured.
        let store = MockRegistry {
            config: Some(Some(embedding_config("gemini", "text-embedding-004", None))),
            org_embedding_default: Some(org_default("openai", "org-model", Some("sk-org"))),
            ..Default::default()
        };
        let r = embeddings_for(
            &store,
            &cfg("openai", "gpt-4o-mini", "sk-platform"),
            Some("text-embedding-3-large"),
        )
        .await
        .unwrap();
        assert_eq!(r.model, "text-embedding-3-large");
        // Level 2 brings its own provider and key as a set — never the config's Gemini
        // endpoint with the request's OpenAI model.
        assert_eq!(r.provider, "openai");
        assert_eq!(r.api_key, "sk-platform");
        assert!(r.platform_paid);
    }

    #[tokio::test]
    async fn embedding_config_wins_when_the_request_names_no_model() {
        let store = MockRegistry {
            config: Some(Some(embedding_config("gemini", "text-embedding-004", None))),
            org_embedding_default: Some(org_default("openai", "org-model", Some("sk-org"))),
            ..Default::default()
        };
        let r = embeddings_for(&store, &cfg("openai", "gpt-4o-mini", "sk-platform"), None)
            .await
            .unwrap();
        assert_eq!(r.provider, "gemini");
        assert_eq!(r.model, "text-embedding-004");
    }

    #[tokio::test]
    async fn embedding_override_makes_the_config_beat_the_request() {
        // The escape hatch for "the platform decides, not the agent". It only reorders
        // levels 1 and 2 — it adds no chaining.
        let mut c = embedding_config("gemini", "text-embedding-004", None);
        c.embedding_model_override = true;
        let store = MockRegistry {
            config: Some(Some(c)),
            ..Default::default()
        };
        let r = embeddings_for(
            &store,
            &cfg("openai", "gpt-4o-mini", "sk-platform"),
            Some("text-embedding-3-large"),
        )
        .await
        .unwrap();
        assert_eq!(r.provider, "gemini");
        assert_eq!(r.model, "text-embedding-004");
    }

    #[tokio::test]
    async fn embedding_override_without_an_embedding_model_changes_nothing() {
        // The flag reorders two levels; it does not invent a level. With no embedding
        // model on the config there is nothing to promote, so the request still wins.
        let mut c = llm_config("anthropic", "claude-3-5-sonnet-20241022", None);
        c.embedding_model_override = true;
        let store = MockRegistry {
            config: Some(Some(c)),
            ..Default::default()
        };
        let r = embeddings_for(
            &store,
            &cfg("openai", "gpt-4o-mini", "sk-platform"),
            Some("text-embedding-3-large"),
        )
        .await
        .unwrap();
        assert_eq!(r.provider, "openai");
        assert_eq!(r.model, "text-embedding-3-large");
    }

    #[tokio::test]
    async fn embedding_org_default_wins_when_request_and_config_are_silent() {
        let store = MockRegistry {
            config: Some(Some(llm_config("anthropic", "claude-x", None))),
            org_embedding_default: Some(org_default(
                "gemini",
                "text-embedding-004",
                Some("sk-org"),
            )),
            ..Default::default()
        };
        let r = embeddings_for(&store, &cfg("openai", "gpt-4o-mini", "sk-platform"), None)
            .await
            .unwrap();
        assert_eq!(r.provider, "gemini");
        assert_eq!(r.model, "text-embedding-004");
        assert_eq!(r.api_key, "sk-org");
        assert!(r.platform_paid);
    }

    #[tokio::test]
    async fn embedding_org_default_without_a_key_uses_the_platform_key() {
        // Provider + model set, key left empty ⇒ "use the platform key for this provider".
        let store = MockRegistry {
            config: Some(None),
            org_embedding_default: Some(org_default("gemini", "text-embedding-004", None)),
            ..Default::default()
        };
        let r = embeddings_for(&store, &cfg("openai", "gpt-4o-mini", "sk-platform"), None)
            .await
            .unwrap();
        assert_eq!(r.api_key, "sk-platform");
        assert!(r.platform_paid);
    }

    #[tokio::test]
    async fn embedding_with_nothing_set_anywhere_is_bad_request() {
        let store = MockRegistry {
            config: Some(Some(llm_config("anthropic", "claude-x", None))),
            ..Default::default()
        };
        let err = embeddings_for(&store, &cfg("openai", "gpt-4o-mini", "sk-platform"), None)
            .await
            .unwrap_err();
        assert!(matches!(err, GatewayError::BadRequest(_)));
        assert!(err.to_string().contains("no embedding model available"));
    }

    #[tokio::test]
    async fn embedding_chat_columns_are_provably_unread() {
        // `embedding_config` loads every chat field with an anthropic value — a provider
        // with no embeddings API at all. Nothing from that side may appear here.
        let store = MockRegistry {
            config: Some(Some(embedding_config("gemini", "text-embedding-004", None))),
            agent_pinned_model: Some("claude-agent-pin".into()),
            secret: Some("must-not-be-read".into()),
            ..Default::default()
        };
        let r = embeddings_for(&store, &cfg("openai", "gpt-4o-mini", "sk-platform"), None)
            .await
            .unwrap();
        assert_eq!(r.provider, "gemini");
        assert_eq!(r.model, "text-embedding-004");
        // The chat `api_key_secret_name` ("CHAT_KEY") must not have been consulted; with
        // no embedding secret name this is the platform key for gemini.
        assert_eq!(r.api_key, "sk-platform");
        assert!(r.platform_paid);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn embedding_secret_name_makes_the_call_bring_your_own_key() {
        use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
        let owner = Uuid::parse_str(OWNER).unwrap();
        // SAFETY: serialized via #[serial]; no other test reads the env concurrently.
        unsafe {
            std::env::set_var("SECRETS_ENCRYPTION_KEY", BASE64.encode([7u8; 32]));
        }
        let ciphertext = SecretsCrypto::for_user(owner).encrypt("sk-user-embedding");
        let store = MockRegistry {
            config: Some(Some(embedding_config(
                "openai",
                "text-embedding-3-small",
                Some("EMBEDDING_KEY"),
            ))),
            secret: Some(ciphertext),
            ..Default::default()
        };
        let r = embeddings_for(&store, &cfg("openai", "gpt-4o-mini", "sk-platform"), None)
            .await
            .unwrap();
        assert_eq!(r.api_key, "sk-user-embedding");
        // Its own key, so it is not platform-paid — and this must follow the *embedding*
        // secret name, never the chat one.
        assert!(!r.platform_paid);
    }

    #[tokio::test]
    async fn embedding_config_resolves_a_custom_provider_endpoint() {
        let store = MockRegistry {
            config: Some(Some(embedding_config("azure-prod", "embed-3-large", None))),
            custom_provider: Some(custom(
                "https://acme.openai.azure.com",
                "sk-azure",
                "unused",
            )),
            ..Default::default()
        };
        let r = embeddings_for(&store, &cfg("openai", "gpt-4o-mini", "sk-platform"), None)
            .await
            .unwrap();
        assert_eq!(r.provider, "azure-prod");
        assert_eq!(r.api_key, "sk-azure");
        assert_eq!(
            r.custom_endpoint.expect("custom endpoint").base_url,
            "https://acme.openai.azure.com"
        );
    }

    #[tokio::test]
    async fn embedding_org_default_resolves_a_custom_provider_endpoint() {
        // Level 4 has the same reach as level 3: an Azure-only organization can set a
        // usable default. Its own stored key still wins over the endpoint's.
        let store = MockRegistry {
            config: Some(None),
            org_embedding_default: Some(org_default("azure-prod", "embed-3-large", Some("sk-org"))),
            custom_provider: Some(custom(
                "https://acme.openai.azure.com",
                "sk-azure",
                "unused",
            )),
            ..Default::default()
        };
        let r = embeddings_for(&store, &cfg("openai", "gpt-4o-mini", "sk-platform"), None)
            .await
            .unwrap();
        assert_eq!(r.provider, "azure-prod");
        assert_eq!(r.api_key, "sk-org");
        assert!(r.custom_endpoint.is_some());
    }

    #[tokio::test]
    async fn embedding_unregistered_custom_provider_is_bad_request() {
        // Fail closed, exactly as chat does: never fall through to OpenAI with the
        // platform key pointed at an endpoint nobody registered.
        let store = MockRegistry {
            config: Some(Some(embedding_config("ghost-gateway", "embed-x", None))),
            ..Default::default()
        };
        let err = embeddings_for(&store, &cfg("openai", "gpt-4o-mini", "sk-platform"), None)
            .await
            .unwrap_err();
        assert!(matches!(err, GatewayError::BadRequest(_)));
    }

    #[tokio::test]
    async fn embedding_blank_request_model_falls_through_rather_than_routing_to_empty() {
        // `{"model": ""}` is not a choice of model. Treating it as one would send an
        // empty model id upstream instead of using the configured embedding model.
        let store = MockRegistry {
            config: Some(Some(embedding_config("gemini", "text-embedding-004", None))),
            ..Default::default()
        };
        let r = embeddings_for(
            &store,
            &cfg("openai", "gpt-4o-mini", "sk-platform"),
            Some("  "),
        )
        .await
        .unwrap();
        assert_eq!(r.model, "text-embedding-004");
    }

    #[tokio::test]
    async fn a_blank_embedding_pair_on_the_config_reads_as_unset() {
        // `""` is not a destination. The write path normalizes blanks to NULL, but a row
        // stored before it did must not route to a provider with no name.
        let store = MockRegistry {
            config: Some(Some(embedding_config("", "", None))),
            ..Default::default()
        };
        let err = embeddings_for(&store, &cfg("openai", "gpt-4o-mini", "sk-platform"), None)
            .await
            .unwrap_err();
        assert!(matches!(err, GatewayError::BadRequest(_)));
        assert!(err.to_string().contains("no embedding model available"));
    }

    #[tokio::test]
    async fn a_blank_config_pair_still_lets_the_request_model_through() {
        // The blank must read as "nothing configured", not as a broken config that fails
        // the whole call — an agent naming its own model is unaffected by it.
        let store = MockRegistry {
            config: Some(Some(embedding_config("", "", None))),
            ..Default::default()
        };
        let r = embeddings_for(
            &store,
            &cfg("openai", "gpt-4o-mini", "sk-platform"),
            Some("text-embedding-3-large"),
        )
        .await
        .unwrap();
        assert_eq!(r.provider, "openai");
        assert_eq!(r.model, "text-embedding-3-large");
    }

    #[test]
    fn embedding_source_labels_are_stable() {
        // These strings are what an operator greps for to see which level won.
        assert_eq!(EmbeddingSource::ConfigOverride.label(), "config-override");
        assert_eq!(EmbeddingSource::Request.label(), "request");
        assert_eq!(EmbeddingSource::Config.label(), "config");
        assert_eq!(EmbeddingSource::OrgDefault.label(), "org-default");
    }
}

/// A shared agent runs on its caller's own config and keys (NAS-1459).
#[cfg(test)]
mod shared_user_tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    const AGENT: &str = "11111111-1111-1111-1111-111111111111";
    const OWNER: &str = "22222222-2222-2222-2222-222222222222";
    const SHARED_USER: &str = "33333333-3333-3333-3333-333333333333";

    /// The agent's owner has a config naming their key; the shared user may have their own.
    struct SharedAgentStore {
        user_default: Option<LLMConfig>,
        /// Every `user_secrets` owner read — the owner's must never appear for a shared caller.
        secret_reads: Mutex<Vec<Uuid>>,
    }

    impl SharedAgentStore {
        fn new(user_default: Option<LLMConfig>) -> Self {
            Self {
                user_default,
                secret_reads: Mutex::new(Vec::new()),
            }
        }
        fn read_owners_secret(&self) -> bool {
            let owner = Uuid::parse_str(OWNER).unwrap();
            self.secret_reads.lock().unwrap().contains(&owner)
        }
    }

    fn config(provider: &str, model: &str, secret_name: Option<&str>) -> LLMConfig {
        LLMConfig {
            provider: provider.into(),
            model: Some(model.into()),
            fallback_models: vec![],
            temperature: None,
            max_tokens: None,
            api_key_secret_name: secret_name.map(str::to_string),
            pinned: false,
            pinned_model: None,
            tier1_model: None,
            tier2_model: None,
            tier3_model: None,
            embedding_provider: None,
            embedding_model: None,
            embedding_api_key_secret_name: None,
            embedding_model_override: false,
        }
    }

    #[async_trait]
    impl RegistryStore for SharedAgentStore {
        async fn fetch_llm_config(
            &self,
            _: Uuid,
        ) -> Result<Option<AgentConfigResult>, sqlx::Error> {
            Ok(Some(AgentConfigResult {
                config: Some(config("anthropic", "owner-model", Some("OWNER_KEY"))),
                agent_pinned_model: Some("owner-pin".into()),
                is_coding_agent: false,
                is_discovered_agent: false,
                compress_enabled: true,
            }))
        }
        async fn fetch_agent_owner(&self, _: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
            Ok(Uuid::parse_str(OWNER).ok())
        }
        async fn fetch_user_default_config(
            &self,
            user_id: Uuid,
        ) -> Result<Option<LLMConfig>, sqlx::Error> {
            assert_eq!(
                user_id.to_string(),
                SHARED_USER,
                "only the caller's own config"
            );
            Ok(self.user_default.clone())
        }
        async fn fetch_user_secret(
            &self,
            owner_id: Uuid,
            _: &str,
        ) -> Result<Option<String>, sqlx::Error> {
            self.secret_reads.lock().unwrap().push(owner_id);
            let plaintext = format!("key-of-{owner_id}");
            Ok(Some(SecretsCrypto::for_user(owner_id).encrypt(&plaintext)))
        }
        async fn fetch_live_flow(
            &self,
            _: &str,
            _: Uuid,
            _: i64,
        ) -> Result<Option<crate::routing::attribution::LiveFlow>, sqlx::Error> {
            unreachable!("resolution never attributes flows")
        }
        async fn fetch_custom_provider(
            &self,
            _: &str,
        ) -> Result<Option<CustomProvider>, sqlx::Error> {
            Ok(None)
        }
        async fn fetch_org_embedding_default(
            &self,
        ) -> Result<Option<OrgEmbeddingDefault>, sqlx::Error> {
            Ok(None)
        }
    }

    fn install_master_key() {
        use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
        // SAFETY: serialized via #[serial]; no other test reads the env concurrently.
        unsafe {
            std::env::set_var("SECRETS_ENCRYPTION_KEY", BASE64.encode([7u8; 32]));
        }
    }

    fn gemini_cfg() -> GatewayConfig {
        GatewayConfig {
            default_provider: "gemini".into(),
            default_model: "gemini-4-hydrogen".into(),
            platform_gemini_api_key: "g-key".into(),
            platform_openai_api_key: "sk".into(),
            ..GatewayConfig::default()
        }
    }

    fn hint<'a>(provider: &'a str, model: &'a str) -> RequestHint<'a> {
        RequestHint {
            provider: Some(provider),
            model: Some(model),
        }
    }

    #[test]
    fn openai_sdk_on_gemini_platform_uses_default_provider_and_model() {
        let cfg = gemini_cfg();
        // Provider comes from the platform, not the SDK surface — even with an OpenAI key.
        for model in ["gpt-4o-mini", "o3", "ft:gpt-4o:x", "gemini-2.0-flash"] {
            let h = super::usable_hint(&cfg, hint("openai", model));
            assert_eq!(h.provider, Some("gemini"), "{model}");
            assert_eq!(h.model, Some("gemini-4-hydrogen"), "{model}");
        }
        let h = super::usable_hint(&cfg, hint("anthropic", "claude-haiku-5-5"));
        assert_eq!(
            (h.provider, h.model),
            (Some("gemini"), Some("gemini-4-hydrogen"))
        );
    }

    #[test]
    fn native_surface_keeps_requested_model() {
        let cfg = gemini_cfg();
        let h = super::usable_hint(&cfg, hint("gemini", "gemini-2.5-pro"));
        assert_eq!(
            (h.provider, h.model),
            (Some("gemini"), Some("gemini-2.5-pro"))
        );
        let openai = GatewayConfig {
            default_provider: "openai".into(),
            default_model: "gpt-4o-mini".into(),
            platform_openai_api_key: "sk".into(),
            ..GatewayConfig::default()
        };
        let h = super::usable_hint(&openai, hint("openai", "gpt-4o"));
        assert_eq!((h.provider, h.model), (Some("openai"), Some("gpt-4o")));
    }

    #[test]
    fn explicit_prefix_picks_keyed_provider() {
        let cfg = gemini_cfg();
        let h = super::usable_hint(&cfg, hint("openai", "gemini/gemini-2.5-pro"));
        assert_eq!(
            (h.provider, h.model),
            (Some("gemini"), Some("gemini-2.5-pro"))
        );
        let h = super::usable_hint(&cfg, hint("openai", "openai/gpt-6.1-sol"));
        assert_eq!((h.provider, h.model), (Some("openai"), Some("gpt-6.1-sol")));
        // Unkeyed prefix is ignored → platform default.
        let h = super::usable_hint(&cfg, hint("openai", "anthropic/claude-haiku-5-5"));
        assert_eq!(
            (h.provider, h.model),
            (Some("gemini"), Some("gemini-4-hydrogen"))
        );
    }

    #[test]
    fn no_surface_foreign_model_gets_default_model() {
        let cfg = gemini_cfg();
        let h = super::usable_hint(
            &cfg,
            RequestHint {
                provider: None,
                model: Some("gpt-4o"),
            },
        );
        assert_eq!(
            (h.provider, h.model),
            (Some("gemini"), Some("gemini-4-hydrogen"))
        );
    }

    #[test]
    fn surface_family_match_ignores_case() {
        let cfg = gemini_cfg();
        let h = super::usable_hint(&cfg, hint("Gemini", "gemini-2.5-pro"));
        assert_eq!(
            (h.provider, h.model),
            (Some("gemini"), Some("gemini-2.5-pro"))
        );
    }

    #[test]
    fn openrouter_default_keeps_vendor_model_ids() {
        let cfg = GatewayConfig {
            default_provider: "openrouter".into(),
            default_model: "openai/gpt-6.1-sol".into(),
            platform_openrouter_api_key: "or-key".into(),
            ..GatewayConfig::default()
        };
        let h = super::usable_hint(&cfg, hint("openai", "meta-llama/llama-4"));
        assert_eq!(
            (h.provider, h.model),
            (Some("openrouter"), Some("meta-llama/llama-4"))
        );
        let h = super::usable_hint(&cfg, hint("anthropic", "claude-haiku-5-5"));
        assert_eq!(
            (h.provider, h.model),
            (Some("openrouter"), Some("openai/gpt-6.1-sol"))
        );
    }

    #[test]
    fn unavailable_provider_prefix_falls_back_to_default_model() {
        let cfg: GatewayConfig = gateway();
        let h: RequestHint<'_> =
            super::usable_hint(&cfg, hint("openai", "anthropic/claude-haiku-5-5"));
        assert_eq!((h.provider, h.model), (Some("openai"), Some("gpt-4o-mini")));
        let h: RequestHint<'_> = super::usable_hint(&cfg, hint("openai", "Gemini/gemini-2.5-pro"));
        assert_eq!((h.provider, h.model), (Some("openai"), Some("gpt-4o-mini")));
        let h: RequestHint<'_> = super::usable_hint(&cfg, hint("openai", "gpt-4o"));
        assert_eq!((h.provider, h.model), (Some("openai"), Some("gpt-4o")));
    }

    fn gateway() -> GatewayConfig {
        GatewayConfig {
            default_provider: "openai".into(),
            default_model: "gpt-4o-mini".into(),
            platform_openai_api_key: "sk-platform".into(),
            ..Default::default()
        }
    }

    async fn resolve_as(
        store: &SharedAgentStore,
        subject: ConfigSubject<'_>,
    ) -> Result<ResolvedConfig, GatewayError> {
        let cache = ConfigCache::new(Duration::from_secs(30));
        let hint = RequestHint::default();
        resolve_for(store, &cache, &gateway(), AGENT, subject, hint).await
    }

    fn shared_user() -> Uuid {
        Uuid::parse_str(SHARED_USER).unwrap()
    }

    #[test]
    fn a_caller_other_than_the_owner_is_a_shared_user() {
        assert_eq!(
            ConfigSubject::for_call(OWNER, Some(shared_user())),
            ConfigSubject::SharedUser(shared_user())
        );
    }

    #[test]
    fn the_owner_on_their_own_agent_and_an_unattributed_call_stay_on_the_owner() {
        let owner = Uuid::parse_str(OWNER).unwrap();
        assert_eq!(
            ConfigSubject::for_call(OWNER, Some(owner)),
            ConfigSubject::Owner(OWNER)
        );
        assert_eq!(
            ConfigSubject::for_call(OWNER, None),
            ConfigSubject::Owner(OWNER)
        );
        // A token without an owner claim already pays with the platform key, never an owner's.
        assert_eq!(
            ConfigSubject::for_call("", Some(shared_user())),
            ConfigSubject::Owner("")
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn a_shared_user_runs_on_their_own_default_config_and_key() {
        install_master_key();
        let store = SharedAgentStore::new(Some(config("openai", "user-model", Some("MY_KEY"))));
        let r = resolve_as(&store, ConfigSubject::SharedUser(shared_user()))
            .await
            .unwrap();
        assert_eq!(r.provider, "openai");
        assert_eq!(r.model, "user-model");
        assert_eq!(r.api_key, format!("key-of-{SHARED_USER}"));
        assert!(!r.platform_paid);
        assert!(!store.read_owners_secret(), "the owner's key is never read");
        // The owner's agent-level pin names a model on the owner's provider: not carried over.
        assert_eq!(r.pinned_model, None);
        // Agent flags still apply.
        assert!(r.compress_enabled);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn a_shared_user_without_a_config_is_refused_never_put_on_the_owners_key() {
        install_master_key();
        let store = SharedAgentStore::new(None);
        let err = resolve_as(&store, ConfigSubject::SharedUser(shared_user()))
            .await
            .unwrap_err();
        assert!(matches!(err, GatewayError::NoSharedUserConfig));
        assert_eq!(err.status(), axum::http::StatusCode::BAD_REQUEST);
        assert!(err.to_string().starts_with("Connect a model in LLM router"));
        assert!(!store.read_owners_secret());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn a_shared_users_embeddings_without_a_config_are_refused_too() {
        install_master_key();
        let store = SharedAgentStore::new(None);
        let cache = ConfigCache::new(Duration::from_secs(30));
        let subject = ConfigSubject::SharedUser(shared_user());
        let err = resolve_embeddings_for(&store, &cache, &gateway(), AGENT, subject, Some("m"))
            .await
            .unwrap_err();
        assert!(matches!(err, GatewayError::NoSharedUserConfig));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn the_owner_still_runs_on_the_agents_config_and_their_key() {
        install_master_key();
        let store = SharedAgentStore::new(None);
        let r = resolve_as(&store, ConfigSubject::Owner(OWNER))
            .await
            .unwrap();
        assert_eq!(r.model, "owner-model");
        assert_eq!(r.api_key, format!("key-of-{OWNER}"));
        assert_eq!(r.pinned_model.as_deref(), Some("owner-pin"));
    }

    const MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("../migrations");

    async fn insert_user(pool: &PgPool) -> Result<Uuid, sqlx::Error> {
        let name = Uuid::new_v4().simple().to_string();
        sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
            .bind(format!("u-{name}"))
            .bind(format!("{name}@test.local"))
            .fetch_one(pool)
            .await
    }

    async fn insert_agent(pool: &PgPool, owner: Uuid) -> Result<Uuid, sqlx::Error> {
        sqlx::query_scalar("INSERT INTO agents (name, owner_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("a-{}", Uuid::new_v4().simple()))
            .bind(owner)
            .fetch_one(pool)
            .await
    }

    /// A config whose model is its name, so a read says which row it returned.
    async fn insert_config(
        pool: &PgPool,
        owner: Uuid,
        name: &str,
        is_default: bool,
    ) -> Result<Uuid, sqlx::Error> {
        sqlx::query_scalar(
            "INSERT INTO llm_configs (created_by, name, provider, model, is_default) \
             VALUES ($1, $2, 'openai', $2, $3) RETURNING id",
        )
        .bind(owner)
        .bind(name)
        .bind(is_default)
        .fetch_one(pool)
        .await
    }

    async fn choose(
        pool: &PgPool,
        user: Uuid,
        agent: Uuid,
        config: Uuid,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO agent_user_llm_configs (user_id, agent_id, llm_config_id) \
             VALUES ($1, $2, $3)",
        )
        .bind(user)
        .bind(agent)
        .bind(config)
        .execute(pool)
        .await
        .map(|_| ())
    }

    /// A config that is neither the owner's default nor attached to the agent is not the agent's,
    /// so the owner's call finds no key and a deployment may cover it.
    #[sqlx::test(migrator = "MIGRATIONS")]
    #[ignore = "requires disposable PostgreSQL"]
    async fn an_owner_config_neither_default_nor_attached_resolves_to_none(
        pool: PgPool,
    ) -> Result<(), sqlx::Error> {
        let store = PgRegistry::new(pool.clone());
        let alice = insert_user(&pool).await?;
        let agent = insert_agent(&pool, alice).await?;
        insert_config(&pool, alice, "spare", false).await?;
        let read = store.fetch_llm_config(agent).await?.expect("agent row");
        assert!(read.config.is_none());

        insert_config(&pool, alice, "alice-default", true).await?;
        let read = store.fetch_llm_config(agent).await?.expect("agent row");
        let model = read.config.and_then(|c| c.model);
        assert_eq!(model.as_deref(), Some("alice-default"));
        Ok(())
    }

    #[sqlx::test(migrator = "MIGRATIONS")]
    #[ignore = "requires disposable PostgreSQL"]
    async fn a_users_choice_for_an_agent_is_read_live_and_never_anothers(
        pool: PgPool,
    ) -> Result<(), sqlx::Error> {
        let store = PgRegistry::new(pool.clone());
        let model = |config: Option<LLMConfig>| config.and_then(|c| c.model);
        let (alice, bob, carol) = (
            insert_user(&pool).await?,
            insert_user(&pool).await?,
            insert_user(&pool).await?,
        );
        let (agent, other_agent) = (
            insert_agent(&pool, alice).await?,
            insert_agent(&pool, alice).await?,
        );
        insert_config(&pool, alice, "alice-default", true).await?;
        let chosen = insert_config(&pool, alice, "alice-chosen", false).await?;
        insert_config(&pool, bob, "bob-default", true).await?;
        choose(&pool, alice, agent, chosen).await?;

        // The choice wins for its own agent; every other agent gets the default.
        assert!(store.has_user_agent_config(alice, agent).await?);
        let on_agent = store.fetch_user_agent_config(alice, agent).await?;
        assert_eq!(model(on_agent).as_deref(), Some("alice-chosen"));
        assert!(!store.has_user_agent_config(alice, other_agent).await?);
        let on_other = store.fetch_user_agent_config(alice, other_agent).await?;
        assert_eq!(model(on_other).as_deref(), Some("alice-default"));

        // Alice's choice is not Bob's, even from a row of his that names her config: the
        // payer is still asked about him, not short-circuited by that row.
        choose(&pool, bob, agent, chosen).await?;
        let payer = crate::payer::RecordingPayer::answering(Some(crate::payer::Payer::Owner));
        let alice_id = alice.to_string();
        let (subject, _) = crate::payer::subject_for_call(
            &store,
            payer.as_ref(),
            &agent.to_string(),
            &alice_id,
            Some(bob),
        )
        .await
        .unwrap();
        assert_eq!(subject, ConfigSubject::Sponsored(&alice_id));
        assert_eq!(*payer.calls.lock().unwrap(), [(alice, bob)]);
        assert!(!store.has_user_agent_config(bob, agent).await?);
        let bobs = store.fetch_user_agent_config(bob, agent).await?;
        assert_eq!(model(bobs).as_deref(), Some("bob-default"));

        // A soft-deleted config reads as no choice: the default takes over.
        sqlx::query("UPDATE llm_configs SET deleted_at = now() WHERE id = $1")
            .bind(chosen)
            .execute(&pool)
            .await?;
        assert!(!store.has_user_agent_config(alice, agent).await?);
        let after_delete = store.fetch_user_agent_config(alice, agent).await?;
        assert_eq!(model(after_delete).as_deref(), Some("alice-default"));

        // Neither a choice nor a default is no config.
        assert!(!store.has_user_agent_config(carol, agent).await?);
        assert!(store.fetch_user_agent_config(carol, agent).await?.is_none());
        Ok(())
    }
}
