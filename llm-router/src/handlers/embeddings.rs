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
use crate::resolver::{PgRegistry, RegistryStore, resolve};
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

    // `model` is discarded (C4) just like chat — resolved config is authoritative.
    let inbound = inbound_for(InboundFormat::OpenAi);
    let req = inbound.parse_embeddings(body)?;
    let resolved = resolve(store, &ctx.cache, &ctx.cfg, &agent_id, &owner_id).await?;

    // Ordered fallbacks (same rules as chat); usage records the effective provider/model.
    let started = Instant::now();
    let (resp, (provider, model)) =
        fallback::execute_embeddings(&ctx.http, &ctx.cfg, &resolved, &req).await?;
    let latency_ms = started.elapsed().as_millis() as i64;

    usage::spawn_log(
        ctx.db.clone(),
        UsageRecord {
            owner_id,
            agent_id,
            operation_type: "embedding",
            provider,
            model,
            usage: resp.usage.clone(),
            latency_ms,
            streaming: false,
            finish_reason: None,
        },
    );

    Ok(Json(inbound.render_embeddings(resp)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GatewayConfig;
    use crate::resolver::{ConfigCache, LLMConfig};
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
        ) -> Result<Option<Option<LLMConfig>>, sqlx::Error> {
            // defaults → openai / gpt-4o-mini (the configured default model)
            Ok(Some(None))
        }
        async fn fetch_user_secret(&self, _: Uuid, _: &str) -> Result<Option<String>, sqlx::Error> {
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
            tier_registry: Arc::new(crate::routing::StaticTierRegistry),
        }
    }

    fn auth_headers(token: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        h
    }

    async fn body_json(resp: Response) -> Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn openai_embeddings_end_to_end() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/embeddings")
            .match_body(mockito::Matcher::PartialJson(json!({ "model": "text-embedding-3-small" })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "object": "list",
                    "data": [{ "object": "embedding", "embedding": [0.1, 0.2, 0.3], "index": 0 }],
                    "model": "text-embedding-3-small",
                    "usage": { "prompt_tokens": 4, "total_tokens": 4 }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let ctx = ctx_with(server.url());
        let token = crate::auth::mint_agent_token(AGENT, OWNER, SECRET, 3600, Algorithm::HS256).unwrap();
        let body = json!({ "model": "irrelevant", "input": "hello" });
        let resp = embeddings_core(&ctx, &Store, &auth_headers(&token), body).await.unwrap();

        let v = body_json(resp).await;
        assert_eq!(v["object"], "list");
        assert_eq!(v["model"], "text-embedding-3-small");
        assert_eq!(v["data"][0]["embedding"][1], 0.2);
    }
}
