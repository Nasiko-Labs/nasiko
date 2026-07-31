//! `nasiko maf` — create, run, and inspect MAF (Multi-Agent Flow) workflows.
//!
//! Wraps `/api/maf/workflows*` (workflow CRUD + run) and `/api/maf/workflow/{id}/executions` +
//! `/api/maf/executions` + `/api/maf/execution/{id}` + `/api/maf/workflow/result/{exec_id}`
//! (execution listing/inspection). See `oss/server/src/maf.rs` for the route definitions this
//! mirrors.

use anyhow::Result;
use serde_json::{Value, json};

use crate::api::{Client, unwrap_data};
use crate::commands::agents::resolve_agent_id;

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
        println!("No MAF workflows. Create one with `nasiko maf workflow create`.");
        return Ok(());
    }
    println!("Your MAF workflows:");
    for w in &workflows {
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
        println!("  {name:<28} {id}  {n_steps} step(s)  {exec_count} run(s)  [{status}]");
    }
    print_truncation_note(workflows.len(), total);
    Ok(())
}

/// `nasiko maf workflow create --step "..." [--step "..."] [--agent ...]` — define a new
/// workflow. Steps run in the order given; an omitted (or "-") `--agent` for a step lets the
/// routing engine auto-assign it.
pub fn workflow_create(
    name: Option<&str>,
    description: Option<&str>,
    steps: &[String],
    agents: &[String],
) -> Result<()> {
    if steps.is_empty() {
        anyhow::bail!("at least one --step is required");
    }
    let step_bodies = build_step_bodies(steps, agents)?;
    let body = json!({
        "name": name,
        "description": description,
        "steps": step_bodies,
    });

    let client = Client::from_active_cluster()?;
    let resp: Value = unwrap_data(client.post_json("/maf/workflows", &body)?)?;
    let id = resp.get("id").and_then(Value::as_str).unwrap_or("?");
    let created_name = resp.get("name").and_then(Value::as_str).unwrap_or("?");
    println!(
        "Created workflow '{created_name}' ({id}) with {} step(s)",
        steps.len()
    );
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
pub fn workflow_update(
    workflow: &str,
    name: Option<String>,
    description: Option<String>,
    clear_description: bool,
    steps: &[String],
    agents: &[String],
) -> Result<()> {
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
    }

    let resp: Value =
        unwrap_data(client.put_json(&format!("/maf/workflow/{id}"), &Value::Object(body))?)?;
    let updated_name = resp.get("name").and_then(Value::as_str).unwrap_or("?");
    println!("Updated workflow '{updated_name}' ({id})");
    Ok(())
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
    println!("Deleted workflow '{workflow}'");
    Ok(())
}

/// `nasiko maf workflow run <name|id> [--wait]` — queue a run; with `--wait`, poll until it
/// finishes and print the result.
pub fn workflow_run(workflow: &str, wait: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let id = resolve_workflow_id(&client, workflow)?;
    let resp: Value =
        unwrap_data(client.post_json(&format!("/maf/workflow/{id}/run"), &json!({}))?)?;
    let exec_id = resp
        .get("execution_id")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_string();
    let exec_number = resp
        .get("execution_number")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    println!("Queued execution #{exec_number} ({exec_id}) for workflow '{workflow}'");

    if !wait {
        println!("Check status with: nasiko maf execution result {exec_id}");
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
/// (mirrors [`resolve_agent_id`]); otherwise scans the caller's workflows for a name match.
/// Requests a larger page than the default (mirrors [`resolve_agent_id`]'s `?limit=100`) since a
/// name lookup needs every workflow, not just the first page.
fn resolve_workflow_id(client: &Client, workflow: &str) -> Result<String> {
    if uuid::Uuid::parse_str(workflow).is_ok() {
        return Ok(workflow.to_string());
    }
    let (workflows, _total) = list_paginated(client, "/maf/workflows?limit=100")?;
    let matches: Vec<&Value> = workflows
        .iter()
        .filter(|w| {
            w.get("name")
                .and_then(Value::as_str)
                .is_some_and(|n| n.eq_ignore_ascii_case(workflow))
        })
        .collect();
    match matches.as_slice() {
        [one] => Ok(one
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()),
        [] => anyhow::bail!(
            "no MAF workflow named '{workflow}' found (run `nasiko maf workflow list`)"
        ),
        many => anyhow::bail!(
            "multiple workflows named '{workflow}': {} — use an ID instead",
            many.iter()
                .filter_map(|w| w.get("id").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(", ")
        ),
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
        println!("No executions found.");
        return Ok(());
    }
    for e in executions {
        let number = e
            .get("execution_number")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let id = e.get("id").and_then(Value::as_str).unwrap_or("?");
        let status = e.get("status").and_then(Value::as_str).unwrap_or("?");
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
    print_field("status", e.get("status"));
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
        println!("  error: {error}");
    }
}

/// Polls `GET /maf/workflow/result/{exec_id}` every 2s until the execution reaches a terminal
/// state (`success` | `failed`), mirroring [`Client::poll_mcp_build_status`]'s plain-polling loop.
/// A stalled execution (e.g. the server has no `OPENAI_API_KEY` configured, so the MAF worker
/// never started — jobs then sit at `pending` in Redis indefinitely; this is a documented,
/// supported "degrades gracefully" server configuration, not a transient blip) must not hang
/// `--wait` forever. Bounds the poll to ~5 minutes before giving up with an actionable message.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
const MAX_POLL_ATTEMPTS: u32 = 150;

fn poll_execution(client: &Client, exec_id: &str) -> Result<()> {
    let mut last_status = String::new();
    let mut spin = Some(nasiko_utils::term::start_status("waiting for execution"));

    for _ in 0..MAX_POLL_ATTEMPTS {
        let resp: Value =
            unwrap_data(client.get_json(&format!("/maf/workflow/result/{exec_id}"))?)?;
        let status = resp
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("pending")
            .to_string();

        if status != last_status {
            last_status = status.clone();
            // Reassigning drops the previous spinner first, clearing its line before the
            // next status (or the final result print) appears.
            spin = match status.as_str() {
                "success" | "failed" => None,
                other => Some(nasiko_utils::term::start_status(other.to_string())),
            };
        }

        if status == "success" || status == "failed" {
            drop(spin);
            print_execution(&resp);
            if status == "failed" {
                anyhow::bail!("execution failed");
            }
            return Ok(());
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    drop(spin);
    anyhow::bail!(
        "still waiting after {}s (last status: '{last_status}') — the MAF worker may not be \
running (e.g. OPENAI_API_KEY not configured on the server). Check again later with: \
nasiko maf execution result {exec_id}",
        MAX_POLL_ATTEMPTS as u64 * POLL_INTERVAL.as_secs()
    );
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
}
