use std::sync::Arc;

use a2a::*;
use a2a_server::*;
use futures::stream::BoxStream;
use tool_compact::{CompactToolSet, StreamDecoder};
mod telemetry;
mod tools;

struct DevOpsAgent {
    model: String,
    api_key: String,
    base_url: String,
    http: reqwest::Client,
}

impl DevOpsAgent {
    fn new() -> Self {
        Self {
            base_url: std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".into()),
            api_key: std::env::var("OPENAI_API_KEY").unwrap_or_default(),
            model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into()),
            http: reqwest::Client::new(),
        }
    }

    #[tracing::instrument(name = "ChatCompletion", skip_all, fields(
        gen_ai.operation.name = "chat",
        gen_ai.provider.name = "openai",
        gen_ai.request.model = %self.model,
        gen_ai.usage.input_tokens = tracing::field::Empty,
        gen_ai.usage.output_tokens = tracing::field::Empty,
        gen_ai.input.messages = tracing::field::Empty,
        gen_ai.output.messages = tracing::field::Empty,
    ))]
    async fn chat(
        &self,
        messages: &[serde_json::Value],
        tools: &[serde_json::Value],
        parent_cx: Option<&opentelemetry::Context>,
    ) -> Result<serde_json::Value, String> {
        // The remote parent must be set on THIS span explicitly: contextual
        // inheritance from a2a.execute strands the span in an orphan trace —
        // tracing-opentelemetry children inherit the parent's originally
        // sampled (local) trace id, not the one `set_parent` re-homed it to.
        if let Some(cx) = parent_cx {
            use tracing_opentelemetry::OpenTelemetrySpanExt as _;
            tracing::Span::current().set_parent(cx.clone());
        }
        let capture = telemetry::capture_content();
        if capture {
            tracing::Span::current().record(
                "gen_ai.input.messages",
                telemetry::genai_input_messages(messages)
                    .to_string()
                    .as_str(),
            );
        }
        let temperature: f64 = std::env::var("OPENAI_TEMPERATURE")
            .ok()
            .and_then(|t| t.parse().ok())
            .unwrap_or(0.0);
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "tools": tools,
            "temperature": temperature,
        });
        // OpenAI-compatible APIs reject an empty tools array.
        if tools.is_empty() {
            body.as_object_mut().unwrap().remove("tools");
        }

        let mut req = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key);
        if let Some(tp) = parent_cx.and_then(telemetry::traceparent_for_context) {
            req = req.header("traceparent", tp);
        }
        let resp = req
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("HTTP error: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("LLM API {status}: {body}"));
        }

        let response = resp
            .json::<serde_json::Value>()
            .await
            .map_err(|e| format!("JSON parse: {e}"))?;

        if let Some(usage) = response.get("usage") {
            let span = tracing::Span::current();
            if let Some(v) = usage.get("prompt_tokens").and_then(|v| v.as_u64()) {
                span.record("gen_ai.usage.input_tokens", v);
            }
            if let Some(v) = usage.get("completion_tokens").and_then(|v| v.as_u64()) {
                span.record("gen_ai.usage.output_tokens", v);
            }
        }

        if capture {
            let msg = &response["choices"][0]["message"];
            let text = msg["content"].as_str().unwrap_or("");
            let tool_calls = msg["tool_calls"].as_array().cloned().unwrap_or_default();
            let finish_reason = if tool_calls.is_empty() {
                "stop"
            } else {
                "tool_call"
            };
            tracing::Span::current().record(
                "gen_ai.output.messages",
                telemetry::genai_output_message(text, &tool_calls, finish_reason)
                    .to_string()
                    .as_str(),
            );
        }

        Ok(response)
    }
}

impl AgentExecutor for DevOpsAgent {
    fn execute(
        &self,
        ctx: ExecutorContext,
    ) -> BoxStream<'static, Result<StreamResponse, A2AError>> {
        // Join the caller's W3C trace (the platform forwards `traceparent`
        // through the agent proxy/orchestrator). Without adopting it, the OTel
        // SDK mints a fresh root trace id per request and the control plane's
        // session-trace view can never find this agent's spans.
        let remote_cx = ctx
            .service_params
            .get("traceparent")
            .and_then(|v| v.first())
            .and_then(|tp| telemetry::remote_context_from_traceparent(tp));

        let task_id = ctx.task_id.clone();
        let context_id = ctx.context_id.clone();

