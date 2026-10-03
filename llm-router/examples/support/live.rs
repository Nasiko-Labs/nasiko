//! Live mode: POST each case's recorded request to one OpenAI-compatible endpoint and run the
//! reply through the router's own `finalize`, so a live result is "successful" under exactly
//! the production rules (completion state, representation, schema validation).
//!
//! Conservative by design for a shared key: sequential, bounded timeout, at most two retries
//! and only on 429/5xx (honouring `Retry-After`), no retries after 401/403, credentials sent
//! only to the configured endpoint (redirects are not followed).

use std::time::Duration;

use nasiko_llm_router::compact_tools::{self, Compiled, Finalized};
use nasiko_llm_router::ir::ChatResponse;
use nasiko_llm_router::providers::dialect::ProviderDialect;
use nasiko_tool_compact::ToolDef;
use serde_json::{Value, json};

use super::eval::{BuiltRequest, DEFAULT_MAX_OUTPUT_TOKENS};

pub const DEFAULT_TIMEOUT_SECS: u64 = 60;
const MAX_RETRIES: u32 = 2;
const MAX_RETRY_AFTER_SECS: u64 = 30;
const MAX_MESSAGE_CHARS: usize = 200;

#[derive(Debug, Clone)]
pub struct LiveConfig {
    pub base_url: String,
    pub model: String,
    /// Sent as `Authorization: Bearer …` only when present and non-empty.
    pub api_key: Option<String>,
    pub max_output_tokens: u32,
    pub timeout: Duration,
}

impl LiveConfig {
    /// Live mode is on when both `PROVIDER_BASE_URL` and `MODEL` are set. The key comes from
    /// `PROVIDER_API_KEY`, or the documented alias `BEDROCK_API_KEY`; a keyless proxy needs none.
    pub fn from_env() -> Option<Self> {
        let base_url = std::env::var("PROVIDER_BASE_URL")
            .ok()
            .filter(|s| !s.is_empty())?;
        let model = std::env::var("MODEL").ok().filter(|s| !s.is_empty())?;
        let api_key = std::env::var("PROVIDER_API_KEY")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                std::env::var("BEDROCK_API_KEY")
                    .ok()
                    .filter(|s| !s.is_empty())
            });
        let max_output_tokens = std::env::var("MAX_OUTPUT_TOKENS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS);
        let timeout = Duration::from_secs(
            std::env::var("LIVE_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_TIMEOUT_SECS),
        );
        Some(Self {
            base_url,
            model,
            api_key,
            max_output_tokens,
            timeout,
        })
    }
}

/// `{base}/chat/completions`, preserving a `/v1` suffix and never doubling the path.
pub fn chat_completions_url(base: &str) -> String {
    ProviderDialect::OpenAi.chat_url(base, "")
}

pub fn live_client(timeout: Duration) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

#[derive(Debug, Clone, PartialEq)]
pub enum LiveOutcome {
    /// HTTP 2xx with a JSON body.
    Completed(Value),
    /// A non-2xx status after retries, or a transport failure.
    Failed {
        status: Option<u16>,
        message: String,
    },
    /// 401/403: stop immediately and skip the rest of the run.
    AuthFailed { status: u16, message: String },
}

/// Remove control characters, redact the key, and cap the length.
pub fn sanitize_message(raw: &str, api_key: Option<&str>) -> String {
    let mut text = raw.to_owned();
    if let Some(key) = api_key.filter(|k| !k.is_empty()) {
        text = text.replace(key, "[redacted]");
    }
    let cleaned: String = text.chars().filter(|c| !c.is_control()).collect();
    cleaned.chars().take(MAX_MESSAGE_CHARS).collect()
}

fn retry_after(resp: &reqwest::Response, attempt: u32) -> Duration {
    let header = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    match header {
        Some(secs) => Duration::from_secs(secs.min(MAX_RETRY_AFTER_SECS)),
        None => Duration::from_secs(u64::from(attempt) + 1),
    }
}

