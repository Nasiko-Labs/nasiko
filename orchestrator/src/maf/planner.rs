use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::llm::{ChatMessage, LlmClient, LlmError, LlmUsage};

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
///
/// Returns the plan and what the model call cost. The usage is `Some` whenever the model
/// answered, including when its answer could not be turned into a plan: that call was still
/// paid for, and the caller meters it either way.
pub async fn plan_maf(
    description: &str,
    agents: &[AgentInfo],
    llm: &LlmClient,
) -> (Result<MafPlan, LlmError>, Option<LlmUsage>) {
    // Build a simple table so the LLM sees UUID → name clearly
    let agent_table = agents
        .iter()
        .map(|a| {
            let desc = a.description.as_deref().unwrap_or("no description");
            format!(
                "  id: {}  name: \"{}\"  description: \"{}\"",
                a.id, a.name, desc
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let user_msg =
        format!("Available agents:\n{agent_table}\n\nWorkflow description:\n{description}");

    let messages = vec![
        ChatMessage::system(SYSTEM_PROMPT),
        ChatMessage::user(user_msg),
    ];

    match llm.chat_json(messages).await {
        Ok((json, usage)) => (
            parse_plan(json, agents).map_err(LlmError::Other),
            Some(usage),
        ),
        Err(e) => (Err(e), None),
    }
}

/// Turn the planner model's JSON into a plan whose every step names one of `agents`.
fn parse_plan(json: serde_json::Value, agents: &[AgentInfo]) -> Result<MafPlan, String> {
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
            format!(
                "step {i}: '{}' does not match any available agent",
                step.agent_id
            )
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

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(name: &str) -> AgentInfo {
        AgentInfo {
            id: Uuid::new_v4(),
            name: name.to_string(),
            description: None,
        }
    }

    fn plan_json(agent_ids: &[&str]) -> serde_json::Value {
        let steps: Vec<_> = agent_ids
            .iter()
            .map(|id| serde_json::json!({"agent_id": id, "task_description": "do it"}))
            .collect();
        serde_json::json!({
            "name": "n", "description": "d", "output_generation": "o", "steps": steps,
        })
    }

    #[test]
    fn steps_resolve_by_uuid_or_case_insensitive_name() {
        let agents = [agent("Weather"), agent("Writer")];
        let by_id = agents[0].id.to_string();
        let plan = parse_plan(plan_json(&[&by_id, "WRITER"]), &agents).expect("valid plan");
        let ids: Vec<Uuid> = plan.steps.iter().map(|s| s.agent_id).collect();
        assert_eq!(ids, vec![agents[0].id, agents[1].id]);
    }

    #[test]
    fn a_step_naming_no_offered_agent_is_refused() {
        let agents = [agent("Weather")];
        let err = parse_plan(plan_json(&["Nobody"]), &agents).expect_err("unknown agent");
        assert!(err.contains("does not match any available agent"), "{err}");
    }

    #[test]
    fn a_plan_with_no_steps_is_refused() {
        let err = parse_plan(plan_json(&[]), &[agent("Weather")]).expect_err("zero steps");
        assert_eq!(err, "planner returned zero steps");
    }
}
