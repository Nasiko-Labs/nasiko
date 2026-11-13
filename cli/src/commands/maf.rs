//! `nasiko maf` — create, run, and inspect MAF (Multi-Agent Flow) workflows.
//!
//! Wraps `/api/maf/workflows*` (workflow CRUD + run) and `/api/maf/workflow/{id}/executions` +
//! `/api/maf/executions` + `/api/maf/execution/{id}` + `/api/maf/workflow/result/{exec_id}`
//! (execution listing/inspection). See `oss/server/src/maf.rs` for the route definitions this
//! mirrors.

use anyhow::Result;
use nasiko_types::maf::STOPPED;
use nasiko_utils::term;
use serde_json::{Value, json};

use crate::api::{Attempt, Client, unwrap_data};
use crate::commands::agents::resolve_agent_id;
use crate::commands::local_agents::maf_hint;

// ─── End-to-end trace ───────────────────────────────────────────────────────

/// `nasiko maf trace "<instruction>"` — the whole MAF pipeline in one command.
///
/// Creates a workflow from one compound instruction, runs it, and reports every
/// stage as it happens: how the planner split the sentence and which agent it
/// gave each step, each step's outcome as it lands, and the closing token/cost
/// breakdown.
///
/// This exists because no single existing subcommand answers "is MAF working
/// end to end" — `workflow create`, `workflow run --wait` and `execution get`
/// each show one stage, and a failure in the seam between them is invisible
/// until you run all three by hand and compare.
pub fn trace(
    instruction: &str,
    content: Option<&str>,
    cleanup: bool,
    json_out: bool,
) -> Result<()> {
    let client = Client::from_active_cluster()?;

    let (workflow, plan_secs) = create_from_instruction(&client, instruction)?;
    let workflow_id = workflow
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let steps = planned_steps(&workflow);

    if !json_out {
        print_plan(&workflow, &steps, plan_secs);
    }

    let (exec_id, number) = queue_run(&client, &workflow_id, content)?;
    if !json_out {
        println!(
            "\n{}",
            term::bold(&format!("3. Running execution #{number} ({exec_id})"))
        );
    }
    let execution = poll_with_step_progress(&client, &exec_id, &steps, json_out)?;

    if json_out {
        // stdout carries the JSON document and nothing else, so `--cleanup` deletes silently.
        let combined = json!({ "workflow": workflow, "execution": execution });
        println!("{}", serde_json::to_string_pretty(&combined)?);
        if cleanup {
            delete_workflow(&client, &workflow_id)?;
        }
    } else {
        print_trace_summary(&client, &execution, &exec_id);
        print_followups(&client, &workflow_id, cleanup)?;
    }

    // A failed or stopped run is reported in full above and *then* fails the
    // command, so the details stay on screen and the exit code still tells a
    // script the truth.
    fail_unless_succeeded(
        execution
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("?"),
    )
}

/// An execution's status that nothing will change any more.
fn is_finished(status: &str) -> bool {
    matches!(status, "success" | "failed" | STOPPED)
}

/// The exit status for a finished execution: an error unless it succeeded.
fn fail_unless_succeeded(status: &str) -> Result<()> {
    match status {
        "failed" => anyhow::bail!("execution failed"),
        STOPPED => anyhow::bail!("execution was stopped"),
        _ => Ok(()),
    }
}

/// Creates the workflow, returning it with how long the call took.
///
/// The elapsed time is worth surfacing on its own: planning is an LLM call, so
/// when a trace feels slow this number says whether that is where the time went.
fn create_from_instruction(client: &Client, instruction: &str) -> Result<(Value, f64)> {
    let spin = nasiko_utils::term::start_status("planning instruction");
    let start = std::time::Instant::now();
    let workflow = plan_and_create(client, instruction, None, None);
    drop(spin);
    Ok((workflow?, start.elapsed().as_secs_f64()))
}

