use std::time::Instant;

use nasiko_observability::ObservabilityProvider;
use sqlx::PgPool;
use uuid::Uuid;

use super::llm::{ChatMessage, LlmClient};
use super::types::{ExecutionResult, MafDefinition, MafStep, StepResult};

pub async fn run_maf(
    client: &reqwest::Client,
    db: &PgPool,
    observability: &dyn ObservabilityProvider,
    execution_id: Uuid,
    user_id: Uuid,
    maf_def: &MafDefinition,
    llm: &LlmClient,
) -> Result<ExecutionResult, String> {
    // Seed one "pending" entry per step and persist immediately, so the full
    // step list is visible in the DB before the (possibly slow) planning LLM
    // call even starts.
    let mut step_results: Vec<StepResult> = maf_def.steps.iter().map(pending_result).collect();
    persist_progress(db, execution_id, &step_results, 0).await;

    // ── LLM call 1: plan all steps at runtime ────────────────────────────────
    // Generates prompt templates (with <placeholders>), to_extract goals, and
    // the output_generation guideline from the task descriptions.
    // Planning happens on every execution (Python MAF parity).
    let (step_plans, output_generation, plan_tokens) = plan_execution(&maf_def.steps, llm).await?;
    let mut total_tokens = plan_tokens;

    // Fill in the prompt template / extraction goal now that planning is
    // done — steps stay "pending" until their turn in the loop below.
    for (result, plan) in step_results.iter_mut().zip(step_plans.iter()) {
        result.prompt_template = plan.prompt.clone();
        result.to_extract = plan.to_extract.clone();
    }
    persist_progress(db, execution_id, &step_results, total_tokens).await;

    for (i, (step, plan)) in maf_def.steps.iter().zip(step_plans.iter()).enumerate() {
        let context = build_context(&step_results[..i]);

        step_results[i].status = "running".to_string();
        persist_progress(db, execution_id, &step_results, total_tokens).await;

        // ── LLM call 2: fill <placeholders> with context from previous steps ─
        let (actual_prompt, prompt_tokens) =
            match generate_step_prompt(&plan.prompt, &step.task_description, &context, llm).await {
                Ok(v) => v,
                Err(e) => {
                    let err = format!("step {}: prompt generation failed: {e}", step.step_index);
                    step_results[i].status = "failed".to_string();
                    step_results[i].error = Some(err.clone());
                    persist_progress(db, execution_id, &step_results, total_tokens).await;
                    return Err(err);
                }
            };

        // ── Agent call ────────────────────────────────────────────────────────
        let start = Instant::now();
        let (traceparent, trace_id) = build_traceparent(execution_id, step.step_index);

        // Register this step as a flow so the LLM gateway sees it as IN-FLOW (not
        // inert) and its tier classifier can fire. The invariant the gateway relies
        // on (see `derive_boundary_signals`): the trace_id we forward in
        // `traceparent` IS the `flow_id` in this row — mirroring the orchestrator /
        // agent-proxy ingress. `context_id = execution_id` is stable across every
        // step, so the gateway keys its decision cache on the whole MAF run: the
        // first step writes the tier decision, later steps reuse it. Best-effort —
        // a failed insert only means this step falls back to the default model.
        let flow_metadata = serde_json::json!({
            "context_id": execution_id.to_string(),
            "mode": "free_flowing",
        });
        let _ = sqlx::query(
            r#"INSERT INTO flows (flow_id, user_id, root_agent_name, title, status, metadata)
               VALUES ($1, $2, $3, $4, 'running', $5)
               ON CONFLICT (flow_id) DO NOTHING"#,
        )
        .bind(&trace_id)
        .bind(user_id)
        .bind(&step.agent_name)
        .bind(&step.task_description)
        .bind(&flow_metadata)
        .execute(db)
        .await;

        let raw_response = match call_agent(
            client,
            &step.agent_endpoint,
            &execution_id.to_string(),
            &user_id.to_string(),
            &step.agent_id.to_string(),
            &actual_prompt,
            &traceparent,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                let err = format!(
                    "step {} (agent '{}') failed: {e}",
                    step.step_index, step.agent_name
                );
                step_results[i].status = "failed".to_string();
                step_results[i].prompt = actual_prompt;
                step_results[i].error = Some(err.clone());
                persist_progress(db, execution_id, &step_results, total_tokens).await;
                return Err(err);
            }
        };
        let latency_ms = start.elapsed().as_millis() as i64;

        // ── LLM call 3: extract relevant info from agent response ─────────────
        let (extracted, extract_tokens) = match extract_info(
            &plan.prompt,
            &actual_prompt,
            &raw_response,
            &plan.to_extract,
            &context,
            llm,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                let err = format!("step {}: extraction failed: {e}", step.step_index);
                step_results[i].status = "failed".to_string();
                step_results[i].prompt = actual_prompt;
                step_results[i].latency_ms = latency_ms;
                step_results[i].error = Some(err.clone());
                persist_progress(db, execution_id, &step_results, total_tokens).await;
                return Err(err);
            }
        };

        let llm_tokens = prompt_tokens + extract_tokens;

        // Wait for the agent's own token usage to land in Tempo (it batches
        // span export, so it's usually not there the instant the call
        // returns) so the persisted step total already reflects LLM + agent
        // cost together, not just MAF's own reasoning cost.
        let agent_tokens = wait_for_agent_tokens(observability, &trace_id).await;
        let step_tokens = llm_tokens + agent_tokens;
        total_tokens += step_tokens;

        let new_context = if context.is_empty() {
            format!(
                "Step {} ({}): {}",
                step.step_index, step.agent_name, extracted
            )
        } else {
            format!(
                "{}\nStep {} ({}): {}",
                context, step.step_index, step.agent_name, extracted
            )
        };

        step_results[i].status = "success".to_string();
        step_results[i].prompt = actual_prompt;
        step_results[i].extracted_info = Some(extracted);
        step_results[i].tokens_used = step_tokens;
        step_results[i].latency_ms = latency_ms;
        step_results[i].context = Some(new_context);
        persist_progress(db, execution_id, &step_results, total_tokens).await;
    }

    // ── LLM call 4: synthesise final output ───────────────────────────────────
    // Use the guidelines generated by the planner at runtime.
    let guidelines = &output_generation;

    let (output, output_tokens) = generate_final_output(&step_results, guidelines, llm)
        .await
        .map_err(|e| format!("final output generation failed: {e}"))?;
    total_tokens += output_tokens;

    Ok(ExecutionResult {
        output,
        step_results,
        tokens_used: total_tokens,
    })
}