        let user_text = ctx
            .message
            .as_ref()
            .map(|m| {
                m.parts
                    .iter()
                    .filter_map(|p| match &p.content {
                        PartContent::Text(t) => Some(t.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();

        // GenAI agent-span semconv: the A2A request/response is the agent
        // invocation, so record it as invoke_agent with the exchanged messages
        // (content gated by the platform capture flag). `session.id` lets the
        // control plane find this trace by A2A contextId directly in Tempo.
        let agent_name =
            std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| env!("CARGO_PKG_NAME").into());
        let span = tracing::info_span!(
            "a2a.execute",
            otel.kind = "server",
            gen_ai.operation.name = "invoke_agent",
            gen_ai.agent.name = %agent_name,
            session.id = %context_id,
            gen_ai.input.messages = tracing::field::Empty,
            gen_ai.output.messages = tracing::field::Empty,
        );
        if let Some(ref cx) = remote_cx {
            use tracing_opentelemetry::OpenTelemetrySpanExt as _;
            span.set_parent(cx.clone());
        }
        let capture = telemetry::capture_content();
        if capture && !user_text.is_empty() {
            span.record(
                "gen_ai.input.messages",
                telemetry::genai_text_message("user", &user_text)
                    .to_string()
                    .as_str(),
            );
        }

        let model = self.model.clone();
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        let http = self.http.clone();
        let record_span = span.clone();

        let stream = async_stream::stream! {
            yield Ok(status_working(&task_id, &context_id, None));

            let agent = DevOpsAgent { model, api_key, base_url, http };
            let tool_defs = tools::definitions();
            let cts = CompactToolSet::new(tool_defs.clone());
            let compact_block = cts.compact_system_block();
            let bypass_tools: Vec<serde_json::Value> =
                cts.bypass_tools().into_iter().cloned().collect();

            let system = format!(
        "You are a DevOps Engineer agent. You MUST use your tools for every answer — never respond from \
memory alone. Every claim must be backed by tool output.

Available tools:
{compact_block}
When you need to call a tool, output:
tool_name({{\"argument\":\"value\"}})

Rules:
- ALWAYS call at least one tool before answering
- For how-to questions, use web_search to find current documentation
- For repo questions, use github_repo_info or github_actions_runs
- Cite your sources (URLs from tool results)
- Be concise and actionable"
            );

            let mut messages = vec![
                serde_json::json!({"role": "system", "content": system}),
                serde_json::json!({"role": "user", "content": user_text}),
            ];

            let mut final_text = String::new();

            for _ in 0..4 {
                let resp = match agent.chat(&messages, &bypass_tools, remote_cx.as_ref()).await {
                    Ok(r) => r,
                    Err(e) => {
                        yield Ok(status_failed(&task_id, &context_id, &e));
                        return;
                    }
                };

                let choice = &resp["choices"][0]["message"];
                let raw_content = choice["content"].as_str().unwrap_or("");

                // Decode compact tool calls via StreamDecoder
                let mut decoder = StreamDecoder::new(tool_defs.clone());
                let compact_calls = match decoder.push(raw_content) {
                    Ok(calls) => calls,
                    Err(e) => {
                        yield Ok(status_failed(
                            &task_id,
                            &context_id,
                            &format!("Failed to decode tool call: {e}"),
                        ));
                        return;
                    }
                };

                if !compact_calls.is_empty() {
                    messages.push(choice.clone());
                    for call in compact_calls {
                        let name = call["function"]["name"].as_str().unwrap_or("");
                        let args_val = &call["function"]["arguments"];
                        let args_str = serde_json::to_string(args_val).unwrap_or_else(|_| "{}".into());

                        let preview = extract_preview(&args_str);
                        yield Ok(status_working(
                            &task_id, &context_id,
                            Some(&format!("{name}: {preview}")),
                        ));

                        let result = tools::execute(name, &args_str).await;

                        messages.push(serde_json::json!({
                            "role": "user",
                            "content": format!("Tool `{name}` result:\n{result}"),
                        }));
                    }
                } else if let Some(calls) = choice["tool_calls"].as_array() {
                    messages.push(choice.clone());
                    for tc in calls {
                        let name = tc["function"]["name"].as_str().unwrap_or("");
                        let args = tc["function"]["arguments"].as_str().unwrap_or("{}");
                        let call_id = tc["id"].as_str().unwrap_or("");

                        let preview = extract_preview(args);
                        yield Ok(status_working(
                            &task_id, &context_id,
                            Some(&format!("{name}: {preview}")),
                        ));

                        let result = tools::execute(name, args).await;

                        messages.push(serde_json::json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": result,
                        }));
                    }
                } else {
                    final_text = strip_tool_markup(raw_content);
                    break;
                }
            }

            // Tool budget exhausted while the model still wanted tools: force a
            // final answer from the gathered context, else the artifact is empty.
            if final_text.is_empty() {
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": "Tool calls are no longer available. Answer the original question now, using only the information already gathered above. Respond with plain text only."
                }));
                match agent.chat(&messages, &[], remote_cx.as_ref()).await {
                    Ok(resp) => {
                        final_text = strip_tool_markup(
                            resp["choices"][0]["message"]["content"].as_str().unwrap_or(""));
                    }
                    Err(e) => {
                        yield Ok(status_failed(&task_id, &context_id, &e));
                        return;
                    }
                }
            }

            yield Ok(StreamResponse::ArtifactUpdate(TaskArtifactUpdateEvent {
                task_id: task_id.clone(),
                context_id: context_id.clone(),
                artifact: Artifact {
                    artifact_id: new_artifact_id(),
                    name: None,
                    description: None,
                    parts: vec![Part::text(&final_text)],
                    metadata: None,
                    extensions: None,
                },
                append: Some(false),
                last_chunk: Some(true),
                metadata: None,
            }));

            if capture && !final_text.is_empty() {
                record_span.record(
                    "gen_ai.output.messages",
                    telemetry::genai_text_message("assistant", &final_text)
                        .to_string()
                        .as_str(),
                );
            }
            yield Ok(status_completed(&task_id, &context_id));
        };

        // Poll the stream inside `span` so every span created during execution
        // (ChatCompletion, tool calls) lands under the remote parent — even
        // though the body streams after the HTTP handler has returned.
        // (tracing's Instrumented wraps Futures, not Streams, so instrument
        // each item-poll future rather than the stream itself.)
        use futures::StreamExt as _;
        use tracing::Instrument as _;
        Box::pin(async_stream::stream! {
            let mut inner = std::pin::pin!(stream);
            while let Some(item) = inner.next().instrument(span.clone()).await {
                yield item;
            }
        })
    }

    fn cancel(&self, ctx: ExecutorContext) -> BoxStream<'static, Result<StreamResponse, A2AError>> {
        let task_id = ctx.task_id.clone();
        let context_id = ctx.context_id.clone();
        Box::pin(futures::stream::once(async move {
            Ok(status_completed(&task_id, &context_id))
        }))
    }
}