/// Splits `instruction` into steps with the server's MAF planner (`POST /maf/generate`, the call
/// the web UI makes). The planner gives every step its own agent, so the returned steps need no
/// further routing. Also returns the planner's own name/description, for a caller with none of
/// its own to fall back to.
fn plan_steps(
    client: &Client,
    instruction: &str,
) -> Result<(Vec<Value>, Option<String>, Option<String>)> {
    let plan: Value =
        unwrap_data(client.post_json("/maf/generate", &json!({ "description": instruction }))?)?;
    let steps: Vec<Value> = plan
        .get("steps")
        .and_then(Value::as_array)
        .map(|steps| {
            steps
                .iter()
                .map(|s| {
                    json!({
                        "task_description": s.get("task_description"),
                        "agent_id": s.get("agent_id"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    if steps.is_empty() {
        anyhow::bail!("the planner returned no steps for this instruction");
    }
    let name = plan.get("name").and_then(Value::as_str).map(String::from);
    let description = plan
        .get("description")
        .and_then(Value::as_str)
        .map(String::from);
    Ok((steps, name, description))
}

/// Plans `instruction` and saves the result as a live workflow in one call —
/// `name`/`description` override the planner's own.
fn plan_and_create(
    client: &Client,
    instruction: &str,
    name: Option<&str>,
    description: Option<&str>,
) -> Result<Value> {
    let (steps, plan_name, plan_description) = plan_steps(client, instruction)?;
    let body = json!({
        "name": name.or(plan_name.as_deref()),
        "description": description.or(plan_description.as_deref()),
        "steps": steps,
    });
    unwrap_data(client.post_json("/maf/workflows", &body)?)
}

/// `POST /maf/workflow/draft` — the bare-sentence row `save_generated_as_draft`/
/// `save_steps_as_draft` then fill in with `PUT`. Returns the new draft's id.
fn save_bare_draft(client: &Client, instruction: &str) -> Result<String> {
    let draft: Value = unwrap_data(client.post_json(
        "/maf/workflow/draft",
        &json!({ "instruction": instruction }),
    )?)?;
    draft
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("draft response did not include an id"))
}

/// Plans `instruction` like [`plan_and_create`], but saves the result as a draft instead of a
/// live workflow — the CLI counterpart of the web UI's Generate-then-"Save draft" action.
/// `nasiko maf workflow promote` deploys it once it's ready.
fn save_generated_as_draft(
    client: &Client,
    instruction: &str,
    name: Option<&str>,
    description: Option<&str>,
) -> Result<Value> {
    let draft_id = save_bare_draft(client, instruction)?;
    let (steps, plan_name, plan_description) = plan_steps(client, instruction)?;
    let body = json!({
        "name": name.or(plan_name.as_deref()),
        "description": description.or(plan_description.as_deref()),
        "steps": steps,
    });
    unwrap_data(client.put_json(&format!("/maf/workflow/{draft_id}"), &body)?)
}

/// Saves caller-given steps as a draft instead of a live workflow — the CLI counterpart of the
/// web UI's "Save draft" action on hand-typed steps.
fn save_steps_as_draft(
    client: &Client,
    name: Option<&str>,
    description: Option<&str>,
    step_bodies: Vec<Value>,
) -> Result<Value> {
    // The draft needs some text before it has steps (`save_bare_draft`'s `instruction`); the
    // name or description stands in for it when given, else the steps themselves.
    let working_title = name.or(description).map(str::to_string).unwrap_or_else(|| {
        step_bodies
            .iter()
            .filter_map(|s| s.get("task_description").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("; ")
    });
    let draft_id = save_bare_draft(client, &working_title)?;
    let body = json!({ "name": name, "description": description, "steps": step_bodies });
    unwrap_data(client.put_json(&format!("/maf/workflow/{draft_id}"), &body)?)
}

/// One step of the plan, as stored in the workflow definition.
struct PlannedStep {
    index: i64,
    agent: String,
    task: String,
}

fn planned_steps(workflow: &Value) -> Vec<PlannedStep> {
    let empty = Vec::new();
    workflow
        .get("maf_json")
        .and_then(|m| m.get("steps"))
        .and_then(Value::as_array)
        .unwrap_or(&empty)
        .iter()
        .map(|s| PlannedStep {
            index: s.get("step_index").and_then(Value::as_i64).unwrap_or(0),
            agent: s
                .get("agent_name")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string(),
            task: s
                .get("task_description")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string(),
        })
        .collect()
}

fn print_plan(workflow: &Value, steps: &[PlannedStep], plan_secs: f64) {
    let name = workflow.get("name").and_then(Value::as_str).unwrap_or("?");
    let id = workflow.get("id").and_then(Value::as_str).unwrap_or("?");
    println!(
        "\n{}",
        term::bold(&format!(
            "1. Planned into {} step(s) in {plan_secs:.1}s",
            steps.len()
        ))
    );
    println!("\n{}", term::bold(&format!("2. Workflow '{name}' ({id})")));
    for step in steps {
        println!("     {}. [{}] {}", step.index, step.agent, step.task);
    }
}

/// Queues a run of the workflow, returning the execution's id and number. Prints nothing:
/// under `--json` stdout must stay a single JSON document.
fn queue_run(client: &Client, workflow_id: &str, content: Option<&str>) -> Result<(String, i64)> {
    let resp: Value = unwrap_data(client.post_json(
        &format!("/maf/workflow/{workflow_id}/run"),
        &json!({ "content": content }),
    )?)?;
    let exec_id = resp
        .get("execution_id")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_string();
    let number = resp
        .get("execution_number")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    Ok((exec_id, number))
}

/// Polls the execution, printing each step the moment it reaches a terminal
/// state rather than only dumping the lot at the end.
///
/// A MAF run is a sequence of agent calls that can each take tens of seconds,
/// so a silent wait gives no way to tell a slow step from a wedged one. Steps
/// are reported once each, in the order they finish — which, execution being
/// sequential, is also step order.
fn poll_with_step_progress(
    client: &Client,
    exec_id: &str,
    steps: &[PlannedStep],
    json_out: bool,
) -> Result<Value> {
    let mut reported = vec![false; steps.len()];
    let mut spin = Some(nasiko_utils::term::start_status("waiting for execution"));

    for _ in 0..MAX_POLL_ATTEMPTS {
        let execution: Value = unwrap_data(client.get_json(&format!("/maf/execution/{exec_id}"))?)?;

        if !json_out {
            // The spinner owns the current line; drop it before printing a step
            // and start a fresh one after, or the two interleave.
            let newly_done = report_finished_steps(&execution, &mut reported, &mut spin);
            if newly_done {
                spin = Some(nasiko_utils::term::start_status("waiting for execution"));
            }
        }

        if execution
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(is_finished)
        {
            drop(spin);
            return Ok(execution);
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    drop(spin);
    anyhow::bail!(
        "still running after {}s — the run continues server-side. Check it with: \
{} execution result {exec_id}",
        MAX_POLL_ATTEMPTS as u64 * POLL_INTERVAL.as_secs(),
        maf_hint()
    )
}

/// Prints any step that has finished since the last poll. Returns whether it
/// printed anything, so the caller knows to restart its spinner.
fn report_finished_steps(
    execution: &Value,
    reported: &mut [bool],
    spin: &mut Option<nasiko_utils::term::StatusHandle>,
) -> bool {
    let empty = Vec::new();
    let results = execution
        .get("step_results")
        .and_then(Value::as_array)
        .unwrap_or(&empty);

    let mut printed = false;
    for step in results {
        let idx = step.get("step_index").and_then(Value::as_i64).unwrap_or(0) as usize;
        let status = step
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("pending");
        if idx >= reported.len() || reported[idx] || !is_finished(status) {
            continue;
        }
        if !printed {
            spin.take();
            printed = true;
        }
        reported[idx] = true;
        let agent = step
            .get("agent_name")
            .and_then(Value::as_str)
            .unwrap_or("?");
        let latency = step.get("latency_ms").and_then(Value::as_i64).unwrap_or(0);
        let tokens = step.get("tokens_used").and_then(Value::as_i64).unwrap_or(0);
        println!(
            "     {idx}. [{agent}] {}  {latency}ms  {tokens} tokens",
            colored_status(status)
        );
        if let Some(Value::String(error)) = step.get("error")
            && !error.is_empty()
        {
            println!("        {}", term::red(&format!("error: {error}")));
        }
    }
    printed
}

fn print_trace_summary(client: &Client, execution: &Value, exec_id: &str) {
    let status = execution
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let duration = execution
        .get("duration_ms")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let tokens = execution
        .get("tokens_used")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    println!(
        "\n{} {}  {duration}ms  {tokens} orchestration tokens",
        term::bold("4. Result:"),
        colored_status(status)
    );

    print_agent_usage(client, exec_id);

    if let Some(Value::String(output)) = execution.get("output")
        && !output.is_empty()
    {
        println!("\n   output:\n{output}");
    }
    if let Some(Value::String(error)) = execution.get("error")
        && !error.is_empty()
    {
        println!("\n   {}", term::red(&format!("error: {error}")));
    }
}

/// Prints the per-step agent-side token figures.
///
/// These are served separately from the execution row because collecting them
/// inline costs seconds per step, so they are only ever available from this
/// endpoint. A step shows `-` when its agent emits no instrumented spans: that
/// is unknown, not zero, and printing `0` would read as a fact.
fn print_agent_usage(client: &Client, exec_id: &str) {
    let Ok(raw) = client.get_json(&format!("/maf/execution/{exec_id}/usage")) else {
        return;
    };
    let Ok(usage) = unwrap_data::<Value>(raw) else {
        return;
    };
    let empty = Vec::new();
    let steps = usage
        .get("steps")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    if steps.is_empty() {
        return;
    }

    println!("\n   agent-side usage:");
    for step in steps {
        let idx = step.get("step_index").and_then(Value::as_i64).unwrap_or(0);
        let agent = step
            .get("agent_name")
            .and_then(Value::as_str)
            .unwrap_or("?");
        let resolved = step
            .get("resolved")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if resolved {
            let input = step
                .get("input_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let output = step
                .get("output_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let model = step.get("model").and_then(Value::as_str).unwrap_or("?");
            println!("     {idx}. [{agent}] in={input} out={output} model={model}");
        } else {
            println!("     {idx}. [{agent}] - (agent emits no instrumented spans)");
        }
    }
}

fn print_followups(client: &Client, workflow_id: &str, cleanup: bool) -> Result<()> {
    if cleanup {
        delete_workflow(client, workflow_id)?;
        println!();
        term::print_success(&format!("Deleted workflow {workflow_id}"));
        return Ok(());
    }
    println!("\nWorkflow kept: {workflow_id}");
    let maf = maf_hint();
    println!(
        "   re-run:  {}",
        term::cyan(&format!("{maf} workflow run {workflow_id} --wait"))
    );
    println!(
        "   delete:  {}",
        term::cyan(&format!("{maf} workflow delete {workflow_id}"))
    );
    Ok(())
}

/// `trace --cleanup`: removes the workflow the trace created.
fn delete_workflow(client: &Client, workflow_id: &str) -> Result<()> {
    client.delete(&format!("/maf/workflow/{workflow_id}"))
}

// ─── Workflow commands ──────────────────────────────────────────────────────

/// `nasiko maf workflow list` — list your MAF workflows.
pub fn workflow_list(json_out: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let (workflows, total) = list_paginated(&client, "/maf/workflows")?;

    if json_out {
        println!("{}", serde_json::to_string_pretty(&workflows)?);
        return Ok(());
    }
    if workflows.is_empty() {
        term::print_info(&format!(
            "No MAF workflows. Create one with `{}`, or check `{}` for unfinished ones.",
            term::cyan(&format!("{} workflow create", maf_hint())),
            term::cyan(&format!("{} workflow drafts", maf_hint()))
        ));
        return Ok(());
    }
    println!("Your MAF workflows:");
    print_workflow_rows(&workflows);
    print_truncation_note(workflows.len(), total);
    Ok(())
}

/// `nasiko maf workflow drafts` — list your MAF drafts (NAS-697): a bare sentence not yet split
/// into steps, or a decomposed-but-unpromoted plan, either way. A promoted draft stays listed
/// here too (`status` tells the two apart) — the drafts view follows an idea from the sentence
/// typed for it through to deployment, rather than losing it the moment it goes live.
pub fn workflow_drafts(json_out: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let (drafts, total) = list_paginated(&client, "/maf/workflow/drafts")?;

    if json_out {
        println!("{}", serde_json::to_string_pretty(&drafts)?);
        return Ok(());
    }
    if drafts.is_empty() {
        term::print_info(&format!(
            "No MAF drafts. Save one with `{}`.",
            term::cyan(&format!("{} workflow create --draft", maf_hint()))
        ));
        return Ok(());
    }
    println!("Your MAF drafts:");
    print_workflow_rows(&drafts);
    print_truncation_note(drafts.len(), total);
    Ok(())
}

/// Shared row format for `workflow list` and `workflow drafts`.
fn print_workflow_rows(workflows: &[Value]) {
    for w in workflows {
        let id = w.get("id").and_then(Value::as_str).unwrap_or("?");
        let name = w.get("name").and_then(Value::as_str).unwrap_or("?");
        let status = w.get("status").and_then(Value::as_str).unwrap_or("?");
        let n_steps = w
            .get("maf_json")
            .and_then(|m| m.get("steps"))
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        let exec_count = w
            .get("execution_count")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        println!(
            "  {name:<28} {id}  {n_steps} step(s)  {exec_count} run(s)  [{}]",
            colored_status(status)
        );
    }
}

/// `nasiko maf workflow create --step "..." [--step "..."] [--agent ...]` — define a new
/// workflow. Steps run in the order given; an omitted (or "-") `--agent` for a step lets the
/// routing engine auto-assign it.
///
/// `--instruction "..."` is the alternative, mutually-exclusive form: the server's MAF planner
/// splits one compound sentence into steps and picks each step's agent ([`plan_and_create`]).
///
/// `--draft` saves the result as a draft instead of a live workflow — the CLI counterpart of the
/// web UI's "Save draft" action. A draft holds the same steps and runs the same way a live
/// workflow does; `nasiko maf workflow promote` is what deploys it.
pub fn workflow_create(
    name: Option<&str>,
    description: Option<&str>,
    steps: &[String],
    agents: &[String],
    instruction: Option<&str>,
    draft: bool,
) -> Result<()> {
    let client = Client::from_active_cluster()?;

    if let Some(instruction) = instruction {
        let resp = if draft {
            save_generated_as_draft(&client, instruction, name, description)?
        } else {
            plan_and_create(&client, instruction, name, description)?
        };
        return print_created(&resp, None, draft);
    }

    if steps.is_empty() {
        anyhow::bail!("at least one --step or --instruction is required");
    }
    let step_bodies = build_step_bodies(steps, agents)?;
    let resp = if draft {
        save_steps_as_draft(&client, name, description, step_bodies)?
    } else {
        let body = json!({
            "name": name,
            "description": description,
            "steps": step_bodies,
        });
        unwrap_data(client.post_json("/maf/workflows", &body)?)?
    };
    print_created(&resp, Some(steps.len()), draft)
}

/// Prints the `Created workflow '<name>' (<id>) with N step(s)` confirmation line
/// (`Created draft ...` when `draft` is set). `known_step_count` is `None` when the step count
/// isn't known client-side (the `--instruction` path — the planner decides it server-side), so
/// it's read back from the response's `maf_json.steps` instead.
fn print_created(resp: &Value, known_step_count: Option<usize>, draft: bool) -> Result<()> {
    let id = resp.get("id").and_then(Value::as_str).unwrap_or("?");
    let created_name = resp.get("name").and_then(Value::as_str).unwrap_or("?");
    let step_count = known_step_count.unwrap_or_else(|| {
        resp.get("maf_json")
            .and_then(|m| m.get("steps"))
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0)
    });
    let kind = if draft { "draft" } else { "workflow" };
    term::print_success(&format!(
        "Created {kind} '{created_name}' ({id}) with {step_count} step(s)"
    ));
    Ok(())
}

/// `nasiko maf workflow get <name|id>` — show a workflow's steps and metadata.
pub fn workflow_get(workflow: &str, json_out: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let id = resolve_workflow_id(&client, workflow)?;
    let resp: Value = unwrap_data(client.get_json(&format!("/maf/workflow/{id}"))?)?;

    if json_out {
        println!("{}", serde_json::to_string_pretty(&resp)?);
        return Ok(());
    }
    print_workflow(&resp);
    Ok(())
}

/// `nasiko maf workflow update <name|id> [flags]` — rename, redescribe, or replace the steps of
/// an existing workflow. Omitting `--step` entirely leaves the current steps untouched; passing
/// any `--step` replaces the whole list (the server's `PUT` is a full step replace).
///
/// `--add-step`/`--edit-step` are CLI-side sugar over that same full-replace endpoint — there is
/// no server-side patch API. Both fetch the current step list first, splice in the requested
/// change, then resend the whole list, so unrelated steps survive untouched.
#[allow(clippy::too_many_arguments)]
pub fn workflow_update(
    workflow: &str,
    name: Option<String>,
    description: Option<String>,
    clear_description: bool,
    steps: &[String],
    agents: &[String],
    add_steps: &[String],
    add_agents: &[String],
    edit_step: Option<String>,
    edit_description: Option<String>,
    edit_agent: Option<String>,
) -> Result<()> {
    if !steps.is_empty() && (!add_steps.is_empty() || edit_step.is_some()) {
        anyhow::bail!(
            "--step (full replace) cannot be combined with --add-step or --edit-step — \
pick one way of changing steps"
        );
    }
    if edit_step.is_some() && edit_description.is_none() && edit_agent.is_none() {
        anyhow::bail!("--edit-step requires --edit-description and/or --edit-agent");
    }
    if edit_step.is_none() && (edit_description.is_some() || edit_agent.is_some()) {
        anyhow::bail!("--edit-description/--edit-agent require --edit-step <step>");
    }
    validate_agent_count(add_steps.len(), add_agents.len())?;

    let client = Client::from_active_cluster()?;
    let id = resolve_workflow_id(&client, workflow)?;

    let mut body = serde_json::Map::new();
    body.insert("name".to_string(), json!(name));
    if clear_description {
        body.insert("description".to_string(), Value::Null);
    } else if let Some(d) = &description {
        body.insert("description".to_string(), json!(d));
    }

    if !steps.is_empty() {
        body.insert(
            "steps".to_string(),
            json!(build_update_step_bodies(steps, agents)?),
        );
    } else if !add_steps.is_empty() || edit_step.is_some() {
        let current: Value = unwrap_data(client.get_json(&format!("/maf/workflow/{id}"))?)?;
        let mut existing: Vec<Value> = current["maf_json"]["steps"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if existing.is_empty() {
            anyhow::bail!("workflow '{workflow}' has no steps to edit — use --step to create some");
        }

        if let Some(target) = &edit_step {
            let idx = resolve_step_target(&existing, target)?;
            if let Some(desc) = &edit_description {
                existing[idx]["task_description"] = json!(desc);
            }
            if let Some(agent) = &edit_agent {
                existing[idx]["agent_id"] = json!(resolve_step_agent(Some(agent))?);
            }
        }

        for (i, task) in add_steps.iter().enumerate() {
            let agent_id = resolve_step_agent(add_agents.get(i))?;
            existing.push(json!({ "task_description": task, "agent_id": agent_id }));
        }

        // Rebuild as `UpdateStepRequest` bodies ({step_index, agent_id, task_description}),
        // re-numbering step_index sequentially now that a step may have been appended.
        let rebuilt: Vec<Value> = existing
            .iter()
            .enumerate()
            .map(|(i, s)| {
                json!({
                    "step_index": i as i32,
                    "agent_id": s.get("agent_id").cloned().unwrap_or(Value::Null),
                    "task_description": s.get("task_description").and_then(Value::as_str).unwrap_or(""),
                })
            })
            .collect();
        body.insert("steps".to_string(), json!(rebuilt));
    }

    let resp: Value =
        unwrap_data(client.put_json(&format!("/maf/workflow/{id}"), &Value::Object(body))?)?;
    let updated_name = resp.get("name").and_then(Value::as_str).unwrap_or("?");
    term::print_success(&format!("Updated workflow '{updated_name}' ({id})"));
    Ok(())
}

/// Resolve `--edit-step`'s argument to a 0-based index into `existing`: either the step's own
/// `step_id` (UUID) or a 1-based position matching what `workflow get`/`--json` displays.
fn resolve_step_target(existing: &[Value], target: &str) -> Result<usize> {
    if let Some(idx) = existing
        .iter()
        .position(|s| s.get("step_id").and_then(Value::as_str) == Some(target))
    {
        return Ok(idx);
    }
    if let Ok(n) = target.parse::<usize>()
        && n >= 1
        && n <= existing.len()
    {
        return Ok(n - 1);
    }
    anyhow::bail!(
        "--edit-step '{target}' is not a valid step_id or 1-based position (workflow has {} step(s))",
        existing.len()
    )
}

/// `nasiko maf workflow delete <name|id>` — soft-delete a workflow.
pub fn workflow_delete(workflow: &str, force: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let id = resolve_workflow_id(&client, workflow)?;
    if !force {
        let confirm = dialoguer::Confirm::new()
            .with_prompt(format!("Delete workflow '{workflow}'?"))
            .default(false)
            .interact()?;
        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }
    client.delete(&format!("/maf/workflow/{id}"))?;
    term::print_success(&format!("Deleted workflow '{workflow}'"));
    Ok(())
}

/// `nasiko maf workflow promote <name|id>` — deploy a draft: flips its `status` to `active` and
/// changes nothing else, since the steps and agents were already resolved when the draft was
/// made. The server refuses a workflow that isn't a draft, or a draft with no steps yet (add
/// some first with `workflow update --step`/`--add-step`).
pub fn workflow_promote(workflow: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let id = resolve_workflow_id(&client, workflow)?;
    match client
        .post_json_attempt::<Value, _>(&format!("/maf/workflow/{id}/promote"), &json!({}))?
    {
        Attempt::Ok(resp) => {
            let name = resp.get("name").and_then(Value::as_str).unwrap_or(workflow);
            term::print_success(&format!("Promoted draft '{name}' ({id}) — it's live"));
            Ok(())
        }
        Attempt::Rejected(failure) => Err(failure.into_error()),
    }
}

/// `nasiko maf workflow run <name|id> [--wait] [--content "..."]` — queue a run; with `--wait`,
/// poll until it finishes and print the result. `--content` is run-time data folded into step
/// 0's task description before planning, so the same saved workflow can be re-run against
/// different input each time instead of baking it in at creation.
pub fn workflow_run(workflow: &str, wait: bool, content: Option<&str>) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let id = resolve_workflow_id(&client, workflow)?;
    let body = json!({ "content": content });
    let resp: Value = unwrap_data(client.post_json(&format!("/maf/workflow/{id}/run"), &body)?)?;
    let exec_id = resp
        .get("execution_id")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_string();
    let exec_number = resp
        .get("execution_number")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    term::print_success(&format!(
        "Queued execution #{exec_number} ({exec_id}) for workflow '{workflow}'"
    ));

    if !wait {
        println!(
            "Check status with: {}",
            term::cyan(&format!("{} execution result {exec_id}", maf_hint()))
        );
        return Ok(());
    }
    poll_execution(&client, &exec_id)
}

/// `nasiko maf workflow executions <name|id>` — list executions of one workflow.
pub fn workflow_executions(workflow: &str, json_out: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let id = resolve_workflow_id(&client, workflow)?;
    let (executions, total) = list_paginated(&client, &format!("/maf/workflow/{id}/executions"))?;
    print_execution_list(&executions, total, json_out)
}

// ─── Execution commands ─────────────────────────────────────────────────────

/// `nasiko maf execution list` — every execution you've run, across all workflows.
pub fn execution_list(json_out: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let (executions, total) = list_paginated(&client, "/maf/executions")?;
    print_execution_list(&executions, total, json_out)
}

/// `nasiko maf execution get <id>` — show one execution by its UUID.
pub fn execution_get(execution_id: &str, json_out: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let resp: Value = unwrap_data(client.get_json(&format!("/maf/execution/{execution_id}"))?)?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&resp)?);
        return Ok(());
    }
    print_execution(&resp);
    Ok(())
}

/// `nasiko maf execution result <id>` — show one execution's result by its UUID.
pub fn execution_result(execution_id: &str, json_out: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let resp: Value =
        unwrap_data(client.get_json(&format!("/maf/workflow/result/{execution_id}"))?)?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&resp)?);
        return Ok(());
    }
    print_execution(&resp);
    Ok(())
}

/// `nasiko maf execution stop <id>... | --workflow <name|id>` — stop runs that haven't finished.
/// A queued run never starts, a running one is cut short where it is, and a paused one's open
/// question is withdrawn. Runs that already finished are left as they are and reported.
pub fn execution_stop(
    execution_ids: &[String],
    workflow: Option<&str>,
    json_out: bool,
) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let body = match (workflow, execution_ids.is_empty()) {
        (Some(_), false) => anyhow::bail!("give execution ids or --workflow, not both"),
        (None, true) => anyhow::bail!("give one or more execution ids, or --workflow <name|id>"),
        (Some(workflow), true) => json!({ "workflow_id": resolve_workflow_id(&client, workflow)? }),
        (None, false) => {
            for id in execution_ids {
                if uuid::Uuid::parse_str(id).is_err() {
                    anyhow::bail!("'{id}' is not an execution id (a UUID)");
                }
            }
            json!({ "execution_ids": execution_ids })
        }
    };
    let report: Value = unwrap_data(client.post_json("/maf/executions/stop", &body)?)?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_stop_report(&report);
    }
    // A typo shouldn't read as a stop that worked, in either output.
    if found_nothing(&report) {
        anyhow::bail!("no such execution among your workflows' runs");
    }
    Ok(())
}

/// Ids were given and not one of them is the caller's run.
fn found_nothing(report: &Value) -> bool {
    let count = |key: &str| {
        report
            .get(key)
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    };
    count("not_found") > 0 && count("stopped") == 0 && count("already_finished") == 0
}

/// Prints what a stop did.
fn print_stop_report(report: &Value) {
    let list = |key: &str| {
        report
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let (stopped, finished, not_found) =
        (list("stopped"), list("already_finished"), list("not_found"));

    if stopped.is_empty() {
        term::print_info("Nothing to stop.");
    } else {
        term::print_success(&format!("Stopped {} execution(s):", stopped.len()));
        for run in &stopped {
            let id = run.get("id").and_then(Value::as_str).unwrap_or("?");
            let was = run
                .get("previous_status")
                .and_then(Value::as_str)
                .unwrap_or("?");
            println!("  {id}  (was {was})");
        }
    }
    if !finished.is_empty() {
        println!("Already finished:");
        for run in &finished {
            let id = run.get("id").and_then(Value::as_str).unwrap_or("?");
            let status = run.get("status").and_then(Value::as_str).unwrap_or("?");
            println!("  {id}  {}", colored_status(status));
        }
    }
    if !not_found.is_empty() {
        println!("Not found:");
        for id in &not_found {
            println!("  {}", id.as_str().unwrap_or("?"));
        }
    }
}

// ─── Shared helpers ─────────────────────────────────────────────────────────

/// Every MAF list route wraps `crate::Paginated<T>` (`{data, total}`) inside the shared
/// `{data, status_code, message}` envelope, so listing needs an extra unwrap beyond
/// [`unwrap_data`] — the outer `data` field is itself `{data: [...], total}`. Returns the
/// page's items alongside the server's `total` so callers can tell whether the page was
/// truncated (the MAF list routes default to a 50-row page server-side).
fn list_paginated(client: &Client, path: &str) -> Result<(Vec<Value>, usize)> {
    let raw: Value = client.get_json(path)?;
    let paginated: Value = unwrap_data(raw)?;
    let items = paginated
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let total = paginated
        .get("total")
        .and_then(Value::as_u64)
        .map(|t| t as usize)
        .unwrap_or(items.len());
    Ok((items, total))
}

/// Prints a note when the server has more rows than this page returned, so a truncated list
/// doesn't silently read as "that's everything".
fn print_truncation_note(shown: usize, total: usize) {
    if total > shown {
        println!("  ... showing {shown} of {total} total");
    }
}

/// Resolve a workflow reference (name or UUID) to its id. Fast-paths a syntactically valid UUID
/// (mirrors [`resolve_agent_id`]); otherwise scans the caller's workflows for a name match, then
/// — since a draft is addressed through the same commands as a live workflow, just not listed
/// among them — the caller's drafts. Requests a larger page than the default (mirrors
/// [`resolve_agent_id`]'s `?limit=100`) since a name lookup needs every row, not just the first page.
fn resolve_workflow_id(client: &Client, workflow: &str) -> Result<String> {
    if uuid::Uuid::parse_str(workflow).is_ok() {
        return Ok(workflow.to_string());
    }
    let (active, _total) = list_paginated(client, "/maf/workflows?limit=100")?;
    match find_by_name(&active, workflow) {
        NameLookup::One(id) => return Ok(id),
        NameLookup::Many(ids) => anyhow::bail!(
            "multiple workflows named '{workflow}': {} — use an ID instead",
            ids.join(", ")
        ),
        NameLookup::None => {}
    }
    let (drafts, _total) = list_paginated(client, "/maf/workflow/drafts?limit=100")?;
    match find_by_name(&drafts, workflow) {
        NameLookup::One(id) => Ok(id),
        NameLookup::Many(ids) => anyhow::bail!(
            "multiple drafts named '{workflow}': {} — use an ID instead",
            ids.join(", ")
        ),
        NameLookup::None => anyhow::bail!(
            "no MAF workflow or draft named '{workflow}' found (run `{m} workflow list` or `{m} workflow drafts`)",
            m = maf_hint()
        ),
    }
}

/// How many of `rows` have `name` (case-insensitive): none, exactly one (its id), or several
/// (their ids) — the three outcomes [`resolve_workflow_id`] acts on for each list it searches.
enum NameLookup {
    None,
    One(String),
    Many(Vec<String>),
}

fn find_by_name(rows: &[Value], name: &str) -> NameLookup {
    let ids: Vec<String> = rows
        .iter()
        .filter(|w| {
            w.get("name")
                .and_then(Value::as_str)
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
        .filter_map(|w| w.get("id").and_then(Value::as_str).map(str::to_string))
        .collect();
    match ids.len() {
        0 => NameLookup::None,
        1 => NameLookup::One(ids.into_iter().next().unwrap()),
        _ => NameLookup::Many(ids),
    }
}

/// Validates that `--agent` was given either zero times (auto-assign every step) or exactly once
/// per `--step`. Pure so the count-matching rule is unit-tested without a live agent lookup.
fn validate_agent_count(step_count: usize, agent_count: usize) -> Result<()> {
    if agent_count != 0 && agent_count != step_count {
        anyhow::bail!(
            "--agent given {agent_count} time(s) but --step given {step_count} time(s) — pass \
one --agent per --step (use \"-\" to auto-assign a step), or omit --agent entirely to \
auto-assign every step"
        );
    }
    Ok(())
}

/// Resolve the `--agent` value at `index` (if any) to an agent id. `None`, `""`, and `"-"` all
/// mean "auto-assign this step via the routing engine".
fn resolve_step_agent(agent_ref: Option<&String>) -> Result<Option<String>> {
    match agent_ref {
        Some(a) if !a.is_empty() && a != "-" => Ok(Some(resolve_agent_id(a)?)),
        _ => Ok(None),
    }
}

/// Build `CreateStepRequest` bodies (`{task_description, agent_id}`) from `--step`/`--agent`.
fn build_step_bodies(steps: &[String], agents: &[String]) -> Result<Vec<Value>> {
    validate_agent_count(steps.len(), agents.len())?;
    steps
        .iter()
        .enumerate()
        .map(|(i, task)| {
            let agent_id = resolve_step_agent(agents.get(i))?;
            Ok(json!({ "task_description": task, "agent_id": agent_id }))
        })
        .collect()
}

/// Build `UpdateStepRequest` bodies (`{step_index, agent_id, task_description}`) from
/// `--step`/`--agent`.
fn build_update_step_bodies(steps: &[String], agents: &[String]) -> Result<Vec<Value>> {
    validate_agent_count(steps.len(), agents.len())?;
    steps
        .iter()
        .enumerate()
        .map(|(i, task)| {
            let agent_id = resolve_step_agent(agents.get(i))?;
            Ok(json!({ "step_index": i as i32, "agent_id": agent_id, "task_description": task }))
        })
        .collect()
}

fn print_workflow(w: &Value) {
    let id = w.get("id").and_then(Value::as_str).unwrap_or("?");
    let name = w.get("name").and_then(Value::as_str).unwrap_or("?");
    println!("{name}  ({id})");
    print_field("status", w.get("status"));
    print_field("description", w.get("description"));
    print_field("executions", w.get("execution_count"));

    let empty = Vec::new();
    let steps = w
        .get("maf_json")
        .and_then(|m| m.get("steps"))
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    if steps.is_empty() {
        return;
    }
    println!("  steps:");
    for s in steps {
        let idx = s.get("step_index").and_then(Value::as_i64).unwrap_or(0);
        let agent = s.get("agent_name").and_then(Value::as_str).unwrap_or("?");
        let task = s
            .get("task_description")
            .and_then(Value::as_str)
            .unwrap_or("?");
        println!("    {idx}. [{agent}] {task}");
    }
}

fn print_execution_list(executions: &[Value], total: usize, json_out: bool) -> Result<()> {
    if json_out {
        println!("{}", serde_json::to_string_pretty(executions)?);
        return Ok(());
    }
    if executions.is_empty() {
        term::print_info("No executions found.");
        return Ok(());
    }
    for e in executions {
        let number = e
            .get("execution_number")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let id = e.get("id").and_then(Value::as_str).unwrap_or("?");
        let status = colored_status(e.get("status").and_then(Value::as_str).unwrap_or("?"));
        let tokens = e.get("tokens_used").and_then(Value::as_i64).unwrap_or(0);
        let workflow_name = e.get("workflow_name").and_then(Value::as_str);
        match workflow_name {
            Some(wn) => {
                println!("  #{number:<5} {id}  [{status}]  {tokens} tokens  workflow: {wn}")
            }
            None => println!("  #{number:<5} {id}  [{status}]  {tokens} tokens"),
        }
    }
    print_truncation_note(executions.len(), total);
    Ok(())
}

fn print_execution(e: &Value) {
    print_field("id", e.get("id"));
    print_field("execution_number", e.get("execution_number"));
    print_field("workflow_id", e.get("maf_id"));
    if e.get("workflow_name").is_some() {
        print_field("workflow_name", e.get("workflow_name"));
    }
    match e.get("status").and_then(Value::as_str) {
        Some(status) => println!("  {:<18} {}", "status", colored_status(status)),
        None => print_field("status", e.get("status")),
    }
    print_field("attempt_count", e.get("attempt_count"));
    print_field("max_attempts", e.get("max_attempts"));
    print_field("tokens_used", e.get("tokens_used"));
    print_field("started_at", e.get("started_at"));
    print_field("completed_at", e.get("completed_at"));
    print_field("duration_ms", e.get("duration_ms"));
    if let Some(Value::String(output)) = e.get("output") {
        println!("  output:\n{output}");
    }
    if let Some(Value::String(error)) = e.get("error")
        && !error.is_empty()
    {
        println!("  {}", term::red(&format!("error: {error}")));
    }
}

/// Polls `GET /maf/workflow/result/{exec_id}` every 2s until the execution reaches a terminal
/// state (`success` | `failed`) or pauses (`awaiting_human`), mirroring
/// [`Client::poll_mcp_build_status`]'s plain-polling loop. `awaiting_human` stops polling
/// immediately rather than waiting out the rest of the budget: nothing about the execution
/// changes until a human answers, so continuing to poll only delayed telling the caller what
/// actually needs to happen — and previously produced the same "may not be running" timeout
/// message a genuinely stalled execution gets, which is actively misleading for one that's
/// paused and working exactly as designed.
/// A stalled execution (e.g. the server has no `OPENAI_API_KEY` configured, so the MAF worker
/// never started — jobs then sit at `pending` in Redis indefinitely; this is a documented,
/// supported "degrades gracefully" server configuration, not a transient blip) must not hang
/// `--wait` forever. Bounds the poll to ~5 minutes of actual polling before giving up with an
/// actionable message — time spent blocked on a human answering an `awaiting_human` pause never
/// counts against this budget, since that wait has nothing to do with whether the worker itself
/// is alive.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
const MAX_POLL_ATTEMPTS: u32 = 150;

fn poll_execution(client: &Client, exec_id: &str) -> Result<()> {
    let mut last_status = String::new();
    let mut spin = Some(nasiko_utils::term::start_status("waiting for execution"));
    // The one id we've already answered — `ExecWithHitlResponse` guarantees at most one
    // `pending` row at a time, so a single slot is enough to tell "still the same pause,
    // resume dispatcher just hasn't caught up yet" apart from "a genuinely new pause".
    let mut resolved_hitl_id: Option<String> = None;

    for _ in 0..MAX_POLL_ATTEMPTS {
        let resp: Value =
            unwrap_data(client.get_json(&format!("/maf/workflow/result/{exec_id}"))?)?;
        let status = resp
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("pending")
            .to_string();

        if status == "awaiting_human" {
            match pending_hitl_pause(&resp) {
                Some(pause) if resolved_hitl_id.as_deref() != Some(pause.id.as_str()) => {
                    drop(spin.take());
                    let id = pause.id.clone();
                    crate::hitl::prompt_and_resolve_hitl(&pause)?;
                    resolved_hitl_id = Some(id);
                    spin = Some(nasiko_utils::term::start_status("resuming"));
                }
                // Either nothing `pending` right now, or it's the same id we already
                // resolved: the resume dispatcher hasn't propagated that off
                // `maf_executions.status` yet on its own ~2s cycle. Either way, not a new
                // pause to re-answer — re-prompting here would force the user to redo the
                // whole widget for a question they already answered.
                _ => {
                    spin.get_or_insert_with(|| nasiko_utils::term::start_status("resuming"));
                }
            }
            last_status = status;
            std::thread::sleep(POLL_INTERVAL);
            continue;
        }

        if status != last_status {
            last_status = status.clone();
            // Reassigning drops the previous spinner first, clearing its line before the
            // next status (or the final result print) appears.
            spin = match status.as_str() {
                finished if is_finished(finished) => None,
                other => Some(nasiko_utils::term::start_status(other.to_string())),
            };
        }

        if is_finished(&status) {
            drop(spin);
            print_execution(&resp);
            return fail_unless_succeeded(&status);
        }
        if status == nasiko_types::maf::AWAITING_HUMAN {
            // Not a terminal state (the execution resumes once a human answers), but polling
            // further is pointless: nothing changes until that happens, so burning the rest of
            // the ~5-minute budget here just delayed telling the caller what actually needs to
            // happen. `hitl` — already returned by this same endpoint, see
            // `ExecWithHitlResponse`'s own doc comment — carries the pending request's id.
            drop(spin);
            print_execution(&resp);
            let pending_id = resp
                .get("hitl")
                .and_then(Value::as_array)
                .and_then(|rows| {
                    rows.iter()
                        .find(|r| r.get("status").and_then(Value::as_str) == Some("pending"))
                })
                .and_then(|r| r.get("id"))
                .and_then(Value::as_str);
            match pending_id {
                Some(id) => term::print_info(&format!(
                    "awaiting human input — resolve via POST /api/hitl/{id}/resolve (id: {id})"
                )),
                None => term::print_info("awaiting human input"),
            }
            return Ok(());
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    drop(spin);
    anyhow::bail!(
        "still waiting after {}s (last status: '{last_status}') — the MAF worker may not be \
running (e.g. OPENAI_API_KEY not configured on the server). Check again later with: \
{} execution result {exec_id}",
        MAX_POLL_ATTEMPTS as u64 * POLL_INTERVAL.as_secs(),
        maf_hint()
    );
}

/// The one `hitl[]` entry still `status: "pending"` on this execution response, if any —
/// `ExecWithHitlResponse`'s own doc comment guarantees at most one at a time, since MAF steps run
/// strictly sequentially. `agent` is a step label (`execution.maf_step_index`), not an agent name —
/// the response carries only the agent's id here, not its display name, and a step number is
/// enough context for a human answering inline.
fn pending_hitl_pause(resp: &Value) -> Option<crate::hitl::HitlPause> {
    let hitl = resp.get("hitl")?.as_array()?;
    let entry = hitl
        .iter()
        .find(|h| h.get("status").and_then(Value::as_str) == Some("pending"))?;
    Some(crate::hitl::HitlPause {
        id: entry.get("id")?.as_str()?.to_string(),
        kind: entry
            .get("kind")?
            .as_str()
            .unwrap_or("input_required")
            .to_string(),
        question: entry
            .get("question")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        agent: entry
            .pointer("/execution/maf_step_index")
            .and_then(Value::as_i64)
            .map(|i| format!("step {i}")),
    })
}

/// A workflow, execution or step status, colored on a terminal: green once it worked, red when
/// it failed, yellow while it is still going, dim when its owner stopped it. Anything else stays
/// plain.
fn colored_status(status: &str) -> String {
    match status {
        "success" | "completed" | "active" => term::green(status),
        "failed" => term::red(status),
        "pending" | "running" | nasiko_types::maf::AWAITING_HUMAN => term::yellow(status),
        STOPPED => term::dim(status),
        _ => status.to_string(),
    }
}

/// Print `  <label>  <value>`, showing `-` for null/missing so the layout stays stable.
fn print_field(label: &str, value: Option<&Value>) {
    let rendered = match value {
        None | Some(Value::Null) => "-".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
    };
    println!("  {label:<18} {rendered}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stop_that_found_none_of_its_ids_fails() {
        let report = |stopped: usize, finished: usize, missing: usize| {
            json!({
                "stopped": vec![json!({}); stopped],
                "already_finished": vec![json!({}); finished],
                "not_found": vec![json!("x"); missing],
            })
        };
        assert!(found_nothing(&report(0, 0, 2)));
        assert!(!found_nothing(&report(1, 0, 1)), "something was stopped");
        assert!(
            !found_nothing(&report(0, 1, 1)),
            "something was found, already over"
        );
        assert!(
            !found_nothing(&report(0, 0, 0)),
            "a workflow with nothing running"
        );
    }

    #[test]
    fn validate_agent_count_allows_zero_agents() {
        assert!(validate_agent_count(3, 0).is_ok());
    }

    #[test]
    fn validate_agent_count_allows_exact_match() {
        assert!(validate_agent_count(2, 2).is_ok());
    }

    #[test]
    fn validate_agent_count_rejects_mismatch() {
        let err = validate_agent_count(3, 2).unwrap_err();
        assert!(err.to_string().contains("--agent given 2"), "got: {err}");
    }

    #[test]
    fn resolve_step_agent_treats_dash_and_empty_and_absent_as_auto_assign() {
        assert_eq!(resolve_step_agent(None).unwrap(), None);
        assert_eq!(resolve_step_agent(Some(&"".to_string())).unwrap(), None);
        assert_eq!(resolve_step_agent(Some(&"-".to_string())).unwrap(), None);
    }

    fn step(step_id: &str, task: &str) -> Value {
        json!({ "step_id": step_id, "task_description": task, "agent_id": null })
    }

    #[test]
    fn resolve_step_target_by_step_id() {
        let steps = vec![step("aaa", "first"), step("bbb", "second")];
        assert_eq!(resolve_step_target(&steps, "bbb").unwrap(), 1);
    }

    #[test]
    fn resolve_step_target_by_one_based_position() {
        let steps = vec![step("aaa", "first"), step("bbb", "second")];
        assert_eq!(resolve_step_target(&steps, "1").unwrap(), 0);
        assert_eq!(resolve_step_target(&steps, "2").unwrap(), 1);
    }

    #[test]
    fn resolve_step_target_rejects_out_of_range_position() {
        let steps = vec![step("aaa", "first")];
        assert!(resolve_step_target(&steps, "0").is_err());
        assert!(resolve_step_target(&steps, "2").is_err());
    }

    #[test]
    fn resolve_step_target_rejects_unknown_id() {
        let steps = vec![step("aaa", "first")];
        let err = resolve_step_target(&steps, "not-a-real-id").unwrap_err();
        assert!(
            err.to_string().contains("not a valid step_id"),
            "got: {err}"
        );
    }
}