/// Builds a placeholder "pending" entry for a step before planning/execution
/// has produced any of its actual content.
fn pending_result(step: &MafStep) -> StepResult {
    StepResult {
        step_id: step.step_id,
        step_index: step.step_index,
        agent_id: step.agent_id,
        agent_name: step.agent_name.clone(),
        status: "pending".to_string(),
        error: None,
        prompt_template: String::new(),
        to_extract: String::new(),
        prompt: String::new(),
        extracted_info: None,
        tokens_used: 0,
        latency_ms: 0,
        context: None,
        obs_logs: serde_json::Value::Null,
    }
}

/// Writes the current step progress snapshot to `maf_executions.step_results`.
/// Best-effort: a transient write failure here shouldn't abort the run — the
/// next transition will just overwrite with fresher data, and the final
/// write in worker.rs remains the source of truth once the run completes.
async fn persist_progress(
    db: &PgPool,
    execution_id: Uuid,
    step_results: &[StepResult],
    tokens_used: i64,
) {
    let step_json = serde_json::to_value(step_results).unwrap_or_default();
    let _ = sqlx::query(
        "UPDATE maf_executions SET step_results = $1::jsonb, tokens_used = $2 WHERE id = $3",
    )
    .bind(step_json.to_string())
    .bind(tokens_used)
    .bind(execution_id)
    .execute(db)
    .await;
}

