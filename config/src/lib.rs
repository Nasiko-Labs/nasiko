use std::collections::BTreeMap;

use nasiko_utils::{env_bool, env_or, env_parse, require_all, required_env};

mod coding_agent_ingest;
mod seed_agents;
pub use coding_agent_ingest::CodingAgentIngestConfig;
pub use seed_agents::{SeedAgentSettings, parse_seed_agent_settings};

/// Model for a platform task call (selector, titles, capability generation, MCP
/// descriptions): its own env var, else `DEFAULT_MODEL`, else empty. Empty lets the
/// LLM router pick its resolved provider's default model, so no deployment ever
/// sends a hard-coded model to a provider that cannot serve it.
fn task_model(var: &str) -> String {
    pick_task_model(std::env::var(var).ok(), std::env::var("DEFAULT_MODEL").ok())
}

/// Pure core of [`task_model`]; blank values count as unset.
fn pick_task_model(own: Option<String>, default_model: Option<String>) -> String {
    [own, default_model]
        .into_iter()
        .flatten()
        .map(|m: String| m.trim().to_string())
        .find(|m: &String| !m.is_empty())
        .unwrap_or_default()
}

#[cfg(test)]
mod task_model_tests {
    use super::pick_task_model;

    #[test]
    fn own_var_wins() {
        let m: String = pick_task_model(Some("a".into()), Some("b".into()));
        assert_eq!(m, "a");
    }

    #[test]
    fn falls_back_to_default_model() {
        let m: String = pick_task_model(Some("  ".into()), Some("gemini-3.8-flash".into()));
        assert_eq!(m, "gemini-3.8-flash");
    }

    #[test]
    fn empty_when_nothing_set_so_the_router_picks() {
        assert_eq!(pick_task_model(None, None), "");
    }
}

/// The audience a control plane falls back to when nothing identifies its
/// public origin. Fine for a standalone deployment; never unique across a
/// fleet, which is why the assertion exchange refuses it.
pub const DEFAULT_DELEGATION_AUDIENCE: &str = "nasiko-cp";

fn nonempty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// `scheme://host[:port]` of a URL, or `None` when it has no scheme.
fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?.trim();
    if host.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{host}"))
}