#[tokio::main]
async fn main() {
    telemetry::init();

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8000);

    let handler = Arc::new(DefaultRequestHandler::new(
        DevOpsAgent::new(),
        InMemoryTaskStore::new(),
    ));

    let agent_card = AgentCard {
        name: "DevOps Engineer".to_string(),
        description:
            "CI/CD pipelines, infrastructure provisioning, K8s operations, and incident response"
                .to_string(),
        version: "1.0.0".to_string(),
        provider: Some(AgentProvider {
            organization: "Nasiko".to_string(),
            url: "https://nasiko.io".to_string(),
        }),
        capabilities: AgentCapabilities {
            streaming: Some(true),
            push_notifications: Some(false),
            extensions: None,
            extended_agent_card: None,
        },
        skills: vec![
            AgentSkill {
                id: "repo-info".into(),
                name: "GitHub Repository Info".into(),
                description: "Look up GitHub repo details, stars, forks, and recent activity"
                    .into(),
                tags: vec!["devops".into(), "github".into(), "repositories".into()],
                examples: Some(vec![
                    "What's the status of the kubernetes/kubernetes repo?".into(),
                ]),
                input_modes: None,
                output_modes: None,
                security_requirements: None,
            },
            AgentSkill {
                id: "ci-cd-status".into(),
                name: "CI/CD Status".into(),
                description: "Check recent GitHub Actions workflow runs and their results".into(),
                tags: vec!["devops".into(), "ci-cd".into(), "pipelines".into()],
                examples: Some(vec!["Show recent CI runs for tokio-rs/tokio".into()]),
                input_modes: None,
                output_modes: None,
                security_requirements: None,
            },
            AgentSkill {
                id: "container-search".into(),
                name: "Container Image Search".into(),
                description: "Search Docker Hub for container images".into(),
                tags: vec!["devops".into(), "docker".into(), "containers".into()],
                examples: Some(vec!["Find official PostgreSQL images on Docker Hub".into()]),
                input_modes: None,
                output_modes: None,
                security_requirements: None,
            },
            AgentSkill {
                id: "web-search".into(),
                name: "Web Search".into(),
                description:
                    "Search the web for DevOps documentation, tutorials, and best practices".into(),
                tags: vec!["devops".into(), "documentation".into(), "search".into()],
                examples: Some(vec!["How do I set up GitHub Actions for Rust?".into()]),
                input_modes: None,
                output_modes: None,
                security_requirements: None,
            },
        ],
        default_input_modes: vec!["text/plain".to_string()],
        default_output_modes: vec!["text/plain".to_string()],
        supported_interfaces: vec![AgentInterface::new(
            format!("http://0.0.0.0:{port}/"),
            TRANSPORT_PROTOCOL_JSONRPC,
        )],
        security_schemes: None,
        security_requirements: None,
        documentation_url: None,
        icon_url: None,
        signatures: None,
    };

    let card_producer = Arc::new(StaticAgentCard::new(agent_card));

    let app = axum::Router::new()
        .merge(a2a_server::jsonrpc::jsonrpc_router(handler.clone()))
        .merge(a2a_server::agent_card::agent_card_router(card_producer))
        .layer(tower_http::trace::TraceLayer::new_for_http());

    tracing::info!("DevOps Engineer listening on 0.0.0.0:{port}");

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .expect("failed to bind");

    axum::serve(listener, app).await.expect("server failed");
}