// ─── Step plan produced by plan_execution ────────────────────────────────────

struct StepPlan {
    prompt: String,
    to_extract: String,
}

// ─── LLM call 1: runtime planner ─────────────────────────────────────────────

async fn plan_execution(
    steps: &[MafStep],
    llm: &LlmClient,
) -> Result<(Vec<StepPlan>, String, i64), String> {
    let system = "You are a MAF (Multi-Agent Flow) step planner.\n\
                  Given a list of steps (each with a task description and the agent that will \
                  handle it), generate:\n\
                  1. For each step — a prompt template and a to_extract goal.\n\
                     - IMPORTANT: For the FIRST step (step 0), NEVER use placeholders. Use the exact \
                     values (amounts, currencies, names, etc.) from the task description verbatim.\n\
                     - For subsequent steps, use <variable_name> syntax (e.g. <jpy_amount>) ONLY to \
                     reference data that was extracted from a previous step — never for values that are \
                     already stated in the task description.\n\
                     - Include the placeholder name in to_extract only when a later step needs the value.\n\
                  2. An output_generation string describing how to present the final answer to the user.\n\n\
                  Return ONLY valid JSON:\n\
                  {\n\
                    \"output_generation\": \"...\",\n\
                    \"steps\": [{\"prompt\": \"...\", \"to_extract\": \"...\"}, ...]\n\
                  }\n\
                  The steps array must have exactly one entry per input step, in the same order.";

    let one_shot_human = "Steps:\n\
                          Step 0 (Fantasy Book Recommender): Recommend at least three fantasy books\n\
                          Step 1 (Online Book Shopping Agent): Find the best deals for the recommended books";

    let one_shot_assistant = r#"{"output_generation": "Present the top three fantasy book recommendations along with the best online deal for each, including store name and price.", "steps": [{"prompt": "Recommend at least three fantasy books.", "to_extract": "The top three fantasy book recommendations including title and author (<top_three_recommendations>)"}, {"prompt": "Here are the three books I want to buy: <top_three_recommendations>. Find out the best deals for these books online.", "to_extract": "Best online deals for each book including store name, price, and a direct purchase link if available"}]}"#;

    let step_list = steps
        .iter()
        .map(|s| {
            format!(
                "Step {} ({}): {}",
                s.step_index, s.agent_name, s.task_description
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let user = format!("Steps:\n{step_list}");

    // Schema matches Python's MAFTemplate Pydantic model — strict enforcement via
    // OpenAI structured outputs, equivalent to `with_structured_output(MAFTemplate)`.
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "output_generation": {"type": "string"},
            "steps": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "prompt": {"type": "string"},
                        "to_extract": {"type": "string"}
                    },
                    "required": ["prompt", "to_extract"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["output_generation", "steps"],
        "additionalProperties": false
    });

    let (json, tokens) = llm
        .chat_json_schema(
            vec![
                ChatMessage::system(system),
                ChatMessage::user(one_shot_human),
                ChatMessage::assistant(one_shot_assistant),
                ChatMessage::user(user),
            ],
            "execution_plan",
            schema,
        )
        .await?;

    let output_generation = json["output_generation"]
        .as_str()
        .unwrap_or("Summarise all extracted information into a clear, well-structured response.")
        .to_string();

    let plans_json = json["steps"]
        .as_array()
        .ok_or_else(|| "planner returned no 'steps' array".to_string())?;

    if plans_json.len() != steps.len() {
        return Err(format!(
            "planner returned {} step plans but MAF has {} steps",
            plans_json.len(),
            steps.len()
        ));
    }

    let plans = plans_json
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let prompt = p["prompt"]
                .as_str()
                .ok_or_else(|| format!("step {i}: planner returned no 'prompt'"))?
                .to_string();
            let to_extract = p["to_extract"]
                .as_str()
                .ok_or_else(|| format!("step {i}: planner returned no 'to_extract'"))?
                .to_string();
            Ok(StepPlan { prompt, to_extract })
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok((plans, output_generation, tokens))
}

// ─── LLM call 2: prompt generator ────────────────────────────────────────────

async fn generate_step_prompt(
    template: &str,
    task_description: &str,
    context: &str,
    llm: &LlmClient,
) -> Result<(String, i64), String> {
    // No placeholders — send the template verbatim, no LLM call needed.
    if !template.contains('<') {
        return Ok((template.to_string(), 0));
    }

    // When there are no prior step results, use the task description so the LLM can
    // fill placeholders from values stated explicitly in the task (e.g. "10 rupees").
    let effective_context = if context.is_empty() {
        format!("Task description for this step: {task_description}")
    } else {
        context.to_string()
    };

    // System prompt matches Python's MAFExecutor._create_user_prompt exactly.
    let system = "You are a Multi-Agent Flow (MAF) prompt generator.\n\
                  Your task is to generate a specific, actionable user prompt for an agent \
                  in a linear workflow.\n\n\
                  You will be provided with:\n\
                  1. The **prompt template** for the current step.\n\
                  2. **Context** from previous steps in the flow, including:\n\
                     - The agents used.\n\
                     - The prompt templates used to generate prompts and the actual prompts generated.\n\
                     - What information was intended to be extracted from the agent response \
                  (Goal of Extraction).\n\
                     - The actual information that was extracted from the agent response.\n\n\
                  Your goal is to:\n\
                  - Generate a user prompt for the current step by combining the current prompt \
                  template with the available context.\n\
                  - Replace any placeholders (like <variable_name>) in the prompt template with \
                  actual data from the context.\n\
                  - Ensure the resulting prompt is clear and directly tells the agent what to do, \
                  leveraging the history of the flow.\n\n\
                  Output the result in the specified structured format.";

    // One-shot uses the verbose context format that build_context produces.
    let one_shot_human = "Current Step Prompt Template: Here are the three books I want to buy: \
                          <top_three_recommendations>. Find out the best deals for these books online.\n\n\
                          Context from Previous Steps:\n\
                          --- Step 1 (Fantasy Book Recommender) ---\n\
                          Prompt Template: Recommend at least three fantasy books.\n\
                          User Prompt Sent: Recommend at least three fantasy books.\n\
                          Goal of Extraction: The top three fantasy book recommendations including \
                          title and author (<top_three_recommendations>).\n\
                          Actual Extracted Information: 1. 'The Way of Kings' by Brandon Sanderson, \
                          2. 'The Name of the Wind' by Patrick Rothfuss, \
                          3. 'The Lies of Locke Lamora' by Scott Lynch";

    let one_shot_assistant = r#"{"prompt": "Here are the three books I want to buy: 1. 'The Way of Kings' by Brandon Sanderson, 2. 'The Name of the Wind' by Patrick Rothfuss, 3. 'The Lies of Locke Lamora' by Scott Lynch. Find out the best deals for these books online."}"#;

    let user = if effective_context.is_empty() {
        format!("Current Step Prompt Template: {template}")
    } else {
        format!(
            "Current Step Prompt Template: {template}\n\nContext from Previous Steps:\n{effective_context}"
        )
    };

    // Schema matches Python's GeneratedPrompt Pydantic model.
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "prompt": {"type": "string"}
        },
        "required": ["prompt"],
        "additionalProperties": false
    });

    let (json, tokens) = llm
        .chat_json_schema(
            vec![
                ChatMessage::system(system),
                ChatMessage::user(one_shot_human),
                ChatMessage::assistant(one_shot_assistant),
                ChatMessage::user(user),
            ],
            "generated_prompt",
            schema,
        )
        .await?;

    let prompt = json["prompt"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "LLM prompt generation returned no 'prompt' field".to_string())?;

    Ok((prompt, tokens))
}