/// See [`Config::delegation_audience`].
fn delegation_audience_from_env() -> String {
    if let Some(explicit) = nonempty_env("NASIKO_CP_AUDIENCE") {
        return explicit.trim_end_matches('/').to_string();
    }
    if let Some(domain) = nonempty_env("CP_DOMAIN") {
        return origin_of(&domain).unwrap_or_else(|| format!("https://{domain}"));
    }
    if let Some(origin) = nonempty_env("APP_BASE_URL").and_then(|u| origin_of(&u)) {
        return origin;
    }
    DEFAULT_DELEGATION_AUDIENCE.to_string()
}

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: String,
    pub domain: Option<String>,
    /// Audience string this control plane stamps into, and requires on,
    /// delegated tokens (ui/docs/adr/0001a). It is this deployment's public
    /// origin: `NASIKO_CP_AUDIENCE`, else `https://<CP_DOMAIN>`, else the origin
    /// of `APP_BASE_URL`, else [`DEFAULT_DELEGATION_AUDIENCE`]. Two workspaces
    /// must never share one, so the cross-workspace (BFF assertion) path refuses
    /// to run on the shared default — see `oss/server/src/auth/assertion.rs`.
    pub delegation_audience: String,
    /// Kill switch for `POST /api/auth/delegate` (default on). Existing
    /// delegated tokens die within `DELEGATION_MAX_TTL_SECS` anyway.
    pub delegation_enabled: bool,
    pub database_url: String,
    pub redis_url: String,
    pub agent_runtime: String,
    pub k8s_namespace: String,
    pub kubeconfig: Option<String>,
    /// Which object-storage protocol backs artifacts: `s3` (default; any
    /// S3-compatible store) or `azure-blob`. Carried here as an opaque string
    /// and interpreted by the composition root that selects the backend — the
    /// same arrangement as `agent_runtime`, whose `kubernetes` value only one
    /// edition can serve. Held on `Config` so the value is visible to every
    /// consumer without a second env read.
    pub storage_provider: String,
    pub s3_endpoint: String,
    pub s3_bucket: String,
    pub s3_access_key: String,
    pub s3_secret_key: String,
    pub s3_region: String,
    /// Where `POST /api/assessments` forwards the free-assessment form: the Nasiko
    /// intake that stores it. Unset → the route answers 503.
    pub assessment_intake_url: Option<String>,
    pub secrets_encryption_key: String,
    pub oci_storage_bucket: String,
    /// Registry prefix prepended to agent image tags at build time.
    /// e.g. `"host.docker.internal:5001"` for local K8s dev.
    /// Empty string → no prefix (Docker local mode).
    /// TODO: this needs to be removed.
    pub agent_image_registry: String,
    /// An `agent_image_registry` value that means "the registry address is
    /// not known yet" — set by installers that discover the address after
    /// the server first starts (the server is restarted once it is real).
    /// Work that bakes the address into deployed agents, such as seeding,
    /// waits while the two are equal.
    pub agent_image_registry_placeholder: Option<String>,
    /// Other hosts this control plane's registry answers on
    /// (`AGENT_IMAGE_REGISTRY_ALIASES`, comma-separated): its names before a
    /// rename. An image naming one of them is this registry's, so it is held to
    /// the namespace rules, and at startup every stored reference to one is
    /// moved to `agent_image_registry` (`agents::rehost`).
    pub agent_image_registry_aliases: Vec<String>,
    /// Username for authenticating agent-image pulls from a private registry
    /// (e.g. a private Docker Hub repo). `None` (default) means anonymous
    /// pulls only — unchanged behavior for public images. Only meaningful
    /// together with `agent_registry_password`; `DockerRuntime` treats a
    /// pair where only one is set as "not configured."
    pub agent_registry_username: Option<String>,
    /// Password or access token paired with `agent_registry_username`. Never
    /// logged, never returned in any API response.
    pub agent_registry_password: Option<String>,
    /// Bare host of the registry the credential above belongs to (e.g.
    /// `ghcr.io`, `us-central1-docker.pkg.dev`). `None` means Docker Hub. The
    /// credential is only ever sent to this host.
    pub agent_registry_host: Option<String>,
    /// `os/arch` copied when a seed image is mirrored from a multi-platform
    /// index into the built-in registry — the platform the agent nodes run.
    /// Default `linux/amd64`.
    pub seed_image_platform: String,
    /// Shared credential the in-cluster BuildKit build Job presents (HTTP
    /// Basic auth, username `"build-service"`) to push freshly-built agent
    /// images into the built-in OCI registry — see
    /// `nasiko_oci::authz::Writer::BuildService`. Empty means not configured
    /// (fine for `AGENT_RUNTIME=local`, where no such build path exists).
    pub build_push_token: String,
    /// Shared HMAC secret for the agent-identity / coding-agent session JWTs
    /// (the same `AGENT_JWT_SECRET` the LLM router verifies with). Empty means
    /// not configured; mint/verify fail closed in that case.
    pub agent_jwt_secret: String,
    /// Algorithm for the above (`HS256`/`HS384`/`HS512`); defaults to `HS256`.
    pub agent_jwt_algorithm: String,
    /// Shared credential the EE injection proxy's internal route requires on every request —
    /// mirrors `build_push_token`'s shape exactly (one cluster-wide secret, not a per-caller
    /// identity): the route's only caller is the control plane's own outbound forward to an
    /// agent, so there is no untrusted external identity to distinguish, just "is this genuinely
    /// the control plane." Empty means not configured (fine on OSS and on EE with
    /// `MEMORY_ENABLED=false`, where the route is never mounted at all).
    pub injection_proxy_token: String,
    pub seed_agents: Option<String>,
    /// `SEED_AGENT_SETTINGS`: replica bounds and CPU/memory per seeded agent (`SEED_AGENTS`), by
    /// agent name. Empty when unset; an invalid value fails startup.
    pub seed_agent_settings: BTreeMap<String, SeedAgentSettings>,
    /// `SEED_WORKFLOWS`: space-separated workflow template ids installed at boot,
    /// after `SEED_AGENTS`, as public workflows every user can run
    /// (`oss/server/src/seed_workflows.rs`). Empty when unset: nothing installed.
    pub seed_workflows: String,
    /// `NASIKO_TIERING_ENABLED` (default false): run the daily job that moves
    /// event rows older than 30 days from Postgres to Lance and deletes them
    /// from Postgres (docs/LAKEHOUSE_TRD.md §8). Off = no tiering at all.
    pub tiering_enabled: bool,
    /// `NASIKO_TIERING_INTERVAL_SECS` (default 24h = 86400s).
    pub tiering_interval_secs: u64,
    /// `NASIKO_COLD_PATH`: root of the Lance datasets (dir, `file://`, `s3://` or `gs://`).
    /// Must be set explicitly when tiering is on (see [`validate_tiering`]).
    pub cold_path: String,
    pub openai_api_key: Option<String>,
    pub openai_base_url: Option<String>,
    pub openai_model: String,
    /// Whether agents get the platform's OpenAI API key as a fallback when they have no key of
    /// their own. Default true for OSS backward compatibility. Set false to require agents to
    /// bring their own keys.
    pub agent_platform_key_fallback: bool,
    /// MAF "decompose one instruction into atomic sub-queries" service.
    /// `None` disables `/maf/workflow/from-instruction` (503).
    pub decomposer_api_url: Option<String>,
    pub decomposer_api_key: Option<String>,
    pub router_model: String,
    pub capability_generator_model: String,
    /// Model for the MCP-connector description LLM fallback — only called when
    /// a connector/tool description couldn't be fetched from its native source.
    pub mcp_description_model: String,
    pub a2a_discovery_url: Option<String>,
    pub otel_endpoint: Option<String>,
    pub otel_protocol: String,
    pub otel_headers: Option<String>,
    pub otel_service_name: String,
    pub otel_sample_ratio: String,
    pub otel_collector_endpoint: String,
    pub otel_capture_content: bool,
    /// OTLP/HTTP JSON base endpoint used only by the durable coding-agent
    /// telemetry outbox. Unset leaves receipts pending and disables its worker.
    pub coding_agent_otlp_endpoint: Option<String>,
    /// Rate, concurrency and Tempo-age limits on coding-agent telemetry ingest.
    pub coding_agent_ingest: CodingAgentIngestConfig,
    pub tempo_url: String,
    pub loki_url: String,
    /// Whether the Tempo/Loki observability backend is enabled — the SINGLE
    /// source of truth for "is observability configured". True iff both
    /// `TEMPO_URL` and `LOKI_URL` were explicitly set in the environment
    /// (the URLs above always carry a default, so their presence alone can't
    /// answer this). Everything that gates on observability reads this flag
    /// rather than re-inspecting env, so no two code paths can disagree.
    pub observability_enabled: bool,
    /// Opaque identifier for the tenant this deployment belongs to, added as
    /// a `tenant.id` OTel resource attribute on every agent this instance
    /// deploys — see `InstrumentedRuntime`. `None` for a standalone/non-
    /// multi-tenant deployment. This crate has no notion of what a "tenant"
    /// is; it only passes the value through.
    pub tenant_id: Option<String>,
    /// When true, a background loop refreshes `provider_models` from each
    /// configured provider's `GET /models`. Reaches the network at boot, so
    /// tests and benches turn it off; a disabled sync just means tier routing
    /// falls back to whatever catalog rows already exist.
    pub model_catalog_sync_enabled: bool,
    /// When true, a background loop mirrors the Portkey price book into
    /// `model_pricing`. Reaches the network at boot, so tests and benches turn
    /// it off; a disabled sync leaves the boot seed rows as the only pricing.
    /// Also the switch for air-gapped installs that must not call out.
    pub model_pricing_sync_enabled: bool,
    pub flow_max_depth: i32,
    pub flow_max_fan_out: i32,
    pub flow_max_tokens: i64,
    /// Wall-clock budget for one flow, from `NASIKO_FLOW_TIMEOUT_SECS`. The
    /// platform's widest window: the flow guard enforces it, and both the MCP
    /// gateway (`tools/call`) and the LLM router (token attribution) refuse to
    /// serve a flow older than this, so nothing an agent turn depends on may
    /// outlive it.
    pub flow_timeout_secs: i32,
    /// How long a HITL pause (`hitl_requests`) stays answerable before the dispatcher's poll
    /// loop expires it. `oss/hitl`'s own store applies this at row-creation time — see
    /// `PgHitlStore::with_ttl_days`.
    pub hitl_request_ttl_days: i64,
    /// `nasiko_hitl::dispatcher::DispatcherConfig`'s remaining tunables (the `mcp_tool`-origin
    /// resume dispatcher, `oss/hitl/src/dispatcher.rs`) — every comparable tunable elsewhere in
    /// this codebase goes through this single `Config` struct, and `hitl_request_ttl_days` right
    /// above is the same feature's own TTL knob, so these were the odd ones out as compile-time
    /// constants (found in review).
    pub hitl_resume_poll_interval_secs: u64,
    pub hitl_resume_recovery_interval_secs: u64,
    /// Claim-lease floor shared by *both* resume dispatchers — `nasiko_hitl::dispatcher`'s
    /// `mcp_tool` one (via `DispatcherConfig::effective_lease_minutes`, which holds one claim
    /// across its own in-process retry loop) and `oss/server/src/hitl/mod.rs`'s `direct_chat`/
    /// `agent_proxy`/`orchestrator`/`maf` one (via its `lease_secs` helper, which claims once per
    /// delivery attempt). Each dispatcher floors its own effective lease at what its own delivery
    /// shape needs, so raising or lowering this one knob can never reopen either's
    /// double-delivery window.
    pub hitl_resume_lease_minutes: i64,
    pub hitl_resume_max_attempts: u32,
    pub hitl_resume_retry_delay_secs: u64,
    /// Coding-agent (ACP) session lifecycle tunables.
    ///
    /// `coding_agent_idle_ttl_secs` is the idle threshold, measured from a session's
    /// `last_turn_at`, after which the session reaper *considers* suspending it (the
    /// reaper still requires the turn to have ended, the sandbox to be CPU-quiescent
    /// for `session_idle_grace_secs`, and no outbound LLM/MCP call in flight).
    /// **Default `900`** (15 minutes): long enough that a user pausing to read or
    /// think resumes into a warm sandbox, short enough that an abandoned session
    /// does not hold a sandbox for hours. **`0` disables suspension entirely**, so a
    /// sandbox lives until its session is closed. A non-zero value must be at least
    /// `session_idle_grace_secs`: the grace window is measured *inside* the idle
    /// TTL (a session already idle for the whole TTL and found quiet is suspended at
    /// once; a busy sample restarts the window), so a smaller TTL could never be
    /// satisfied (see [`Config::validate_session_lifecycle`]).
    pub coding_agent_idle_ttl_secs: i64,
    /// Where coding-agent sandboxes run, from `SANDBOX_PROVIDER`: `docker`
    /// (default), `pod`, `shared-pod` or `harness-agent`. Docker is the OSS default and
    /// needs no cluster; the others are implemented by enterprise crates and
    /// selected at cluster create. An unknown value fails startup rather than
    /// silently falling back, because running sessions on the wrong provider
    /// changes isolation and cost.
    pub sandbox_provider: SandboxProviderSetting,
    /// Whether users can open coding sessions on this server, from `HARNESS_SESSIONS_ENABLED`
    /// (**default `false`**). Off, startup creates no harness agents and hides (soft-deletes)
    /// the ones an earlier start created, so no agent list, chat picker or registry offers them;
    /// `GET /api/harnesses` is empty and `POST /api/sessions` is refused. On, startup restores
    /// the hidden rows (same ids, so earlier sessions still resolve) and creates any missing.
    /// Read once at startup: changing it takes a restart.
    pub harness_sessions_enabled: bool,
    /// Whether a successful agent build is followed by one smoke run of the image under gVisor,
    /// from `BUILD_SANDBOX_SMOKE`: `off` (default) or `runsc`. The outcome sets
    /// `agents.sandbox_compatible`; it never fails the build. Docker runtime only.
    pub build_sandbox_smoke: BuildSandboxSmoke,
    /// How long a session must stay CPU-quiescent after its last turn before the
    /// reaper suspends it, from `SESSION_IDLE_GRACE_SECS`, default `120`. It
    /// guards against suspending a sandbox that is still finishing background work
    /// (a build, a test run) the moment a turn ends. Lives inside
    /// `coding_agent_idle_ttl_secs`, which must be at least this when non-zero.
    pub session_idle_grace_secs: u64,
    /// CPU usage, in millicores, at or below which a sandbox counts as idle, from
    /// `SESSION_CPU_QUIESCENT_MILLICORES`, default `20` (2% of one core): above
    /// an idle shell and agent runtime, below any real work.
    pub session_cpu_quiescent_millicores: u32,
    /// How often the session reaper scans for idle sessions, from
    /// `SESSION_REAPER_INTERVAL_SECS`, default `30`. Clamped to 5..=600: below 5s
    /// the scan is churn, above 10 minutes it adds more latency to suspension than
    /// the idle TTL itself.
    pub session_reaper_interval_secs: u64,
    /// How long the reaper keeps an open-task row (a non-terminal A2A task an agent returned)
    /// before pruning it, from `AGENT_OPEN_TASK_MAX_AGE_SECS`, default `3600`, clamped to
    /// 60..=86400. A row is never pruned younger than two hours, the longest an open pause holds
    /// a workload park, so a lower value prunes at two hours. It does not change how long an open
    /// task keeps its agent from being parked: that is the two hours alone.
    pub agent_open_task_max_age_secs: u64,
    /// How long a running framework agent whose parking policy is `auto` may go without a request
    /// before the reaper scales its workload to zero (parks it), from `AGENT_PARK_IDLE_SECS`, the
    /// one switch for agent parking: `0` turns it off (agents already parked still wake on their
    /// next request). Unset is `3600`, raised to the floor below; any other set value must be at
    /// least `AGENT_CALL_TIMEOUT_SECS` plus [`LAST_REQUEST_STAMP_THROTTLE_SECS`] plus one
    /// `SESSION_REAPER_INTERVAL_SECS` or startup fails: a call on another replica is visible to the
    /// reaper only through `agents.last_request_at`, which may be up to the throttle older than the
    /// call, so a shorter window could park an agent under a call still inside its timeout.
    pub agent_park_idle_secs: u64,
    /// Registry prefix the platform-built harness images are published under, from
    /// `HARNESS_IMAGE_PREFIX`, default `nasiko/harness-`. The catalog's image
    /// names are `<prefix><harness>:<tag>`; override it to pull from a mirror.
    pub harness_image_prefix: String,
    /// How long one turn's lock on a session is held before it lapses. A turn
    /// that runs longer must renew it (heartbeat); this only bounds how long a
    /// *dead* turn-owner can block the next turn.
    pub coding_agent_turn_lease_secs: i64,
    /// How often a running turn renews its lease. Must be well under
    /// `coding_agent_turn_lease_secs` so a live turn never lets its own lock
    /// lapse mid-turn.
    pub coding_agent_lease_heartbeat_secs: u64,
    /// How many times the `direct_chat`/`agent_proxy`/`orchestrator` resume dispatcher re-sends a
    /// resume the agent answered with "task not found" before it concludes the agent lost the
    /// paused task (`agent_lost_task`). A round-robin Service can send the first attempt to a
    /// replica that never saw the task, so one such answer proves nothing. Total POSTs = this + 1.
    pub hitl_lost_task_retries: u32,
    /// How long an agent's durable state row (task or key-value entry) lives after its last
    /// write, in days. AGENT_STATE_TTL_DAYS, default 7, clamped to 1..=365: zero would expire
    /// every row the instant it is written. Reads never extend it.
    pub agent_state_ttl_days: i32,
    /// Largest task `body` or key-value `value` the agent state API accepts, measured on the
    /// bytes of that field as sent. AGENT_STATE_MAX_BODY_BYTES, default 1 MiB, floored at 1.
    /// Larger writes get a 413.
    pub agent_state_max_body_bytes: usize,
    /// Requests per minute one agent may make to the agent state API, for each of its ready
    /// replicas: an agent running three replicas may make three times this. Replicas the server
    /// does not know of yet count as one. AGENT_STATE_RATE_LIMIT_PER_MINUTE, default 1200,
    /// floored at 1. The default leaves headroom over what a durable agent turn costs (a handful
    /// of calls) so a burst of turns is not refused. Over it gets a 429. Counted in memory by
    /// each server process, like the other limiters (`rate_limit.rs`), so the ceiling across
    /// several processes is this value times their number.
    pub agent_state_rate_limit_per_minute: u32,
    /// Requests per minute one agent may make to its peers through `/api/agents/{id}/*`, counted
    /// per calling agent. AGENT_PEER_CALL_RATE_LIMIT_PER_MINUTE, default 300, clamped 1-10000.
    /// Over it gets a 429 with `Retry-After`. Counted in memory by each server process.
    pub agent_peer_call_rate_limit_per_minute: u32,
    /// Most body bytes one agent state listing page carries. AGENT_STATE_LIST_BYTES_BUDGET,
    /// default 8 MiB, floored at 1. A page stops before the next row would pass it and hands back
    /// a cursor, so a short page is normal; a single row larger than it still lists, alone.
    pub agent_state_list_bytes_budget: usize,
    pub github_client_id: Option<String>,
    pub github_client_secret: Option<String>,
    /// Multi-tenant mode (per-CP): when on, this control plane runs behind the
    /// multi-tenant BFF — it serves no UI (root 302s to the BFF) and enforces
    /// the corporate-only admission gate below. Default off = ordinary
    /// single-tenant behavior, unchanged.
    pub multi_tenant_mode: bool,
    /// Only consulted when `multi_tenant_mode` is on. Off (the default)
    /// restricts logins to corporate identities (a Google `hd`, or a verified
    /// email whose domain isn't a known personal provider). On also admits
    /// personal emails, which may only ever *join* a workspace, never create
    /// one. No effect outside multi-tenant mode.
    pub allow_personal_emails: bool,
    /// Base URL of the multi-tenant BFF/dashboard. Used only in
    /// `multi_tenant_mode`: this headless control plane serves no UI, so browser
    /// navigations are redirected here. `None` (the default) outside
    /// multi-tenant mode.
    pub nasiko_bff_url: Option<String>,
    pub router_shortlist_threshold: usize,
    pub router_shortlist_size: usize,
    /// How many of the most recent chat messages the PACMS context selector
    /// draws candidates from (a wide pool for the selector to choose a
    /// budget-fitting subset from). See `SessionHistory::fetch_pacms`.
    pub pacms_history_pool_size: usize,
    /// Structurally compress tool results as the ReAct loop stores them
    /// (PRD §9 IP-3). Shrinks what the loop carries, which also defers the
    /// context-compaction cliff. On by default — gated by the agent's own
    /// switch, so this is a fleet kill switch rather than an enabler.
    /// How often the brevity holdout is re-analysed into a measured effect factor.
    pub savings_factor_refresh_secs: u64,
    /// Minimum samples **per arm** before a measured factor replaces the seeded one. Below this
    /// the arm means are noise, and a noisy `fixture` figure is worse than an honest seed.
    pub savings_factor_min_samples: i64,
    /// Trailing window the holdout comparison reads.
    pub savings_factor_window_days: i64,
    /// Percent of otherwise-eligible minimal-code flows deliberately withheld from the ladder, to
    /// give `optimization_effect_factors`'s `minimal_code` row a real control group — same
    /// mechanism as the brevity directive's own holdout, applied at the a2a-dispatch seam instead
    /// of the LLM router (`oss/coding-policy::in_minimal_code_holdout`).
    pub minimal_code_holdout_pct: u8,
    pub react_compress_enabled: bool,
    /// Skip tool results below this size.
    pub react_compress_min_bytes: usize,
    /// Structurally compress each history message before context selection
    /// (PRD §9 IP-4). On by default — gated by the agent's own switch, so this
    /// is a fleet kill switch rather than an enabler.
    pub history_compress_enabled: bool,
    /// Skip history messages below this size. A short turn is mostly prose,
    /// which does not compress, so the attempt is pure cost.
    pub history_compress_min_bytes: usize,
    /// Token budget for a user on the PACMS "low" tier (`users.pacms_budget_level`).
    pub pacms_budget_low: usize,
    /// Token budget for a user on the PACMS "medium" tier — the default tier
    /// for a user who hasn't picked one.
    pub pacms_budget_medium: usize,
    /// Token budget for a user on the PACMS "high" tier.
    pub pacms_budget_high: usize,
    /// How many of the most-recent messages in the pool are force-included
    /// (PACMS `mandatory` set) regardless of relevance/coverage score, so the
    /// immediate conversational thread is never dropped.
    pub pacms_history_mandatory_recent: usize,
    /// Item count for a user on the "low" tier (`users.pacms_budget_level`),
    /// shared by the `topk` strategy's query/answer-pair count
    /// (`SessionHistory::fetch_topk`) and the `lastk` strategy's recency
    /// window (`SessionHistory::fetch`) — same tier the PACMS token budget
    /// above reads, resolved via `PacmsBudgetLevel::k`.
    pub context_k_low: usize,
    /// Item count for a user on the "medium" tier — the default tier for a
    /// user who hasn't picked one.
    pub context_k_medium: usize,
    /// Item count for a user on the "high" tier.
    pub context_k_high: usize,
    /// OpenAI-compatible model used for Stage 1 vector embeddings.
    /// Default: `text-embedding-3-small`. Stage 1 is skipped if `openai_api_key` is unset.
    pub embedding_model: String,
    /// Wall-clock budget for a single agent HTTP hop — the A2A proxy, the
    /// orchestrator's streaming and non-streaming agent calls, and the MAF
    /// executor's. An agent turn can legitimately run for minutes (long tool
    /// calls, multi-step orchestration), so this is deliberately far above the
    /// shared `http_client` default, which stays short for embeddings, registry
    /// probes and OAuth. Read from `AGENT_CALL_TIMEOUT_SECS`, falling back to
    /// the former `ROUTER_AGENT_TIMEOUT_SECS`. It also caps how long a coding turn
    /// waits for a human approval (HITL), and must not exceed the session token's
    /// one-hour lifetime, which would expire mid-wait (checked at startup).
    pub agent_call_timeout_secs: u64,
    /// How long an agent may take to start and become reachable, from
    /// `AGENT_STARTUP_BUDGET_SECS` (default 120, clamped to 10..=1800 with a warning). The
    /// Kubernetes runtime's startup probe restarts a pod that has not opened its port by then,
    /// and a call to a parked agent waits up to this long for the agent's workload to come back
    /// (see [`Config::agent_wake_budget_secs`]).
    pub agent_startup_budget_secs: u64,
    pub github_callback_url: Option<String>,
    /// Central OAuth callback relay URL (multi-tenant deployments): used as the
    /// GitHub `redirect_uri` for both authorize and token exchange instead of
    /// `github_callback_url`, so many clusters can share one GitHub OAuth app
    /// whose single registered callback points at the relay. Includes this
    /// cluster's tenant-id path suffix. Unset (the default, and always for
    /// standalone deployments) means GitHub calls this cluster back directly.
    pub github_central_callback_url: Option<String>,
    /// Base URL to redirect to after a successful OAuth login. In production
    /// this is the same origin as the server. Override via `APP_BASE_URL` in
    /// dev when the server and app run on different ports.
    pub app_base_url: String,
    pub git_clone_allowed_hosts: Vec<String>,
    /// Allowed OCI registry hosts for `POST /api/catalog/import/registry`.
    /// Comma-separated.  Empty = reject all registry imports (safest default for
    /// new deployments).  Example: "ghcr.io,quay.io,registry.nasiko.com"
    pub registry_import_allowed_hosts: Vec<String>,
    /// Browser origins allowed to make cross-origin requests (comma-separated,
    /// e.g. "https://app.example.com,http://localhost:5173"). Empty (the
    /// default) allows none — the UI is served same-origin by this binary's
    /// own static handler in normal deployments, so cross-origin access is
    /// opt-in only for split dev servers or external integrations.
    pub cors_allowed_origins: Vec<String>,
    /// Lets an agent registered by URL (one this platform does not deploy) point
    /// at a loopback, private or link-local address (`AGENT_ALLOW_PRIVATE_URLS`,
    /// default off). The agent proxy forwards to that address on the caller's
    /// behalf, so with this on any user who may register an agent can reach
    /// whatever the server can: cloud metadata, the database, other agents.
    /// Turn it on only where every such user is trusted, e.g. an on-prem
    /// install whose external agents live on internal hosts.
    pub agent_allow_private_urls: bool,
    /// Whether every new message the platform hands an agent carries the current date and time
    /// as its own text part (`AGENT_CONTEXT_CURRENT_TIME`, default on;
    /// `nasiko_types::agent_clock`). Off sends agent messages exactly as before.
    pub agent_current_time: bool,
    // ─── OIDC SSO ───────────────────────────────────────────────────────────
    pub oidc_issuer_url: Option<String>,
    pub oidc_client_id: Option<String>,
    pub oidc_client_secret: Option<String>,
    pub oidc_redirect_uri: Option<String>,
    pub oidc_allowed_redirect_origins: Vec<String>,
    pub oidc_scopes: String,
    pub oidc_provider_label: String,
    pub oidc_central_callback_url: Option<String>,

    pub admin_username: String,
    pub admin_password: String,
    /// Docker network to attach agent containers to.
    /// Set to the compose network name (e.g. `nasiko-cloud-rs_default`) when the
    /// server itself runs inside Docker so agents are reachable via container IP.
    pub docker_agent_network: Option<String>,
    /// OCI registry host to pull agent images from (e.g. `"localhost:8443"`).
    /// When set, the Docker runtime pulls images from this registry before creating containers.
    /// Maps to env var `OCI_REGISTRY_HOST`.
    pub oci_registry_host: Option<String>,
    /// Poll interval in seconds for the container-hours meter. 0 disables metering.
    pub container_hours_poll_secs: u64,

    // ─── Trace Materializer ─────────────────────────────────────────────────
    /// Poll interval in seconds for the trace-usage materializer. 0 disables.
    pub trace_usage_sync_secs: u64,
    /// Overlap window in seconds: the materializer re-queries this far before
    /// the high-water mark to catch late-arriving spans and pricing corrections.
    pub trace_usage_overlap_secs: u64,
    /// Longest window one materializer pass searches. Must exceed the overlap
    /// (see [`validate_trace_usage_window`]): the excess is how far the cursor
    /// moves per pass.
    pub trace_usage_max_window_secs: u64,
    /// Max traces to fetch from Tempo per concurrent batch.
    pub trace_usage_batch_size: usize,

    // ─── MCP Gateway ────────────────────────────────────────────────────────
    /// Composio platform API key. When unset, Composio integration is disabled
    /// (generic MCP servers still work).
    pub composio_api_key: Option<String>,
    /// Composio v3 HTTP API base URL.
    pub composio_base_url: String,
    /// HMAC secret used to verify inbound Composio webhooks. When unset,
    /// signature verification is skipped (dev only).
    pub composio_webhook_secret: Option<String>,
    /// Public URL of the MCP gateway, injected into every deployed agent as
    /// `MCP_GATEWAY_URL`. When unset, no MCP env is injected at deploy time.
    pub mcp_gateway_public_url: Option<String>,
    /// Base URL for the generic-connector OAuth 2.1 browser redirect
    /// (`{base}/oauth/callback`), distinct from `mcp_gateway_public_url` on
    /// purpose: that value is told to agent *containers* (may be a
    /// Docker-internal address like `host.docker.internal`, meaningless to a
    /// browser or a real OAuth provider's redirect-uri validation —
    /// confirmed live: Notion's DCR endpoint rejects it with "Redirect URI
    /// must use HTTPS unless it is a loopback HTTP URI"). This one is opened
    /// in the *user's own browser*, so it needs to satisfy that requirement
    /// instead. Falls back to `mcp_gateway_public_url` when unset, which is
    /// correct in production (a real HTTPS domain satisfies both audiences)
    /// but not for local dev with a Docker-only `MCP_GATEWAY_PUBLIC_URL`.
    pub mcp_oauth_redirect_base_url: Option<String>,
    /// Same browser-reachable-redirect problem as `mcp_oauth_redirect_base_url`
    /// above, but for the separate Composio OAuth connect flow
    /// (`oss/mcp-gateway/src/connect.rs`). COMPOSIO_CALLBACK_BASE_URL, optional.
    pub composio_callback_base_url: Option<String>,
    /// TTL (seconds) for the Redis-cached resolved backend/session list.
    pub mcp_session_ttl_seconds: u64,
    /// TTL (seconds) for the Redis-cached per-agent permission context.
    pub mcp_perm_cache_ttl_seconds: u64,
    /// TTL (seconds) for the Redis-cached aggregated tool manifest.
    pub mcp_manifest_ttl_seconds: u64,
    /// Max upload size for a user's own MCP server zip. MCP_UPLOAD_MAX_BYTES,
    /// default 50 MiB — deliberately smaller than agents' 100 MiB default,
    /// since MCP servers are typically much smaller than full agent codebases.
    pub mcp_upload_max_bytes: u64,
    /// Max request body size for the agent-facing MCP gateway routes
    /// (`POST /api/mcp`, `POST /api/mcp/s/{token}`). MCP_GATEWAY_MAX_BODY_BYTES,
    /// default 8 MiB — axum's own default of 2 MiB would reject the inline
    /// `save_file` payloads a later task introduces.
    pub mcp_gateway_max_body_bytes: usize,
    /// Port an uploaded MCP server container is expected to bind via `$PORT`.
    /// MCP_UPLOAD_DEFAULT_PORT, default 8080.
    pub mcp_upload_default_port: u16,
    /// Docker network uploaded MCP server containers are deployed onto,
    /// isolated from the default network (DB/Redis/agents). MCP_SERVERS_NETWORK,
    /// default "nasiko-mcp-servers-net" (the server's own compose config must
    /// also join this network — see docker-compose.infra.yml).
    pub mcp_servers_network: String,
    /// Maximum replica count for uploaded MCP server pods under Kubernetes
    /// (KEDA ScaledObject). MCP_UPLOAD_MAX_REPLICAS, default 1 (matches
    /// agents; set higher when KEDA is installed). Ignored by DockerRuntime.
    pub mcp_upload_max_replicas: u32,
    /// AGENT_MAX_REPLICAS, default 1. No longer read for agent deploys: every agent deploys
    /// through `AgentScaling` (OSS: one replica; EE: its per-agent autoscaling). Kept so a
    /// server and an install of different versions still agree on the key during an upgrade.
    pub agent_max_replicas: u32,
    /// Connections in the server's Postgres pool, shared by every request and background
    /// worker. DATABASE_MAX_CONNECTIONS, default 50, clamped to 1..=500. A turn costs about 20
    /// round trips on the proxy hop and about 50 with the agent's own LLM, MCP and state calls,
    /// so at 50 turns a second the pool carries about 2,500 a second. Keep it under Postgres's
    /// own `max_connections` (100 by default), less what other clients use.
    pub database_max_connections: u32,
    /// Number of agent/connector image builds the build worker runs
    /// concurrently. BUILD_CONCURRENCY, default 4. Clamped to 1..=256: each
    /// in-flight build is a `docker build` on this host (OSS) or a build on the
    /// cluster's build backend (EE), so size it to what that backend holds, not
    /// higher. An in-flight build mostly waits, which is why a high-rate cluster
    /// needs more than 16. Set 1 to restore the old strictly-serial behavior.
    ///
    /// Two builds for the *same* agent or connector never overlap regardless of
    /// this value — the claim query skips a target that already has a build in
    /// flight (see `build_worker::claim_next_job`).
    pub build_concurrency: usize,
    /// Requests per minute the in-cluster build service may make to the embedded `/v2`
    /// registry, all builds together (they share one credential, `build-service`).
    /// BUILD_PUSH_RATE_LIMIT_PER_MINUTE; unset is 600 times `build_concurrency`, clamped
    /// 60-100000. One push is dozens of requests (a HEAD per layer, the uploads, the manifest),
    /// so the per-identity OCI limit of 300 refused pushes once a few builds overlapped. Over
    /// it a push gets a 429. Counted in memory by each server process.
    pub build_push_rate_limit_per_minute: u32,
    /// Number of MAF workflow executions each server process runs concurrently.
    /// MAF_WORKER_CONCURRENCY, default 4. Clamped to 1..=32. An execution mostly
    /// waits on agents and LLMs, so the limit that matters is shared capacity, not
    /// CPU: every execution makes its planning, extraction and final-answer calls on
    /// the platform's LLM key, so raise this with that key's rate limit in mind.
    /// Set 1 to restore the old strictly-serial behavior.
    pub maf_worker_concurrency: usize,
    /// How often a MAF worker refreshes the stream messages of the runs it is
    /// executing, in seconds. MAF_HEARTBEAT_SECS, default 20, clamped 5..=120. A
    /// message is taken over by another worker's recovery once its worker has missed
    /// six refreshes in a row (2 min at the default), and recovery passes run every
    /// three (1 min) — both derived from this, never set on their own.
    pub maf_heartbeat_secs: u64,
    /// Default memory limit for every agent container, Kubernetes notation
    /// (`"512Mi"`, `"1Gi"`) — see `nasiko_runtime::ResourceLimits::memory`.
    /// AGENT_DEFAULT_MEMORY, default `"1Gi"`. `nasiko_runtime::ResourceLimits`
    /// itself still defaults to `"512Mi"` (its own hermetic fallback for
    /// callers outside the server, e.g. tests) — this is the value the server
    /// actually uses for every agent deploy unless a future per-agent
    /// override is added. Raised from the runtime crate's original 512Mi
    /// after opencode (3 concurrent Node/Bun processes) was repeatedly
    /// OOM-killed under real chat load at that limit.
    pub agent_default_memory: String,
    /// Name of the single named Docker volume every `--writable` agent shares
    /// (each mounted at its own `volume-subpath`) — see
    /// `nasiko_runtime::DockerRuntimeConfig::agent_memory_volume`.
    /// AGENT_MEMORY_VOLUME, default `"nasiko-agent-memory"`.
    pub agent_memory_volume: String,
    /// Image for the short-lived helper container that pre-creates a
    /// `--writable` agent's subdirectory inside `agent_memory_volume` — see
    /// `nasiko_runtime::DockerRuntimeConfig::agent_memory_init_image`.
    /// AGENT_MEMORY_INIT_IMAGE, default `"alpine:3.21"` (override for
    /// air-gapped or internal-mirror setups).
    pub agent_memory_init_image: String,
    /// TTL (seconds) for the Redis-cached Composio toolkit tool count shown on
    /// unconnected catalog cards — changes rarely, so a much longer TTL than
    /// the permission/session caches.
    pub mcp_toolcount_ttl_seconds: u64,
    /// Comma-separated Composio toolkit names to auto-register at first boot.
    /// SEED_TOOLKITS, default empty. Requires COMPOSIO_API_KEY to be set.
    pub seed_toolkits: Vec<String>,
    /// Remote MCP connectors to auto-register at boot, as a JSON array.
    /// SEED_CONNECTORS, default empty (no-op). Each entry needs `name` and
    /// `url`; optional `auth_type` (default `none`), `transport` (default
    /// `streamable_http`), `headers`, `description`, `display_name`,
    /// `logo_url`. Each is owned by the first superuser and shared publicly;
    /// one that already exists is not re-registered, but its public share is
    /// re-asserted and its tool catalog re-synced every boot. Invalid JSON is
    /// logged and ignored.
    /// Static `headers` with `auth_type` `none` send one shared key for every
    /// user; an `oauth2` entry is registered and shared, and each user still
    /// clicks Connect once. For a per-user key use `auth_type` `bearer` with no
    /// `headers`: each user enters their own key in the UI; a `bearer`/`basic`
    /// entry that also carries an `Authorization` header is refused at boot,
    /// since users without a key would silently run on that shared one.
    /// Holds API keys — deliver it as a secret. Example:
    /// `[{"name":"firecrawl","url":"https://mcp.firecrawl.dev/v2/mcp","auth_type":"bearer"},{"name":"anakin","url":"https://mcp.anakin.io/mcp","auth_type":"oauth2"}]`
    /// (both per-user; the Helm chart seeds exactly these by default).
    pub seed_connectors: String,
    /// MCP tool search mode: `semantic` (embedding cosine, default), `keyword`
    /// (BM25 fallback), or `none` (eager fan-out, no search — rollback).
    pub mcp_tool_search_mode: String,
    /// Max tools returned by query-aware `tools/list`.
    pub mcp_tool_search_tool_limit: usize,
    /// Max tools returned by the `nasiko_search_tools` meta-tool.
    pub mcp_tool_search_meta_limit: usize,
    /// The gateway's own sentence advertised in `initialize.instructions`,
    /// ahead of any per-connector instructions (`oss/mcp-gateway/src/protocol.rs`).
    /// MCP_GATEWAY_INSTRUCTIONS. Blank (unset or whitespace-only) means "let the
    /// gateway crate apply its own default sentence" — deliberately carried here
    /// as a raw, possibly-blank string rather than pre-defaulted, since the
    /// default text lives in `oss/mcp-gateway` (a crate that syncs to the public
    /// repo) and this crate must not duplicate it.
    pub mcp_gateway_instructions: String,
    /// HMAC key the gateway signs the `x-nasiko-identity` header with when
    /// forwarding a caller's `(agent_id, user_id, flow_id)` to a system backend
    /// (`oss/mcp-gateway/src/identity.rs`) — the backend trusts that header
    /// instead of re-deriving who is calling. MCP_IDENTITY_SIGNING_KEY; falls
    /// back to a value *derived* from the already-required `JWT_SECRET` when
    /// unset/empty (domain-separated with an `mcp-identity::` prefix, not a
    /// bare reuse — see `from_env`), so a deployment doesn't need to mint a
    /// second secret just for this, without literally handing the session-JWT
    /// key to whichever out-of-process backend this key gets configured into.
    /// Empty only when both are unset, which `McpConfig::from_config` rejects
    /// at startup with a clear message.
    pub mcp_identity_signing_key: String,
}

