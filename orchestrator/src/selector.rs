use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::*;
use crate::providers::{CompletionResult, LLMProvider, ProviderError};

/// Stage 3: LLM-based final agent selection using structured output.
pub struct AgentSelector {
    provider: LLMProvider,
    model: String,
}

impl AgentSelector {
    pub fn new(provider: LLMProvider, model: String) -> Self {
        Self { provider, model }
    }

    /// Returns the model name used for agent selection.
    pub fn model_name(&self) -> &str {
        &self.model
    }

    /// Select best agent using structured output (response_format json_schema).
    pub async fn select_agent(
        &self,
        query: &str,
        conversation_history: &[ConversationMessage],
        agents: &[AgentCardSummary],
    ) -> Result<(AgentSelection, CompletionResult), SelectorError> {
        if agents.is_empty() {
            return Err(SelectorError::NoAgentsAvailable);
        }

        let system_prompt = self.build_system_prompt(agents);
        let user_prompt = self.build_user_prompt(query, conversation_history);

        let request = ChatCompletionRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage { role: "system".to_string(), content: Some(system_prompt) },
                ChatMessage { role: "user".to_string(), content: Some(user_prompt) },
            ],
            stream: false,
            temperature: Some(0.0),
            max_tokens: Some(500),
            response_format: Some(ResponseFormat::JsonSchema {
                json_schema: JsonSchema {
                    name: "agent_selection".to_string(),
                    strict: Some(true),
                    schema: json!({
                        "type": "object",
                        "properties": {
                            "agent_id":   { "type": "string", "description": "UUID of the selected agent" },
                            "agent_name": { "type": "string", "description": "Name of the selected agent" },
                            "reasoning":  { "type": "string", "description": "Why this agent was selected" }
                        },
                        "required": ["agent_id", "agent_name", "reasoning"],
                        "additionalProperties": false
                    }),
                },
            }),
            stream_options: None,
        };

        let result = self.provider.chat_completion(&request).await?;

        let selection: AgentSelection = serde_json::from_str(&result.content)
            .map_err(|e| SelectorError::ParseError(e.to_string()))?;

        // Validate agent UUID exists in the candidate list; fall back to first if hallucinated.
        if !agents.iter().any(|a| a.id == selection.agent_id)
            && let Some(first) = agents.first() {
                return Ok((
                    AgentSelection {
                        agent_id: first.id,
                        agent_name: first.name.clone(),
                        reasoning: format!(
                            "LLM selected unknown agent '{}', falling back to '{}'",
                            selection.agent_name, first.name
                        ),
                    },
                    result,
                ));
            }

        Ok((selection, result))
    }

    /// Fetch running agents directly from DB — used by the orchestrator path.
    pub async fn fetch_active_agents(db: &PgPool) -> Result<Vec<AgentCardSummary>, sqlx::Error> {
        let rows = sqlx::query_as::<_, AgentCardRow>(
            "SELECT id, name, description, skills, tags FROM agents WHERE status = 'running' ORDER BY name",
        )
        .fetch_all(db)
        .await?;

        Ok(rows
            .into_iter()
            .map(|a| AgentCardSummary {
                id: a.id,
                name: a.name,
                description: a.description.unwrap_or_default(),
                skills: extract_skills(a.skills.0),
                tags: a.tags,
            })
            .collect())
    }

    fn build_system_prompt(&self, agents: &[AgentCardSummary]) -> String {
        let list: Vec<String> = agents
            .iter()
            .map(|a| {
                let skills_text = if a.skills.is_empty() {
                    "(none)".to_string()
                } else {
                    a.skills
                        .iter()
                        .map(|s| format!("{}: {}", s.name, s.description))
                        .collect::<Vec<_>>()
                        .join("; ")
                };
                format!(
                    "- {} (ID: {}): {}\n  Skills: {}\n  Tags: {}",
                    a.name,
                    a.id,
                    a.description,
                    skills_text,
                    a.tags.join(", ")
                )
            })
            .collect();

        format!(
            "You are a routing assistant. Select the best agent to handle the user's query.\n\nAvailable agents:\n{}\n\nSelect the most specialized agent. If no perfect match, choose the closest option.",
            list.join("\n\n")
        )
    }

    fn build_user_prompt(&self, query: &str, history: &[ConversationMessage]) -> String {
        let mut prompt = String::new();

        if !history.is_empty() {
            prompt.push_str("Conversation history:\n");
            for msg in history.iter().rev().take(5).rev() {
                prompt.push_str(&format!("{}: {}\n", msg.role, msg.content));
            }
            prompt.push('\n');
        }

        prompt.push_str(&format!("Current query: {}", query));
        prompt
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMessage {
    pub role: String,
    pub content: String,
}

#[derive(sqlx::FromRow)]
struct AgentCardRow {
    id: Uuid,
    name: String,
    description: Option<String>,
    skills: sqlx::types::Json<serde_json::Value>,
    tags: Vec<String>,
}

fn extract_skills(skills_json: serde_json::Value) -> Vec<super::models::SkillSummary> {
    let Some(arr) = skills_json.as_array() else {
        return vec![];
    };
    arr.iter()
        .filter_map(|s| {
            let name = s.get("name").and_then(|n| n.as_str())?.to_string();
            let description = s
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or(&name)
                .to_string();
            Some(super::models::SkillSummary { name, description })
        })
        .collect()
}

#[derive(Debug, thiserror::Error)]
pub enum SelectorError {
    #[error("no agents available")]
    NoAgentsAvailable,
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("failed to parse selection: {0}")]
    ParseError(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}