// ─── LLM call 3: extractor ────────────────────────────────────────────────────

async fn extract_info(
    template: &str,
    actual_prompt: &str,
    response: &str,
    goal: &str,
    context: &str,
    llm: &LlmClient,
) -> Result<(String, i64), String> {
    // System prompt matches Python's MAFExecutor._extract_info exactly.
    let system = "You are a Multi-Agent Flow (MAF) information extractor.\n\
                  Your task is to extract specific information from an agent's response based \
                  on the \"goal of extraction\" for the current step.\n\n\
                  You will be provided with:\n\
                  1. The **prompt template** for the current step.\n\
                  2. The **actual prompt** sent to the agent in the current step.\n\
                  3. The **goal of extraction** for the current step.\n\
                  4. The **agent's response** for the current step.\n\
                  5. **Context** from previous steps in the flow (if any), including:\n\
                     - The agents used.\n\
                     - The prompt templates used to generate prompts.\n\
                     - The actual prompts sent to the agents.\n\
                     - What information was intended to be extracted from the agent response \
                  (Goal of Extraction).\n\
                     - The actual information that was extracted from the agent response.\n\n\
                  Your goal is to:\n\
                  - Extract all information from the agent response that is required by the \
                  goal of extraction.\n\
                  - The goal of extraction may contain placeholders (like <variable_name>), but \
                  it might also mention other specific details to capture. Ensure EVERYTHING \
                  mentioned in the goal is extracted correctly.\n\
                  - The extracted information should be formatted as a clear, standalone piece of \
                  data that can be used as a direct replacement for its context in the workflow.\n\
                  - Use the provided context from previous steps if necessary to understand the \
                  full scope of what needs to be extracted (e.g., if the goal refers to something \
                  previously mentioned).\n\
                  - Ignore any conversational filler or irrelevant information in the agent's \
                  response.\n\n\
                  Output the result in the specified structured format.";

    // One-shot agent response matches Python's _extract_info example exactly.
    let one_shot_human = "Current Step Prompt Template: Recommend at least three fantasy books.\n\
                          Current Step User Prompt Sent: Recommend at least three fantasy books.\n\
                          Current Step Goal of Extraction: The top three fantasy book recommendations \
                          including title and author (<top_three_recommendations>).\n\
                          Current Step Agent Response: Hello! I'd be happy to help. Based on your \
                          interest in fantasy, here are some great reads. First, there's 'The Way of \
                          Kings' by Brandon Sanderson, which is the start of a massive epic. Then, \
                          'The Name of the Wind' by Patrick Rothfuss is a must-read for its beautiful \
                          prose. Finally, I highly recommend 'The Lies of Locke Lamora' by Scott Lynch \
                          for some high-stakes thievery. I've also heard 'Mistborn' is good, but these \
                          three are my top picks for you. Hope this helps!\n\n\
                          Context from Previous Steps:\n\
                          None";

    let one_shot_assistant = r#"{"extracted_info": "1. 'The Way of Kings' by Brandon Sanderson, 2. 'The Name of the Wind' by Patrick Rothfuss, 3. 'The Lies of Locke Lamora' by Scott Lynch"}"#;

    // Match Python: conditionally include context section; write "None" when empty.
    let context_section = if context.is_empty() {
        "Context from Previous Steps:\nNone".to_string()
    } else {
        format!("Context from Previous Steps:\n{context}")
    };

    let user = format!(
        "Current Step Prompt Template: {template}\n\n\
         Current Step User Prompt Sent: {actual_prompt}\n\n\
         Current Step Goal of Extraction: {goal}\n\n\
         Current Step Agent Response: {response}\n\n\
         {context_section}"
    );

    // Schema matches Python's ExtractedInfo Pydantic model — strict enforcement via
    // OpenAI structured outputs, equivalent to `with_structured_output(ExtractedInfo)`.
    // The API guarantees extracted_info is always present and always a string.
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "extracted_info": {
                "type": "string",
                "description": "The information extracted from an agent's response."
            }
        },
        "required": ["extracted_info"],
        "additionalProperties": false
    });

    let (json, tokens) = llm
        .chat_json_schema(
            vec![
                ChatMessage::system(system),
                ChatMessage::user(one_shot_human),
                ChatMessage::assistant(one_shot_assistant),
                ChatMessage::user(user),
            ],
            "extraction_result",
            schema,
        )
        .await?;

    let extracted = json["extracted_info"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "LLM extraction returned no 'extracted_info' field".to_string())?;

    Ok((extracted, tokens))
}