/// Domain-separation prefix for the `JWT_SECRET`-derived fallback identity
/// key (see [`derive_identity_signing_key`]) — the single source both this
/// crate and `oss/mcp-gateway/src/config.rs` read, so the two never drift
/// onto different literals (the gateway strips this exact prefix before
/// measuring the fallback key's actual entropy for its short-key warning).
pub const IDENTITY_KEY_PREFIX: &str = "mcp-identity::";

/// `MCP_IDENTITY_SIGNING_KEY`, trimmed; when unset/blank, derives one from a
/// trimmed `JWT_SECRET` with an [`IDENTITY_KEY_PREFIX`] domain-separation
/// prefix — never a bare reuse of the session-JWT key, mirroring
/// `oauth_state_signing_key`'s own `mcp-oauth-state::` prefix in
/// `oss/mcp-gateway/src/config.rs`. The point of domain separation here is
/// specifically that this key may end up configured into an out-of-process
/// backend (e.g. the workspace server, a later consumer of
/// `identity_signing_key`) — an operator handing that backend the platform's
/// "identity key" must never be handing over the same key that also signs
/// every session JWT. Empty only when `JWT_SECRET` is empty too, which
/// `McpConfig::from_config` (`oss/mcp-gateway/src/config.rs`) rejects at
/// startup with a clear message — this function does not fail fast itself,
/// since `nasiko-config` has no such policy for values it merely carries.
fn mcp_identity_signing_key() -> String {
    let dedicated = std::env::var("MCP_IDENTITY_SIGNING_KEY").ok();
    let jwt_secret = env_or("JWT_SECRET", "");
    derive_identity_signing_key(dedicated.as_deref(), &jwt_secret)
}

