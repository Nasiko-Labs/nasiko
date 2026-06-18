use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::models::*;
use super::providers::{CompletionResult, LLMProvider, ProviderError};

/// Agent selector service - uses LLM structured output to choose best agent
pub struct AgentSelector {
    provider: LLMProvider,
    model: String,
}

impl AgentSelector {
    pub fn new(provider: LLMProvider, model: String) -> Self {
        Self { provider, model }
    }

    /// Select best agent using structured output (response_format json_schema)
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
            temperature: Some(0.0),
            max_tokens: Some(500),
            response_format: Some(ResponseFormat::JsonSchema {
                json_schema: JsonSchema {
                    name: "agent_selection".to_string(),
                    strict: Some(true),
                    schema: json!({
                        "type": "object",
                        "properties": {
                            "agent_id": {
                                "type": "string",
                                "description": "UUID of the selected agent"
                            },
                            "agent_name": {
                                "type": "string",
                                "description": "Name of the selected agent"
                            },
                            "reasoning": {
                                "type": "string",
                                "description": "Why this agent was selected"
                            }
                        },
                        "required": ["agent_id", "agent_name", "reasoning"],
                        "additionalProperties": false
                    }),
                },
            }),
            stream_options: None,
        };

        let result = self.provider.chat_completion(&request).await?;

        // Parse structured output (guaranteed valid JSON by the API)
        let selection: AgentSelection = serde_json::from_str(&result.content)
            .map_err(|e| SelectorError::ParseError(e.to_string()))?;

        // Validate agent exists in list
        if !agents.iter().any(|a| a.id == selection.agent_id) {
            if let Some(first) = agents.first() {
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
        }

        Ok((selection, result))
    }

    /// Fetch active agents from database
    pub async fn fetch_active_agents(db: &PgPool) -> Result<Vec<AgentCardSummary>, sqlx::Error> {
        let agents = sqlx::query_as::<_, AgentCardRow>(
            r#"
            SELECT id, name, description, skills, tags
            FROM agents
            WHERE status = 'running'
            ORDER BY name
            "#,
        )
        .fetch_all(db)
        .await?;

        Ok(agents
            .into_iter()
            .map(|a| AgentCardSummary {
                id: a.id,
                name: a.name,
                description: a.description.unwrap_or_default(),
                skills: extract_skill_names(a.skills.0),
                tags: a.tags,
            })
            .collect())
    }

    fn build_system_prompt(&self, agents: &[AgentCardSummary]) -> String {
        let agent_list: Vec<String> = agents
            .iter()
            .map(|a| {
                format!(
                    "- {} (ID: {}): {}\n  Skills: {}\n  Tags: {}",
                    a.name,
                    a.id,
                    a.description,
                    a.skills.join(", "),
                    a.tags.join(", ")
                )
            })
            .collect();

        format!(
            r#"You are a routing assistant. Select the best agent to handle the user's query.

Available agents:
{}

Select the most specialized agent that can handle the query. If no agent is a perfect match, choose the closest option."#,
            agent_list.join("\n\n")
        )
    }

    fn build_user_prompt(
        &self,
        query: &str,
        conversation_history: &[ConversationMessage],
    ) -> String {
        let mut prompt = String::new();

        if !conversation_history.is_empty() {
            prompt.push_str("Conversation history:\n");
            for msg in conversation_history.iter().rev().take(5).rev() {
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

fn extract_skill_names(skills_json: serde_json::Value) -> Vec<String> {
    if let Some(arr) = skills_json.as_array() {
        arr.iter()
            .filter_map(|s| s.get("name").and_then(|n| n.as_str()).map(String::from))
            .collect()
    } else {
        vec![]
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SelectorError {
    #[error("No agents available")]
    NoAgentsAvailable,
    #[error("Provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("Failed to parse selection: {0}")]
    ParseError(String),
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
}

