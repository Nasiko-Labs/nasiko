//! Replay evals the server runs (`/api/evals/*`): it builds each task's image, tests it, and
//! replays it on other models in containers of its own. Off unless enabled, because it runs
//! repositories' code and model-written code on the server's container backend.

use nasiko_utils::env_parse;

/// Default eval jobs one server runs at once.
const DEFAULT_WORKER_CONCURRENCY: usize = 2;
/// Most eval jobs one server runs at once.
const MAX_WORKER_CONCURRENCY: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EvalsConfig {
    /// Whether this server builds datasets and runs evals. `EVAL_ENABLED`, default `false`.
    /// Off, the eval routes still list harnesses and sessions but refuse to start work.
    pub enabled: bool,
    /// The server's base URL as an eval container reaches it (without `/v1`), e.g.
    /// `http://nasiko-server:8080`, or `http://host.docker.internal:8082` for a server running
    /// on the Docker host. `EVAL_ROUTER_URL`. Unset, attempts cannot reach the model router and
    /// runs are refused.
    pub router_url: Option<String>,
    /// Eval jobs (a build, a gate run, an attempt) one server runs at once.
    /// `EVAL_WORKER_CONCURRENCY`, default 2, clamped to 1-32.
    pub worker_concurrency: usize,
}

impl EvalsConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: nasiko_utils::env_bool("EVAL_ENABLED", false),
            router_url: std::env::var("EVAL_ROUTER_URL")
                .ok()
                .map(|u| u.trim().trim_end_matches('/').to_string())
                .filter(|u| !u.is_empty()),
            worker_concurrency: env_parse("EVAL_WORKER_CONCURRENCY", DEFAULT_WORKER_CONCURRENCY)
                .clamp(1, MAX_WORKER_CONCURRENCY),
        }
    }
}