/// Pure derivation behind [`mcp_identity_signing_key`], split out so the four
/// cases below are unit-testable without touching the environment: a
/// non-blank `dedicated` key (trimmed) always wins; otherwise a non-blank
/// `jwt_secret` (trimmed) is domain-separated with the [`IDENTITY_KEY_PREFIX`]
/// prefix; if both are blank the result is empty, which `McpConfig::from_config`
/// rejects at startup.
fn derive_identity_signing_key(dedicated: Option<&str>, jwt_secret: &str) -> String {
    if let Some(key) = dedicated.map(str::trim).filter(|s| !s.is_empty()) {
        return key.to_string();
    }
    let jwt_secret = jwt_secret.trim();
    if jwt_secret.is_empty() {
        return String::new();
    }
    format!("{IDENTITY_KEY_PREFIX}{jwt_secret}")
}

/// Where to ship coding-agent OTLP (traces + logs) when `CODING_AGENT_OTLP_ENDPOINT`
/// is not set — so a standard deployment needs no dedicated var.
///
/// The coding-agent exporter speaks OTLP/**HTTP** (it `POST`s `/v1/traces`), while
/// the general `OTEL_EXPORTER_OTLP_ENDPOINT` is commonly gRPC on `:4317`. The OTel
/// Collector serves HTTP on `:4318`, so a bare `:4317` endpoint is mapped to `:4318`
/// here; an endpoint that is already HTTP (any other port, or none) is reused as-is.
/// Set `CODING_AGENT_OTLP_ENDPOINT` explicitly to override for a non-standard setup.
fn coding_agent_otlp_fallback() -> Option<String> {
    let base = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .or_else(|_| std::env::var("OTEL_COLLECTOR_ENDPOINT"))
        .ok()?;
    let base = base.trim();
    if base.is_empty() {
        return None;
    }
    Some(base.replace(":4317", ":4318"))
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        // Checked together first, so a first start names every missing variable at once.
        let mut required = vec![
            "DATABASE_URL",
            "REDIS_URL",
            "SECRETS_ENCRYPTION_KEY",
            "ADMIN_PASSWORD",
        ];
        if uses_s3_storage(&env_or("STORAGE_PROVIDER", "s3")) {
            required.push("S3_SECRET_KEY");
        }
        require_all(&required)?;
        let tiering_enabled = env_bool("NASIKO_TIERING_ENABLED", false);
        let tiering_interval_secs = std::env::var("NASIKO_TIERING_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(24 * 3600);
        let cold_path_set = std::env::var("NASIKO_COLD_PATH").ok();
        validate_tiering(tiering_enabled, cold_path_set.as_deref()).map_err(anyhow::Error::msg)?;
        let agent_call_timeout_secs = std::env::var("AGENT_CALL_TIMEOUT_SECS")
            .or_else(|_| std::env::var("ROUTER_AGENT_TIMEOUT_SECS"))
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(600);
        let session_reaper_interval_secs =
            env_parse("SESSION_REAPER_INTERVAL_SECS", 30).clamp(5, 600);
        let agent_park_idle_secs = parse_agent_park_idle_secs(
            nonempty_env("AGENT_PARK_IDLE_SECS").as_deref(),
            park_idle_floor_secs(agent_call_timeout_secs, session_reaper_interval_secs),
        )
        .map_err(anyhow::Error::msg)?;
        let build_concurrency = env_parse("BUILD_CONCURRENCY", 4).clamp(1, 256);
        let config = Self {
            bind: env_or("CP_BIND", "0.0.0.0:8080"),
            domain: std::env::var("CP_DOMAIN").ok(),
            delegation_audience: delegation_audience_from_env(),
            delegation_enabled: std::env::var("DELEGATION_ENABLED")
                .map(|v| !matches!(v.trim(), "0" | "false" | "no" | "off"))
                .unwrap_or(true),
            database_url: required_env("DATABASE_URL")?,
            redis_url: required_env("REDIS_URL")?,
            agent_runtime: env_or("AGENT_RUNTIME", "local"),
            k8s_namespace: env_or("K8S_NAMESPACE", "nasiko-agents"),
            kubeconfig: std::env::var("KUBECONFIG").ok().filter(|s| !s.is_empty()),
            storage_provider: env_or("STORAGE_PROVIDER", "s3"),
            s3_endpoint: env_or("S3_ENDPOINT", "http://localhost:9000"),
            s3_bucket: env_or("S3_BUCKET", "nasiko"),
            s3_access_key: env_or("S3_ACCESS_KEY", "nasiko"),
            // Required only for an S3 backend. An Azure Blob deployment holds
            // no S3 credential at all, and demanding one there turned a
            // correct config into a startup failure — so the requirement
            // follows the selected provider rather than being unconditional.
            s3_secret_key: if uses_s3_storage(&env_or("STORAGE_PROVIDER", "s3")) {
                required_env("S3_SECRET_KEY")?
            } else {
                String::new()
            },
            s3_region: env_or("S3_REGION", "us-east-1"),
            assessment_intake_url: std::env::var("ASSESSMENT_INTAKE_URL")
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty()),
            secrets_encryption_key: required_env("SECRETS_ENCRYPTION_KEY")?,
            oci_storage_bucket: env_or("OCI_STORAGE_BUCKET", "nasiko-artifacts"),
            agent_image_registry: env_or("AGENT_IMAGE_REGISTRY", ""),
            agent_image_registry_placeholder: std::env::var("AGENT_IMAGE_REGISTRY_PLACEHOLDER")
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty()),
            agent_image_registry_aliases: registry_aliases(
                &std::env::var("AGENT_IMAGE_REGISTRY_ALIASES").unwrap_or_default(),
                &env_or("AGENT_IMAGE_REGISTRY", ""),
            ),
            agent_registry_username: std::env::var("AGENT_REGISTRY_USERNAME")
                .ok()
                .filter(|s| !s.is_empty()),
            agent_registry_password: std::env::var("AGENT_REGISTRY_PASSWORD")
                .ok()
                .filter(|s| !s.is_empty()),
            agent_registry_host: std::env::var("AGENT_REGISTRY_HOST")
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty()),
            seed_image_platform: env_or("SEED_IMAGE_PLATFORM", "linux/amd64"),
            build_push_token: env_or("BUILD_PUSH_TOKEN", ""),
            agent_jwt_secret: env_or("AGENT_JWT_SECRET", ""),
            agent_jwt_algorithm: env_or("AGENT_JWT_ALGORITHM", "HS256"),
            injection_proxy_token: env_or("INJECTION_PROXY_TOKEN", ""),
            seed_agents: std::env::var("SEED_AGENTS").ok(),
            seed_agent_settings: parse_seed_agent_settings(
                std::env::var("SEED_AGENT_SETTINGS").ok().as_deref(),
            )
            .map_err(anyhow::Error::msg)?,
            seed_workflows: env_or("SEED_WORKFLOWS", ""),
            tiering_enabled,
            tiering_interval_secs,
            cold_path: env_or("NASIKO_COLD_PATH", "./data/cold"),
            openai_api_key: std::env::var("OPENAI_API_KEY").ok(),
            openai_base_url: std::env::var("OPENAI_BASE_URL").ok(),
            openai_model: task_model("OPENAI_MODEL"),
            agent_platform_key_fallback: env_bool("AGENT_PLATFORM_KEY_FALLBACK", true),
            decomposer_api_url: std::env::var("MODEL_API_URL").ok(),
            decomposer_api_key: std::env::var("MODEL_APIKEY").ok(),
            router_model: task_model("ROUTER_MODEL"),
            capability_generator_model: task_model("CAPABILITY_GENERATOR_MODEL"),
            mcp_description_model: task_model("MCP_DESCRIPTION_MODEL"),
            a2a_discovery_url: std::env::var("A2A_DISCOVERY_URL").ok(),
            otel_endpoint: std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok(),
            otel_protocol: env_or("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc"),
            otel_headers: std::env::var("OTEL_EXPORTER_OTLP_HEADERS").ok(),
            otel_service_name: env_or("OTEL_SERVICE_NAME", "nasiko-cp"),
            otel_sample_ratio: env_or("OTEL_TRACES_SAMPLER_ARG", "1.0"),
            // Port 4317 (OTLP gRPC), not 4318 (OTLP HTTP), because this endpoint is
            // injected into agents alongside `otel_protocol`, which defaults to
            // "grpc" — the pairing has to agree or every agent's exporter speaks
            // gRPC at an HTTP port and silently fails to export. 4317+grpc is also
            // the conventional OTLP default pairing.
            otel_collector_endpoint: env_or(
                "OTEL_COLLECTOR_ENDPOINT",
                "http://otel-collector:4317",
            ),
            otel_capture_content: std::env::var(
                "OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT",
            )
            .map(|v| v == "true")
            .unwrap_or(true),
            coding_agent_otlp_endpoint: std::env::var("CODING_AGENT_OTLP_ENDPOINT")
                .ok()
                // No dedicated var? Reuse the general OTLP endpoint (mapped to the
                // collector's HTTP port), so coding-agent traces ship out of the box.
                .or_else(coding_agent_otlp_fallback)
                .map(|value| value.trim_end_matches('/').to_owned())
                .filter(|value| !value.is_empty()),
            coding_agent_ingest: CodingAgentIngestConfig::from_env(),
            tempo_url: env_or("TEMPO_URL", ""),
            loki_url: env_or("LOKI_URL", ""),
            // Enabled only when BOTH backends are explicitly configured; a
            // partial config is treated as disabled. Computed here, the one place
            // env is read, so every consumer agrees on whether it's enabled.
            observability_enabled: std::env::var("TEMPO_URL").is_ok_and(|v| !v.is_empty())
                && std::env::var("LOKI_URL").is_ok_and(|v| !v.is_empty()),
            tenant_id: std::env::var("TENANT_ID").ok(),
            model_catalog_sync_enabled: env_bool("MODEL_CATALOG_SYNC_ENABLED", true),
            model_pricing_sync_enabled: env_bool("MODEL_PRICING_SYNC_ENABLED", true),
            flow_max_depth: env_parse("NASIKO_FLOW_MAX_DEPTH", 5),
            flow_max_fan_out: env_parse("NASIKO_FLOW_MAX_FAN_OUT", 20),
            flow_max_tokens: env_parse("NASIKO_FLOW_MAX_TOKENS", 100000),
            // Keep in step with `nasiko_flow::DEFAULT_FLOW_TIMEOUT_SECS` (this
            // crate is a leaf and can't reference it): an agent turn may run
            // the full `agent_call_timeout_secs`, so the flow that authorizes
            // it has to live at least as long.
            flow_timeout_secs: env_parse("NASIKO_FLOW_TIMEOUT_SECS", 600),
            hitl_request_ttl_days: env_parse("HITL_REQUEST_TTL_DAYS", 7),
            // Defaults match `nasiko_hitl::dispatcher::DispatcherConfig::default()` exactly, so
            // an unset env var changes nothing.
            hitl_resume_poll_interval_secs: env_parse("HITL_RESUME_POLL_INTERVAL_SECS", 5),
            hitl_resume_recovery_interval_secs: env_parse(
                "HITL_RESUME_RECOVERY_INTERVAL_SECS",
                10 * 60,
            ),
            // Must outlast one whole delivery, not one request: the MCP resume dispatcher holds
            // its claim across every in-process retry (3 attempts x the notifier's 300s timeout
            // + backoff ~= 15 min). At the old default of 2 the recovery sweep quarantined
            // deliveries that were still in flight. `DispatcherConfig::effective_lease_minutes`
            // enforces the floor regardless, so this default only keeps the two in agreement.
            hitl_resume_lease_minutes: env_parse("HITL_RESUME_LEASE_MINUTES", 16),
            hitl_resume_max_attempts: env_parse("HITL_RESUME_MAX_ATTEMPTS", 3),
            hitl_resume_retry_delay_secs: env_parse("HITL_RESUME_RETRY_DELAY_SECS", 2),
            coding_agent_idle_ttl_secs: env_parse("CODING_AGENT_IDLE_TTL_SECS", 900),
            sandbox_provider: sandbox_provider_from_env()?,
            harness_sessions_enabled: env_bool("HARNESS_SESSIONS_ENABLED", false),
            build_sandbox_smoke: env_or("BUILD_SANDBOX_SMOKE", "off")
                .parse()
                .map_err(anyhow::Error::msg)?,
            session_idle_grace_secs: env_parse("SESSION_IDLE_GRACE_SECS", 120),
            session_cpu_quiescent_millicores: env_parse("SESSION_CPU_QUIESCENT_MILLICORES", 20),
            session_reaper_interval_secs,
            agent_open_task_max_age_secs: env_parse("AGENT_OPEN_TASK_MAX_AGE_SECS", 3600)
                .clamp(60, 86400),
            agent_park_idle_secs,
            harness_image_prefix: env_or("HARNESS_IMAGE_PREFIX", "nasiko/harness-"),
            coding_agent_turn_lease_secs: env_parse("CODING_AGENT_TURN_LEASE_SECS", 60),
            coding_agent_lease_heartbeat_secs: env_parse("CODING_AGENT_LEASE_HEARTBEAT_SECS", 20),
            hitl_lost_task_retries: env_parse("HITL_LOST_TASK_RETRIES", 3),
            agent_state_ttl_days: env_parse("AGENT_STATE_TTL_DAYS", 7).clamp(1, 365),
            agent_state_max_body_bytes: env_parse("AGENT_STATE_MAX_BODY_BYTES", 1_048_576).max(1),
            agent_state_rate_limit_per_minute: env_parse("AGENT_STATE_RATE_LIMIT_PER_MINUTE", 1200)
                .max(1),
            agent_peer_call_rate_limit_per_minute: env_parse(
                "AGENT_PEER_CALL_RATE_LIMIT_PER_MINUTE",
                300,
            )
            .clamp(1, 10_000),
            agent_state_list_bytes_budget: env_parse(
                "AGENT_STATE_LIST_BYTES_BUDGET",
                8 * 1024 * 1024,
            )
            .max(1),
            github_client_id: std::env::var("GITHUB_CLIENT_ID").ok(),
            github_client_secret: std::env::var("GITHUB_CLIENT_SECRET").ok(),
            multi_tenant_mode: std::env::var("MULTI_TENANT_MODE")
                .map(|v| v == "true")
                .unwrap_or(false),
            allow_personal_emails: std::env::var("ALLOW_PERSONAL_EMAILS")
                .map(|v| v == "true")
                .unwrap_or(false),
            nasiko_bff_url: std::env::var("NASIKO_BFF_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            router_shortlist_threshold: env_parse("ROUTER_SHORTLIST_THRESHOLD", 15),
            router_shortlist_size: env_parse("ROUTER_SHORTLIST_SIZE", 10),
            pacms_history_pool_size: env_parse("PACMS_HISTORY_POOL_SIZE", 150),
            savings_factor_refresh_secs: env_parse("SAVINGS_FACTOR_REFRESH_SECS", 86_400),
            savings_factor_min_samples: env_parse("SAVINGS_FACTOR_MIN_SAMPLES", 1_600),
            savings_factor_window_days: env_parse("SAVINGS_FACTOR_WINDOW_DAYS", 30),
            minimal_code_holdout_pct: env_parse("MINIMAL_CODE_HOLDOUT_PCT", 5u8),
            react_compress_enabled: env_parse("TOKEN_COMPRESS_TOOL_RESULTS", true),
            react_compress_min_bytes: env_parse("TOKEN_COMPRESS_TOOL_RESULTS_MIN_BYTES", 2048),
            history_compress_enabled: env_parse("TOKEN_COMPRESS_HISTORY", true),
            history_compress_min_bytes: env_parse("TOKEN_COMPRESS_HISTORY_MIN_BYTES", 2048),
            pacms_budget_low: env_parse("PACMS_BUDGET_LOW", 500),
            pacms_budget_medium: env_parse("PACMS_BUDGET_MEDIUM", 1000),
            pacms_budget_high: env_parse("PACMS_BUDGET_HIGH", 5000),
            pacms_history_mandatory_recent: env_parse("PACMS_HISTORY_MANDATORY_RECENT", 3),
            context_k_low: env_parse("CONTEXT_K_LOW", 1),
            context_k_medium: env_parse("CONTEXT_K_MEDIUM", 5),
            context_k_high: env_parse("CONTEXT_K_HIGH", 20),
            embedding_model: env_or("EMBEDDING_MODEL", "text-embedding-3-small"),
            agent_call_timeout_secs,
            agent_startup_budget_secs: agent_startup_budget_secs_from_env(),
            github_callback_url: std::env::var("GITHUB_CALLBACK_URL").ok(),
            github_central_callback_url: std::env::var("GITHUB_CENTRAL_CALLBACK_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            app_base_url: env_or("APP_BASE_URL", ""),
            docker_agent_network: std::env::var("DOCKER_AGENT_NETWORK")
                .ok()
                .filter(|s| !s.is_empty()),
            oci_registry_host: std::env::var("OCI_REGISTRY_HOST")
                .ok()
                .filter(|s| !s.is_empty()),
            container_hours_poll_secs: env_parse("CONTAINER_HOURS_POLL_SECS", 60),
            trace_usage_sync_secs: env_parse("TRACE_USAGE_SYNC_SECS", 120),
            trace_usage_overlap_secs: env_parse("TRACE_USAGE_OVERLAP_SECS", 180),
            trace_usage_max_window_secs: env_parse("TRACE_USAGE_MAX_WINDOW_SECS", 600),
            trace_usage_batch_size: env_parse("TRACE_USAGE_BATCH_SIZE", 8),
            git_clone_allowed_hosts: std::env::var("GIT_CLONE_ALLOWED_HOSTS")
                .unwrap_or_else(|_| "github.com,gitlab.com,bitbucket.org".to_owned())
                .split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect(),
            registry_import_allowed_hosts: std::env::var("REGISTRY_IMPORT_ALLOWED_HOSTS")
                .unwrap_or_default()
                .split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect(),
            cors_allowed_origins: std::env::var("CORS_ALLOWED_ORIGINS")
                .unwrap_or_default()
                .split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect(),
            agent_allow_private_urls: env_bool("AGENT_ALLOW_PRIVATE_URLS", false),
            agent_current_time: env_bool("AGENT_CONTEXT_CURRENT_TIME", true),
            oidc_issuer_url: std::env::var("OIDC_ISSUER_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            oidc_client_id: std::env::var("OIDC_CLIENT_ID")
                .ok()
                .filter(|s| !s.is_empty()),
            oidc_client_secret: std::env::var("OIDC_CLIENT_SECRET")
                .ok()
                .filter(|s| !s.is_empty()),
            oidc_redirect_uri: std::env::var("OIDC_REDIRECT_URI")
                .ok()
                .filter(|s| !s.is_empty()),
            oidc_allowed_redirect_origins: std::env::var("OIDC_ALLOWED_REDIRECT_ORIGINS")
                .unwrap_or_default()
                .split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect(),
            oidc_scopes: env_or("OIDC_SCOPES", "openid profile email"),
            oidc_provider_label: env_or("OIDC_PROVIDER_LABEL", ""),
            oidc_central_callback_url: std::env::var("OIDC_CENTRAL_CALLBACK_URL")
                .ok()
                .filter(|s| !s.is_empty()),

            admin_username: env_or("ADMIN_USERNAME", "admin"),
            admin_password: required_env("ADMIN_PASSWORD")?,

            composio_api_key: std::env::var("COMPOSIO_API_KEY")
                .ok()
                .filter(|s| !s.is_empty()),
            composio_base_url: env_or("COMPOSIO_BASE_URL", "https://backend.composio.dev"),
            composio_webhook_secret: std::env::var("COMPOSIO_WEBHOOK_SECRET")
                .ok()
                .filter(|s| !s.is_empty()),
            mcp_gateway_public_url: std::env::var("MCP_GATEWAY_PUBLIC_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            mcp_oauth_redirect_base_url: std::env::var("MCP_OAUTH_REDIRECT_BASE_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            composio_callback_base_url: std::env::var("COMPOSIO_CALLBACK_BASE_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            mcp_session_ttl_seconds: env_parse("MCP_SESSION_TTL_SECONDS", 300),
            mcp_perm_cache_ttl_seconds: env_parse("MCP_PERM_CACHE_TTL_SECONDS", 30),
            mcp_manifest_ttl_seconds: env_parse("MCP_MANIFEST_TTL_SECONDS", 300),
            mcp_upload_max_bytes: env_parse("MCP_UPLOAD_MAX_BYTES", 50 * 1024 * 1024),
            mcp_gateway_max_body_bytes: env_parse("MCP_GATEWAY_MAX_BODY_BYTES", 8 * 1024 * 1024),
            mcp_upload_default_port: env_parse("MCP_UPLOAD_DEFAULT_PORT", 8080),
            mcp_servers_network: env_or("MCP_SERVERS_NETWORK", "nasiko-mcp-servers-net"),
            mcp_upload_max_replicas: env_parse("MCP_UPLOAD_MAX_REPLICAS", 1),
            agent_max_replicas: env_parse("AGENT_MAX_REPLICAS", 1),
            database_max_connections: env_parse("DATABASE_MAX_CONNECTIONS", 50).clamp(1, 500),
            build_concurrency,
            build_push_rate_limit_per_minute: env_parse(
                "BUILD_PUSH_RATE_LIMIT_PER_MINUTE",
                600 * build_concurrency as u32,
            )
            .clamp(60, 100_000),
            maf_worker_concurrency: env_parse("MAF_WORKER_CONCURRENCY", 4).clamp(1, 32),
            maf_heartbeat_secs: env_parse("MAF_HEARTBEAT_SECS", 20).clamp(5, 120),
            agent_default_memory: env_or("AGENT_DEFAULT_MEMORY", "1Gi"),
            agent_memory_volume: env_or("AGENT_MEMORY_VOLUME", "nasiko-agent-memory"),
            agent_memory_init_image: env_or("AGENT_MEMORY_INIT_IMAGE", "alpine:3.21"),
            mcp_toolcount_ttl_seconds: env_parse("MCP_TOOLCOUNT_TTL_SECONDS", 3600),
            seed_toolkits: std::env::var("SEED_TOOLKITS")
                .unwrap_or_default()
                .split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect(),
            seed_connectors: env_or("SEED_CONNECTORS", ""),
            mcp_tool_search_mode: env_or("MCP_TOOL_SEARCH_MODE", "semantic"),
            mcp_tool_search_tool_limit: env_parse("MCP_TOOL_SEARCH_TOOL_LIMIT", 15),
            mcp_tool_search_meta_limit: env_parse("MCP_TOOL_SEARCH_META_LIMIT", 10),
            mcp_gateway_instructions: env_or("MCP_GATEWAY_INSTRUCTIONS", ""),
            mcp_identity_signing_key: mcp_identity_signing_key(),
        };
        config
            .validate_session_lifecycle()
            .map_err(anyhow::Error::msg)?;
        validate_trace_usage_window(
            config.trace_usage_overlap_secs,
            config.trace_usage_max_window_secs,
        )
        .map_err(anyhow::Error::msg)?;
        Ok(config)
    }

    /// Fail fast if the idle TTL cannot contain the idle grace window. The reaper
    /// measures `session_idle_grace_secs` of CPU quiescence *inside*
    /// `coding_agent_idle_ttl_secs`, so a non-zero TTL below the grace could never
    /// be satisfied and would silently disable suspension. `0` (suspension off) is
    /// always valid. Also checks the lease/heartbeat pair, the call timeout against the session
    /// token, and the agent park window against the call timeout.
    pub fn validate_session_lifecycle(&self) -> Result<(), String> {
        validate_idle_ttl_covers_grace(
            self.coding_agent_idle_ttl_secs,
            self.session_idle_grace_secs,
        )?;
        validate_heartbeat_within_lease(
            self.coding_agent_turn_lease_secs,
            self.coding_agent_lease_heartbeat_secs,
        )?;
        validate_call_timeout_within_session_token(self.agent_call_timeout_secs)?;
        validate_park_idle_covers_a_call(
            self.agent_park_idle_secs,
            park_idle_floor_secs(
                self.agent_call_timeout_secs,
                self.session_reaper_interval_secs,
            ),
        )
    }

    /// Optional settings left unset that switch a feature off, each with what stops working.
    /// Logged once at startup ([`Self::warn_unset_optional_keys`]) so the cause is on record
    /// before the first request that needs it.
    pub fn unset_optional_keys(&self) -> Vec<&'static str> {
        let mut unset = Vec::new();
        if self.agent_jwt_secret.trim().is_empty() {
            unset.push("AGENT_JWT_SECRET unset: LLM gateway and coding sessions are disabled");
        }
        if self.agent_runtime == "kubernetes" && self.build_push_token.trim().is_empty() {
            unset.push("BUILD_PUSH_TOKEN unset: in-cluster builds cannot push agent images");
        }
        if self
            .openai_api_key
            .as_deref()
            .is_none_or(|k| k.trim().is_empty())
        {
            unset.push(
                "OPENAI_API_KEY unset: no server LLM key, so routing and agent runs fail \
                 until a user saves their own key",
            );
        }
        unset
    }

    /// Log [`Self::unset_optional_keys`], one warning each.
    pub fn warn_unset_optional_keys(&self) {
        for problem in self.unset_optional_keys() {
            tracing::warn!("{problem}");
        }
    }

    /// Fail fast if `SECRETS_ENCRYPTION_KEY` can't actually be used to construct
    /// a `SecretsCrypto` (base64-decodes to exactly 32 bytes). Both the OSS
    /// HKDF-per-scope crypto and EE's `nasiko-secrets::SecretsCrypto::from_key`
    /// require this shape; previously an invalid key (e.g. 32 raw alphanumeric
    /// characters, which decode to only 24 bytes) passed config validation
    /// silently and only surfaced as a panic/error on the first secret
    /// encrypt/decrypt call, at request time, long after boot.
    pub fn validate_secrets_key(&self) -> Result<(), String> {
        validate_secrets_key_format(&self.secrets_encryption_key)
    }

    /// Whether the reaper parks idle agents: `AGENT_PARK_IDLE_SECS` is not `0`.
    pub fn parks_agents(&self) -> bool {
        self.agent_park_idle_secs > 0
    }

    /// How long a call waits for a parked agent to wake: the agent startup budget, held below the
    /// agent call budget so a wake never outlasts the call that waits for it.
    pub fn agent_wake_budget_secs(&self) -> u64 {
        wake_budget_secs(self.agent_startup_budget_secs, self.agent_call_timeout_secs)
    }
}

/// Stop startup over a configuration problem: one plain line per problem on stderr, then exit 1.
/// A panic would print a thread name and a backtrace hint and read as a crash, not as a setting
/// to fix.
pub fn exit_invalid_config(problem: impl std::fmt::Display) -> ! {
    let problem = problem.to_string();
    tracing::error!(%problem, "configuration invalid");
    eprintln!("error: configuration invalid:");
    for line in problem.lines() {
        eprintln!("  {line}");
    }
    std::process::exit(1)
}

/// Default `AGENT_STARTUP_BUDGET_SECS`.
const DEFAULT_AGENT_STARTUP_BUDGET_SECS: u64 = 120;
/// Accepted `AGENT_STARTUP_BUDGET_SECS`: a budget under 10 s restarts any agent that boots slowly,
/// and one over 30 minutes hides a pod that will never start.
const AGENT_STARTUP_BUDGET_SECS_RANGE: std::ops::RangeInclusive<u64> = 10..=1800;

/// `AGENT_STARTUP_BUDGET_SECS`, with a malformed or out-of-range value replaced, and a warning
/// rather than a silent fallback.
fn agent_startup_budget_secs_from_env() -> u64 {
    let raw = std::env::var("AGENT_STARTUP_BUDGET_SECS").ok();
    let (secs, problem) = parse_agent_startup_budget(raw.as_deref());
    if let Some(problem) = problem {
        tracing::warn!(
            key = "AGENT_STARTUP_BUDGET_SECS",
            raw = raw.as_deref().unwrap_or(""),
            secs,
            "{problem}"
        );
    }
    secs
}

fn parse_agent_startup_budget(raw: Option<&str>) -> (u64, Option<&'static str>) {
    let Some(raw) = raw else {
        return (DEFAULT_AGENT_STARTUP_BUDGET_SECS, None);
    };
    let Ok(parsed) = raw.trim().parse::<u64>() else {
        return (
            DEFAULT_AGENT_STARTUP_BUDGET_SECS,
            Some("not a whole number, using the default"),
        );
    };
    let clamped = parsed.clamp(
        *AGENT_STARTUP_BUDGET_SECS_RANGE.start(),
        *AGENT_STARTUP_BUDGET_SECS_RANGE.end(),
    );
    if clamped == parsed {
        (parsed, None)
    } else {
        (clamped, Some("outside the accepted range, clamped"))
    }
}

/// The startup budget, clamped to at least one second and to under the call budget. Clamped
/// rather than refused at startup: a startup budget at or above the call timeout is valid for
/// the runtime's own startup probe, it only cannot all be spent inside one call.
fn wake_budget_secs(startup_budget_secs: u64, call_timeout_secs: u64) -> u64 {
    startup_budget_secs
        .min(call_timeout_secs.saturating_sub(1))
        .max(1)
}

/// Strips a trailing `/v1` (and any trailing slashes) from an OpenAI-compatible
/// base URL, for callers that append their own `/v1/...` path segment.
///
/// `OPENAI_BASE_URL` is commonly written *with* the `/v1` — that is how
/// `cp.app.nasiko.com` and typical deployment env files have it — so appending `/v1/whatever`
/// to the raw value doubles up into `.../v1/v1/whatever`, which 404s.
///
/// Deliberately a free function rather than normalization applied to
/// [`Config::openai_base_url`] itself: the artifact registry uses the opposite
/// convention (base URL *includes* `/v1`, it appends bare `/embeddings`), so
/// the stored value has to stay verbatim.
pub fn openai_base_url_without_v1(base_url: &str) -> &str {
    let trimmed = base_url.trim_end_matches('/');
    trimmed.strip_suffix("/v1").unwrap_or(trimmed)
}

/// Which provider runs coding-agent sandboxes. See [`Config::sandbox_provider`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxProviderSetting {
    Docker,
    Pod,
    SharedPod,
    /// Each user's own harness agent runs every session of that user and harness.
    HarnessAgent,
}