// ─── LLM call 4: final output synthesiser ────────────────────────────────────

async fn generate_final_output(
    step_results: &[StepResult],
    guidelines: &str,
    llm: &LlmClient,
) -> Result<(String, i64), String> {
    // System prompt matches Python's MAFExecutor._create_final_output exactly.
    let system = "You are a Multi-Agent Flow (MAF) final output generator.\n\
                  Your task is to consolidate all information extracted from various agents \
                  into a single, cohesive, and helpful response for the user.\n\n\
                  You will be provided with:\n\
                  1. **Guidelines** on how to construct the final output.\n\
                  2. **Context** from previous steps in the flow, including:\n\
                     - The agents used.\n\
                     - The prompt templates used to generate prompts.\n\
                     - The actual prompts sent to the agents.\n\
                     - What information was intended to be extracted from the agent response \
                  (Goal of Extraction).\n\
                     - The actual information that was extracted from the agent response.\n\n\
                  Your goal is to:\n\
                  - Follow the provided guidelines to construct the final answer.\n\
                  - Ensure the response is well-structured, clear, and directly addresses \
                  the user's initial intent.\n\
                  - Leverage all the extracted data to provide a comprehensive result.\n\n\
                  Output the result in the specified structured format.";

    // One-shot context uses the same verbose format that build_context produces.
    let one_shot_human = "Guidelines for Output Generation: Present the top three fantasy book \
                          recommendations along with the best online deals for each, including \
                          store name, price, and a direct purchase link if available.\n\n\
                          Context from All Steps:\n\
                          --- Step 1 (Fantasy Book Recommender) ---\n\
                          Prompt Template: Recommend at least three fantasy books.\n\
                          User Prompt Sent: Recommend at least three fantasy books.\n\
                          Goal of Extraction: The top three fantasy book recommendations including \
                          title and author (<top_three_recommendations>).\n\
                          Actual Extracted Information: 1. 'The Way of Kings' by Brandon Sanderson, \
                          2. 'The Name of the Wind' by Patrick Rothfuss, \
                          3. 'The Lies of Locke Lamora' by Scott Lynch\n\n\
                          --- Step 2 (Online Book Shopping Agent) ---\n\
                          Prompt Template: Here are the three books I want to buy: \
                          <top_three_recommendations>. Find out the best deals for these books online.\n\
                          User Prompt Sent: Here are the three books I want to buy: \
                          1. 'The Way of Kings' by Brandon Sanderson, \
                          2. 'The Name of the Wind' by Patrick Rothfuss, \
                          3. 'The Lies of Locke Lamora' by Scott Lynch. \
                          Find out the best deals for these books online.\n\
                          Goal of Extraction: Best online deals for each book including store name, \
                          price, and a direct purchase link if available.\n\
                          Actual Extracted Information: \
                          1. 'The Way of Kings': Amazon $18.99 — best deal. \
                          2. 'The Name of the Wind': Powell's $15.99 — best deal. \
                          3. 'The Lies of Locke Lamora': Target $14.99 — best deal.";

    let one_shot_assistant = r#"{"final_output": "Here are the top three fantasy book recommendations with their best online deals:\n\n1. **The Way of Kings** by Brandon Sanderson\n   Best deal: Amazon — $18.99\n\n2. **The Name of the Wind** by Patrick Rothfuss\n   Best deal: Powell's Books — $15.99\n\n3. **The Lies of Locke Lamora** by Scott Lynch\n   Best deal: Target — $14.99"}"#;

    let context = build_context(step_results);
    let user = format!(
        "Guidelines for Output Generation: {guidelines}\n\nContext from All Steps:\n{context}"
    );

    // Schema matches Python's FinalOutput Pydantic model.
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "final_output": {
                "type": "string",
                "description": "The final consolidated response for the user."
            }
        },
        "required": ["final_output"],
        "additionalProperties": false
    });

    let (json, tokens) = llm
        .chat_json_schema(
            vec![
                ChatMessage::system(system),
                ChatMessage::user(one_shot_human),
                ChatMessage::assistant(one_shot_assistant),
                ChatMessage::user(user),
            ],
            "final_output",
            schema,
        )
        .await?;

    let output = json["final_output"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "LLM final output returned no 'final_output' field".to_string())?;

    Ok((output, tokens))
}

