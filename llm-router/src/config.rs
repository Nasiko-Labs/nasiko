//! Gateway-only configuration.
//!
//! Owned by this crate (not `nasiko-config`) so the LLM router stays decoupled and
//! can be promoted to a standalone binary later without dragging in the platform's
//! full `Config`. Env-var *names* match the platform for deployment consistency.

/// Configuration for the LLM router, read from the environment.
///
/// See `RUST_PLAN_V1.md` §5. All fields have sane defaults so `from_env` never fails;
/// fail-closed behaviour (e.g. an empty `agent_jwt_secret`) is enforced at use sites.
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    /// Shared HS256 secret the orchestrator mints agent-identity JWTs with. Empty ⇒
    /// every request is rejected 401 (fail closed) — never fail open.
    pub agent_jwt_secret: String,
    /// JWT signing algorithm. Default `HS256`.
    pub agent_jwt_algorithm: String,

    /// Backward-compat provider when an agent has no `llm_config`. Default `openai`.
    pub default_provider: String,
    /// Backward-compat model when an agent has no `llm_config`. Default `gpt-4o-mini`.
    pub default_model: String,
    /// Platform-owned key used when an agent sets no `api_key_secret_name`.
    pub platform_openai_api_key: String,

    /// TTL (seconds) for the in-process per-agent `llm_config` cache. Default 30.
    pub llm_config_cache_ttl_secs: u64,

    /// Provider base URLs (overridable for tests / self-hosted gateways).
    pub openai_api_base: String,
    pub anthropic_api_base: String,
    pub gemini_api_base: String,

    /// Gateway origin (`scheme://host[:port]`) that deployed agents reach this router
    /// at, used by the deploy-time injector (Phase 2). The injector appends `/llm/v1`
    /// (the Pingora `/llm` strip route) when building the agent's `*_BASE_URL`. Empty ⇒
    /// the injector skips LLM wiring (fail closed — no broken base URL without a key).
    pub llm_gateway_base_url: String,
}

impl Default for GatewayConfig {
    /// The canonical defaults (also the values `from_env` falls back to per key).
    fn default() -> Self {
        Self {
            agent_jwt_secret: String::new(),
            agent_jwt_algorithm: "HS256".into(),
            default_provider: "openai".into(),
            default_model: "gpt-4o-mini".into(),
            platform_openai_api_key: String::new(),
            llm_config_cache_ttl_secs: 30,
            openai_api_base: "https://api.openai.com/v1".into(),
            anthropic_api_base: "https://api.anthropic.com/v1".into(),
            gemini_api_base: "https://generativelanguage.googleapis.com/v1beta".into(),
            llm_gateway_base_url: String::new(),
        }
    }
}

impl GatewayConfig {
    /// Load configuration from the process environment, falling back to [`Default`]
    /// per key.
    pub fn from_env() -> Self {
        let d = Self::default();
        Self {
            agent_jwt_secret: env_or("AGENT_JWT_SECRET", &d.agent_jwt_secret),
            agent_jwt_algorithm: env_or("AGENT_JWT_ALGORITHM", &d.agent_jwt_algorithm),
            default_provider: env_or("DEFAULT_PROVIDER", &d.default_provider),
            default_model: env_or("DEFAULT_MODEL", &d.default_model),
            platform_openai_api_key: env_or("PLATFORM_OPENAI_API_KEY", &d.platform_openai_api_key),
            llm_config_cache_ttl_secs: std::env::var("LLM_CONFIG_CACHE_TTL")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d.llm_config_cache_ttl_secs),
            openai_api_base: env_or("OPENAI_API_BASE", &d.openai_api_base),
            anthropic_api_base: env_or("ANTHROPIC_API_BASE", &d.anthropic_api_base),
            gemini_api_base: env_or("GEMINI_API_BASE", &d.gemini_api_base),
            llm_gateway_base_url: env_or("LLM_GATEWAY_BASE_URL", &d.llm_gateway_base_url),
        }
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