impl std::str::FromStr for SandboxProviderSetting {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "docker" => Ok(Self::Docker),
            "pod" => Ok(Self::Pod),
            "shared-pod" => Ok(Self::SharedPod),
            "harness-agent" => Ok(Self::HarnessAgent),
            _ => Err(format!(
                "SANDBOX_PROVIDER must be one of docker, pod, shared-pod, harness-agent, got {raw:?}"
            )),
        }
    }
}

/// Whether builds smoke-run their image under gVisor. See [`Config::build_sandbox_smoke`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildSandboxSmoke {
    Off,
    Runsc,
}

impl std::str::FromStr for BuildSandboxSmoke {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "off" => Ok(Self::Off),
            "runsc" => Ok(Self::Runsc),
            _ => Err(format!(
                "BUILD_SANDBOX_SMOKE must be one of off, runsc, got {raw:?}"
            )),
        }
    }
}

fn sandbox_provider_from_env() -> anyhow::Result<SandboxProviderSetting> {
    env_or("SANDBOX_PROVIDER", "docker")
        .parse()
        .map_err(anyhow::Error::msg)
}

fn validate_idle_ttl_covers_grace(idle_ttl_secs: i64, grace_secs: u64) -> Result<(), String> {
    if idle_ttl_secs != 0 && idle_ttl_secs < i64::try_from(grace_secs).unwrap_or(i64::MAX) {
        return Err(format!(
            "CODING_AGENT_IDLE_TTL_SECS ({idle_ttl_secs}) must be 0 (suspension off) or at least \
             SESSION_IDLE_GRACE_SECS ({grace_secs}): the grace window is measured inside the idle TTL"
        ));
    }
    Ok(())
}

