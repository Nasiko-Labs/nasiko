//! `POST /v1/embeddings` — verify JWT → resolve config → call provider → render
//! OpenAI-shaped embeddings, with a fire-and-forget usage row. Always non-streaming.

use std::time::Instant;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use axum::response::{IntoResponse, Response};
use serde_json::Value;

use crate::LlmRouterCtx;
use crate::auth::verify_agent_jwt;
use crate::error::GatewayError;
use crate::inbound::{InboundFormat, inbound_for};
use crate::providers::fallback;
use crate::resolver::{PgRegistry, RegistryStore, RequestHint, resolve};
use crate::routing::boundary::{TRACEPARENT_HEADER, parse_flow_id};
use crate::usage::{self, UsageRecord};

/// Axum handler. Builds the Postgres-backed store and delegates to [`embeddings_core`].
pub async fn embeddings(
    State(ctx): State<LlmRouterCtx>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, GatewayError> {
    let store = PgRegistry::new(ctx.db.clone());
    embeddings_core(&ctx, &store, &headers, body).await
}

async fn embeddings_core(
    ctx: &LlmRouterCtx,
    store: &dyn RegistryStore,
    headers: &HeaderMap,
    body: Value,
) -> Result<Response, GatewayError> {
    let authz = headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok());
    let (agent_id, owner_id) = verify_agent_jwt(authz, &ctx.cfg)?;

    // Strict flow attribution — same rule as chat (previously embeddings
    // attempted none at all): the traceparent must name a live flow this agent
    // participates in, or the call is refused. Accepted consequence: an agent
    // cannot embed outside a user flow (startup/ingest-time indexing is not
    // currently supported) — see oss/docs/TOKEN_ATTRIBUTION.md.
    let raw_traceparent = headers
        .get(TRACEPARENT_HEADER)
        .and_then(|v| v.to_str().ok());
    let attribution = crate::routing::attribution::resolve(
        store,
        &agent_id,
        raw_traceparent.and_then(parse_flow_id),
        ctx.cfg.attribution_window_secs as i64,
    )
    .await
    .map_err(|denied| {
        GatewayError::Forbidden(format!(
            "{denied} (received traceparent: {})",
            raw_traceparent.unwrap_or("<none>")
        ))
    })?;

    // Embeddings speak the OpenAI surface only. A configured agent ignores the request
    // model; a no-llm_config agent is routed to what it asked for (openai + request model),
    // with the platform default as the last-resort safety net.
    let inbound = inbound_for(InboundFormat::OpenAi);
    let req = inbound.parse_embeddings(body)?;
    let hint = RequestHint {
        provider: Some(InboundFormat::OpenAi.provider_label()),
        model: req.model.as_deref(),
    };
    let resolved = resolve(store, &ctx.cache, &ctx.cfg, &agent_id, &owner_id, hint).await?;

    // Ordered fallbacks (same rules as chat); usage records the effective provider/model.
    let started = Instant::now();
    let (resp, (provider, model)) =
        fallback::execute_embeddings(&ctx.http, &ctx.cfg, &resolved, &req).await?;
    let latency_ms = started.elapsed().as_millis() as i64;

    usage::spawn_log(
        ctx.db.clone(),
        ctx.pricing.clone(),
        UsageRecord {
            // Billed to the flow's caller (strict attribution guarantees a
            // flow); the JWT's owner is only the no-user safety net.
            owner_id: attribution
                .user_id
                .map(|u| u.to_string())
                .unwrap_or(owner_id),
            agent_id,
            operation_type: "embedding",
            provider,
            model,
            usage: resp.usage.clone(),
            cached_tokens: None,
            reasoning_tokens: None,
            latency_ms,
            streaming: false,
            finish_reason: None,
            flow_id: Some(attribution.flow_id.clone()),
            attribution_source: Some(attribution.source),
            // Never compressed: this surface does not go through `chat_core`.
            compress_metadata: None,
            // Embeddings are not chat completions; IP-2 never runs here.
            brevity_metadata: None,
            // Nothing was compressed, so there is nothing to credit to a savings layer.
            compress_bytes: None,
            request_bytes: None,
            platform_paid: resolved.platform_paid,
        },
    );

    Ok(Json(inbound.render_embeddings(resp)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GatewayConfig;
    use crate::resolver::{AgentConfigResult, ConfigCache};
    use async_trait::async_trait;
    use jsonwebtoken::Algorithm;
    use serde_json::json;
    use sqlx::PgPool;
    use std::sync::Arc;
    use std::time::Duration;
    use uuid::Uuid;

    const AGENT: &str = "11111111-1111-1111-1111-111111111111";
    const OWNER: &str = "22222222-2222-2222-2222-222222222222";
    const SECRET: &str = "gateway-secret";

    struct Store;
    #[async_trait]
    impl RegistryStore for Store {
        async fn fetch_llm_config(
            &self,
            _: Uuid,
        ) -> Result<Option<AgentConfigResult>, sqlx::Error> {
            // No llm_config ⇒ passthrough: provider openai (the embeddings surface) + the
            // request's own model, defaults only as the safety net.
            Ok(Some(AgentConfigResult {
                config: None,
                agent_pinned_model: None,
                is_coding_agent: false,
                compress_enabled: false,
            }))
        }
        async fn fetch_user_secret(&self, _: Uuid, _: &str) -> Result<Option<String>, sqlx::Error> {
            Ok(None)
        }
        async fn fetch_live_flow(
            &self,
            _: &str,
            _: Uuid,
            _: i64,
        ) -> Result<Option<crate::routing::attribution::LiveFlow>, sqlx::Error> {
            Ok(Some(crate::routing::attribution::LiveFlow {
                user_id: None,
                context_id: None,
                mode: None,
                agent_is_participant: true,
            }))
        }
        async fn fetch_custom_provider(
            &self,
            _: &str,
        ) -> Result<Option<crate::resolver::CustomProvider>, sqlx::Error> {
            Ok(None)
        }
    }

    fn ctx_with(base: String) -> LlmRouterCtx {
        let cfg = GatewayConfig {
            agent_jwt_secret: SECRET.into(),
            openai_api_base: base,
            platform_openai_api_key: "sk-platform".into(),
            default_provider: "openai".into(),
            default_model: "text-embedding-3-small".into(),
            ..Default::default()
        };
        LlmRouterCtx {
            db: PgPool::connect_lazy("postgres://u:p@127.0.0.1:5999/none").unwrap(),
            http: reqwest::Client::new(),
            cfg: Arc::new(cfg),
            cache: Arc::new(ConfigCache::new(Duration::from_secs(30))),
            router_cache: Arc::new(crate::routing::NoopCache),
            tier_registry: Arc::new(crate::routing::registry::test_support::StubRegistry),
            cell_store: Arc::new(crate::routing::InMemoryCellStore::new()),
            salience_gate: Arc::new(crate::routing::AllowAllGate),
            request_classifier: Arc::new(
                crate::routing::request_classifier::RegexRequestClassifier,
            ),
            pricing: Arc::new(nasiko_pricing::PricingEngine::new(
                PgPool::connect_lazy("postgres://u:p@127.0.0.1:5999/none").unwrap(),
            )),
        }
    }

    fn auth_headers(token: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        // Strict attribution: embeddings must carry trace context too.
        h.insert(
            TRACEPARENT_HEADER,
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
                .parse()
                .unwrap(),
        );
        h
    }

    #[tokio::test]
    async fn missing_traceparent_is_403() {
        // Embeddings previously attempted no attribution at all — under strict
        // enforcement they are refused without a resolvable flow.
        let ctx = ctx_with("http://unused".into());
        let token =
            crate::auth::mint_agent_token(AGENT, OWNER, SECRET, 3600, Algorithm::HS256).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        let body = json!({ "model": "text-embedding-3-large", "input": "hello" });
        let err = embeddings_core(&ctx, &Store, &headers, body)
            .await
            .unwrap_err();
        assert!(matches!(err, GatewayError::Forbidden(_)));
    }

    async fn body_json(resp: Response) -> Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn openai_embeddings_end_to_end_honors_request_model() {
        // No llm_config ⇒ the request's own model ("text-embedding-3-large") is honored,
        // not the platform default ("text-embedding-3-small").
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/embeddings")
            .match_body(mockito::Matcher::PartialJson(
                json!({ "model": "text-embedding-3-large" }),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "object": "list",
                    "data": [{ "object": "embedding", "embedding": [0.1, 0.2, 0.3], "index": 0 }],
                    "model": "text-embedding-3-large",
                    "usage": { "prompt_tokens": 4, "total_tokens": 4 }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let ctx = ctx_with(server.url());
        let token =
            crate::auth::mint_agent_token(AGENT, OWNER, SECRET, 3600, Algorithm::HS256).unwrap();
        let body = json!({ "model": "text-embedding-3-large", "input": "hello" });
        let resp = embeddings_core(&ctx, &Store, &auth_headers(&token), body)
            .await
            .unwrap();

        let v = body_json(resp).await;
        assert_eq!(v["object"], "list");
        assert_eq!(v["model"], "text-embedding-3-large");
        assert_eq!(v["data"][0]["embedding"][1], 0.2);
    }
}