// ─── A2A agent call ───────────────────────────────────────────────────────────

/// Builds a valid W3C `traceparent` scoped to this one step — not the whole
/// execution — so each step's agent call lands under its own Tempo trace_id.
/// That's what lets per-step (not just per-execution) agent token usage be
/// looked up later without ambiguity when the same agent is used in more
/// than one step. Deterministic (execution_id + step_index) via UUIDv5, so no
/// new dependency and no random-id bookkeeping is needed to reconstruct it
/// later when reading the execution back.
///
/// Returns `(traceparent_header, trace_id)`. The bare `trace_id` (32 hex, no
/// dashes) is the value both the flow-registration insert and the Tempo token
/// lookup key on, so it's returned rather than re-derived at each call site.
fn build_traceparent(execution_id: Uuid, step_index: i32) -> (String, String) {
    let trace_uuid = Uuid::new_v5(&execution_id, step_index.to_string().as_bytes());
    let span_uuid = Uuid::new_v5(&execution_id, format!("{step_index}-span").as_bytes());
    let trace_id = trace_uuid.simple().to_string();
    let span_id = &span_uuid.simple().to_string()[..16];
    // flags=01 (sampled) — otherwise a conforming exporter may decide not to
    // export the span at all, and this whole mechanism would silently no-op.
    let traceparent = format!("00-{trace_id}-{span_id}-01");
    (traceparent, trace_id)
}

