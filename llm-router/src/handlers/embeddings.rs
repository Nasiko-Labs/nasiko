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
use crate::providers::execute_embeddings_once;
use crate::resolver::{PgRegistry, RegistryStore, resolve_embeddings};
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

    // Embeddings speak the OpenAI surface only, and they resolve through their own
    // priority order — never the chat one. The chat resolver treats a configured agent's
    // config as final, which sent a *chat* model to `POST /embeddings` and 400'd for
    // every configured agent. See `resolve_embeddings` for the order and why it has no
    // fallback.
    let inbound = inbound_for(InboundFormat::OpenAi);
    let req = inbound.parse_embeddings(body)?;
    let resolved = resolve_embeddings(
        store,
        &ctx.cache,
        &ctx.cfg,
        &agent_id,
        &owner_id,
        req.model.as_deref(),
    )
    .await?;

    // Exactly one attempt. A failure here is the caller's answer — swapping in another
    // embedding model would return vectors in a different space, which is worse than an
    // error because it is silent.
    let started = Instant::now();
    let resp = execute_embeddings_once(&ctx.http, &ctx.cfg, &resolved, &req).await?;
    let latency_ms = started.elapsed().as_millis() as i64;
    let (provider, model) = (resolved.provider.clone(), resolved.model.clone());

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

    #[derive(Default)]
    struct Store {
        config: Option<crate::resolver::LLMConfig>,
        org_embedding_default: Option<crate::resolver::OrgEmbeddingDefault>,
    }

    /// An agent configured for *chat* against Anthropic — which has no embeddings API at
    /// all — with an embedding model of its own. This is the shape that was broken:
    /// embeddings resolved through the chat config and sent `claude-*` to an embeddings
    /// endpoint.
    fn anthropic_chat_config(embedding: Option<(&str, &str)>) -> crate::resolver::LLMConfig {
        crate::resolver::LLMConfig {
            provider: "anthropic".into(),
            model: Some("claude-sonnet-4-6".into()),
            fallback_models: vec!["anthropic/claude-haiku-4-5".into()],
            temperature: None,
            max_tokens: None,
            api_key_secret_name: None,
            pinned: false,
            pinned_model: None,
            tier1_model: None,
            tier2_model: None,
            tier3_model: None,
            embedding_provider: embedding.map(|(p, _)| p.to_string()),
            embedding_model: embedding.map(|(_, m)| m.to_string()),
            embedding_api_key_secret_name: None,
            embedding_model_override: false,
        }
    }

    #[async_trait]
    impl RegistryStore for Store {
        async fn fetch_llm_config(
            &self,
            _: Uuid,
        ) -> Result<Option<AgentConfigResult>, sqlx::Error> {
            Ok(Some(AgentConfigResult {
                config: self.config.clone(),
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
                user_message: None,
                agent_is_participant: true,
            }))
        }
        async fn fetch_custom_provider(
            &self,
            _: &str,
        ) -> Result<Option<crate::resolver::CustomProvider>, sqlx::Error> {
            Ok(None)
        }
        async fn fetch_org_embedding_default(
            &self,
        ) -> Result<Option<crate::resolver::OrgEmbeddingDefault>, sqlx::Error> {
            Ok(self.org_embedding_default.clone())
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
        let err = embeddings_core(&ctx, &Store::default(), &headers, body)
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
        let resp = embeddings_core(&ctx, &Store::default(), &auth_headers(&token), body)
            .await
            .unwrap();

        let v = body_json(resp).await;
        assert_eq!(v["object"], "list");
        assert_eq!(v["model"], "text-embedding-3-large");
        assert_eq!(v["data"][0]["embedding"][1], 0.2);
    }

    /// Build a mock that answers `POST /embeddings` for exactly one model, and fails the
    /// assertion if any other model is sent. `expect(1)` is the no-chaining check: a
    /// second attempt against the same endpoint would trip it.
    async fn openai_embeddings_mock(model: &str) -> (mockito::ServerGuard, mockito::Mock) {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/embeddings")
            .match_body(mockito::Matcher::PartialJson(json!({ "model": model })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "object": "list",
                    "data": [{ "object": "embedding", "embedding": [0.1, 0.2], "index": 0 }],
                    "model": model,
                    "usage": { "prompt_tokens": 4, "total_tokens": 4 }
                })
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;
        (server, mock)
    }

    #[tokio::test]
    async fn anthropic_chat_config_still_embeds_via_its_embedding_model() {
        // The regression this feature exists for. Before, this agent resolved through the
        // chat config and sent `claude-sonnet-4-6` to `POST /embeddings` — a guaranteed
        // 400, followed by chat-model "fallbacks" that also 400. Now the request's own
        // model is honored and the Anthropic chat config is simply not consulted.
        let (server, mock) = openai_embeddings_mock("text-embedding-3-large").await;
        let ctx = ctx_with(server.url());
        let store = Store {
            config: Some(anthropic_chat_config(None)),
            ..Default::default()
        };
        let token =
            crate::auth::mint_agent_token(AGENT, OWNER, SECRET, 3600, Algorithm::HS256).unwrap();
        let body = json!({ "model": "text-embedding-3-large", "input": "hello" });
        let resp = embeddings_core(&ctx, &store, &auth_headers(&token), body)
            .await
            .unwrap();
        assert_eq!(body_json(resp).await["model"], "text-embedding-3-large");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn config_embedding_model_is_used_when_the_request_names_none() {
        let (server, mock) = openai_embeddings_mock("text-embedding-3-small").await;
        let ctx = ctx_with(server.url());
        let store = Store {
            config: Some(anthropic_chat_config(Some((
                "openai",
                "text-embedding-3-small",
            )))),
            ..Default::default()
        };
        let token =
            crate::auth::mint_agent_token(AGENT, OWNER, SECRET, 3600, Algorithm::HS256).unwrap();
        // No `model` key at all — the raw-HTTP caller shape.
        let body = json!({ "input": "hello" });
        let resp = embeddings_core(&ctx, &store, &auth_headers(&token), body)
            .await
            .unwrap();
        assert_eq!(body_json(resp).await["model"], "text-embedding-3-small");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn org_default_is_used_when_request_and_config_are_silent() {
        let (server, mock) = openai_embeddings_mock("org-embedding-model").await;
        let ctx = ctx_with(server.url());
        let store = Store {
            config: Some(anthropic_chat_config(None)),
            org_embedding_default: Some(crate::resolver::OrgEmbeddingDefault {
                provider: "openai".into(),
                model: "org-embedding-model".into(),
                api_key: Some("sk-org".into()),
            }),
        };
        let token =
            crate::auth::mint_agent_token(AGENT, OWNER, SECRET, 3600, Algorithm::HS256).unwrap();
        let resp = embeddings_core(
            &ctx,
            &store,
            &auth_headers(&token),
            json!({ "input": "hi" }),
        )
        .await
        .unwrap();
        assert_eq!(body_json(resp).await["model"], "org-embedding-model");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn an_upstream_failure_is_final_and_tries_nothing_else() {
        // The whole point of §2.2: the chosen model failing must not cause a second call
        // with a different model. `expect(1)` on the mock is the assertion — a retry or a
        // fallback would make it two.
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/embeddings")
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(json!({ "error": { "message": "unknown model" } }).to_string())
            .expect(1)
            .create_async()
            .await;
        let ctx = ctx_with(server.url());
        let store = Store {
            // Both a config embedding model and an org default are available. Neither may
            // be reached, because the request's model was chosen and it failed.
            config: Some(anthropic_chat_config(Some((
                "openai",
                "text-embedding-3-small",
            )))),
            org_embedding_default: Some(crate::resolver::OrgEmbeddingDefault {
                provider: "openai".into(),
                model: "org-embedding-model".into(),
                api_key: Some("sk-org".into()),
            }),
        };
        let token =
            crate::auth::mint_agent_token(AGENT, OWNER, SECRET, 3600, Algorithm::HS256).unwrap();
        let body = json!({ "model": "text-embedding-3-large", "input": "hello" });
        let err = embeddings_core(&ctx, &store, &auth_headers(&token), body)
            .await
            .unwrap_err();
        assert!(matches!(err, GatewayError::Upstream(_)));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn no_model_anywhere_is_a_bad_request() {
        let ctx = ctx_with("http://unused".into());
        let store = Store {
            config: Some(anthropic_chat_config(None)),
            ..Default::default()
        };
        let token =
            crate::auth::mint_agent_token(AGENT, OWNER, SECRET, 3600, Algorithm::HS256).unwrap();
        let err = embeddings_core(
            &ctx,
            &store,
            &auth_headers(&token),
            json!({ "input": "hi" }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, GatewayError::BadRequest(_)));
    }
}