/// A turn renews its lease every `heartbeat_secs`; at least two renewals must fit in one lease
/// so a single late or lost heartbeat does not let a live turn's lock lapse mid-turn.
fn validate_heartbeat_within_lease(lease_secs: i64, heartbeat_secs: u64) -> Result<(), String> {
    let heartbeat = i64::try_from(heartbeat_secs).unwrap_or(i64::MAX);
    if lease_secs <= 0 || heartbeat == 0 || heartbeat.saturating_mul(2) > lease_secs {
        return Err(format!(
            "CODING_AGENT_LEASE_HEARTBEAT_SECS ({heartbeat_secs}) must be positive and at most \
             half of CODING_AGENT_TURN_LEASE_SECS ({lease_secs}), which must be positive: a live \
             turn must renew its lease at least twice per lease"
        ));
    }
    Ok(())
}

/// The lifetime of the session token a coding turn carries (`sessions::identity::
/// SESSION_TOKEN_TTL_SECS`, kept equal by hand: this crate sits below the server).
pub const SESSION_TOKEN_TTL_SECS: u64 = 3600;

/// A turn waiting on a human approval waits up to `AGENT_CALL_TIMEOUT_SECS`, holding the session
/// token it was started with; a longer wait would outlive the token.
fn validate_call_timeout_within_session_token(call_timeout_secs: u64) -> Result<(), String> {
    if call_timeout_secs > SESSION_TOKEN_TTL_SECS {
        return Err(format!(
            "AGENT_CALL_TIMEOUT_SECS ({call_timeout_secs}) must not exceed the session token \
             lifetime ({SESSION_TOKEN_TTL_SECS}s, SESSION_TOKEN_TTL_SECS): a turn waiting on an \
             approval would outlive its token"
        ));
    }
    Ok(())
}

