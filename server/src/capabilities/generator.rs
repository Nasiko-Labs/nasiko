use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::catalog::models::Skill;
use crate::router::models::*;
use crate::router::providers::{CompletionResult, LLMProvider, ProviderError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedCard {
    pub description: String,
    pub skills: Vec<Skill>,
    pub tags: Vec<String>,
    pub capabilities: GeneratedCapabilities,
    pub default_input_modes: Vec<String>,
    pub default_output_modes: Vec<String>,
    /// Primary framework detected from imports/deps (fastapi, express, gin, axum, etc.), or null.
    pub framework: Option<String>,
    /// Communication transport inferred from route definitions and server setup.
    pub transport: String,
    /// LLM provider the agent wraps (openai, anthropic, groq, etc.), or null if not LLM-based.
    pub llm_provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedCapabilities {
    pub streaming: bool,
    #[serde(rename = "pushNotifications")]
    pub push_notifications: bool,
    #[serde(rename = "stateTransitionHistory")]
    pub state_transition_history: bool,
    pub chat_agent: bool,
}

pub struct CapabilityGenerator {
    provider: LLMProvider,
    model: String,
}

impl CapabilityGenerator {
    pub fn new(provider: LLMProvider, model: String) -> Self {
        Self { provider, model }
    }

    pub async fn generate(
        &self,
        source_code: &str,
        agent_name: &str,
    ) -> Result<(GeneratedCard, CompletionResult), GeneratorError> {
        let system_prompt = self.build_system_prompt();
        let user_prompt = self.build_user_prompt(source_code, agent_name);

        let request = ChatCompletionRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: Some(system_prompt),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: Some(user_prompt),
                },
            ],
            stream: false,
            temperature: Some(0.2),
            max_tokens: Some(4000),
            response_format: Some(ResponseFormat::JsonSchema {
                json_schema: JsonSchema {
                    name: "agent_card".to_string(),
                    strict: Some(true),
                    schema: Self::output_schema(),
                },
            }),
            stream_options: None,
        };

        let result = self.provider.chat_completion(&request).await?;

        let card: GeneratedCard = serde_json::from_str(&result.content)
            .map_err(|e| GeneratorError::ParseError(e.to_string()))?;

        Ok((card, result))
    }

    fn build_system_prompt(&self) -> String {
        r#"You are an expert at analyzing agent source code and generating A2A-compatible agent cards.

Given the source code of an agent (and optionally its dependency manifest), produce:
1. A concise description of what the agent does (1-2 sentences)
2. A list of skills the agent exposes — each skill is a discrete capability that can be invoked
3. Tags for discovery (lowercase, hyphenated)
4. Capabilities (boolean flags for protocol features)
5. Input/output MIME types the agent handles
6. Framework: the primary framework used to build the agent (fastapi, flask, django, express, nestjs, gin, axum, langchain, crewai, etc.) — null if not detectable from imports or deps
7. Transport: how the agent communicates externally — one of "http", "websocket", "grpc", "stdio", or "unknown"
8. LLM Provider: which LLM provider the agent directly wraps or calls (openai, anthropic, groq, ollama, huggingface, gemini, etc.) — null if the agent is not LLM-based

Guidelines for skills:
- Each skill should have a unique kebab-case id, a human-readable name, a description explaining what it does, relevant tags, and 1-2 example invocations (as plain text strings)
- Extract skills from route handlers, tool definitions, function signatures, or method names
- If the agent has a single main function, create one skill for it
- Skills should be specific and actionable, not generic

Guidelines for tags:
- Use domain-specific tags (e.g., "code-review", "data-pipeline", "image-generation")
- Include technology tags if relevant (e.g., "python", "kubernetes")
- 3-8 tags total

Guidelines for capabilities:
- streaming: true if the agent supports streaming responses (SSE, WebSocket, async generators)
- pushNotifications: true if the agent can send unsolicited notifications
- stateTransitionHistory: true if the agent tracks and exposes task state changes
- chat_agent: true if the agent maintains conversational context across messages

Guidelines for framework detection:
- Python: look for `from fastapi import`, `import flask`, `from django`, `import starlette`, `from langchain`, `import crewai`
- JS/TS: look for `express`, `@nestjs`, `fastify`, `hono` in imports or package.json
- Go: look for `gin`, `echo`, `fiber`, `chi` in import paths
- Rust: look for `axum`, `actix`, `warp`, `rocket` in Cargo.toml
- Use exact lowercase name: "fastapi", "flask", "express", "gin", "axum", "langchain", etc.

Guidelines for transport detection:
- "http": REST routes, HTTP handlers, ASGI/WSGI apps
- "websocket": websocket handlers, socket.io, ws library
- "grpc": grpc server definitions, proto imports
- "stdio": stdin/stdout communication, subprocess-based agents
- "unknown": cannot determine from available source"#.to_string()
    }

    fn build_user_prompt(&self, source_code: &str, agent_name: &str) -> String {
        let max_source_chars = 60_000;

        // Separate dependency manifests from source code so the LLM sees them
        // as distinct sections — manifests give strong framework/provider signal
        // with low token cost and should not be crowded out by source.
        let (manifests, source) = split_manifests(source_code);

        let truncated = if source.len() > max_source_chars {
            &source[..max_source_chars]
        } else {
            &source
        };

        let mut prompt = format!("Agent name: {agent_name}\n\n");

        if !manifests.is_empty() {
            prompt.push_str("Dependency manifests (requirements.txt / package.json / Cargo.toml / go.mod / pyproject.toml):\n");
            prompt.push_str("```\n");
            // Cap manifests at 4KB — they're dense with signal but rarely need more.
            let manifest_cap = 4_000;
            if manifests.len() > manifest_cap {
                prompt.push_str(&manifests[..manifest_cap]);
                prompt.push_str("\n[... truncated]\n");
            } else {
                prompt.push_str(&manifests);
            }
            prompt.push_str("```\n\n");
        }

        prompt.push_str("Source code:\n```\n");
        prompt.push_str(truncated);
        prompt.push_str("\n```");
        prompt
    }

    fn output_schema() -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "description": "1-2 sentence description of the agent"
                },
                "skills": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string", "description": "kebab-case unique identifier" },
                            "name": { "type": "string", "description": "human-readable name" },
                            "description": { "type": "string", "description": "what this skill does" },
                            "tags": { "type": "array", "items": { "type": "string" } },
                            "examples": { "type": "array", "items": { "type": "string" } }
                        },
                        "required": ["id", "name", "description", "tags", "examples"],
                        "additionalProperties": false
                    }
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" }
                },
                "capabilities": {
                    "type": "object",
                    "properties": {
                        "streaming": { "type": "boolean" },
                        "pushNotifications": { "type": "boolean" },
                        "stateTransitionHistory": { "type": "boolean" },
                        "chat_agent": { "type": "boolean" }
                    },
                    "required": ["streaming", "pushNotifications", "stateTransitionHistory", "chat_agent"],
                    "additionalProperties": false
                },
                "default_input_modes": {
                    "type": "array",
                    "items": { "type": "string" }
                },
                "default_output_modes": {
                    "type": "array",
                    "items": { "type": "string" }
                },
                "framework": {
                    "anyOf": [
                        { "type": "string", "description": "e.g. fastapi, flask, express, gin, axum, langchain" },
                        { "type": "null" }
                    ]
                },
                "transport": {
                    "type": "string",
                    "enum": ["http", "websocket", "grpc", "stdio", "unknown"]
                },
                "llm_provider": {
                    "anyOf": [
                        { "type": "string", "description": "e.g. openai, anthropic, groq, ollama, gemini" },
                        { "type": "null" }
                    ]
                }
            },
            "required": [
                "description", "skills", "tags", "capabilities",
                "default_input_modes", "default_output_modes",
                "framework", "transport", "llm_provider"
            ],
            "additionalProperties": false
        })
    }
}

