pub mod a2a_dispatch;

pub use a2a_dispatch::{a2a_dispatch_handler, a2a_upload_handler, router_stats_handler};
pub use nasiko_orchestrator::{AgentCardSummary, AgentSelector};

/// Mount all A2A dispatch routes on the server.
/// The engine implementation is resolved from AppState.routing_engine,
/// set at startup by each binary (OssRoutingEngine for OSS, EeRoutingEngine for EE).
///
/// `a2a_limiter` bounds LLM cost abuse — dispatch is the most expensive route
/// in the app; `/orchestrator/stats` (a cheap aggregate read) is not rate
/// limited. Must be layered so it runs after `require_auth`, whose Claims it reads.
pub fn router_routes(
    a2a_limiter: crate::rate_limit::RateLimiter,
) -> axum::Router<crate::state::AppState> {
    let dispatch_routes = axum::Router::new()
        .route(
            "/orchestrator/a2a",
            axum::routing::post(a2a_dispatch_handler),
        )
        .route(
            "/orchestrator/a2a/upload",
            axum::routing::post(a2a_upload_handler),
        )
        .layer(axum::middleware::from_fn_with_state(
            a2a_limiter,
            crate::rate_limit::limit_by_user,
        ));

    axum::Router::new().merge(dispatch_routes).route(
        "/orchestrator/stats",
        axum::routing::get(router_stats_handler),
    )
}