// ─── Event helpers ──────────────────────────────────────────────────────────

fn status_working(task_id: &str, context_id: &str, msg: Option<&str>) -> StreamResponse {
    StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
        task_id: task_id.into(),
        context_id: context_id.into(),
        status: TaskStatus {
            state: TaskState::Working,
            message: msg.map(|t| Message {
                message_id: new_message_id(),
                context_id: Some(context_id.into()),
                task_id: Some(task_id.into()),
                role: Role::Agent,
                parts: vec![Part::text(t)],
                metadata: None,
                extensions: None,
                reference_task_ids: None,
            }),
            timestamp: Some(chrono::Utc::now()),
        },
        metadata: None,
    })
}

fn status_completed(task_id: &str, context_id: &str) -> StreamResponse {
    StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
        task_id: task_id.into(),
        context_id: context_id.into(),
        status: TaskStatus {
            state: TaskState::Completed,
            message: None,
            timestamp: Some(chrono::Utc::now()),
        },
        metadata: None,
    })
}

fn extract_preview(args: &str) -> String {
    serde_json::from_str::<serde_json::Value>(args)
        .ok()
        .and_then(|v| {
            v.as_object()?.values().find_map(|val| {
                val.as_str().map(|s| {
                    if s.len() > 60 {
                        format!("{}...", &s[..60])
                    } else {
                        s.to_string()
                    }
                })
            })
        })
        .unwrap_or_else(|| "...".into())
}

fn status_failed(task_id: &str, context_id: &str, error: &str) -> StreamResponse {
    StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
        task_id: task_id.into(),
        context_id: context_id.into(),
        status: TaskStatus {
            state: TaskState::Failed,
            message: Some(Message {
                message_id: new_message_id(),
                context_id: Some(context_id.into()),
                task_id: Some(task_id.into()),
                role: Role::Agent,
                parts: vec![Part::text(error)],
                metadata: None,
                extensions: None,
                reference_task_ids: None,
            }),
            timestamp: Some(chrono::Utc::now()),
        },
        metadata: None,
    })
}

