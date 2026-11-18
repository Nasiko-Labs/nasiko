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

    /// Whether platform keys fall back to the generic provider env var (e.g.
    /// `OPENAI_API_KEY`) when the `PLATFORM_*` name is unset. Default `true`
    /// (today's behaviour: a single provider key works for both orchestrator
    /// and agents). When `false`, the keys an agent's call may fall back to
    /// come only from the `PLATFORM_*` names, so a deployment can give the
    /// platform `OPENAI_API_KEY` while agents get no platform key. The
    /// platform's own calls are not affected: see [`GatewayConfig::service_key_for`].
    pub generic_key_fallback: bool,
    /// The platform's own OpenAI key: `PLATFORM_OPENAI_API_KEY`, else `OPENAI_API_KEY`,
    /// whatever `generic_key_fallback` says. Select via [`GatewayConfig::service_key_for`].
    pub service_openai_api_key: String,
    /// The platform's own Anthropic key, read as `service_openai_api_key` is.
    pub service_anthropic_api_key: String,
    /// The platform's own Gemini key, read as `service_openai_api_key` is.
    pub service_gemini_api_key: String,
    /// The platform's own OpenRouter key, read as `service_openai_api_key` is.
    pub service_openrouter_api_key: String,

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

    /// Longest a provider connection may go without receiving a byte before the call
    /// fails. A read timeout, not a total one: it restarts on every chunk, so a stream
    /// that keeps arriving is never cut, while a non-streaming call (silent until its
    /// reply is ready) is bounded by it. It is also the deadline for the whole fallback
    /// chain — every attempt and parameter-fix retry together, up to the first streamed
    /// chunk — so fallbacks never multiply it. Unset, it sits just below the calling agent's
    /// own timeout (`AGENT_CALL_TIMEOUT_SECS` less a small margin), so the agent never
    /// retries while the router still waits; 300 when that is unset too.
    pub provider_timeout_secs: u64,

    pub max_body_bytes: usize,
    pub max_inflight_body_bytes: usize,
    pub attachment_cache_bytes: usize,
    pub attachment_max_bytes: usize,

    /// Interval between provider model-catalog syncs (`GET /models` →
    /// `provider_models`). Provider model lists move slowly — default 86400 (24 h).
    pub model_catalog_sync_interval_secs: u64,

    /// Interval between Portkey price-book syncs (`model_pricing`). Prices move
    /// slowly — default 86400 (24 h).
    pub pricing_sync_interval_secs: u64,

    /// Provider base URLs (overridable for tests / self-hosted gateways).
    pub openai_api_base: String,
    /// Most function tools OpenAI's `/v1/chat/completions` accepts in one request; it
    /// rejects a longer array outright (`array_above_max_length`), so an agent whose
    /// tool set has grown past this — a coding client with several MCP servers
    /// attached, say — fails every turn rather than degrading. Over the cap the
    /// OpenAI spoke keeps the caller's own tools and fills the remainder with the MCP
    /// tools most relevant to the turn. `0` disables the cap, for an
    /// OpenAI-compatible endpoint that has none.
    pub openai_max_tools: usize,
    pub anthropic_api_base: String,
    pub gemini_api_base: String,
    /// Vertex AI (ADC auth): GCP project id. Empty disables the `vertex` provider.
    /// Env `GOOGLE_CLOUD_PROJECT`.
    pub vertex_project: String,
    /// Vertex location, env `GOOGLE_CLOUD_LOCATION` (default `global`).
    pub vertex_location: String,
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

    /// Request classifier backend: `local` (default; in-process linear model, embedded weights, no
    /// network, regex as its fallback), `regex` (keyword rules only) or `hosted`
    /// (OpenAI-compatible endpoint, opt-in). Unknown values are treated as `regex`.
    pub classifier_backend: String,
    /// `local` only: weights JSON file to load instead of the embedded weights. Empty = embedded.
    /// A file that cannot be loaded degrades to the regex classifier (logged), never an outage.
    pub classifier_model_path: String,
    /// `hosted` only: full chat-completions URL. Only this host receives the key.
    pub classifier_endpoint: String,
    /// `hosted` only: model id sent in the request.
    pub classifier_model: String,
    /// `hosted` only: bearer key for the endpoint. Never logged.
    pub classifier_api_key: String,
    /// Per-decision budget. Past it the router uses the regex answer and counts a fallback.
    pub classifier_timeout_ms: u64,
    /// Below this confidence the router uses the safe default (`general`). 0 disables.
    pub classifier_min_confidence: f32,
    /// Seed for Thompson tier sampling (`ROUTER_TIER_SEED`). Unset (default) = entropy, as
    /// before. Set = the tier is a deterministic function of (seed, provider, query, request
    /// type, learned cells), so identical inputs and state route identically.
    pub router_tier_seed: Option<u64>,

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
    /// There is no prefix-cache cost to weigh against it either: the directive is sent after the
    /// conversation, so the cached prefix is untouched. On OpenAI and OpenRouter it is a trailing
    /// system message; Anthropic, Bedrock and Gemini, which hoist system messages ahead of the
    /// conversation, get it as the last block of the last user turn instead (`brevity.rs`).
    /// That is what separates this from the compression layers, where §14 R2 governs.
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

    /// Replace native tool definitions with compact signatures and decode the model's compact
    /// calls back into `tool_calls` (`nasiko-tool-compact`; non-streaming requests only).
    ///
    /// Defaults **off**, unlike the layers above: it changes what the model is asked to write, so
    /// an operator opts in, and this flag alone turns it on (the per-agent `compress_enabled`
    /// switch is not consulted). Off, requests and responses are byte-identical to a build
    /// without it.
    pub compact_tools_enabled: bool,

    /// Add the router's own prompt-cache markers where the agent left slots free
    /// (`TOKEN_CACHE_MARKERS`): on Anthropic, on Bedrock where the model caches, and on Claude
    /// models through OpenRouter. Each adapter places its own, since providers differ in where a
    /// marker may go. On OpenAI, which takes no markers, it sets `prompt_cache_key` to the agent
    /// id on models before GPT-5.6; on OpenRouter it sets `session_id` to it, which keeps the
    /// agent on one upstream provider. On Gemini it turns the agent's own markers into an
    /// explicit `cachedContents` cache of the marked prefix (`providers/gemini_cache.rs`);
    /// off, Gemini calls rely on its implicit caching alone.
    ///
    /// Defaults **on** and needs no per-agent switch, unlike compression: a marker changes what a
    /// call costs, never what the model sees. Off, the router sends only the agent's own markers
    /// and key.
    pub cache_markers_enabled: bool,

    /// Persist pre-compression originals so an agent can recover what was elided (IP-5).
    /// Defaults **on**; only ever writes a row when compression actually elided something, which
    /// requires the agent's switch, so an opted-out fleet stores nothing.
    pub compress_recovery_enabled: bool,
    /// Only mint a handle for payloads above this size. Below it the elided content is small
    /// enough that a recovery round-trip costs more than re-sending would have.
    pub compress_recovery_min_bytes: usize,
    /// How long an original stays recoverable. Sized to outlive the flow that produced it.
    pub compress_recovery_ttl_secs: u64,
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
            generic_key_fallback: true,
            service_openai_api_key: String::new(),
            service_anthropic_api_key: String::new(),
            service_gemini_api_key: String::new(),
            service_openrouter_api_key: String::new(),
            llm_config_cache_ttl_secs: 30,
            redis_url: String::new(),
            router_decision_ttl_secs: 3600,
            attribution_window_secs: 600,
            provider_timeout_secs: 300,
            max_body_bytes: 48 * 1024 * 1024,
            max_inflight_body_bytes: 256 * 1024 * 1024,
            attachment_cache_bytes: 256 * 1024 * 1024,
            attachment_max_bytes: 30 * 1024 * 1024,
            model_catalog_sync_interval_secs: 86_400,
            pricing_sync_interval_secs: 86_400,
            openai_api_base: "https://api.openai.com/v1".into(),
            openai_max_tools: 128,
            anthropic_api_base: "https://api.anthropic.com/v1".into(),
            gemini_api_base: "https://generativelanguage.googleapis.com/v1beta".into(),
            vertex_project: String::new(),
            vertex_location: "global".into(),
            openrouter_api_base: "https://openrouter.ai/api/v1".into(),
            openrouter_http_referer: String::new(),
            openrouter_x_title: String::new(),
            llm_gateway_base_url: String::new(),
            salience_gate_enabled: true,
            salience_weights_path: String::new(),
            salience_low_threshold: 0.20,
            salience_high_threshold: 0.80,
            classifier_backend: "local".into(),
            classifier_model_path: String::new(),
            classifier_endpoint: String::new(),
            classifier_model: String::new(),
            classifier_api_key: String::new(),
            classifier_timeout_ms: 250,
            classifier_min_confidence: 0.0,
            router_tier_seed: None,
            compress_kill_switch: true,
            compress_min_bytes: 2048,
            compress_types: nasiko_compress::TypeMask::DEFAULT,
            compress_level: nasiko_compress::Level::Conservative,
            compress_dry_run: false,
            brevity_enabled: true,
            brevity_min_bytes: 0,
            brevity_holdout_pct: 5,
            compact_tools_enabled: false,
            cache_markers_enabled: true,
            compress_recovery_enabled: true,
            compress_recovery_min_bytes: 8192,
            compress_recovery_ttl_secs: 86_400,
        }
    }
}