/// Default `AGENT_PARK_IDLE_SECS`: an agent nobody has called for an hour is parked.
const DEFAULT_AGENT_PARK_IDLE_SECS: u64 = 3600;

/// How often at most a served request writes its agent's `last_request_at`, in seconds: a request
/// within this long of the last stamp leaves it unchanged. The one value both the stamp and the
/// park window's floor read.
pub const LAST_REQUEST_STAMP_THROTTLE_SECS: u64 = 60;

/// The shortest park window that cannot park an agent under a call another replica is serving:
/// that call began at most the stamp throttle after the `last_request_at` the reaper reads, runs
/// for at most the call timeout, and the reaper looks once per interval.
fn park_idle_floor_secs(call_timeout_secs: u64, reaper_interval_secs: u64) -> u64 {
    call_timeout_secs
        .saturating_add(LAST_REQUEST_STAMP_THROTTLE_SECS)
        .saturating_add(reaper_interval_secs)
}

/// `AGENT_PARK_IDLE_SECS` as set, or, when unset, the default raised to `floor_secs`, so a long
/// call timeout alone never fails startup. A set value is kept as written, for
/// [`validate_park_idle_covers_a_call`] to judge.
fn parse_agent_park_idle_secs(raw: Option<&str>, floor_secs: u64) -> Result<u64, String> {
    match raw {
        None => Ok(DEFAULT_AGENT_PARK_IDLE_SECS.max(floor_secs)),
        Some(raw) => raw.trim().parse().map_err(|_| {
            format!("AGENT_PARK_IDLE_SECS must be a whole number of seconds (got {raw:?})")
        }),
    }
}

/// `0` (parking off) always passes; any other window must cover `floor_secs`.
fn validate_park_idle_covers_a_call(park_idle_secs: u64, floor_secs: u64) -> Result<(), String> {
    if park_idle_secs != 0 && park_idle_secs < floor_secs {
        return Err(format!(
            "AGENT_PARK_IDLE_SECS ({park_idle_secs}) must be at least AGENT_CALL_TIMEOUT_SECS plus \
             the {LAST_REQUEST_STAMP_THROTTLE_SECS}s last-request stamp throttle plus one \
             SESSION_REAPER_INTERVAL_SECS ({floor_secs}), or 0 to turn agent parking off: a call on \
             another replica is seen only through the agent's last request time, so a shorter \
             window could park an agent under a call still inside its timeout"
        ));
    }
    Ok(())
}

/// Each materializer pass searches `[cursor − overlap, cursor − overlap + window]`
/// and moves the cursor to the window's end, so a window no longer than the
/// overlap never moves the cursor past where it was.
fn validate_trace_usage_window(overlap_secs: u64, max_window_secs: u64) -> Result<(), String> {
    if max_window_secs <= overlap_secs {
        return Err(format!(
            "TRACE_USAGE_MAX_WINDOW_SECS ({max_window_secs}) must exceed TRACE_USAGE_OVERLAP_SECS \
             ({overlap_secs}): the excess is how far the materializer advances per pass"
        ));
    }
    Ok(())
}

/// Tiering deletes rows from Postgres once they are in the cold store, so the
/// cold store must be a location the operator chose. The `./data/cold` default
/// is fine for reads and manual runs, but with the daily job on it would put the
/// only copy on whichever replica holds the lock, on disk that may not survive a
/// restart and that other replicas cannot read.
fn validate_tiering(enabled: bool, cold_path: Option<&str>) -> Result<(), String> {
    if enabled && cold_path.is_none_or(|p| p.trim().is_empty()) {
        return Err("NASIKO_TIERING_ENABLED=true requires NASIKO_COLD_PATH \
                    (s3://<bucket>/<prefix>, gs://<bucket>/<prefix> or a durable shared directory)"
            .into());
    }
    Ok(())
}