/// DeepSeek sometimes emits its internal tool-call markup (`<｜DSML｜…`) as
/// plain content instead of structured tool_calls. Anything from the first
/// marker onward is machinery, not an answer — cut it so an all-markup
/// response reads as empty and triggers the forced-answer fallback.
fn strip_tool_markup(content: &str) -> String {
    match content.find("<｜") {
        Some(idx) => content[..idx].trim().to_string(),
        None => content.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compact_system_prompt_and_bypass() {
        let tool_defs = tools::definitions();
        let cts = CompactToolSet::new(tool_defs.clone());
        let analysis = cts.analyze();

        assert_eq!(analysis.compacted_count(), 5);
        assert_eq!(analysis.bypassed_count(), 0);

        let compact_block = cts.compact_system_block();
        assert!(compact_block.contains("github_repo_info(owner:string, repo:string)"));
        assert!(compact_block.contains("github_actions_runs(owner:string, repo:string)"));
        assert!(compact_block.contains("docker_hub_search(query:string)"));
        assert!(compact_block.contains("check_endpoint(url:string)"));
        assert!(compact_block.contains("web_search(query:string)"));

        let bypass_tools = cts.bypass_tools();
        assert!(
            bypass_tools.is_empty(),
            "all 5 DevOps tools should be compactable"
        );
    }

    #[test]
    fn test_local_integration_github_repo_info() {
        let tool_defs = tools::definitions();
        let mut decoder = StreamDecoder::new(tool_defs);

        let raw_call = r#"github_repo_info({"owner":"octocat","repo":"hello-world"})"#;
        let decoded = decoder.push(raw_call).expect("decode should succeed");
        assert_eq!(decoded.len(), 1);

        let call = &decoded[0];
        let name = call["function"]["name"].as_str().unwrap();
        assert_eq!(name, "github_repo_info");

        let args_val = &call["function"]["arguments"];
        assert_eq!(args_val["owner"].as_str(), Some("octocat"));
        assert_eq!(args_val["repo"].as_str(), Some("hello-world"));

        // Verify arguments serialize to string format expected by tools::execute
        let args_str = serde_json::to_string(args_val).expect("arguments must serialize");
        assert!(args_str.contains(r#""owner":"octocat""#));
        assert!(args_str.contains(r#""repo":"hello-world""#));

        // Verify parsed back
        let parsed: serde_json::Value =
            serde_json::from_str(&args_str).expect("serialized args must be valid JSON");
        assert_eq!(parsed["owner"].as_str(), Some("octocat"));
        assert_eq!(parsed["repo"].as_str(), Some("hello-world"));

        decoder.finish().expect("stream should finish cleanly");
    }

    #[test]
    fn test_local_integration_check_endpoint() {
        let tool_defs = tools::definitions();
        let mut decoder = StreamDecoder::new(tool_defs);

        let raw_call = r#"check_endpoint({"url":"https://example.com"})"#;
        let decoded = decoder.push(raw_call).expect("decode should succeed");
        assert_eq!(decoded.len(), 1);

        let call = &decoded[0];
        let name = call["function"]["name"].as_str().unwrap();
        assert_eq!(name, "check_endpoint");

        let args_val = &call["function"]["arguments"];
        assert_eq!(args_val["url"].as_str(), Some("https://example.com"));

        let args_str = serde_json::to_string(args_val).expect("arguments must serialize");
        assert_eq!(args_str, r#"{"url":"https://example.com"}"#);
    }

    #[test]
    fn test_local_integration_docker_hub_search() {
        let tool_defs = tools::definitions();
        let mut decoder = StreamDecoder::new(tool_defs);

        let raw_call = r#"docker_hub_search({"query":"postgres"})"#;
        let decoded = decoder.push(raw_call).expect("decode should succeed");
        assert_eq!(decoded.len(), 1);

        let call = &decoded[0];
        assert_eq!(call["function"]["name"].as_str(), Some("docker_hub_search"));
        assert_eq!(
            call["function"]["arguments"]["query"].as_str(),
            Some("postgres")
        );
    }

    #[test]
    fn test_local_integration_markdown_surrounded_call() {
        let tool_defs = tools::definitions();
        let mut decoder = StreamDecoder::new(tool_defs);

        let text = "I will check the repository:\n```\ngithub_repo_info({\"owner\":\"rust-lang\",\"repo\":\"rust\"})\n```";
        let decoded = decoder.push(text).expect("decode should succeed");
        assert_eq!(decoded.len(), 1);

        let call = &decoded[0];
        assert_eq!(call["function"]["name"].as_str(), Some("github_repo_info"));
        assert_eq!(
            call["function"]["arguments"]["owner"].as_str(),
            Some("rust-lang")
        );
        assert_eq!(call["function"]["arguments"]["repo"].as_str(), Some("rust"));
    }

    #[test]
    fn test_local_integration_reject_invalid_tool_call() {
        let tool_defs = tools::definitions();
        let mut decoder = StreamDecoder::new(tool_defs);

        // Unknown tool
        let res = decoder.push(r#"non_existent_tool({"foo":"bar"})"#);
        assert!(res.is_err());

        // Missing required arg
        let mut decoder2 = StreamDecoder::new(tools::definitions());
        let res2 = decoder2.push(r#"github_repo_info({"owner":"octocat"})"#);
        assert!(res2.is_err());

        // Extra arg
        let mut decoder3 = StreamDecoder::new(tools::definitions());
        let res3 =
            decoder3.push(r#"github_repo_info({"owner":"octocat","repo":"hello","extra":"bad"})"#);
        assert!(res3.is_err());
    }
}