/// Polls Tempo for this step's trace (keyed by the `trace_id` that
/// `build_traceparent` forwarded) and returns its total `gen_ai.usage` tokens,
/// or 0 if nothing shows up within the timeout — Tempo/the agent being
/// unreachable never fails the step, it just means this step's persisted total
/// is LLM-only.
async fn wait_for_agent_tokens(observability: &dyn ObservabilityProvider, trace_id: &str) -> i64 {
    // Agents commonly batch-export spans every ~5s, so the trace usually
    // isn't queryable the instant the agent call returns — poll rather than
    // check once.
    for _ in 0..10 {
        if let Ok(trace) = observability.get_trace(trace_id).await {
            let (input, output, _model) = trace.token_totals();
            let total = input + output;
            if total > 0 {
                return total as i64;
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    0
}

async fn call_agent(
    client: &reqwest::Client,
    endpoint: &str,
    context_id: &str,
    user_id: &str,
    agent_id: &str,
    prompt: &str,
    traceparent: &str,
) -> Result<String, String> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "1",
        // Per the A2A spec (§5.3, §9.1): JSON-RPC method names are PascalCase,
        // matching gRPC conventions exactly — "SendMessage", not "message/send"
        // (that string is only the REST binding's URL path, a different
        // transport). Confirmed both against the spec's own example request
        // and empirically against a real deployed `oss/agents/translator`
        // build. Matches `oss/types::build_send_request`.
        "method": "SendMessage",
        "params": {
            "message": {
                "messageId": uuid::Uuid::new_v4().to_string(),
                "contextId": context_id,
                "role": "ROLE_USER",
                "parts": [{"text": prompt}]
            }
        }
    });

    // Some agents expose A2A at /jsonrpc, others at root /. Try /jsonrpc first
    // and fall back to / on 404 so both agent types work without DB changes.
    let base = endpoint.trim_end_matches('/');
    let url_jsonrpc = format!("{base}/jsonrpc");
    let url_root = format!("{base}/");

    // Mint a delegation token so this agent can call back into `/api/mcp`
    // proving "I am agent_id, acting for user_id" — mirrors `agent_proxy.rs`
    // and `a2a_dispatch.rs`. Best-effort: if JWT_SECRET is unset, MCP
    // delegation is simply unavailable to this agent rather than failing the
    // whole MAF step.
    let delegation_token = std::env::var("JWT_SECRET")
        .ok()
        .and_then(|secret| nasiko_auth::jwt::mint_delegation_token(&secret, user_id, agent_id).ok());

    let resp = {
        let mut r = client
            .post(&url_jsonrpc)
            .header("X-User-Id", user_id)
            .header("A2A-Version", "1.0")
            .header("traceparent", traceparent);
        if let Some(token) = &delegation_token {
            r = r.header("x-nasiko-agent-token", token);
        }
        let r = r
            .json(&body)
            .timeout(std::time::Duration::from_secs(300))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if r.status() == reqwest::StatusCode::NOT_FOUND {
            let mut r2 = client
                .post(&url_root)
                .header("X-User-Id", user_id)
                .header("A2A-Version", "1.0")
                .header("traceparent", traceparent);
            if let Some(token) = &delegation_token {
                r2 = r2.header("x-nasiko-agent-token", token);
            }
            r2.json(&body)
                .timeout(std::time::Duration::from_secs(300))
                .send()
                .await
                .map_err(|e| e.to_string())?
        } else {
            r
        }
    };

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

    if let Some(err) = json["error"]["message"].as_str() {
        return Err(format!("A2A error: {err}"));
    }

    Ok(extract_text(&json))
}