fn validate_secrets_key_format(key: &str) -> Result<(), String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(key)
        .map_err(|e| format!("SECRETS_ENCRYPTION_KEY is not valid base64: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!(
            "SECRETS_ENCRYPTION_KEY must decode to exactly 32 bytes, got {} — expected base64(32 random bytes)",
            bytes.len()
        ));
    }
    Ok(())
}

/// `AGENT_IMAGE_REGISTRY_ALIASES`: comma-separated, trimmed, empty entries and
/// the registry's own host dropped (an image naming it is already this
/// registry's, and rehosting it onto itself would be a pointless rewrite).
fn registry_aliases(raw: &str, registry: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|alias| !alias.is_empty() && *alias != registry.trim())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod registry_alias_tests {
    use super::registry_aliases;

    #[test]
    fn aliases_drop_blanks_and_the_registry_itself() {
        assert_eq!(
            registry_aliases(
                " free.old.example , ,free.app.nasiko.com",
                "free.app.nasiko.com"
            ),
            vec!["free.old.example"]
        );
        assert!(registry_aliases("", "free.app.nasiko.com").is_empty());
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn tiering_requires_an_explicit_cold_path() {
        assert!(super::validate_tiering(true, None).is_err());
        assert!(super::validate_tiering(true, Some("  ")).is_err());
        assert!(super::validate_tiering(true, Some("s3://b/cold")).is_ok());
        assert!(super::validate_tiering(true, Some("gs://b/cold")).is_ok());
        assert!(super::validate_tiering(false, None).is_ok());
    }

    #[test]
    fn trace_usage_window_must_exceed_the_overlap() {
        assert!(super::validate_trace_usage_window(180, 600).is_ok());
        let err = super::validate_trace_usage_window(600, 600).unwrap_err();
        assert!(err.contains("TRACE_USAGE_MAX_WINDOW_SECS"), "{err}");
        assert!(super::validate_trace_usage_window(600, 300).is_err());
    }

    #[test]
    fn origin_of_keeps_scheme_and_host_only() {
        assert_eq!(
            super::origin_of("https://acme.app.nasiko.com/app?x=1"),
            Some("https://acme.app.nasiko.com".to_string())
        );
        assert_eq!(super::origin_of("acme.app.nasiko.com"), None);
        assert_eq!(super::origin_of("https:///app"), None);
    }

    use super::*;

    #[test]
    fn an_unset_startup_budget_is_the_default_silently() {
        assert_eq!(parse_agent_startup_budget(None), (120, None));
        assert_eq!(parse_agent_startup_budget(Some(" 300 ")), (300, None));
    }

    #[test]
    fn an_out_of_range_startup_budget_is_clamped_with_a_warning() {
        let (low, low_note) = parse_agent_startup_budget(Some("0"));
        assert_eq!((low, low_note.is_some()), (10, true));
        let (high, high_note) = parse_agent_startup_budget(Some("99999"));
        assert_eq!((high, high_note.is_some()), (1800, true));
    }

    #[test]
    fn a_malformed_startup_budget_is_the_default_with_a_warning() {
        let (secs, note) = parse_agent_startup_budget(Some("two minutes"));
        assert_eq!((secs, note.is_some()), (120, true));
    }

    #[test]
    fn the_wake_budget_is_the_startup_budget_when_it_fits_in_a_call() {
        assert_eq!(wake_budget_secs(120, 600), 120);
        assert_eq!(wake_budget_secs(599, 600), 599);
    }

    #[test]
    fn the_wake_budget_stays_under_the_call_budget() {
        assert_eq!(wake_budget_secs(600, 600), 599);
        assert_eq!(wake_budget_secs(1800, 600), 599);
    }

    #[test]
    fn the_wake_budget_is_never_zero() {
        assert_eq!(wake_budget_secs(0, 600), 1);
        assert_eq!(wake_budget_secs(120, 1), 1);
        assert_eq!(wake_budget_secs(120, 0), 1);
    }

    #[test]
    fn sandbox_provider_parses_each_value_case_insensitively() {
        for (raw, want) in [
            ("docker", SandboxProviderSetting::Docker),
            ("Pod", SandboxProviderSetting::Pod),
            (" Shared-Pod ", SandboxProviderSetting::SharedPod),
            ("harness-agent", SandboxProviderSetting::HarnessAgent),
        ] {
            assert_eq!(raw.parse::<SandboxProviderSetting>(), Ok(want));
        }
    }

    #[test]
    fn unknown_sandbox_provider_names_key_and_allowed_values() {
        let err = "firecracker".parse::<SandboxProviderSetting>().unwrap_err();
        assert!(err.contains("SANDBOX_PROVIDER"), "{err}");
        assert!(
            err.contains("docker, pod, shared-pod, harness-agent"),
            "{err}"
        );
    }

    #[test]
    fn build_sandbox_smoke_parses_off_and_runsc_and_rejects_the_rest() {
        assert_eq!("off".parse(), Ok(BuildSandboxSmoke::Off));
        assert_eq!(" RunSC ".parse(), Ok(BuildSandboxSmoke::Runsc));
        let err = "gvisor".parse::<BuildSandboxSmoke>().unwrap_err();
        assert!(err.contains("BUILD_SANDBOX_SMOKE"), "{err}");
        assert!(err.contains("off, runsc"), "{err}");
    }

    #[test]
    fn idle_ttl_below_grace_fails_with_explanation() {
        let err = validate_idle_ttl_covers_grace(60, 120).unwrap_err();
        assert!(err.contains("inside the idle TTL"), "{err}");
    }

    #[test]
    fn idle_ttl_zero_or_at_least_grace_passes() {
        assert!(validate_idle_ttl_covers_grace(0, 120).is_ok());
        assert!(validate_idle_ttl_covers_grace(120, 120).is_ok());
        assert!(validate_idle_ttl_covers_grace(900, 120).is_ok());
    }

    #[test]
    fn the_park_window_floor_is_a_call_plus_the_stamp_throttle_plus_a_reaper_interval() {
        assert_eq!(LAST_REQUEST_STAMP_THROTTLE_SECS, 60);
        assert_eq!(park_idle_floor_secs(600, 30), 690);
        assert_eq!(park_idle_floor_secs(u64::MAX, 30), u64::MAX);
    }

    #[test]
    fn an_unset_park_window_is_an_hour_raised_to_the_floor() {
        assert_eq!(parse_agent_park_idle_secs(None, 690), Ok(3600));
        assert_eq!(parse_agent_park_idle_secs(None, 3690), Ok(3690));
    }

    #[test]
    fn a_set_park_window_is_kept_as_written() {
        assert_eq!(parse_agent_park_idle_secs(Some(" 900 "), 690), Ok(900));
        assert_eq!(parse_agent_park_idle_secs(Some("60"), 690), Ok(60));
        for bad in ["an hour", "-1", "1.5"] {
            let err = parse_agent_park_idle_secs(Some(bad), 690).unwrap_err();
            assert!(err.contains("AGENT_PARK_IDLE_SECS"), "{err}");
        }
    }

    #[test]
    fn a_park_window_below_the_floor_fails_startup() {
        assert!(
            validate_park_idle_covers_a_call(0, 690).is_ok(),
            "0 turns parking off"
        );
        let err = validate_park_idle_covers_a_call(689, 690).unwrap_err();
        assert!(err.contains("AGENT_PARK_IDLE_SECS (689)"), "{err}");
        assert!(err.contains("AGENT_CALL_TIMEOUT_SECS"), "{err}");
        assert!(err.contains("60s last-request stamp throttle"), "{err}");
        assert!(err.contains("SESSION_REAPER_INTERVAL_SECS"), "{err}");
        assert!(validate_park_idle_covers_a_call(690, 690).is_ok());
        assert!(validate_park_idle_covers_a_call(3600, 690).is_ok());
    }

    #[test]
    fn valid_32_byte_base64_key_passes() {
        assert!(
            validate_secrets_key_format("QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=").is_ok()
        );
    }

    #[test]
    fn raw_32_char_alphanumeric_key_fails() {
        // The original bug: 32 raw characters decode to only 24 bytes.
        assert!(validate_secrets_key_format("dev-only-change-in-prod-32chars!!").is_err());
    }

    #[test]
    fn wrong_byte_length_after_decode_fails() {
        use base64::Engine;
        let sixteen_bytes = base64::engine::general_purpose::STANDARD.encode([0u8; 16]);
        assert!(validate_secrets_key_format(&sixteen_bytes).is_err());
    }

    #[test]
    fn invalid_base64_fails() {
        assert!(validate_secrets_key_format("not base64 at all!!!").is_err());
    }

    #[test]
    fn base_url_written_with_v1_is_stripped() {
        // The cp.app.nasiko.com form — appending `/v1/audio/transcriptions` to the
        // raw value produced `.../v1/v1/audio/transcriptions` and 404'd.
        assert_eq!(
            openai_base_url_without_v1("https://api.openai.com/v1"),
            "https://api.openai.com"
        );
        assert_eq!(
            openai_base_url_without_v1("https://api.openai.com/v1/"),
            "https://api.openai.com"
        );
    }

    #[test]
    fn base_url_written_without_v1_is_unchanged() {
        assert_eq!(
            openai_base_url_without_v1("https://api.deepseek.com"),
            "https://api.deepseek.com"
        );
        assert_eq!(
            openai_base_url_without_v1("http://localhost:11434/"),
            "http://localhost:11434"
        );
    }

    #[test]
    fn only_a_trailing_v1_segment_is_stripped() {
        // A host or path that merely contains "v1" must survive intact.
        assert_eq!(
            openai_base_url_without_v1("https://v1.example.com"),
            "https://v1.example.com"
        );
        assert_eq!(
            openai_base_url_without_v1("https://example.com/openai/v1/proxy"),
            "https://example.com/openai/v1/proxy"
        );
    }

    #[test]
    fn dedicated_identity_key_wins_and_is_trimmed() {
        assert_eq!(
            derive_identity_signing_key(Some("  dedicated-key  "), "jwt-secret"),
            "dedicated-key"
        );
    }

    #[test]
    fn no_dedicated_key_falls_back_to_prefixed_trimmed_jwt_secret() {
        assert_eq!(
            derive_identity_signing_key(None, "  jwt-secret  "),
            format!("{IDENTITY_KEY_PREFIX}jwt-secret")
        );
    }

    #[test]
    fn blank_dedicated_key_falls_through_to_jwt_secret() {
        assert_eq!(
            derive_identity_signing_key(Some("   "), "jwt-secret"),
            format!("{IDENTITY_KEY_PREFIX}jwt-secret")
        );
    }

    #[test]
    fn both_blank_yields_empty_string() {
        assert_eq!(derive_identity_signing_key(Some("  "), "  "), "");
        assert_eq!(derive_identity_signing_key(None, ""), "");
    }
}

/// Whether `provider` selects an S3-compatible backend, and therefore whether
/// the `S3_*` credentials are required at startup.
///
/// Unknown values answer `true`: the authoritative parse lives with whichever
/// edition's composition root selects the backend, and it rejects them with a
/// proper message. Answering `false` here would pre-empt that with a confusing
/// missing-S3_SECRET_KEY error instead.
pub fn uses_s3_storage(provider: &str) -> bool {
    !matches!(
        provider.trim().to_ascii_lowercase().as_str(),
        "azure-blob" | "azure_blob" | "azure"
    )
}

#[cfg(test)]
mod storage_provider_tests {
    use super::uses_s3_storage;

    #[test]
    fn s3_credentials_are_required_only_for_an_s3_backend() {
        assert!(uses_s3_storage("s3"));
        assert!(uses_s3_storage(""));
        assert!(!uses_s3_storage("azure-blob"));
        assert!(!uses_s3_storage(" Azure-Blob "));
        // A typo must not silently waive the S3 requirement — the provider
        // parser is what reports it.
        assert!(uses_s3_storage("azureblob"));
    }
}
