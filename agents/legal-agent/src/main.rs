use std::sync::Arc;

use a2a::*;
use a2a_server::*;
use futures::stream::BoxStream;
mod mcp;
mod telemetry;
mod tools;

/// What a caller sees when this agent has no model key at all. At deploy the platform's LLM
/// router puts the agent's gateway token in `OPENAI_API_KEY`, so an empty value means neither
/// the router nor the agent's secrets supplied a key.
const MISSING_KEY_MESSAGE: &str = "OPENAI_API_KEY is not set for this agent. Add it in the agent's secrets, or configure a platform key.";

/// The error a failed model call reports. A rejected key (401/403) is named as one, without
/// the provider's body, which goes to the log instead.
fn llm_error_message(status: reqwest::StatusCode, body: &str) -> String {
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        tracing::warn!(%status, body, "model provider rejected the API key");
        return format!(
            "The model provider rejected this agent's API key (HTTP {}).",
            status.as_u16()
        );
    }
    // The provider's (or the LLM router's) own `error.message` when the body carries one, so the
    // user reads a sentence rather than a JSON dump; the raw body stays in the log.
    let message = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_owned));
    match message {
        Some(message) => {
            tracing::warn!(%status, body, "model call failed");
            format!("LLM API {status}: {message}")
        }
        None => format!("LLM API {status}: {body}"),
    }
}

struct LegalAgent {
    model: String,
    api_key: String,
    base_url: String,
    http: reqwest::Client,
    /// MCP gateway client -- `None` when the platform didn't inject
    /// `MCP_GATEWAY_URL`/`MCP_GATEWAY_TOKEN`. Configured ONCE at startup;
    /// the per-user binding rides the forwarded traceparent (see mcp.rs).
    mcp: Option<Arc<mcp::McpClient>>,
}

