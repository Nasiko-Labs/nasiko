//! In-process (or standalone, via the `mock-llm` bin) stand-in for the LLM
//! endpoints hit during a benchmark run.
//!
//! The production `/api/orchestrator/a2a` route (`oss/server/src/router/a2a_dispatch.rs`
//! -> `nasiko_react_agent::Orchestrator`) — the one this harness actually
//! benchmarks — talks to `rig`'s OpenAI provider via
//! `rig::providers::openai::Client::from_url(api_key, base_url)`, which posts
//! to `{base_url}/chat/completions` (`rig-core-0.11.1/src/providers/openai/client.rs`).
//! Turn 0 (the only turn a no-tool-call mock ever reaches) uses rig's
//! *streaming* completion API (`react_loop.rs::run_stream_inner`), i.e. an
//! OpenAI-compatible SSE body of `data: {"choices":[{"delta":{"content":"..."}}]}`
//! lines — a flat JSON response is NOT sufficient there, unlike a plain
//! OpenAI-style chat completion. The request body's `"stream"` flag
//! distinguishes the two shapes this handler must support.
//!
//! Returning a response with no tool calls means the ReAct loop finishes in
//! one turn without ever invoking an agent tool — consistent with the fact
//! that agent execution is a separate, out-of-scope concern for this harness
//! (see `sim_agent`) — this benchmark measures orchestrator dispatch
//! control-plane overhead (DB queries, tool/preamble construction, one LLM
//! round trip), not multi-turn tool-calling latency.
//!
//! `ee/orchestrator`'s `LlmClient` (MAF background worker, idle unless flows
//! are queued) also posts to `{base_url}/chat/completions` — the flat JSON
//! branch here satisfies it too. `/v1/embeddings` is served for robustness
//! though the bench harness keeps the seeded agent count under
//! `router_shortlist_threshold` so `oss/orchestrator`'s Stage 1 embedding call
//! (unrelated to the endpoints actually benchmarked here) is skipped by
//! construction.

use axum::{
    Json, Router,
    body::Body,
    http::header,
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::{Value, json};
use tokio::task::JoinHandle;

pub struct MockLlmHandle {
    pub base_url: String,
    _task: JoinHandle<()>,
}

pub async fn spawn_mock_llm() -> MockLlmHandle {
    let app = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/chat/completions", post(chat_completions))
        .route("/v1/embeddings", post(embeddings));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock LLM");
    let addr = listener.local_addr().expect("mock LLM local_addr");

    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    MockLlmHandle { base_url: format!("http://{addr}"), _task: task }
}

const MOCK_TEXT: &str = "Mocked orchestrator response — no real LLM call was made.";

async fn chat_completions(Json(body): Json<Value>) -> Response {
    let wants_stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    if wants_stream {
        streaming_completion()
    } else {
        Json(flat_completion()).into_response()
    }
}

/// A single `data:` line carrying the whole reply as one `delta.content` chunk
/// is sufficient — `rig`'s SSE parser (`streaming.rs::send_compatible_streaming_request`)
/// terminates naturally once the response body ends, no `[DONE]` sentinel needed.
fn streaming_completion() -> Response {
    let content = serde_json::to_string(MOCK_TEXT).expect("serialize mock content");
    let body = format!("data: {{\"choices\":[{{\"delta\":{{\"content\":{content}}}}}]}}\n\n");
    Response::builder()
        .status(200)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(body))
        .expect("build mock streaming completion response")
}

/// Matches `rig::providers::openai::completion::CompletionResponse` (id,
/// object, created, model, choices[].index/message{role,content}/finish_reason,
/// usage{prompt_tokens,total_tokens}) — also satisfies `ee/orchestrator`'s
/// simpler `LlmClient`, which only reads `choices[0].message.content` and
/// `usage.total_tokens`.
fn flat_completion() -> Value {
    json!({
        "id": "mock-completion",
        "object": "chat.completion",
        "created": 0,
        "model": "mock-model",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": MOCK_TEXT },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 10, "total_tokens": 20 }
    })
}

async fn embeddings(Json(_body): Json<Value>) -> Json<Value> {
    Json(json!({
        "data": [{ "embedding": vec![0.01_f32; 8], "index": 0 }],
        "model": "mock-embedding",
        "usage": { "prompt_tokens": 1, "total_tokens": 1 }
    }))
}
