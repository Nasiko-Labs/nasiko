use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::*;
use crate::policy::{RoutingPolicy, SelectionSchemaExtra};
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
    ///
    /// The returned `bool` is `true` when the model named an agent id that
    /// doesn't exist in `agents` and this substituted the first candidate in
    /// its place — the caller's `fallback_used` should follow it, since the
    /// selection the policy approved was the hallucinated pick, not the
    /// substitute.
    pub async fn select_agent(
        &self,
        query: &str,
        conversation_history: &[ConversationMessage],
        agents: &[AgentCardSummary],
        policy: Option<&dyn RoutingPolicy>,
    ) -> Result<(AgentSelection, CompletionResult, bool), SelectorError> {
        if agents.is_empty() {
            return Err(SelectorError::NoAgentsAvailable);
        }

        let system_prompt = self.build_system_prompt(agents, policy);
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
            max_tokens: Some(2048),
            response_format: Some(ResponseFormat::JsonSchema {
                json_schema: JsonSchema {
                    name: "agent_selection".to_string(),
                    strict: Some(true),
                    schema: selection_schema(policy),
                },
            }),
            stream_options: None,
        };

        let result = self.provider.chat_completion(&request).await?;

        // Parsed twice on purpose: once as the raw object the policy judges —
        // it reads the fields it asked for, which this crate has no reason to
        // know the shape of — and once as the pick the engine acts on.
        let raw: serde_json::Value = serde_json::from_str(&result.content)
            .map_err(|e| SelectorError::ParseError(e.to_string()))?;
        let selection: AgentSelection = serde_json::from_value(raw.clone())
            .map_err(|e| SelectorError::ParseError(e.to_string()))?;

        // The policy is consulted BEFORE the hallucination fallback below: a
        // selection it refuses must be refused outright, not quietly redirected
        // to `agents[0]`, which is how a refusal used to turn into an arbitrary
        // pick.
        //
        // The completion rides the error. A refused selection cost exactly as
        // much as an accepted one — the provider was called, the tokens were
        // billed — and dropping the usage here made the refusal free in FinOps
        // and invisible in `token_usage`, which is precisely the wrong shape for
        // a policy an operator is tuning by watching what it spends.
        if let Some(p) = policy
            && let Err(reason) = p.check_selection(&raw)
        {
            return Err(SelectorError::PolicyRefused {
                reason,
                usage: Box::new(result),
            });
        }

        // Validate agent UUID exists in the candidate list; fall back to first if hallucinated.
        if !agents.iter().any(|a| a.id == selection.agent_id)
            && let Some(first) = agents.first()
        {
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
                true,
            ));
        }

        Ok((selection, result, false))
    }

    /// Fetch routable (running or parked) agents directly from DB — used by the orchestrator path.
    ///
    /// A soft-deleted agent is never routable, whatever its row's status says: rows deleted
    /// before the delete route also stopped them still read `running`.
    pub async fn fetch_active_agents(db: &PgPool) -> Result<Vec<AgentCardSummary>, sqlx::Error> {
        let rows = sqlx::query_as::<_, AgentCardRow>(
            "SELECT id, name, description, skills, tags FROM agents \
             WHERE status IN ('running', 'parked') AND deleted_at IS NULL AND NOT is_internal \
             ORDER BY name",
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

    fn build_system_prompt(
        &self,
        agents: &[AgentCardSummary],
        policy: Option<&dyn RoutingPolicy>,
    ) -> String {
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

        // Both halves come from the policy already worded, so this crate never
        // holds an operator-facing sentence it cannot itself enforce.
        let prefix = policy.map(|p| p.prompt_prefix()).unwrap_or_default();
        let closing = policy
            .and_then(|p| p.closing_instruction())
            .unwrap_or_else(|| DEFAULT_CLOSING_INSTRUCTION.to_string());

        format!(
            "You are a routing assistant. Select the best agent to handle the user's query.{}\n\n\
             Available agents:\n{}\n\n{}",
            prefix,
            list.join("\n\n"),
            closing
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

/// What the selector tells the model to do when no policy replaces it.
///
/// Deliberately permissive: with no bar to clear, refusing to pick is worse than
/// picking imperfectly, because nothing downstream can act on the refusal.
const DEFAULT_CLOSING_INSTRUCTION: &str =
    "Select the most specialized agent. If no perfect match, choose the closest option.";

/// The structured-output schema for one selection: what the engine needs back,
/// plus whatever the policy asked the model to judge its own pick on.
fn selection_schema(policy: Option<&dyn RoutingPolicy>) -> serde_json::Value {
    let mut properties = serde_json::Map::new();
    properties.insert(
        "agent_id".to_string(),
        json!({ "type": "string", "description": "UUID of the selected agent" }),
    );
    properties.insert(
        "agent_name".to_string(),
        json!({ "type": "string", "description": "Name of the selected agent" }),
    );
    properties.insert(
        "reasoning".to_string(),
        json!({ "type": "string", "description": "Why this agent was selected" }),
    );
    let mut required = vec![
        "agent_id".to_string(),
        "agent_name".to_string(),
        "reasoning".to_string(),
    ];

    if let Some(SelectionSchemaExtra {
        properties: extra,
        required: extra_required,
    }) = policy.and_then(|p| p.selection_schema_extra())
    {
        for (name, schema) in extra {
            properties.entry(name).or_insert(schema);
        }
        for name in extra_required {
            if !required.contains(&name) {
                required.push(name);
            }
        }
    }

    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
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
            let examples = s
                .get("examples")
                .and_then(|e| e.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|e| e.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            Some(super::models::SkillSummary {
                name,
                description,
                examples,
            })
        })
        .collect()
}

#[derive(Debug, thiserror::Error)]
pub enum SelectorError {
    #[error("no agents available")]
    NoAgentsAvailable,
    /// The caller's `RoutingPolicy` refused the model's pick. `reason` is the
    /// policy's own wording, relayed rather than rephrased — this crate has no
    /// idea what the policy was checking for. `usage` is the completion that
    /// produced the refused pick, so the caller can account for tokens that were
    /// spent whether or not the selection survived; boxed to keep the error
    /// small, since every `select_agent` result carries it.
    #[error("the routing policy refused this selection: {reason}")]
    PolicyRefused {
        reason: String,
        usage: Box<CompletionResult>,
    },
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("failed to parse selection: {0}")]
    ParseError(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

#[cfg(test)]
mod skill_extraction_tests {
    use super::extract_skills;

    /// The AgentCard's `examples` are the literal inputs a skill answers to. Dropping them left
    /// the planner inventing its own wording for every delegation — a skill keyed on an exact
    /// phrase ("hitl auth test") then never fired through the orchestrator, only in direct chat.
    #[test]
    fn examples_survive_extraction() {
        let skills = extract_skills(serde_json::json!([
            {
                "id": "hitl-auth-demo",
                "name": "HITL Auth-Required Fixture",
                "description": "Pauses with AUTH_REQUIRED.",
                "examples": ["hitl auth test"],
            },
            { "name": "No examples here", "description": "Still a skill." },
        ]));

        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].examples, vec!["hitl auth test".to_string()]);
        // A skill that documents none is not a parse failure — it just has nothing to relay.
        assert!(skills[1].examples.is_empty());
    }
}

#[cfg(test)]
mod system_prompt_tests {
    use super::*;

    fn selector() -> AgentSelector {
        AgentSelector::new(
            LLMProvider::new(reqwest::Client::new(), String::new(), String::new()),
            "test-model".to_string(),
        )
    }

    /// Answers every hook with something recognisable, so the assertions below
    /// are about the WIRING rather than about any particular policy: what a
    /// policy decides belongs to whatever implementation is plugged in, but the
    /// fact that its text and its refusal actually reach the model and the
    /// caller belongs to this crate.
    #[derive(Debug)]
    struct StubPolicy;

    impl RoutingPolicy for StubPolicy {
        fn prompt_prefix(&self) -> String {
            "\n\n## Stub Rules\n\n- Be stubby.".to_string()
        }
        fn closing_instruction(&self) -> Option<String> {
            Some("Score it honestly.".to_string())
        }
        fn selection_schema_extra(&self) -> Option<SelectionSchemaExtra> {
            let mut properties = serde_json::Map::new();
            properties.insert("stub_score".to_string(), json!({ "type": "number" }));
            Some(SelectionSchemaExtra {
                properties,
                required: vec!["stub_score".to_string()],
            })
        }
        fn check_selection(&self, selection: &serde_json::Value) -> Result<(), String> {
            match selection.get("stub_score").and_then(|v| v.as_f64()) {
                Some(v) if v >= 50.0 => Ok(()),
                _ => Err("the stub refused this pick".to_string()),
            }
        }
    }

    /// With no policy the selector must behave exactly as it did before the seam
    /// existed — same instruction, and nothing asked for that nothing reads.
    #[test]
    fn no_policy_keeps_the_built_in_instruction() {
        let out = selector().build_system_prompt(&[], None);
        assert!(out.ends_with(DEFAULT_CLOSING_INSTRUCTION), "{out}");
    }

    /// The schema an unconfigured deployment sends, pinned against the literal
    /// this crate shipped before the seam existed — not against a restatement
    /// of it. Asserting only the field names would pass while a description,
    /// `additionalProperties`, or the ordering silently drifted, and this is a
    /// `strict` schema: every one of those is part of the contract the provider
    /// enforces.
    #[test]
    fn no_policy_sends_the_pre_seam_schema_verbatim() {
        assert_eq!(
            selection_schema(None),
            json!({
                "type": "object",
                "properties": {
                    "agent_id":   { "type": "string", "description": "UUID of the selected agent" },
                    "agent_name": { "type": "string", "description": "Name of the selected agent" },
                    "reasoning":  { "type": "string", "description": "Why this agent was selected" }
                },
                "required": ["agent_id", "agent_name", "reasoning"],
                "additionalProperties": false
            })
        );
    }

    /// A policy owns the wording at both ends: its own preamble text, and the
    /// closing instruction it replaces the built-in one with.
    #[test]
    fn a_policy_supplies_both_halves_of_the_prompt() {
        let out = selector().build_system_prompt(&[], Some(&StubPolicy));

        assert!(out.contains("- Be stubby."), "{out}");
        assert!(out.ends_with("Score it honestly."), "{out}");
        assert!(
            !out.contains(DEFAULT_CLOSING_INSTRUCTION),
            "a policy's instruction replaces the built-in one, not argues with it:\n{out}"
        );
    }

    /// The engine's own three fields are never overwritten, and the policy's are
    /// added — it can extend the schema, not rewrite it.
    #[test]
    fn a_policy_adds_to_the_selection_schema_without_replacing_it() {
        let schema = selection_schema(Some(&StubPolicy));

        assert!(schema["properties"]["agent_id"].is_object());
        assert!(schema["properties"]["stub_score"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("agent_id")));
        assert!(required.contains(&json!("stub_score")));
    }

    /// Stands in for the completion a refused selection was produced by — the
    /// provider call that has already been paid for by the time the policy gets
    /// a look at it.
    fn completion_result() -> CompletionResult {
        CompletionResult {
            content: "{}".to_string(),
            finish_reason: Some("stop".to_string()),
            usage: crate::models::CompletionUsage {
                prompt_tokens: 900,
                completion_tokens: 40,
                total_tokens: 940,
                prompt_tokens_details: None,
                completion_tokens_details: None,
            },
            latency_ms: 120,
            provider: "openai".to_string(),
            model: "test-model".to_string(),
        }
    }

    /// Regression: the refusal used to be `PolicyRefused(String)`, so the
    /// completion that produced the refused pick — already billed by the
    /// provider — was dropped on the floor and the request showed up in FinOps
    /// as having cost nothing.
    #[test]
    fn a_refusal_carries_the_tokens_it_already_spent() {
        let err = SelectorError::PolicyRefused {
            reason: "nope".to_string(),
            usage: Box::new(completion_result()),
        };
        let SelectorError::PolicyRefused { usage, .. } = err else {
            unreachable!("constructed as a refusal")
        };
        assert_eq!(usage.usage.total_tokens, 940);
        assert_eq!(usage.model, "test-model");
    }

    /// The refusal reaches the caller as the policy worded it, not rephrased.
    #[test]
    fn a_refused_selection_relays_the_policy_reason_verbatim() {
        let err = StubPolicy
            .check_selection(&json!({ "stub_score": 10 }))
            .expect_err("10 is below the stub's own bar");
        assert_eq!(err, "the stub refused this pick");
        assert!(
            SelectorError::PolicyRefused {
                reason: err.clone(),
                usage: Box::new(completion_result()),
            }
            .to_string()
            .contains("the stub refused this pick")
        );
    }
}