fn extract_text(json: &serde_json::Value) -> String {
    if let Some(text) = json["result"]["task"]["artifacts"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|a| a["parts"].as_array())
        .and_then(|p| p.first())
        .and_then(|p| p["text"].as_str())
    {
        return text.to_string();
    }
    if let Some(text) = json["result"]["task"]["status"]["message"]["parts"]
        .as_array()
        .and_then(|p| p.first())
        .and_then(|p| p["text"].as_str())
    {
        return text.to_string();
    }
    if let Some(text) = json["result"]["parts"]
        .as_array()
        .and_then(|p| p.first())
        .and_then(|p| p["text"].as_str())
    {
        return text.to_string();
    }
    if let Some(text) = json["result"].as_str() {
        return text.to_string();
    }
    String::new()
}

// ─── Utilities ────────────────────────────────────────────────────────────────

/// Mirrors Python's `_get_context`: produces one verbose block per completed step,
/// joined by blank lines, using 1-based step numbering to match Python exactly.
///
/// Format per step:
/// ```text
/// --- Step N (AgentName) ---
/// Prompt Template: <template>
/// User Prompt Sent: <actual_prompt>
/// Goal of Extraction: <to_extract>
/// Actual Extracted Information: <extracted_info>
/// ```
fn build_context(step_results: &[StepResult]) -> String {
    step_results
        .iter()
        .filter_map(|s| {
            s.extracted_info.as_deref().map(|info| {
                format!(
                    "--- Step {} ({}) ---\n\
                     Prompt Template: {}\n\
                     User Prompt Sent: {}\n\
                     Goal of Extraction: {}\n\
                     Actual Extracted Information: {}",
                    s.step_index + 1,
                    s.agent_name,
                    s.prompt_template,
                    s.prompt,
                    s.to_extract,
                    info,
                )
            })
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}