/// Split combined source text into (manifests, source_code).
///
/// Files named `requirements.txt`, `package.json`, `Cargo.toml`, `go.mod`,
/// or `pyproject.toml` carry dense dependency signal.  Separating them lets
/// the user prompt present them as a dedicated section before the source,
/// improving framework and LLM-provider detection without wasting source quota.
fn split_manifests(source_code: &str) -> (String, String) {
    let manifest_names = [
        "requirements.txt",
        "package.json",
        "cargo.toml",
        "go.mod",
        "pyproject.toml",
        "package-lock.json",
    ];

    let mut manifests = String::new();
    let mut source = String::new();
    let mut current_header: Option<&str> = None;
    let mut current_body = String::new();

    for line in source_code.lines() {
        if let Some(stripped) = line.strip_prefix("--- ").and_then(|l| l.strip_suffix(" ---")) {
            // Flush previous section
            if let Some(header) = current_header {
                let lower = header.to_lowercase();
                if manifest_names.iter().any(|m| lower.ends_with(m)) {
                    manifests.push_str(&format!("--- {header} ---\n"));
                    manifests.push_str(&current_body);
                } else {
                    source.push_str(&format!("--- {header} ---\n"));
                    source.push_str(&current_body);
                }
            }
            current_header = Some(stripped);
            current_body = String::new();
        } else {
            current_body.push_str(line);
            current_body.push('\n');
        }
    }

    // Flush the last section
    if let Some(header) = current_header {
        let lower = header.to_lowercase();
        if manifest_names.iter().any(|m| lower.ends_with(m)) {
            manifests.push_str(&format!("--- {header} ---\n"));
            manifests.push_str(&current_body);
        } else {
            source.push_str(&format!("--- {header} ---\n"));
            source.push_str(&current_body);
        }
    } else {
        // No section headers — treat the whole thing as source
        source.push_str(source_code);
    }

    (manifests, source)
}

#[derive(Debug, thiserror::Error)]
pub enum GeneratorError {
    #[error("Provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("Failed to parse generated card: {0}")]
    ParseError(String),
}