/// POST `body` exactly as given. Retries only on 429 and 5xx.
pub async fn call_live(http: &reqwest::Client, cfg: &LiveConfig, body: &Value) -> LiveOutcome {
    let url = chat_completions_url(&cfg.base_url);
    let mut attempt = 0;
    loop {
        let mut request = http.post(&url).json(body);
        if let Some(key) = cfg.api_key.as_deref().filter(|k| !k.is_empty()) {
            request = request.bearer_auth(key);
        }
        match request.send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    return match resp.json::<Value>().await {
                        Ok(v) => LiveOutcome::Completed(v),
                        Err(e) => LiveOutcome::Failed {
                            status: Some(status.as_u16()),
                            message: sanitize_message(
                                &format!("invalid JSON body: {e}"),
                                cfg.api_key.as_deref(),
                            ),
                        },
                    };
                }
                let code = status.as_u16();
                if code == 401 || code == 403 {
                    let text = resp.text().await.unwrap_or_default();
                    return LiveOutcome::AuthFailed {
                        status: code,
                        message: sanitize_message(&text, cfg.api_key.as_deref()),
                    };
                }
                let retryable = code == 429 || status.is_server_error();
                if retryable && attempt < MAX_RETRIES {
                    let wait = retry_after(&resp, attempt);
                    attempt += 1;
                    tokio::time::sleep(wait).await;
                    continue;
                }
                let text = resp.text().await.unwrap_or_default();
                return LiveOutcome::Failed {
                    status: Some(code),
                    message: sanitize_message(&text, cfg.api_key.as_deref()),
                };
            }
            Err(e) => {
                return LiveOutcome::Failed {
                    status: None,
                    message: sanitize_message(
                        &transport_message(&e, cfg.timeout),
                        cfg.api_key.as_deref(),
                    ),
                };
            }
        }
    }
}

/// Name the transport failure precisely: reqwest's `Display` says only "error sending request",
/// which hides the one distinction a reader needs (the model exceeded our timeout versus the
/// endpoint being unreachable).
fn transport_message(e: &reqwest::Error, timeout: Duration) -> String {
    if e.is_timeout() {
        format!(
            "timeout after {}s with no complete response",
            timeout.as_secs()
        )
    } else if e.is_connect() {
        format!("connection failed: {e}")
    } else {
        e.to_string()
    }
}

/// What a live reply contributes to the case line.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveResult {
    /// The model's text, or its native `tool_calls` serialized, verbatim.
    pub raw_output: Value,
    /// `{"calls": […]}` only when `finalize` accepted the reply; otherwise `{"error": kind}`.
    pub live_calls: Value,
    pub finalized: Result<Finalized, String>,
}

/// Run a reply through the production finalization rules.
///
/// Compacted requests use their own `Compiled`; a bypassed (native) request is judged by the
/// same native-call branch, against the same original schemas, with no catalog prompt.
pub fn judge_reply(reply: &Value, built: &BuiltRequest) -> LiveResult {
    let raw_output = extract_raw_output(reply);
    let resp: ChatResponse = match serde_json::from_value(reply.clone()) {
        Ok(r) => r,
        Err(e) => {
            return LiveResult {
                raw_output,
                live_calls: json!({"error": "unparseable_response"}),
                finalized: Err(format!("response does not parse: {e}")),
            };
        }
    };
    let compiled = built
        .compiled
        .clone()
        .unwrap_or_else(|| native_compiled(&built.tool_defs));
    match compact_tools::finalize(&resp, &compiled) {
        Ok(f) => LiveResult {
            raw_output,
            live_calls: json!({"calls": f.calls.iter().map(|c| json!({"name": c.name, "arguments": c.arguments})).collect::<Vec<_>>()}),
            finalized: Ok(f),
        },
        Err(e) => LiveResult {
            raw_output,
            live_calls: json!({"error": e.kind}),
            finalized: Err(e.kind),
        },
    }
}

