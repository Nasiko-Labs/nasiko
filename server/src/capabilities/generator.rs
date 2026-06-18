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

Given the source code of an agent, produce:
1. A concise description of what the agent does (1-2 sentences)
2. A list of skills the agent exposes — each skill is a discrete capability that can be invoked
3. Tags for discovery (lowercase, hyphenated)
4. Capabilities (boolean flags for protocol features)
5. Input/output MIME types the agent handles

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
- chat_agent: true if the agent maintains conversational context across messages"#.to_string()
    }

    fn build_user_prompt(&self, source_code: &str, agent_name: &str) -> String {
        // Truncate source if too long (leave room for system prompt + output)
        let max_source_chars = 60_000;
        let truncated = if source_code.len() > max_source_chars {
            &source_code[..max_source_chars]
        } else {
            source_code
        };

        format!(
            "Agent name: {agent_name}\n\nSource code:\n```\n{truncated}\n```"
        )
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
                }
            },
            "required": ["description", "skills", "tags", "capabilities", "default_input_modes", "default_output_modes"],
            "additionalProperties": false
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GeneratorError {
    #[error("Provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("Failed to parse generated card: {0}")]
    ParseError(String),
}
