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
    /// Platform-owned OpenAI key, used for the `openai` provider (and as the
    /// backward-compat fallback for unknown providers) when an agent sets no
    /// `api_key_secret_name`. Select via [`GatewayConfig::platform_key_for`].
    pub platform_openai_api_key: String,
    /// Platform-owned Anthropic key, used for the `anthropic` provider.
    pub platform_anthropic_api_key: String,
    /// Platform-owned Gemini key, used for the `gemini` provider.
    pub platform_gemini_api_key: String,
    /// Platform-owned OpenRouter key, used for the `openrouter` provider.
    pub platform_openrouter_api_key: String,

    /// TTL (seconds) for the in-process per-agent `llm_config` cache. Default 30.
    pub llm_config_cache_ttl_secs: u64,

    /// Redis URL for the model-routing decision cache (S3). Empty ⇒ the router uses a
    /// no-op cache (every request re-derives its model). The cache is a latency
    /// optimisation only — an unset/unreachable Redis never breaks routing.
    pub redis_url: String,
    /// TTL (seconds) for a cached `(conv_id, agent_id)` routing decision — the stickiness
    /// window for a conversation. Default 3600 (1h). On expiry, a continuation turn falls
    /// through to the configured model (Level 4), same as a cache miss.
    pub router_decision_ttl_secs: u64,

    /// Max age of a `status='running'` flow for traceparent attribution — bounds
    /// orphaned flows (a direct-chat flow whose completion marking never ran stays
    /// 'running' but ages out of attribution, so its trace id stops authorizing
    /// LLM calls).
    ///
    /// Must never be shorter than the platform's flow timeout, or an agent turn
    /// still inside its budget loses the right to make LLM calls part-way
    /// through and takes a 403 mid-answer. It therefore defaults to
    /// `NASIKO_FLOW_TIMEOUT_SECS` and only falls back to a literal when that is
    /// unset too; `LLM_ATTRIBUTION_WINDOW_SECS` still overrides both.
    pub attribution_window_secs: u64,

    /// Interval between provider model-catalog syncs (`GET /models` →
    /// `provider_models`). Provider model lists move slowly — default 86400 (24 h).
    pub model_catalog_sync_interval_secs: u64,

    /// Interval between Portkey price-book syncs (`model_pricing`). Prices move
    /// slowly — default 86400 (24 h).
    pub pricing_sync_interval_secs: u64,

    /// Provider base URLs (overridable for tests / self-hosted gateways).
    pub openai_api_base: String,
    pub anthropic_api_base: String,
    pub gemini_api_base: String,
    pub openrouter_api_base: String,

    /// Optional OpenRouter attribution headers (`HTTP-Referer` / `X-Title`) — affect
    /// openrouter.ai app rankings only, harmless to leave empty.
    pub openrouter_http_referer: String,
    pub openrouter_x_title: String,

    /// Gateway origin (`scheme://host[:port]`) that deployed agents reach this router
    /// at, used by the deploy-time injector (Phase 2). The injector appends `/llm/v1`
    /// (the Pingora `/llm` strip route) when building the agent's `*_BASE_URL`. Empty ⇒
    /// the injector skips LLM wiring (fail closed — no broken base URL without a key).
    pub llm_gateway_base_url: String,

    /// Level 2.5 salience gate: an in-process classifier decides whether a boundary turn
    /// is substantive enough to classify + pin, or is small talk to be served cheaply
    /// without pinning. Enabled by default. When `false`, the router classifies at every
    /// fireable boundary (behaviour before the gate existed).
    pub salience_gate_enabled: bool,
    /// Optional override: path to a trained weights JSON (same schema as the embedded
    /// asset) to load *instead of* the model embedded in the binary. Empty (the
    /// default) ⇒ use the embedded model, which needs no deployment step. Exists so a
    /// candidate model can be trialled without a rebuild; a load failure falls back to
    /// classifying every boundary, never to an outage.
    pub salience_weights_path: String,
    /// Below this probability the classifier confidently judges the turn small talk and
    /// the gate defers. This is the only threshold that changes a routing outcome —
    /// raising it defers more turns. Default 0.20; tune against validation data.
    pub salience_low_threshold: f64,
    /// Above this probability the classifier is confidently substantive. Turns between the
    /// thresholds route too, so this does not change routing on its own — it marks the
    /// uncertain band in the gate's logs so its size can be measured before `low` is
    /// retuned. Default 0.80.
    pub salience_high_threshold: f64,

    /// Fleet-wide kill switch for payload compression. Compression is opted into **per agent**
    /// (`agents.compress_enabled`); this only lets an operator stop all of it at once without
    /// editing every agent's row. Default on, so a UI toggle takes effect without a deploy.
    pub compress_kill_switch: bool,
    /// Skip payloads below this size — compressing them costs more than it saves.
    pub compress_min_bytes: usize,
    /// Which detected content types may be compressed.
    pub compress_types: nasiko_compress::TypeMask,
    pub compress_level: nasiko_compress::Level,
    /// Measure without mutating: stats are recorded, the request is sent untouched.
    pub compress_dry_run: bool,

    /// Append the brevity directive to outbound requests (IP-2, output tokens).
    ///
    /// Defaults **on**, like every layer here: nothing runs until an agent's own
    /// `compress_enabled` switch is set, so the fleet default is a kill switch, never the thing
    /// that turns a layer on. An operator sets this to `false` to stop IP-2 everywhere at once.
    pub brevity_enabled: bool,
    /// Skip the directive below this transcript size. **Defaults to 0 — every turn gets it.**
    ///
    /// The directive is ~114 tokens of input. Output bills at 4x input on the default model, so
    /// it repays itself by saving just ~29 output tokens; at the ~9% output reduction measured
    /// on real traffic that is a turn producing ~300 output tokens or more.
    ///
    /// A floor here gates on *input* size, which is the wrong dimension: the payoff scales with
    /// how much the model is about to **write**, and the two do not correlate. "Summarise the
    /// timeline" is a small input with a large output — the turn brevity helps most, and exactly
    /// the one an input floor would skip.
    ///
    /// There is no prefix-cache cost to weigh against it either: the directive is appended as a
    /// *trailing* system message, so the cached prefix is untouched. That is what separates this
    /// from the compression layers, where §14 R2 governs.
    ///
    /// Raise it only if real traffic turns out to be dominated by very short replies (under
    /// ~250 output tokens), where the fixed cost stops being repaid.
    pub brevity_min_bytes: usize,
    /// Percent of otherwise-eligible calls the directive is withheld from, to keep a control arm.
    ///
    /// This is a real, accepted cost: that slice forgoes the optimization. It buys the only
    /// unbiased measurement of a layer whose saving cannot be subtracted, and it is bounded by
    /// exactly the figure it exists to establish. `0` disables the holdout and leaves `apply`
    /// byte-identical to a build without it.
    pub brevity_holdout_pct: u8,

    /// Persist pre-compression originals so an agent can recover what was elided (IP-5).
    /// Defaults **on**; only ever writes a row when compression actually elided something, which
    /// requires the agent's switch, so an opted-out fleet stores nothing.
    pub compress_recovery_enabled: bool,
    /// Only mint a handle for payloads above this size. Below it the elided content is small
    /// enough that a recovery round-trip costs more than re-sending would have.
    pub compress_recovery_min_bytes: usize,
    /// How long an original stays recoverable. Sized to outlive the flow that produced it.
    pub compress_recovery_ttl_secs: u64,

    /// Opt-in compact tool schemas (nasiko-tool-compact). Default **off**: with it
    /// off the request path is byte-identical to a build without this flag.
    /// Non-streaming requests only; streaming and `tool_choice` requests bypass.
    pub compact_tools_enabled: bool,
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
            platform_anthropic_api_key: String::new(),
            platform_gemini_api_key: String::new(),
            platform_openrouter_api_key: String::new(),
            llm_config_cache_ttl_secs: 30,
            redis_url: String::new(),
            router_decision_ttl_secs: 3600,
            attribution_window_secs: 600,
            model_catalog_sync_interval_secs: 86_400,
            pricing_sync_interval_secs: 86_400,
            openai_api_base: "https://api.openai.com/v1".into(),
            anthropic_api_base: "https://api.anthropic.com/v1".into(),
            gemini_api_base: "https://generativelanguage.googleapis.com/v1beta".into(),
            openrouter_api_base: "https://openrouter.ai/api/v1".into(),
            openrouter_http_referer: String::new(),
            openrouter_x_title: String::new(),
            llm_gateway_base_url: String::new(),
            salience_gate_enabled: true,
            salience_weights_path: String::new(),
            salience_low_threshold: 0.20,
            salience_high_threshold: 0.80,
            compress_kill_switch: true,
            compress_min_bytes: 2048,
            compress_types: nasiko_compress::TypeMask::DEFAULT,
            compress_level: nasiko_compress::Level::Conservative,
            compress_dry_run: false,
            brevity_enabled: true,
            brevity_min_bytes: 0,
            brevity_holdout_pct: 5,
            compress_recovery_enabled: true,
            compress_recovery_min_bytes: 8192,
            compress_recovery_ttl_secs: 86_400,
            compact_tools_enabled: false,
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
            // Per-provider platform keys. Prefer the explicit `PLATFORM_*` name, then
            // fall back to the generic provider key env var (which agents/orchestrator
            // already set), so a single provider key "just works" without duplication.
            platform_openai_api_key: env_first(
                &["PLATFORM_OPENAI_API_KEY", "OPENAI_API_KEY"],
                &d.platform_openai_api_key,
            ),
            platform_anthropic_api_key: env_first(
                &["PLATFORM_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"],
                &d.platform_anthropic_api_key,
            ),
            platform_gemini_api_key: env_first(
                &["PLATFORM_GEMINI_API_KEY", "GEMINI_API_KEY"],
                &d.platform_gemini_api_key,
            ),
            platform_openrouter_api_key: env_first(
                &["PLATFORM_OPENROUTER_API_KEY", "OPENROUTER_API_KEY"],
                &d.platform_openrouter_api_key,
            ),
            llm_config_cache_ttl_secs: std::env::var("LLM_CONFIG_CACHE_TTL")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d.llm_config_cache_ttl_secs),
            redis_url: env_or("REDIS_URL", &d.redis_url),
            router_decision_ttl_secs: std::env::var("ROUTER_DECISION_TTL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d.router_decision_ttl_secs),
            attribution_window_secs: std::env::var("LLM_ATTRIBUTION_WINDOW_SECS")
                .or_else(|_| std::env::var("NASIKO_FLOW_TIMEOUT_SECS"))
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d.attribution_window_secs),
            model_catalog_sync_interval_secs: std::env::var("MODEL_CATALOG_SYNC_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d.model_catalog_sync_interval_secs),
            pricing_sync_interval_secs: env_parse_first(
                &[
                    "PRICING_SYNC_INTERVAL_SECS",
                    "MODEL_PRICING_SYNC_INTERVAL_SECS",
                ],
                d.pricing_sync_interval_secs,
            ),
            openai_api_base: env_or("OPENAI_API_BASE", &d.openai_api_base),
            anthropic_api_base: env_or("ANTHROPIC_API_BASE", &d.anthropic_api_base),
            gemini_api_base: env_or("GEMINI_API_BASE", &d.gemini_api_base),
            openrouter_api_base: env_or("OPENROUTER_API_BASE", &d.openrouter_api_base),
            openrouter_http_referer: env_or("OPENROUTER_HTTP_REFERER", &d.openrouter_http_referer),
            openrouter_x_title: env_or("OPENROUTER_X_TITLE", &d.openrouter_x_title),
            llm_gateway_base_url: env_or("LLM_GATEWAY_BASE_URL", &d.llm_gateway_base_url),
            salience_gate_enabled: std::env::var("SALIENCE_GATE_ENABLED")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d.salience_gate_enabled),
            salience_weights_path: env_or("SALIENCE_WEIGHTS_PATH", &d.salience_weights_path),
            salience_low_threshold: std::env::var("SALIENCE_LOW_THRESHOLD")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d.salience_low_threshold),
            salience_high_threshold: std::env::var("SALIENCE_HIGH_THRESHOLD")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d.salience_high_threshold),
            compress_kill_switch: env_flag("TOKEN_COMPRESS_ENABLED", true),
            compress_min_bytes: env_usize("TOKEN_COMPRESS_MIN_BYTES", 2048),
            // A bad label must not silently widen or narrow what gets rewritten, so an
            // unparseable value falls back to the shipped default and says so.
            compress_types: parse_or_warn(
                "TOKEN_COMPRESS_TYPES",
                nasiko_compress::TypeMask::from_labels,
                nasiko_compress::TypeMask::DEFAULT,
            ),
            compress_level: parse_or_warn(
                "TOKEN_COMPRESS_LEVEL",
                nasiko_compress::Level::parse,
                nasiko_compress::Level::Conservative,
            ),
            compress_dry_run: env_flag("TOKEN_COMPRESS_DRY_RUN", false),
            compact_tools_enabled: env_flag("TOKEN_COMPACT_TOOLS", d.compact_tools_enabled),
            brevity_enabled: env_flag("TOKEN_BREVITY", d.brevity_enabled),
            brevity_min_bytes: env_usize("TOKEN_BREVITY_MIN_BYTES", d.brevity_min_bytes),
            brevity_holdout_pct: env_usize(
                "TOKEN_BREVITY_HOLDOUT_PCT",
                d.brevity_holdout_pct as usize,
            )
            .min(100) as u8,
            compress_recovery_enabled: env_flag(
                "TOKEN_COMPRESS_RECOVERY",
                d.compress_recovery_enabled,
            ),
            compress_recovery_min_bytes: env_usize(
                "TOKEN_COMPRESS_RECOVERY_MIN_BYTES",
                d.compress_recovery_min_bytes,
            ),
            compress_recovery_ttl_secs: env_usize(
                "TOKEN_COMPRESS_RECOVERY_TTL_SECS",
                d.compress_recovery_ttl_secs as usize,
            ) as u64,
        }
    }

    /// The platform-owned fallback key for a **built-in** `provider`, used when an
    /// agent sets no per-user `api_key_secret_name`. An unknown provider (e.g. a
    /// DB-registered custom provider, or a mistyped name) returns `""` — never the
    /// OpenAI key: handing the platform's real OpenAI key to an arbitrary admin-typed
    /// base URL would leak it. Custom providers supply their own key via the resolved
    /// config (see `resolver::resolve`).
    pub fn platform_key_for(&self, provider: &str) -> &str {
        match provider {
            "openai" => &self.platform_openai_api_key,
            "anthropic" => &self.platform_anthropic_api_key,
            "gemini" => &self.platform_gemini_api_key,
            "openrouter" => &self.platform_openrouter_api_key,
            _ => "",
        }
    }
}