impl GatewayConfig {
    pub fn llm_config_cache_ttl_secs_from_env() -> u64 {
        std::env::var("LLM_CONFIG_CACHE_TTL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(Self::default().llm_config_cache_ttl_secs)
    }

    /// Load configuration from the process environment, falling back to [`Default`]
    /// per key.
    pub fn from_env() -> Self {
        let d = Self::default();

        // Parse the generic key fallback flag strictly.
        let generic_key_fallback = match std::env::var("LLM_ROUTER_GENERIC_KEY_FALLBACK") {
            Ok(val) => {
                let lower = val.trim().to_lowercase();
                match lower.as_str() {
                    "true" | "1" => true,
                    "false" | "0" => false,
                    _ => panic!(
                        "LLM_ROUTER_GENERIC_KEY_FALLBACK must be true, false, 1, or 0 (got {:?})",
                        val
                    ),
                }
            }
            Err(_) => d.generic_key_fallback,
        };

        let mut c = Self {
            agent_jwt_secret: env_or("AGENT_JWT_SECRET", &d.agent_jwt_secret),
            agent_jwt_algorithm: env_or("AGENT_JWT_ALGORITHM", &d.agent_jwt_algorithm),
            default_provider: env_or("DEFAULT_PROVIDER", &d.default_provider),
            default_model: env_or("DEFAULT_MODEL", &d.default_model),
            // Per-provider platform keys. When generic_key_fallback is true (default),
            // prefer the explicit `PLATFORM_*` name, then fall back to the generic provider
            // key env var (which agents/orchestrator already set), so a single provider key
            // "just works" without duplication. When false, platform keys come only from the
            // `PLATFORM_*` names, so a deployment can give the platform OPENAI_API_KEY
            // while agents get no platform key. These are the keys an agent's call may
            // fall back to; the platform's own calls use the `service_*` keys below.
            platform_openai_api_key: env_platform_key(
                "PLATFORM_OPENAI_API_KEY",
                "OPENAI_API_KEY",
                generic_key_fallback,
                &d.platform_openai_api_key,
            ),
            platform_anthropic_api_key: env_platform_key(
                "PLATFORM_ANTHROPIC_API_KEY",
                "ANTHROPIC_API_KEY",
                generic_key_fallback,
                &d.platform_anthropic_api_key,
            ),
            platform_gemini_api_key: env_platform_key(
                "PLATFORM_GEMINI_API_KEY",
                "GEMINI_API_KEY",
                generic_key_fallback,
                &d.platform_gemini_api_key,
            ),
            platform_openrouter_api_key: env_platform_key(
                "PLATFORM_OPENROUTER_API_KEY",
                "OPENROUTER_API_KEY",
                generic_key_fallback,
                &d.platform_openrouter_api_key,
            ),
            generic_key_fallback,
            // The platform's own keys always take the generic name too: turning agents' platform
            // key off must not take the platform's own model calls down with it.
            service_openai_api_key: env_platform_key(
                "PLATFORM_OPENAI_API_KEY",
                "OPENAI_API_KEY",
                true,
                &d.service_openai_api_key,
            ),
            service_anthropic_api_key: env_platform_key(
                "PLATFORM_ANTHROPIC_API_KEY",
                "ANTHROPIC_API_KEY",
                true,
                &d.service_anthropic_api_key,
            ),
            service_gemini_api_key: env_platform_key(
                "PLATFORM_GEMINI_API_KEY",
                "GEMINI_API_KEY",
                true,
                &d.service_gemini_api_key,
            ),
            service_openrouter_api_key: env_platform_key(
                "PLATFORM_OPENROUTER_API_KEY",
                "OPENROUTER_API_KEY",
                true,
                &d.service_openrouter_api_key,
            ),
            llm_config_cache_ttl_secs: Self::llm_config_cache_ttl_secs_from_env(),
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
            // Zero would fail every provider call on its first read.
            provider_timeout_secs: parse_or_warn(
                "LLM_PROVIDER_TIMEOUT_SECS",
                |raw| match raw.trim().parse::<u64>() {
                    Ok(0) => Err("must be at least 1".to_string()),
                    Ok(secs) => Ok(secs),
                    Err(e) => Err(e.to_string()),
                },
                provider_timeout_below_agent(d.provider_timeout_secs),
            ),
            max_body_bytes: env_usize("LLM_ROUTER_MAX_BODY_BYTES", d.max_body_bytes),
            max_inflight_body_bytes: env_usize(
                "LLM_ROUTER_MAX_INFLIGHT_BODY_BYTES",
                d.max_inflight_body_bytes,
            ),
            attachment_cache_bytes: env_usize(
                "LLM_ROUTER_ATTACHMENT_CACHE_BYTES",
                d.attachment_cache_bytes,
            ),
            attachment_max_bytes: env_usize(
                "LLM_ROUTER_ATTACHMENT_MAX_BYTES",
                d.attachment_max_bytes,
            ),
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
            openai_api_base: normalize_openai_base(&env_or_first(
                &["OPENAI_BASE_URL", "OPENAI_API_BASE"],
                &d.openai_api_base,
            )),
            openai_max_tools: env_parse_first(&["OPENAI_MAX_TOOLS"], d.openai_max_tools),
            anthropic_api_base: env_or_first(
                &["ANTHROPIC_BASE_URL", "ANTHROPIC_API_BASE"],
                &d.anthropic_api_base,
            ),
            gemini_api_base: env_or_first(
                &["GOOGLE_GEMINI_BASE_URL", "GEMINI_API_BASE"],
                &d.gemini_api_base,
            ),
            vertex_project: env_or("GOOGLE_CLOUD_PROJECT", &d.vertex_project),
            vertex_location: env_or("GOOGLE_CLOUD_LOCATION", &d.vertex_location),
            openrouter_api_base: env_or_first(
                &["OPENROUTER_BASE_URL", "OPENROUTER_API_BASE"],
                &d.openrouter_api_base,
            ),
            openrouter_http_referer: env_or("OPENROUTER_HTTP_REFERER", &d.openrouter_http_referer),
            openrouter_x_title: env_or("OPENROUTER_X_TITLE", &d.openrouter_x_title),
            llm_gateway_base_url: env_or("LLM_GATEWAY_BASE_URL", &d.llm_gateway_base_url),
            salience_gate_enabled: std::env::var("SALIENCE_GATE_ENABLED")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d.salience_gate_enabled),
            classifier_backend: env_or("CLASSIFIER_BACKEND", &d.classifier_backend),
            classifier_model_path: env_or("CLASSIFIER_MODEL_PATH", &d.classifier_model_path),
            classifier_endpoint: env_or("CLASSIFIER_ENDPOINT", &d.classifier_endpoint),
            classifier_model: env_or("CLASSIFIER_MODEL", &d.classifier_model),
            classifier_api_key: env_or("CLASSIFIER_API_KEY", &d.classifier_api_key),
            classifier_timeout_ms: env_parse_first(
                &["CLASSIFIER_TIMEOUT_MS"],
                d.classifier_timeout_ms,
            ),
            classifier_min_confidence: env_parse_first(
                &["CLASSIFIER_MIN_CONFIDENCE"],
                d.classifier_min_confidence,
            ),
            router_tier_seed: std::env::var("ROUTER_TIER_SEED")
                .ok()
                .and_then(|v| v.parse().ok()),
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
            brevity_enabled: env_flag("TOKEN_BREVITY", d.brevity_enabled),
            brevity_min_bytes: env_usize("TOKEN_BREVITY_MIN_BYTES", d.brevity_min_bytes),
            brevity_holdout_pct: env_usize(
                "TOKEN_BREVITY_HOLDOUT_PCT",
                d.brevity_holdout_pct as usize,
            )
            .min(100) as u8,
            compact_tools_enabled: env_flag("TOKEN_COMPACT_TOOLS", d.compact_tools_enabled),
            cache_markers_enabled: env_flag("TOKEN_CACHE_MARKERS", d.cache_markers_enabled),
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
        };
        // No explicit DEFAULT_PROVIDER: pick the first provider that has credentials, so a
        // Vertex-only (ADC) deployment never silently routes to an unkeyed `openai`.
        if std::env::var("DEFAULT_PROVIDER").map_or(true, |v| v.trim().is_empty())
            && let Some((provider, _)) = c.detect_default_provider()
        {
            c.default_provider = provider.to_string();
        }
        // DEFAULT_MODEL unset: use the chosen provider's built-in model, whether the
        // provider was explicit or detected, so `vertex` never receives an OpenAI model.
        let explicit_model: bool =
            std::env::var("DEFAULT_MODEL").is_ok_and(|v| !v.trim().is_empty());
        if !explicit_model && let Some(model) = builtin_default_model(&c.default_provider) {
            c.default_model = model.to_string();
        }
        c
    }

