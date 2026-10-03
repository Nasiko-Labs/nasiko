use std::{net::SocketAddr, sync::Arc, time::{Duration, Instant}};

use axum::{extract::State, http::StatusCode, routing::{get, post}, Json, Router};
use nasiko_llm_router::{config::GatewayConfig, routing::classifier::{ClassifyInput, Classification, FallbackClassifier, ModelClassifier, RegexClassifier, RequestClassifier}};
use serde::{Deserialize, Serialize};

#[derive(Clone)]
struct AppState { classifier: Arc<FallbackClassifier>, config: Arc<GatewayConfig> }

#[derive(Debug, Deserialize)]
struct ClassifyRequest { query: String, context: Option<String>, #[serde(default)] simulate_failure: bool }

#[derive(Debug, Serialize)]
struct ClassifyResponse { request_type: String, complexity: u8, confidence: f32, classifier_backend: String, fallback_used: bool, latency_us: u64 }

#[derive(Debug, Serialize)]
struct ErrorResponse { error: String }

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let config = Arc::new(GatewayConfig::from_env());
    let http = reqwest::Client::new();
    let primary: Arc<dyn RequestClassifier> = if config.classifier_backend.eq_ignore_ascii_case("model") && !config.classifier_endpoint.is_empty() {
        Arc::new(ModelClassifier { http, endpoint: config.classifier_endpoint.clone(), model: if config.classifier_model_path.is_empty() { "openai.gpt-5.6-luna".into() } else { config.classifier_model_path.clone() }, api_key: Some(config.classifier_api_key.clone()), timeout: Duration::from_millis(config.classifier_timeout_ms) })
    } else { Arc::new(RegexClassifier) };
    let state = AppState { classifier: Arc::new(FallbackClassifier { primary, fallback: RegexClassifier }), config };
    let app = Router::new().route("/health", get(health)).route("/api/classify", post(classify)).with_state(state);
    let bind: SocketAddr = std::env::var("CLASSIFIER_BIND").unwrap_or_else(|_| "127.0.0.1:8090".into()).parse().expect("CLASSIFIER_BIND must be host:port");
    let listener = tokio::net::TcpListener::bind(bind).await.expect("failed to bind classifier adapter");
    tracing::info!(%bind, "classifier adapter listening");
    axum::serve(listener, app).await.expect("classifier adapter failed");
}

async fn health() -> Json<serde_json::Value> { Json(serde_json::json!({"status":"ok"})) }

async fn classify(State(state): State<AppState>, Json(input): Json<ClassifyRequest>) -> Result<Json<ClassifyResponse>, (StatusCode, Json<ErrorResponse>)> {
    let started = Instant::now();
    let context = input.context.as_deref();
    let classify_input = ClassifyInput { query: &input.query, context };
    let primary_result = if input.simulate_failure { Err(nasiko_llm_router::routing::classifier::ClassifyError::Request("simulated classifier timeout".into())) } else { state.classifier.primary.classify(&classify_input).await };
    let (classification, fallback_used, backend) = match primary_result {
        Ok(value) if value.confidence >= state.config.classifier_min_confidence => (value, false, state.config.classifier_backend.clone()),
        _ => match state.classifier.fallback.classify(&classify_input).await {
            Ok(value) => (value, true, "regex".into()),
            Err(error) => return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: error.to_string() }))),
        },
    };
    Ok(Json(to_response(classification, backend, fallback_used, started.elapsed().as_micros() as u64)))
}

fn to_response(value: Classification, backend: String, fallback_used: bool, latency_us: u64) -> ClassifyResponse {
    ClassifyResponse { request_type: value.request_type.as_str().into(), complexity: value.complexity, confidence: value.confidence, classifier_backend: backend, fallback_used, latency_us }
}