/// A `Compiled` with no prompt, for judging native replies to bypassed requests. The tool
/// definitions are the originals; nothing about the schemas is relaxed.
fn native_compiled(tools: &[ToolDef]) -> Compiled {
    Compiled {
        tool_count: tools.len(),
        definitions_bytes_in: 0,
        definitions_bytes_out: 0,
        system_message: String::new(),
        tools: tools.to_vec(),
    }
}

fn extract_raw_output(reply: &Value) -> Value {
    let message = &reply["choices"][0]["message"];
    match &message["content"] {
        Value::String(s) => Value::String(s.clone()),
        Value::Array(parts) => Value::String(
            parts
                .iter()
                .filter_map(|p| p["text"].as_str())
                .collect::<String>(),
        ),
        _ => match message.get("tool_calls") {
            Some(tc) if !tc.is_null() => Value::String(nasiko_tool_compact::canonical_json(tc)),
            _ => Value::Null,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::super::eval::{CaseInputs, build_request};
    use super::*;

    fn catalog() -> Vec<Value> {
        vec![
            json!({"type": "function", "function": {"name": "get_weather", "description": "Current weather for a city",
            "parameters": {"type": "object", "properties": {"city": {"type": "string", "description": "City name"}}, "required": ["city"]}}}),
            json!({"type": "function", "function": {"name": "get_forecast", "description": "Five-day forecast for a city",
            "parameters": {"type": "object", "properties": {"city": {"type": "string", "description": "City name"}, "days": {"type": "integer", "minimum": 1, "maximum": 5, "description": "Number of days"}, "units": {"type": "string", "enum": ["metric", "imperial"], "description": "Unit system"}}, "required": ["city"]}}}),
            json!({"type": "function", "function": {"name": "get_alerts", "description": "Active weather alerts for a region",
            "parameters": {"type": "object", "properties": {"region": {"type": "string", "description": "Region or country code"}, "severity": {"type": "string", "enum": ["minor", "moderate", "severe", "extreme"], "description": "Minimum severity"}, "limit": {"type": "integer", "minimum": 1, "maximum": 50, "description": "Maximum alerts to return"}}, "required": ["region"]}}}),
        ]
    }

    fn built(compacted: bool) -> BuiltRequest {
        let mut catalog = catalog();
        if !compacted {
            catalog[0]["function"]["strict"] = json!(true);
        }
        let messages = vec![json!({"role": "user", "content": "Weather in Paris?"})];
        let b = build_request(
            CaseInputs {
                tools: &[
                    "get_weather".to_owned(),
                    "get_forecast".to_owned(),
                    "get_alerts".to_owned(),
                ],
                messages: &messages,
            },
            &catalog,
            "m",
            64,
        )
        .unwrap();
        assert_eq!(b.compacted, compacted);
        b
    }

    fn cfg(base: &str, key: Option<&str>) -> LiveConfig {
        LiveConfig {
            base_url: base.to_owned(),
            model: "m".into(),
            api_key: key.map(str::to_owned),
            max_output_tokens: 64,
            timeout: Duration::from_secs(5),
        }
    }

    #[test]
    fn the_endpoint_keeps_a_v1_suffix_and_never_doubles_the_path() {
        for base in [
            "http://h",
            "http://h/",
            "http://h/v1",
            "http://h/v1/",
            " http://h/v1 ",
        ] {
            let url = chat_completions_url(base);
            assert!(url.ends_with("/chat/completions"), "{url}");
            assert_eq!(url.matches("chat/completions").count(), 1, "{url}");
            if base.contains("v1") {
                assert_eq!(url, "http://h/v1/chat/completions");
            } else {
                assert_eq!(url, "http://h/chat/completions");
            }
        }
    }

    #[test]
    fn messages_are_sanitized() {
        let s = sanitize_message("key sk-123 leaked\n\u{7}bell", Some("sk-123"));
        assert_eq!(s, "key [redacted] leakedbell");
        assert_eq!(
            sanitize_message(&"x".repeat(500), None).len(),
            MAX_MESSAGE_CHARS
        );
    }

    #[tokio::test]
    async fn the_received_body_equals_the_recorded_request_and_auth_is_optional() {
        let b = built(true);
        let seen =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::<(Option<String>, Value)>::new()));
        let mut server = mockito::Server::new_async().await;
        let capture = seen.clone();
        let mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body_from_request(move |req| {
                let auth = req.header("authorization").first().map(|h| h.to_str().unwrap().to_owned());
                let body: Value = serde_json::from_slice(req.body().unwrap()).unwrap();
                capture.lock().unwrap().push((auth, body));
                json!({"id": "r", "object": "chat.completion", "model": "m", "choices": [{"index": 0,
                    "message": {"role": "assistant", "content": "<<call get_weather {\"city\":\"Paris\"}>>"}, "finish_reason": "stop"}]}).to_string().into_bytes()
            })
            .expect(2)
            .create_async()
            .await;
        let http = live_client(Duration::from_secs(5)).unwrap();
        let base = format!("{}/v1", server.url());
        let with_key = call_live(&http, &cfg(&base, Some("k-1")), &b.body).await;
        let without = call_live(&http, &cfg(&base, None), &b.body).await;
        mock.assert_async().await;
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0].0.as_deref(), Some("Bearer k-1"));
        assert_eq!(
            seen[0].1, b.body,
            "the POSTed body must equal compact_request"
        );
        assert_eq!(seen[1].0, None);
        assert_eq!(seen[1].1, b.body);
        assert!(b.body["temperature"] == json!(0.0) && b.body["max_tokens"] == json!(64));
        let LiveOutcome::Completed(reply) = with_key else {
            panic!("{with_key:?}")
        };
        let judged = judge_reply(&reply, &b);
        assert_eq!(
            judged.live_calls,
            json!({"calls": [{"name": "get_weather", "arguments": {"city": "Paris"}}]})
        );
        assert_eq!(
            judged.raw_output,
            json!("<<call get_weather {\"city\":\"Paris\"}>>")
        );
        assert!(matches!(without, LiveOutcome::Completed(_)));
    }

    #[tokio::test]
    async fn retries_honour_retry_after_then_stop_and_auth_failures_do_not_retry() {
        let mut server = mockito::Server::new_async().await;
        let busy = server
            .mock("POST", "/chat/completions")
            .with_status(429)
            .with_header("retry-after", "0")
            .expect(1)
            .create_async()
            .await;
        let http = live_client(Duration::from_secs(5)).unwrap();
        let b = built(true);
        // First call: 429 then (mock consumed) a 200 from the second mock below.
        let ok = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({"id": "r", "object": "chat.completion", "model": "m", "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"}, "finish_reason": "stop"}]}).to_string())
            .expect(1)
            .create_async()
            .await;
        let out = call_live(&http, &cfg(&server.url(), None), &b.body).await;
        busy.assert_async().await;
        ok.assert_async().await;
        assert!(matches!(out, LiveOutcome::Completed(_)), "{out:?}");

        // Persistent 500: exactly three requests (one plus two retries), then a sanitized failure.
        let mut server = mockito::Server::new_async().await;
        let failing = server
            .mock("POST", "/chat/completions")
            .with_status(500)
            .with_body("boom secret-key-value")
            .expect(3)
            .create_async()
            .await;
        let out = call_live(
            &http,
            &cfg(&server.url(), Some("secret-key-value")),
            &b.body,
        )
        .await;
        failing.assert_async().await;
        assert_eq!(
            out,
            LiveOutcome::Failed {
                status: Some(500),
                message: "boom [redacted]".into()
            }
        );

        // 401: one request, no retry.
        let mut server = mockito::Server::new_async().await;
        let denied = server
            .mock("POST", "/chat/completions")
            .with_status(401)
            .with_body("no")
            .expect(1)
            .create_async()
            .await;
        let out = call_live(&http, &cfg(&server.url(), None), &b.body).await;
        denied.assert_async().await;
        assert_eq!(
            out,
            LiveOutcome::AuthFailed {
                status: 401,
                message: "no".into()
            }
        );
    }

    #[tokio::test]
    async fn a_slow_endpoint_is_reported_as_a_timeout_not_a_generic_send_error() {
        let b = built(true);
        let mut server = mockito::Server::new_async().await;
        let _slow = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_body_from_request(|_| {
                std::thread::sleep(Duration::from_millis(1500));
                b"{}".to_vec()
            })
            .create_async()
            .await;
        let http = live_client(Duration::from_millis(300)).unwrap();
        let mut c = cfg(&server.url(), None);
        c.timeout = Duration::from_millis(300);
        match call_live(&http, &c, &b.body).await {
            LiveOutcome::Failed {
                status: None,
                message,
            } => {
                assert!(message.starts_with("timeout after 0s"), "{message}")
            }
            other => panic!("{other:?}"),
        }
        let http = live_client(Duration::from_secs(2)).unwrap();
        let unreachable = cfg("http://127.0.0.1:9", None);
        match call_live(&http, &unreachable, &b.body).await {
            LiveOutcome::Failed {
                status: None,
                message,
            } => {
                assert!(message.starts_with("connection failed"), "{message}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn truncated_and_native_replies_are_judged_by_the_production_rules() {
        let compact = built(true);
        let truncated = json!({"id": "r", "object": "chat.completion", "model": "m", "choices": [{"index": 0,
            "message": {"role": "assistant", "content": "<<call get_weather {\"city\":\"Paris\"}>>"}, "finish_reason": "length"}]});
        let judged = judge_reply(&truncated, &compact);
        assert_eq!(judged.live_calls, json!({"error": "incomplete_completion"}));
        let empty_truncated = json!({"id": "r", "object": "chat.completion", "model": "m", "choices": [{"index": 0,
            "message": {"role": "assistant", "content": ""}, "finish_reason": "length"}]});
        // No executable output: passthrough, zero calls, representation none. Not an error, but
        // callers can see `finalized` says `None` rather than a successful call.
        let judged = judge_reply(&empty_truncated, &compact);
        assert_eq!(judged.live_calls, json!({"calls": []}));
        assert_eq!(
            judged.finalized.unwrap().representation,
            compact_tools::Representation::None
        );

        let native_request = built(false);
        let native_reply = json!({"id": "r", "object": "chat.completion", "model": "m", "choices": [{"index": 0,
            "message": {"role": "assistant", "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\"}"}}]},
            "finish_reason": "tool_calls"}]});
        let judged = judge_reply(&native_reply, &native_request);
        assert_eq!(
            judged.live_calls,
            json!({"calls": [{"name": "get_weather", "arguments": {"city": "Paris"}}]})
        );
        assert_eq!(
            judged.raw_output,
            json!(
                "[{\"function\":{\"arguments\":\"{\\\"city\\\":\\\"Paris\\\"}\",\"name\":\"get_weather\"},\"id\":\"c1\",\"type\":\"function\"}]"
            )
        );
        let bad_native = json!({"id": "r", "object": "chat.completion", "model": "m", "choices": [{"index": 0,
            "message": {"role": "assistant", "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "get_weather", "arguments": "{}"}}]},
            "finish_reason": "tool_calls"}]});
        assert_eq!(
            judge_reply(&bad_native, &native_request).live_calls,
            json!({"error": "invalid_arguments"})
        );
        let stopped_native = json!({"id": "r", "object": "chat.completion", "model": "m", "choices": [{"index": 0,
            "message": {"role": "assistant", "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":\"P\"}"}}]},
            "finish_reason": "stop"}]});
        assert_eq!(
            judge_reply(&stopped_native, &native_request).live_calls,
            json!({"error": "incomplete_completion"})
        );
    }
}