    /// First built-in provider with platform credentials, plus a sensible default model
    /// for it. Order: openai, anthropic, gemini, vertex, openrouter. `None` when nothing
    /// is configured (keeps the static default; requests then fail with `NoApiKey`).
    /// Reads the platform's own keys: the default is the platform's choice, also where
    /// agents' platform key is off.
    pub fn detect_default_provider(&self) -> Option<(&'static str, &'static str)> {
        BUILTIN_DEFAULT_MODELS
            .into_iter()
            .find(|(p, _)| !self.service_key_for(p).is_empty())
    }

    /// Vertex AI base URL up to `/publishers/google`; the Gemini spoke appends
    /// `/models/{model}:{method}`. `global` uses the region-less host.
    pub fn vertex_api_base(&self) -> String {
        format!("{}/publishers/google", self.vertex_location_base())
    }

    /// Vertex AI OpenAI-compatible base (MaaS models: DeepSeek, gpt-oss, Llama, Qwen,
    /// and `google/` Gemini). The OpenAI spoke appends `/chat/completions`.
    pub fn vertex_openapi_base(&self) -> String {
        format!("{}/endpoints/openapi", self.vertex_location_base())
    }

    /// `https://{host}/v1/projects/{p}/locations/{l}`; `global` uses the region-less host.
    fn vertex_location_base(&self) -> String {
        let (p, l) = (&self.vertex_project, &self.vertex_location);
        let host: String = if l == "global" {
            "aiplatform.googleapis.com".to_string()
        } else {
            format!("{l}-aiplatform.googleapis.com")
        };
        format!("https://{host}/v1/projects/{p}/locations/{l}")
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
            // Vertex authenticates with ADC, not a key; a non-empty marker means "configured".
            "vertex" if !self.vertex_project.is_empty() => "adc",
            "openrouter" => &self.platform_openrouter_api_key,
            _ => "",
        }
    }

    /// The platform's own key for a built-in `provider`: what the platform's own model calls
    /// ([`crate::platform::PlatformLlm`]) and the agent calls it sponsors run on. The key an
    /// agent's call may fall back to ([`Self::platform_key_for`]) when one is configured, else
    /// the generic provider key `generic_key_fallback=false` keeps from agents. Unknown
    /// providers return `""`, as there.
    pub fn service_key_for(&self, provider: &str) -> &str {
        let agent_key: &str = self.platform_key_for(provider);
        if !agent_key.is_empty() {
            return agent_key;
        }
        match provider {
            "openai" => &self.service_openai_api_key,
            "anthropic" => &self.service_anthropic_api_key,
            "gemini" => &self.service_gemini_api_key,
            "openrouter" => &self.service_openrouter_api_key,
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

/// How far below the calling agent's timeout the provider timeout sits, so the router's 504
/// reaches the agent before the agent gives up on its own.
const PROVIDER_TIMEOUT_MARGIN_SECS: u64 = 10;

/// The shortest provider timeout derived from the agent's: below it, a healthy but slow
/// model would fail on every non-streaming call.
const MIN_PROVIDER_TIMEOUT_SECS: u64 = 30;

/// The provider timeout when `LLM_PROVIDER_TIMEOUT_SECS` is unset: the calling agent's
/// timeout (`AGENT_CALL_TIMEOUT_SECS`, or the former `ROUTER_AGENT_TIMEOUT_SECS`, read as
/// the server reads it) less [`PROVIDER_TIMEOUT_MARGIN_SECS`], never under
/// [`MIN_PROVIDER_TIMEOUT_SECS`]. `fallback` when neither is set — the same way the
/// attribution window falls back to `NASIKO_FLOW_TIMEOUT_SECS`.
fn provider_timeout_below_agent(fallback: u64) -> u64 {
    std::env::var("AGENT_CALL_TIMEOUT_SECS")
        .or_else(|_| std::env::var("ROUTER_AGENT_TIMEOUT_SECS"))
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map_or(fallback, |agent| {
            agent
                .saturating_sub(PROVIDER_TIMEOUT_MARGIN_SECS)
                .max(MIN_PROVIDER_TIMEOUT_SECS)
        })
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

/// Built-in providers in detection order, each with its default model.
const BUILTIN_DEFAULT_MODELS: [(&str, &str); 5] = [
    ("openai", "gpt-6.1-sol"),
    ("anthropic", "claude-haiku-5-5"),
    ("gemini", "gemini-4-hydrogen"),
    ("vertex", "gemini-4-hydrogen"),
    ("openrouter", "openai/gpt-6.1-sol"),
];

/// Default model for a built-in provider; `None` for custom providers.
pub fn builtin_default_model(provider: &str) -> Option<&'static str> {
    BUILTIN_DEFAULT_MODELS
        .into_iter()
        .find(|(p, _)| *p == provider)
        .map(|(_, m)| m)
}

/// First non-empty env var among `keys` (new name first, legacy aliases after).
fn env_or_first(keys: &[&str], default: &str) -> String {
    keys.iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// OpenAI-compatible base as the router's spoke expects it (ends at the version
/// segment). A bare origin like `https://api.openai.com` gets `/v1`, matching the
/// direct `LLMProvider`, which appends `/v1/chat/completions` to a root.
pub fn normalize_openai_base(raw: &str) -> String {
    // Query/fragment never belong on a base the spoke appends paths to.
    let no_query: &str = raw.trim().split(['?', '#']).next().unwrap_or("");
    let trimmed: &str = no_query.trim_end_matches('/');
    let after_scheme: &str = trimmed.split_once("://").map_or(trimmed, |(_, rest)| rest);
    if after_scheme.contains('/') {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1")
    }
}

/// Select a platform key from explicit platform and generic keys. Empty strings
/// count as unset. When `fallback` is true, prefer platform then fall back to
/// generic; when false, use only the platform key.
fn pick_platform_key(
    platform: Option<String>,
    generic: Option<String>,
    fallback: bool,
    default: &str,
) -> String {
    let platform = platform.filter(|s| !s.is_empty());
    let generic = generic.filter(|s| !s.is_empty());

    if fallback {
        platform.or(generic).unwrap_or_else(|| default.to_string())
    } else {
        platform.unwrap_or_else(|| default.to_string())
    }
}

/// Read a platform key from env: prefer the explicit `platform_name`, then
/// conditionally fall back to `generic_name` when `fallback` is true.
fn env_platform_key(
    platform_name: &str,
    generic_name: &str,
    fallback: bool,
    default: &str,
) -> String {
    let platform = std::env::var(platform_name).ok();
    let generic = std::env::var(generic_name).ok();
    pick_platform_key(platform, generic, fallback, default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_openai_base_adds_v1_to_bare_origin() {
        assert_eq!(
            normalize_openai_base("https://api.openai.com"),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            normalize_openai_base("https://api.openai.com/"),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            normalize_openai_base("https://api.openai.com/v1/"),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            normalize_openai_base("http://127.0.0.1:1234/x/v1"),
            "http://127.0.0.1:1234/x/v1"
        );
        assert_eq!(normalize_openai_base("https://host?x=1"), "https://host/v1");
        assert_eq!(
            normalize_openai_base("https://host/v1/#frag"),
            "https://host/v1"
        );
    }

    #[test]
    fn builtin_default_model_per_provider() {
        assert_eq!(builtin_default_model("vertex"), Some("gemini-4-hydrogen"));
        assert_eq!(builtin_default_model("anthropic"), Some("claude-haiku-5-5"));
        assert_eq!(builtin_default_model("custom-x"), None);
    }

    #[test]
    fn vertex_api_base_global_uses_regionless_host() {
        let cfg = GatewayConfig {
            vertex_project: "p1".into(),
            vertex_location: "global".into(),
            ..Default::default()
        };
        assert_eq!(
            cfg.vertex_api_base(),
            "https://aiplatform.googleapis.com/v1/projects/p1/locations/global/publishers/google"
        );
    }

    #[test]
    fn vertex_api_base_regional_uses_prefixed_host() {
        let cfg = GatewayConfig {
            vertex_project: "p1".into(),
            vertex_location: "us-central1".into(),
            ..Default::default()
        };
        assert_eq!(
            cfg.vertex_api_base(),
            "https://us-central1-aiplatform.googleapis.com/v1/projects/p1/locations/us-central1/publishers/google"
        );
    }

    #[test]
    fn detect_default_provider_picks_vertex_when_only_adc_configured() {
        let mut cfg = GatewayConfig::default();
        assert_eq!(cfg.detect_default_provider(), None);
        cfg.vertex_project = "p1".into();
        assert_eq!(
            cfg.detect_default_provider(),
            Some(("vertex", "gemini-4-hydrogen"))
        );
        cfg.platform_openai_api_key = "sk".into();
        assert_eq!(
            cfg.detect_default_provider(),
            Some(("openai", "gpt-6.1-sol"))
        );
    }

    #[test]
    fn platform_key_for_vertex_needs_project() {
        let mut cfg = GatewayConfig::default();
        assert_eq!(cfg.platform_key_for("vertex"), "");
        cfg.vertex_project = "p1".into();
        assert_eq!(cfg.platform_key_for("vertex"), "adc");
    }

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
    fn service_key_for_takes_the_generic_key_agents_are_refused() {
        // generic_key_fallback=false: agents get no key, the platform still has its own.
        let cfg = GatewayConfig {
            generic_key_fallback: false,
            service_gemini_api_key: "sk-gem-generic".into(),
            ..Default::default()
        };
        assert_eq!(cfg.platform_key_for("gemini"), "");
        assert_eq!(cfg.service_key_for("gemini"), "sk-gem-generic");
    }

    #[test]
    fn service_key_for_prefers_the_agent_key() {
        let cfg = GatewayConfig {
            platform_openai_api_key: "sk-platform".into(),
            service_openai_api_key: "sk-generic".into(),
            ..Default::default()
        };
        assert_eq!(cfg.service_key_for("openai"), "sk-platform");
    }

    #[test]
    fn service_key_for_unknown_and_vertex() {
        let mut cfg = GatewayConfig {
            service_openai_api_key: "sk-generic".into(),
            ..Default::default()
        };
        assert_eq!(cfg.service_key_for("my-gateway"), "");
        assert_eq!(cfg.service_key_for("vertex"), "");
        cfg.vertex_project = "p1".into();
        assert_eq!(cfg.service_key_for("vertex"), "adc");
    }

    #[test]
    fn detect_default_provider_reads_the_platforms_own_keys() {
        let cfg = GatewayConfig {
            generic_key_fallback: false,
            service_gemini_api_key: "sk-gem".into(),
            ..Default::default()
        };
        assert_eq!(
            cfg.detect_default_provider(),
            Some(("gemini", "gemini-4-hydrogen"))
        );
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

    #[test]
    fn generic_key_fallback_enabled_by_default() {
        let cfg = GatewayConfig::default();
        assert!(
            cfg.generic_key_fallback,
            "generic_key_fallback should default to true"
        );
    }

    #[test]
    fn pick_platform_key_fallback_true_prefers_platform_then_generic() {
        // Both set: platform wins
        assert_eq!(
            pick_platform_key(Some("sk-p".into()), Some("sk-g".into()), true, "def"),
            "sk-p"
        );
        // Only platform: use it
        assert_eq!(
            pick_platform_key(Some("sk-p".into()), None, true, "def"),
            "sk-p"
        );
        // Only generic: fall back to it
        assert_eq!(
            pick_platform_key(None, Some("sk-g".into()), true, "def"),
            "sk-g"
        );
        // Neither: use default
        assert_eq!(pick_platform_key(None, None, true, "def"), "def");
        // Empty strings count as unset
        assert_eq!(
            pick_platform_key(Some("".into()), Some("sk-g".into()), true, "def"),
            "sk-g"
        );
        assert_eq!(
            pick_platform_key(Some("sk-p".into()), Some("".into()), true, "def"),
            "sk-p"
        );
        assert_eq!(
            pick_platform_key(Some("".into()), Some("".into()), true, "def"),
            "def"
        );
    }

    #[test]
    fn pick_platform_key_fallback_false_ignores_generic() {
        // Both set: platform wins, generic ignored
        assert_eq!(
            pick_platform_key(Some("sk-p".into()), Some("sk-g".into()), false, "def"),
            "sk-p"
        );
        // Only platform: use it
        assert_eq!(
            pick_platform_key(Some("sk-p".into()), None, false, "def"),
            "sk-p"
        );
        // Only generic: default (generic ignored)
        assert_eq!(
            pick_platform_key(None, Some("sk-g".into()), false, "def"),
            "def"
        );
        // Neither: use default
        assert_eq!(pick_platform_key(None, None, false, "def"), "def");
        // Empty platform with set generic: default (generic ignored)
        assert_eq!(
            pick_platform_key(Some("".into()), Some("sk-g".into()), false, "def"),
            "def"
        );
        // Set platform with empty generic: use platform
        assert_eq!(
            pick_platform_key(Some("sk-p".into()), Some("".into()), false, "def"),
            "sk-p"
        );
    }

    #[test]
    #[serial_test::serial]
    fn provider_timeout_defaults_to_300_and_rejects_zero() {
        const KEY: &str = "LLM_PROVIDER_TIMEOUT_SECS";
        unsafe { std::env::remove_var(KEY) };
        unsafe { std::env::remove_var("AGENT_CALL_TIMEOUT_SECS") };
        unsafe { std::env::remove_var("ROUTER_AGENT_TIMEOUT_SECS") };
        assert_eq!(GatewayConfig::from_env().provider_timeout_secs, 300);
        unsafe { std::env::set_var(KEY, "45") };
        assert_eq!(GatewayConfig::from_env().provider_timeout_secs, 45);
        unsafe { std::env::set_var(KEY, "0") };
        assert_eq!(GatewayConfig::from_env().provider_timeout_secs, 300);
        unsafe { std::env::remove_var(KEY) };
    }

    #[test]
    #[serial_test::serial]
    fn provider_timeout_unset_sits_below_the_agent_timeout() {
        const AGENT: &str = "AGENT_CALL_TIMEOUT_SECS";
        unsafe { std::env::remove_var("LLM_PROVIDER_TIMEOUT_SECS") };
        unsafe { std::env::set_var(AGENT, "180") };
        let derived = GatewayConfig::from_env().provider_timeout_secs;
        assert!(derived < 180, "{derived}");
        assert_eq!(derived, 170);
        // A very short agent timeout still leaves a slow model a usable floor.
        unsafe { std::env::set_var(AGENT, "20") };
        assert_eq!(GatewayConfig::from_env().provider_timeout_secs, 30);
        // An explicit setting wins over the derived one.
        unsafe { std::env::set_var("LLM_PROVIDER_TIMEOUT_SECS", "90") };
        assert_eq!(GatewayConfig::from_env().provider_timeout_secs, 90);
        unsafe { std::env::remove_var("LLM_PROVIDER_TIMEOUT_SECS") };
        unsafe { std::env::remove_var(AGENT) };
    }
}
