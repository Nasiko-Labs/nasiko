use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::llm::{ChatMessage, LlmClient};

/// Minimal agent info needed for planning.
#[derive(Debug, Clone, Serialize)]
pub struct AgentInfo {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
}

/// One step as returned by the planner LLM — agent_id is a string so we can
/// resolve it ourselves (LLMs sometimes return a name instead of the raw UUID).
#[derive(Debug, Clone, Deserialize)]
struct RawPlannedStep {
    /// Either a UUID string or an agent name — we resolve below.
    agent_id: String,
    pub task_description: String,
}

/// One step with a resolved UUID.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PlannedStep {
    pub agent_id: Uuid,
    pub task_description: String,
}

/// Full plan returned by the planner.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MafPlan {
    pub name: String,
    pub description: String,
    pub output_generation: String,
    pub steps: Vec<PlannedStep>,
}

#[derive(Deserialize)]
struct RawMafPlan {
    pub name: String,
    pub description: String,
    pub output_generation: String,
    pub steps: Vec<RawPlannedStep>,
}

const SYSTEM_PROMPT: &str = r#"You are a strict MAF (Multi-Agent Flow) planning engine.

Given a user's workflow description and a list of available agents (each with an "id" UUID and a "name"), produce a JSON execution plan.

Rules:
1. Select the best agent for each numbered step from the provided list only.
2. For "agent_id" use the EXACT "id" UUID string from the agent list — copy it character-for-character.
3. For "task_description" write a clear, concise description of what this step should accomplish.
   Do NOT write the actual prompt — the system will generate prompts and placeholders at execution time.
4. "name": a short title for the workflow.
5. "description": a one-sentence summary of the overall workflow goal.
6. "output_generation": how to format the final synthesised answer for the user.

Return ONLY valid JSON — no markdown, no explanation, no code fences.

Schema:
{
  "name": "string",
  "description": "string",
  "output_generation": "string",
  "steps": [
    {
      "agent_id": "<exact UUID from agent list>",
      "task_description": "string"
    }
  ]
}"#;

/// Plan a MAF from a natural language description + list of available agents.
pub async fn plan_maf(
    description: &str,
    agents: &[AgentInfo],
    llm: &LlmClient,
) -> Result<MafPlan, String> {
    // Build a simple table so the LLM sees UUID → name clearly
    let agent_table = agents
        .iter()
        .map(|a| {
            let desc = a.description.as_deref().unwrap_or("no description");
            format!("  id: {}  name: \"{}\"  description: \"{}\"", a.id, a.name, desc)
        })
        .collect::<Vec<_>>()
        .join("\n");

    let user_msg = format!(
        "Available agents:\n{agent_table}\n\nWorkflow description:\n{description}"
    );

    let messages = vec![ChatMessage::system(SYSTEM_PROMPT), ChatMessage::user(user_msg)];

    let (json, _tokens) = llm.chat_json(messages).await?;
    let raw: RawMafPlan =
        serde_json::from_value(json).map_err(|e| format!("planner output parse error: {e}"))?;

    if raw.steps.is_empty() {
        return Err("planner returned zero steps".into());
    }

    // Resolve each agent_id: try UUID parse first, fall back to name match
    let mut steps = Vec::with_capacity(raw.steps.len());
    for (i, step) in raw.steps.into_iter().enumerate() {
        let agent = if let Ok(uuid) = step.agent_id.parse::<Uuid>() {
            agents.iter().find(|a| a.id == uuid)
        } else {
            // LLM returned a name — match case-insensitively
            let lower = step.agent_id.to_lowercase();
            agents.iter().find(|a| a.name.to_lowercase() == lower)
        };

        let agent = agent.ok_or_else(|| {
            format!("step {i}: '{}' does not match any available agent", step.agent_id)
        })?;

        steps.push(PlannedStep {
            agent_id: agent.id,
            task_description: step.task_description,
        });
    }

    Ok(MafPlan {
        name: raw.name,
        description: raw.description,
        output_generation: raw.output_generation,
        steps,
    })
}
