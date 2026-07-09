use std::sync::Arc;

use a2a::*;
use a2a_server::*;
use futures::stream::BoxStream;
mod telemetry;
mod tools;

struct LegalAgent {
    model: String,
    api_key: String,
    base_url: String,
    http: reqwest::Client,
}

impl LegalAgent {
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
        gen_ai.request.model = %self.model,
        gen_ai.usage.input_tokens = tracing::field::Empty,
        gen_ai.usage.output_tokens = tracing::field::Empty,
    ))]
    async fn chat(
        &self,
        messages: &[serde_json::Value],
        tools: &[serde_json::Value],
    ) -> Result<serde_json::Value, String> {
        let body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "tools": tools,
            "temperature": 0.1,
        });

        let resp = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("HTTP error: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("LLM API {status}: {body}"));
        }

        let response = resp.json::<serde_json::Value>()
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

        Ok(response)
    }
}

impl AgentExecutor for LegalAgent {
    fn execute(&self, ctx: ExecutorContext) -> BoxStream<'static, Result<StreamResponse, A2AError>> {
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

        let model = self.model.clone();
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        let http = self.http.clone();

        let stream = async_stream::stream! {
            yield Ok(status_working(&task_id, &context_id, None));

            let agent = LegalAgent { model, api_key, base_url, http };
            let tool_defs = tools::definitions();

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

            let mut messages = vec![
                serde_json::json!({"role": "system", "content": system}),
                serde_json::json!({"role": "user", "content": user_text}),
            ];

            let mut final_text = String::new();

            for _ in 0..6 {
                let resp = match agent.chat(&messages, &tool_defs).await {
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

                        let result = tools::execute(name, args).await;

                        messages.push(serde_json::json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": result,
                        }));
                    }
                } else {
                    final_text = choice["content"].as_str().unwrap_or("").to_string();
                    break;
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

            yield Ok(status_completed(&task_id, &context_id));
        };

        Box::pin(stream)
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
        LegalAgent::new(),
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
                tags: vec!["legal".into(), "sec".into(), "filings".into(), "corporate".into()],
                examples: Some(vec!["Find Apple's latest 10-K filing".into()]),
                input_modes: None, output_modes: None, security_requirements: None,
            },
            AgentSkill {
                id: "regulations".into(),
                name: "Federal Register".into(),
                description: "Search the Federal Register for regulations and notices".into(),
                tags: vec!["legal".into(), "regulations".into(), "compliance".into()],
                examples: Some(vec!["Find recent AI regulation proposals".into()]),
                input_modes: None, output_modes: None, security_requirements: None,
            },
            AgentSkill {
                id: "case-law".into(),
                name: "Case Law Search".into(),
                description: "Search court opinions and case law via CourtListener".into(),
                tags: vec!["legal".into(), "cases".into(), "courts".into()],
                examples: Some(vec!["Find Supreme Court cases about fair use".into()]),
                input_modes: None, output_modes: None, security_requirements: None,
            },
        ],
        default_input_modes: vec!["text/plain".to_string()],
        default_output_modes: vec!["text/plain".to_string()],
        supported_interfaces: vec![
            AgentInterface::new(
                &format!("http://0.0.0.0:{port}/"),
                TRANSPORT_PROTOCOL_JSONRPC,
            ),
        ],
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
                    if s.len() > 60 { format!("{}...", &s[..60]) } else { s.to_string() }
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