/// First parseable env var among `keys`, else `default`. Used where a setting
/// has been renamed: the current key wins, the legacy key still works. Without
/// this, dropping the old name would silently revert a deliberately tuned
/// operator value back to the built-in default.
fn env_parse_first<T: std::str::FromStr>(keys: &[&str], default: T) -> T {
    for key in keys {
        if let Ok(val) = std::env::var(key)
            && let Ok(parsed) = val.parse()
        {
            return parsed;
        }
    }
    default
}

/// `"true"`/`"1"` is on, `"false"`/`"0"` is off, anything else (including unset) is `default`.
fn env_flag(key: &str, default: bool) -> bool {
    match std::env::var(key).ok().as_deref() {
        Some("true" | "1") => true,
        Some("false" | "0") => false,
        _ => default,
    }
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Parse an env var with `f`, warning loudly and falling back on a bad value.
///
/// Silently defaulting would be worse than usual here: these values decide how much of a
/// request gets rewritten, so a typo that quietly widened the set would be invisible until a
/// model started answering from elided data.
fn parse_or_warn<T, E: std::fmt::Display>(
    key: &str,
    f: impl Fn(&str) -> Result<T, E>,
    default: T,
) -> T {
    let Ok(raw) = std::env::var(key) else {
        return default;
    };
    if raw.trim().is_empty() {
        return default;
    }
    match f(&raw) {
        Ok(parsed) => parsed,
        Err(e) => {
            tracing::warn!(
                target: "nasiko::llm_router::startup",
                env = key, value = %raw, error = %e,
                "llm-router: unparseable value; using the built-in default"
            );
            default
        }
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// First non-empty env var among `keys`, else `default`. Lets a `PLATFORM_*` key take
/// precedence over the generic provider key env var while treating an empty value as unset.
fn env_first(keys: &[&str], default: &str) -> String {
    for key in keys {
        if let Ok(val) = std::env::var(key)
            && !val.is_empty()
        {
            return val;
        }
    }
    default.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_key_for_built_ins() {
        let cfg = GatewayConfig {
            platform_openai_api_key: "sk-openai".into(),
            platform_anthropic_api_key: "sk-ant".into(),
            platform_gemini_api_key: "sk-gem".into(),
            ..Default::default()
        };
        assert_eq!(cfg.platform_key_for("openai"), "sk-openai");
        assert_eq!(cfg.platform_key_for("anthropic"), "sk-ant");
        assert_eq!(cfg.platform_key_for("gemini"), "sk-gem");
    }

    #[test]
    fn platform_key_for_unknown_never_returns_openai_key() {
        // An unknown provider name (a custom provider, or a typo) must NEVER be handed
        // the platform OpenAI key — that key would then be sent to an arbitrary
        // admin-typed base URL. Fail closed with an empty string instead.
        let cfg = GatewayConfig {
            platform_openai_api_key: "sk-openai".into(),
            ..Default::default()
        };
        assert_eq!(cfg.platform_key_for("my-gateway"), "");
        assert_eq!(cfg.platform_key_for("deepseek"), "");
        assert_eq!(cfg.platform_key_for(""), "");
    }
}