impl LegalAgent {
    fn new(mcp: Option<Arc<mcp::McpClient>>) -> Self {
        Self {
            base_url: std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".into()),
            api_key: std::env::var("OPENAI_API_KEY").unwrap_or_default(),
            model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into()),
            http: reqwest::Client::new(),
            mcp,
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
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "tools": tools,
            "temperature": 0.1,
        });
        // OpenAI-compatible APIs reject an empty tools array.
        if tools.is_empty() {
            body.as_object_mut().unwrap().remove("tools");
        }

        if self.api_key.is_empty() {
            return Err(MISSING_KEY_MESSAGE.to_owned());
        }

        let mut req = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key);
        // Propagate THIS span (not `parent_cx`): the LLM router's `gen_ai.chat` span must
        // nest under ChatCompletion so the trace view shows the model the router ran.
        let own_cx: Option<opentelemetry::Context> = parent_cx.map(|_| {
            use tracing_opentelemetry::OpenTelemetrySpanExt as _;
            tracing::Span::current().context()
        });
        if let Some(tp) = own_cx
            .as_ref()
            .and_then(telemetry::traceparent_for_context)
            .or_else(|| parent_cx.and_then(telemetry::traceparent_for_context))
        {
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
            return Err(llm_error_message(status, &body));
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
            let message = &response["choices"][0]["message"];
            let text = message["content"].as_str().unwrap_or("");
            let tool_calls = message["tool_calls"]
                .as_array()
                .cloned()
                .unwrap_or_default();
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

impl AgentExecutor for LegalAgent {
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

        // The same header, kept verbatim: gateway tool calls forward it so the
        // platform can resolve the user and authorize this agent as a recorded
        // participant of that flow (docs/MCP_GATEWAY_AGENT_AUTH.md).
        let traceparent = ctx
            .service_params
            .get("traceparent")
            .and_then(|v| v.first())
            .cloned()
            .unwrap_or_default();

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
        let mcp = self.mcp.clone();
        let record_span = span.clone();

        let stream = async_stream::stream! {
            yield Ok(status_working(&task_id, &context_id, None));

            let agent = LegalAgent { model, api_key, base_url, http, mcp: mcp.clone() };


            let system = "\
        You are a Legal Research assistant. You MUST use your tools for every answer — never respond from \
        memory alone. Every claim must be backed by tool output.\n\n\
        Tools:\n\
        - sec_filing_search — full-text search across SEC filings (10-K, 10-Q, 8-K, S-1, etc.)\n\
        - sec_company_filings — recent filings for a company by CIK number\n\
        - sec_company_search — find a company's CIK number by name\n\
        - federal_register — search proposed/final rules in the Federal Register\n\
        - case_law_search — search court opinions and case law via CourtListener\n\n\
        Rules:\n\
        - ALWAYS call at least one tool before answering\n\
        - For company research: sec_company_search first to get CIK, then sec_company_filings\n\
        - Cite specific filing dates, form types, and SEC URLs\n\
        - SEC data is US-only — state jurisdictional scope clearly\n\
        - End with: 'This is informational only, not legal advice.'";

            // One session per request: `tools/list` is query-aware, so the set
            // depends on this request's traceparent and grows when the model
            // searches (see mcp.rs).
            let mut mcp_session = match &mcp {
                Some(client) => {
                    Some(mcp::McpSession::open(client.clone(), &traceparent).await)
                }
                None => None,
            };

            // The prompt above lists only the built-ins, so without this the model
            // keeps answering account questions with web_search (see mcp.rs).
            let system = match &mcp_session {
                Some(session) => format!("{system}{}", session.system_prompt_hint()),
                None => system.to_string(),
            };

            let mut messages = vec![
                serde_json::json!({"role": "system", "content": system}),
                serde_json::json!({"role": "user", "content": user_text}),
            ];

            let mut final_text = String::new();

            for round in 0..6 {
                // Rebuilt every round: a `nasiko_search_tools` call adds the
                // tools it found, and the model can only call what it is shown.
                let mut tool_defs = tools::definitions();
                if let Some(session) = &mcp_session {
                    tool_defs.extend(session.tool_defs().iter().cloned());
                }
                // Force the last allowed round to be tool-less so the model must
                // produce a plain-text answer instead of the loop exhausting with
                // final_text still empty (silently returning no answer at all).
                let tools_for_call: &[serde_json::Value] =
                    if round == 5 { &[] } else { &tool_defs };
                let resp = match agent.chat(&messages, tools_for_call, remote_cx.as_ref()).await {
                    Ok(r) => r,
                    Err(e) => {
                        yield Ok(status_failed(&task_id, &context_id, &e));
                        return;
                    }
                };

                let choice = &resp["choices"][0]["message"];
                messages.push(choice.clone());

                if let Some(calls) = choice["tool_calls"].as_array() {
                    for tc in calls {
                        let name = tc["function"]["name"].as_str().unwrap_or("");
                        let args = tc["function"]["arguments"].as_str().unwrap_or("{}");
                        let call_id = tc["id"].as_str().unwrap_or("");

                        let preview = extract_preview(args);
                        yield Ok(status_working(
                            &task_id, &context_id,
                            Some(&format!("{name}: {preview}")),
                        ));

                        let result = match &mut mcp_session {
                            Some(session) if session.handles(name) => {
                                session.call_tool(name, args).await
                            }
                            _ => tools::execute(name, args).await,
                        };

                        messages.push(serde_json::json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": result,
                        }));
                    }
                } else {
                    final_text = strip_tool_markup(choice["content"].as_str().unwrap_or(""));
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

    // MCP gateway: configured once from the deploy-injected env. Only a health
    // check here — the tool set is query-scoped and fetched per request
    // (docs/MCP_TOOL_SEARCH_DESIGN.md), see McpSession.
    let mcp = mcp::McpClient::from_env().map(Arc::new);
    if let Some(client) = &mcp {
        client.probe().await;
    }

    let handler = Arc::new(DefaultRequestHandler::new(
        LegalAgent::new(mcp),
        InMemoryTaskStore::new(),
    ));

    let agent_card = AgentCard {
        name: "Legal Research".to_string(),
        description: "SEC filings, Federal Register regulations, and case law search".to_string(),
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
                id: "sec-filings".into(),
                name: "SEC Filings".into(),
                description: "Search SEC EDGAR for corporate filings (10-K, 10-Q, 8-K)".into(),
                tags: vec![
                    "legal".into(),
                    "sec".into(),
                    "filings".into(),
                    "corporate".into(),
                ],
                examples: Some(vec!["Find Apple's latest 10-K filing".into()]),
                input_modes: None,
                output_modes: None,
                security_requirements: None,
            },
            AgentSkill {
                id: "regulations".into(),
                name: "Federal Register".into(),
                description: "Search the Federal Register for regulations and notices".into(),
                tags: vec!["legal".into(), "regulations".into(), "compliance".into()],
                examples: Some(vec!["Find recent AI regulation proposals".into()]),
                input_modes: None,
                output_modes: None,
                security_requirements: None,
            },
            AgentSkill {
                id: "case-law".into(),
                name: "Case Law Search".into(),
                description: "Search court opinions and case law via CourtListener".into(),
                tags: vec!["legal".into(), "cases".into(), "courts".into()],
                examples: Some(vec!["Find Supreme Court cases about fair use".into()]),
                input_modes: None,
                output_modes: None,
                security_requirements: None,
            },
        ],
        default_input_modes: vec!["text/plain".to_string()],
        default_output_modes: vec!["text/plain".to_string()],
        supported_interfaces: vec![AgentInterface::new(
            &format!("http://0.0.0.0:{port}/"),
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

    tracing::info!("Legal Research listening on 0.0.0.0:{port}");

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
