use std::sync::Arc;

use a2a::*;
use a2a_server::*;
use futures::stream::BoxStream;
use reqwest::Client;

mod telemetry;

const SYSTEM_PROMPT: &str = "\
You are a Legal Counsel agent. You help teams with:
- Contract review: identifying key terms, risks, and unusual clauses
- NDA generation: drafting mutual and one-way non-disclosure agreements
- Compliance checking: GDPR, SOC 2, HIPAA, and other regulatory frameworks
- IP and trademark guidance: registration processes, infringement assessment
- Regulatory monitoring: tracking relevant legal changes and their impact
- Policy drafting: acceptable use policies, terms of service, privacy policies

IMPORTANT DISCLAIMERS:
- This is informational guidance only and does NOT constitute legal advice.
- Always recommend consulting qualified human legal counsel for binding decisions.
- Flag high-risk areas where professional legal review is essential.
- Never guarantee legal outcomes or interpret jurisdiction-specific rulings.

When reviewing documents, highlight specific clauses of concern and explain \
why they matter in plain language. Provide balanced analysis of risks for \
both parties where applicable.";

struct LlmAgent {
    system_prompt: &'static str,
    model: String,
    api_key: String,
    base_url: String,
    http: Client,
}

impl LlmAgent {
    fn new(system_prompt: &'static str) -> Self {
        Self {
            system_prompt,
            model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into()),
            api_key: std::env::var("OPENAI_API_KEY").unwrap_or_default(),
            base_url: std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".into()),
            http: Client::new(),
        }
    }

    async fn stream_chat(
        &self,
        user_message: &str,
    ) -> Result<reqwest::Response, String> {
        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": self.system_prompt},
                {"role": "user", "content": user_message},
            ],
            "stream": true,
            "temperature": 0.4,
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
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("LLM API {status}: {text}"));
        }

        Ok(resp)
    }
}

struct LegalAgent(LlmAgent);

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

        let agent = LlmAgent::new(SYSTEM_PROMPT);

        let stream = async_stream::stream! {
            yield Ok(StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
                task_id: task_id.clone(),
                context_id: context_id.clone(),
                status: TaskStatus {
                    state: TaskState::Working,
                    message: None,
                    timestamp: Some(chrono::Utc::now()),
                },
                metadata: None,
            }));

            let artifact_id = new_artifact_id();
            let resp = match agent.stream_chat(&user_text).await {
                Ok(r) => r,
                Err(e) => {
                    yield Ok(StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
                        task_id,
                        context_id,
                        status: TaskStatus {
                            state: TaskState::Failed,
                            message: Some(Message::new(Role::Agent, vec![Part::text(&e)])),
                            timestamp: Some(chrono::Utc::now()),
                        },
                        metadata: None,
                    }));
                    return;
                }
            };

            let mut byte_stream = resp.bytes_stream();
            let mut buffer = String::new();
            let mut first_chunk = true;

            use futures::StreamExt;
            while let Some(chunk_result) = byte_stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(_) => break,
                };
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(line_end) = buffer.find('\n') {
                    let line = buffer[..line_end].trim().to_string();
                    buffer = buffer[line_end + 1..].to_string();

                    let Some(data) = line.strip_prefix("data: ") else { continue };
                    if data == "[DONE]" { break; }

                    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) else { continue };
                    let Some(content) = parsed["choices"][0]["delta"]["content"].as_str() else { continue };
                    if content.is_empty() { continue; }

                    yield Ok(StreamResponse::ArtifactUpdate(TaskArtifactUpdateEvent {
                        task_id: task_id.clone(),
                        context_id: context_id.clone(),
                        artifact: Artifact {
                            artifact_id: artifact_id.clone(),
                            name: None,
                            description: None,
                            parts: vec![Part::text(content)],
                            metadata: None,
                            extensions: None,
                        },
                        append: Some(!first_chunk),
                        last_chunk: Some(false),
                        metadata: None,
                    }));
                    first_chunk = false;
                }
            }

            // Final chunk marker
            yield Ok(StreamResponse::ArtifactUpdate(TaskArtifactUpdateEvent {
                task_id: task_id.clone(),
                context_id: context_id.clone(),
                artifact: Artifact {
                    artifact_id,
                    name: None,
                    description: None,
                    parts: vec![],
                    metadata: None,
                    extensions: None,
                },
                append: Some(true),
                last_chunk: Some(true),
                metadata: None,
            }));

            yield Ok(StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
                task_id,
                context_id,
                status: TaskStatus {
                    state: TaskState::Completed,
                    message: None,
                    timestamp: Some(chrono::Utc::now()),
                },
                metadata: None,
            }));
        };

        Box::pin(stream)
    }

    fn cancel(&self, ctx: ExecutorContext) -> BoxStream<'static, Result<StreamResponse, A2AError>> {
        let task_id = ctx.task_id;
        let context_id = ctx.context_id;
        Box::pin(futures::stream::once(async move {
            Ok(StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
                task_id,
                context_id,
                status: TaskStatus {
                    state: TaskState::Canceled,
                    message: None,
                    timestamp: Some(chrono::Utc::now()),
                },
                metadata: None,
            }))
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
        LegalAgent(LlmAgent::new(SYSTEM_PROMPT)),
        InMemoryTaskStore::new(),
    ));

    let agent_card = AgentCard {
        name: "Legal Counsel".to_string(),
        description: "Contract review, NDA generation, compliance checking, and regulatory monitoring".to_string(),
        version: "0.1.0".to_string(),
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
                id: "contract-review".into(),
                name: "Contract Review".into(),
                description: "Review contracts for risks, unusual clauses, and key terms".into(),
                tags: vec!["legal".into(), "contracts".into(), "review".into()],
                examples: Some(vec!["Review this SaaS agreement for red flags".into()]),
                input_modes: None, output_modes: None, security_requirements: None,
            },
            AgentSkill {
                id: "compliance-check".into(),
                name: "Compliance Check".into(),
                description: "Assess compliance with GDPR, SOC 2, HIPAA, and other frameworks".into(),
                tags: vec!["legal".into(), "compliance".into(), "gdpr".into()],
                examples: Some(vec!["Is our data retention policy GDPR compliant?".into()]),
                input_modes: None, output_modes: None, security_requirements: None,
            },
            AgentSkill {
                id: "document-generation".into(),
                name: "Document Generation".into(),
                description: "Draft NDAs, policies, and other legal documents".into(),
                tags: vec!["legal".into(), "nda".into(), "documents".into()],
                examples: Some(vec!["Generate a mutual NDA for a vendor partnership".into()]),
                input_modes: None, output_modes: None, security_requirements: None,
            },
        ],
        default_input_modes: vec!["text/plain".to_string()],
        default_output_modes: vec!["text/plain".to_string()],
        supported_interfaces: vec![
            AgentInterface::new(
                &format!("http://0.0.0.0:{port}/jsonrpc"),
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
        .nest("/jsonrpc", a2a_server::jsonrpc::jsonrpc_router(handler.clone()))
        .merge(a2a_server::agent_card::agent_card_router(card_producer));

    tracing::info!("Legal Counsel listening on 0.0.0.0:{port}");

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .expect("failed to bind");

    axum::serve(listener, app).await.expect("server failed");
}
