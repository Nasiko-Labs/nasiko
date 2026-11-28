//! `nasiko eval …`: replay evals the cluster runs, over `/api/evals/*`, the same API the Evals
//! section of the UI uses. The cluster reads your sessions, links them to commits through your
//! connected GitHub account, builds and tests the tasks, and replays them on your LLM configs.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use nasiko_utils::display::trunc;
use serde_json::{Value, json};
use tabled::settings::{Alignment, Style};
use tabled::{Table, Tabled};

use crate::api::Client;

/// How often `--wait` asks again.
const POLL: Duration = Duration::from_secs(5);

fn client() -> Result<Client> {
    Client::from_active_cluster()
}

/// The `data` of a `{data}` answer.
fn data(mut reply: Value) -> Value {
    reply.get_mut("data").map(Value::take).unwrap_or(reply)
}

fn get(path: &str) -> Result<Value> {
    Ok(data(client()?.get_json(path)?))
}

fn print_table<T: Tabled>(rows: Vec<T>) {
    println!(
        "{}",
        Table::new(rows)
            .with(Style::blank())
            .with(Alignment::left())
    );
}

fn text(v: &Value) -> String {
    match v {
        Value::Null => "-".into(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn money(v: &Value) -> String {
    v.as_f64()
        .map(|c| format!("${c:.4}"))
        .unwrap_or_else(|| "-".into())
}

/// `nasiko eval harnesses`.
pub fn harnesses(json_out: bool) -> Result<()> {
    let list = get("/evals/harnesses")?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&list)?);
        return Ok(());
    }
    #[derive(Tabled)]
    struct Row {
        #[tabled(rename = "HARNESS")]
        id: String,
        #[tabled(rename = "SESSIONS")]
        sessions: String,
        #[tabled(rename = "LAST ACTIVE")]
        last: String,
        #[tabled(rename = "EVALS")]
        supported: String,
    }
    let rows: Vec<Row> = list["harnesses"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|h| Row {
            id: text(&h["id"]),
            sessions: text(&h["sessions"]),
            last: text(&h["last_active_at"]),
            supported: if h["supported"] == true {
                "yes".into()
            } else {
                "not yet".into()
            },
        })
        .collect();
    if rows.is_empty() {
        println!("No coding harness is connected. Connect one with `nasiko connect claude`.");
    } else {
        print_table(rows);
    }
    let flag = |key: &str, off: &str| {
        if list[key] != true {
            println!("note: {off}");
        }
    };
    flag(
        "github_connected",
        "GitHub is not connected (`nasiko github login`); sessions' repositories are read through it",
    );
    flag(
        "chat_history",
        "this cluster keeps no chat history, which tasks are built from",
    );
    flag(
        "runner_available",
        "this cluster does not run evals (EVAL_ENABLED is off)",
    );
    Ok(())
}

fn fetch_sessions(harness: &str, limit: i64) -> Result<Vec<Value>> {
    let page = get(&format!(
        "/evals/harnesses/{}/sessions?limit={limit}",
        encode(harness)
    ))?;
    Ok(page["sessions"].as_array().cloned().unwrap_or_default())
}

/// `nasiko eval sessions`.
pub fn sessions(harness: &str, limit: i64, json_out: bool) -> Result<()> {
    let list = fetch_sessions(harness, limit)?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&list)?);
        return Ok(());
    }
    #[derive(Tabled)]
    struct Row {
        #[tabled(rename = "SESSION")]
        id: String,
        #[tabled(rename = "STARTED")]
        started: String,
        #[tabled(rename = "TURNS")]
        turns: String,
        #[tabled(rename = "COST")]
        cost: String,
        #[tabled(rename = "REPO")]
        repo: String,
        #[tabled(rename = "USABLE")]
        usable: String,
        #[tabled(rename = "TITLE")]
        title: String,
    }
    print_table(
        list.iter()
            .map(|s| Row {
                id: text(&s["session_id"]),
                started: text(&s["started_at"]),
                turns: text(&s["turns"]),
                cost: money(&s["cost_usd"]),
                repo: text(&s["repo"]),
                usable: if s["eligible"] == true {
                    "yes".into()
                } else {
                    text(&s["ineligible_reason"])
                },
                title: trunc(&text(&s["title"]), 50),
            })
            .collect(),
    );
    Ok(())
}

/// What `nasiko eval datasets create` asks for.
pub struct NewDataset {
    pub harness: String,
    pub sessions: Vec<String>,
    pub limit: usize,
    pub rewriter: Option<String>,
    pub reviewers: Vec<String>,
    pub gate_runs: Option<i32>,
    pub wait: bool,
}

/// Your LLM configs, `(id, name)`.
fn configs() -> Result<Vec<(String, String)>> {
    let list = get("/llm-configs")?;
    Ok(list
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|c| (text(&c["id"]), text(&c["name"])))
        .collect())
}

/// An LLM config's id from its name or id.
fn config_id(configs: &[(String, String)], wanted: &str) -> Result<String> {
    configs
        .iter()
        .find(|(id, name)| id == wanted || name == wanted)
        .map(|(id, _)| id.clone())
        .with_context(|| {
            let names: Vec<&str> = configs.iter().map(|(_, n)| n.as_str()).collect();
            format!("no LLM config named {wanted:?}; yours: {names:?}")
        })
}

/// `nasiko eval datasets create`.
pub fn create_dataset(args: &NewDataset) -> Result<()> {
    let session_ids = if args.sessions.is_empty() {
        let usable: Vec<String> = fetch_sessions(&args.harness, 100)?
            .iter()
            .filter(|s| s["eligible"] == true)
            .take(args.limit)
            .map(|s| text(&s["session_id"]))
            .collect();
        if usable.is_empty() {
            bail!(
                "no usable sessions: see `nasiko eval sessions --harness {}`",
                args.harness
            );
        }
        usable
    } else {
        args.sessions.clone()
    };
    let mut body = json!({"harness": args.harness, "session_ids": session_ids});
    if args.rewriter.is_some() || !args.reviewers.is_empty() {
        let configs = configs()?;
        if let Some(r) = &args.rewriter {
            body["rewriter_id"] = json!(config_id(&configs, r)?);
        }
        if !args.reviewers.is_empty() {
            let ids: Vec<String> = args
                .reviewers
                .iter()
                .map(|r| config_id(&configs, r))
                .collect::<Result<_>>()?;
            body["reviewer_ids"] = json!(ids);
        }
    }
    if let Some(g) = args.gate_runs {
        body["gate_runs"] = json!(g);
    }
    let created = data(client()?.post_json::<Value, _>("/evals/datasets", &body)?);
    let id = text(&created["id"]);
    nasiko_utils::term::print_success(&format!(
        "Dataset {id} is being built from {} sessions",
        session_ids.len()
    ));
    if !args.wait {
        println!("Follow it with `nasiko eval datasets show {id}`.");
        return Ok(());
    }
    let done = wait_until(&format!("/evals/datasets/{id}"), |d| {
        let status = text(&d["status"]);
        let line = format!(
            "{status}: {}/{} tasks prepared",
            text(&d["progress"]["done"]),
            text(&d["progress"]["total"])
        );
        (matches!(status.as_str(), "ready" | "failed"), line)
    })?;
    print_dataset(&done);
    Ok(())
}

/// Polls `path` until `done` says so, showing its line meanwhile; returns the last answer.
fn wait_until(path: &str, done: impl Fn(&Value) -> (bool, String)) -> Result<Value> {
    loop {
        let value = get(path)?;
        let (finished, line) = done(&value);
        if finished {
            return Ok(value);
        }
        let status = nasiko_utils::term::start_status(line);
        std::thread::sleep(POLL);
        drop(status);
    }
}

/// `nasiko eval datasets ls`.
pub fn list_datasets() -> Result<()> {
    #[derive(Tabled)]
    struct Row {
        #[tabled(rename = "ID")]
        id: String,
        #[tabled(rename = "HARNESS")]
        harness: String,
        #[tabled(rename = "STATUS")]
        status: String,
        #[tabled(rename = "SESSIONS")]
        sessions: String,
        #[tabled(rename = "CREATED")]
        created: String,
    }
    let list = get("/evals/datasets")?;
    print_table(
        list.as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|d| Row {
                id: text(&d["id"]),
                harness: text(&d["harness"]),
                status: text(&d["status"]),
                sessions: text(&d["sessions"]),
                created: text(&d["created_at"]),
            })
            .collect(),
    );
    Ok(())
}

/// `nasiko eval datasets show` / `nasiko eval tasks`.
pub fn show_dataset(id: &str, json_out: bool) -> Result<()> {
    let dataset = get(&format!("/evals/datasets/{}", encode(id)))?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&dataset)?);
    } else {
        print_dataset(&dataset);
    }
    Ok(())
}

fn print_dataset(d: &Value) {
    println!(
        "Dataset {} ({}): {}, repositories {}",
        text(&d["id"]),
        text(&d["harness"]),
        text(&d["status"]),
        text(&d["repos"])
    );
    if let Some(error) = d["error"].as_str() {
        println!("error: {error}");
    }
    if let Some(outcomes) = d["outcomes"].as_object() {
        let mut counts: std::collections::BTreeMap<&str, u32> = Default::default();
        for o in outcomes.values().filter_map(Value::as_str) {
            *counts.entry(o).or_default() += 1;
        }
        println!("sessions: {counts:?}");
    }
    #[derive(Tabled)]
    struct Row {
        #[tabled(rename = "TASK")]
        id: String,
        #[tabled(rename = "STATUS")]
        status: String,
        #[tabled(rename = "STAGE")]
        stage: String,
        #[tabled(rename = "F2P/P2P")]
        tests: String,
        #[tabled(rename = "DIFFICULTY")]
        difficulty: String,
        #[tabled(rename = "ORIGINAL")]
        original: String,
        #[tabled(rename = "TITLE")]
        title: String,
    }
    let tasks = d["tasks"].as_array().cloned().unwrap_or_default();
    if tasks.is_empty() {
        println!("No tasks yet.");
    } else {
        print_table(
            tasks
                .iter()
                .map(|t| Row {
                    id: text(&t["id"]),
                    status: match t["reject_reason"].as_str() {
                        Some(reason) => format!("{} ({})", text(&t["status"]), trunc(reason, 40)),
                        None => text(&t["status"]),
                    },
                    stage: text(&t["stage"]),
                    tests: format!("{}/{}", text(&t["fail_to_pass"]), text(&t["pass_to_pass"])),
                    difficulty: text(&t["difficulty"]),
                    original: format!(
                        "{} {}",
                        text(&t["original"]["model"]),
                        money(&t["original"]["cost_usd"])
                    ),
                    title: trunc(&text(&t["title"]), 50),
                })
                .collect(),
        );
    }
    let models: Vec<String> = d["models"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let used = if m["used"] == true { " (original)" } else { "" };
            format!(
                "{} = {}/{}{used}",
                text(&m["name"]),
                text(&m["provider"]),
                text(&m["model"])
            )
        })
        .collect();
    println!("models you can test: {}", models.join(", "));
}

/// `nasiko eval datasets rm`.
pub fn delete_dataset(id: &str) -> Result<()> {
    client()?.delete(&format!("/evals/datasets/{}", encode(id)))?;
    nasiko_utils::term::print_success(&format!("Deleted dataset {id}"));
    Ok(())
}

/// `nasiko eval review`.
pub fn review(
    task: &str,
    clarity: u8,
    validity: u8,
    effort: &str,
    problems: Vec<String>,
) -> Result<()> {
    let task_view = data(client()?.post_json::<Value, _>(
        &format!("/evals/tasks/{}/review", encode(task)),
        &json!({"clarity": clarity, "test_validity": validity, "effort": effort, "other_problems": problems}),
    )?);
    println!("Task {task}: {}", text(&task_view["status"]));
    Ok(())
}

/// `nasiko eval runs create`.
pub fn create_run(
    dataset: &str,
    tasks: &[String],
    models: &[String],
    attempts: i32,
    wait: bool,
) -> Result<()> {
    let view = get(&format!("/evals/datasets/{}", encode(dataset)))?;
    let offered: Vec<(String, String)> = view["models"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|m| (text(&m["id"]), text(&m["name"])))
        .collect();
    let model_ids: Vec<String> = models
        .iter()
        .map(|m| config_id(&offered, m))
        .collect::<Result<_>>()?;
    let task_ids: Vec<String> = if tasks.is_empty() {
        view["tasks"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter(|t| t["status"] == "verified")
            .map(|t| text(&t["id"]))
            .collect()
    } else {
        tasks.to_vec()
    };
    if task_ids.is_empty() {
        bail!("the dataset has no verified task yet: see `nasiko eval tasks {dataset}`");
    }
    let run = data(client()?.post_json::<Value, _>(
        &format!("/evals/datasets/{}/runs", encode(dataset)),
        &json!({"task_ids": task_ids, "model_ids": model_ids, "attempts": attempts}),
    )?);
    let id = text(&run["id"]);
    nasiko_utils::term::print_success(&format!(
        "Run {id}: {} attempts queued",
        text(&run["progress"]["total"])
    ));
    if !wait {
        println!(
            "Follow it with `nasiko eval runs show {id}`; read it with `nasiko eval report {id}`."
        );
        return Ok(());
    }
    wait_until(&format!("/evals/runs/{id}"), |r| {
        let status = text(&r["status"]);
        let line = format!(
            "{status}: {}/{} attempts ended",
            text(&r["progress"]["done"]),
            text(&r["progress"]["total"])
        );
        (
            matches!(status.as_str(), "done" | "failed" | "cancelled"),
            line,
        )
    })?;
    report(&id, "text")
}

/// `nasiko eval runs ls`.
pub fn list_runs(dataset: Option<&str>) -> Result<()> {
    let path = match dataset {
        Some(d) => format!("/evals/runs?dataset_id={}", encode(d)),
        None => "/evals/runs".to_string(),
    };
    #[derive(Tabled)]
    struct Row {
        #[tabled(rename = "RUN")]
        id: String,
        #[tabled(rename = "DATASET")]
        dataset: String,
        #[tabled(rename = "STATUS")]
        status: String,
        #[tabled(rename = "MODELS")]
        models: String,
        #[tabled(rename = "CREATED")]
        created: String,
    }
    let list = get(&path)?;
    print_table(
        list.as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|r| Row {
                id: text(&r["id"]),
                dataset: text(&r["dataset_id"]),
                status: text(&r["status"]),
                models: model_names(&r["models"]).join(", "),
                created: text(&r["created_at"]),
            })
            .collect(),
    );
    Ok(())
}

fn model_names(models: &Value) -> Vec<String> {
    models
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|m| text(&m["name"]))
        .collect()
}

/// `nasiko eval runs show`.
pub fn show_run(id: &str, json_out: bool) -> Result<()> {
    let run = get(&format!("/evals/runs/{}", encode(id)))?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&run)?);
        return Ok(());
    }
    let names: std::collections::BTreeMap<String, String> = run["models"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|m| (text(&m["id"]), text(&m["name"])))
        .collect();
    println!(
        "Run {}: {}, {}/{} attempts ended",
        text(&run["id"]),
        text(&run["status"]),
        text(&run["progress"]["done"]),
        text(&run["progress"]["total"])
    );
    #[derive(Tabled)]
    struct Row {
        #[tabled(rename = "TASK")]
        task: String,
        #[tabled(rename = "MODEL")]
        model: String,
        #[tabled(rename = "#")]
        attempt: String,
        #[tabled(rename = "STATUS")]
        status: String,
        #[tabled(rename = "SCORE")]
        score: String,
        #[tabled(rename = "COST")]
        cost: String,
        #[tabled(rename = "TIME")]
        time: String,
        #[tabled(rename = "WHY")]
        why: String,
    }
    print_table(
        run["results"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|r| {
                let model = text(&r["model_id"]);
                let builder = if r["builder"] == true { " *" } else { "" };
                Row {
                    task: trunc(&text(&r["task_id"]), 8),
                    model: format!("{}{builder}", names.get(&model).unwrap_or(&model)),
                    attempt: text(&r["attempt"]),
                    status: text(&r["status"]),
                    score: text(&r["score"]),
                    cost: money(&r["cost_usd"]),
                    time: r["latency_ms"]
                        .as_u64()
                        .map(|ms| format!("{}s", ms / 1000))
                        .unwrap_or_else(|| "-".into()),
                    why: r["failure"]
                        .as_str()
                        .or(r["error"].as_str())
                        .map(|why| trunc(why, 40))
                        .unwrap_or_default(),
                }
            })
            .collect(),
    );
    if run["results"]
        .as_array()
        .is_some_and(|r| r.iter().any(|r| r["builder"] == true))
    {
        println!("* also rewrote or screened the tasks: its scores are optimistic");
    }
    Ok(())
}

/// `nasiko eval runs cancel`.
pub fn cancel_run(id: &str) -> Result<()> {
    client()?.post_void(&format!("/evals/runs/{}/cancel", encode(id)))?;
    nasiko_utils::term::print_success(&format!("Cancelled run {id}"));
    Ok(())
}

/// `nasiko eval runs rm`.
pub fn delete_run(id: &str) -> Result<()> {
    client()?.delete(&format!("/evals/runs/{}", encode(id)))?;
    nasiko_utils::term::print_success(&format!("Deleted run {id}"));
    Ok(())
}

/// `nasiko eval report`.
pub fn report(id: &str, format: &str) -> Result<()> {
    let run = get(&format!("/evals/runs/{}", encode(id)))?;
    let Some(report) = run.get("report").filter(|r| !r.is_null()) else {
        bail!(
            "run {id} has no report yet ({}): wait for it to end",
            text(&run["status"])
        );
    };
    match format {
        "json" => println!("{}", serde_json::to_string_pretty(report)?),
        "text" | "md" | "markdown" => print!("{}", render(report, format != "text")),
        other => bail!("unknown format {other:?}: use text, json or md"),
    }
    Ok(())
}

/// `nasiko eval verdict`: whether switching to a cheaper model is safe, from a cluster run.
pub fn verdict(id: &str, opts: &super::verdict::Options) -> Result<()> {
    let run = get(&format!("/evals/runs/{}", encode(id)))?;
    super::verdict::remote(&run, opts)
}

/// The report's summary as a table, then each model's headline.
fn render(report: &Value, markdown: bool) -> String {
    let pct = |v: &Value| {
        v.as_f64()
            .map(|x| format!("{:.0}%", x * 100.0))
            .unwrap_or_else(|| "-".into())
    };
    let rate = |r: &Value| {
        if r.is_null() {
            return "-".to_string();
        }
        format!(
            "{} ({}–{})",
            pct(&r["value"]),
            pct(&r["low95"]),
            pct(&r["high95"])
        )
    };
    let mut rows = vec![[
        "model".to_string(),
        "resolved".into(),
        "matched control".into(),
        "cost".into(),
        "saving vs original".into(),
        "saving vs control".into(),
        "attempts".into(),
    ]];
    for m in report["summary"].as_array().cloned().unwrap_or_default() {
        let control = if m["is_control"] == true {
            " (control)"
        } else {
            ""
        };
        rows.push([
            format!("{}{control}", text(&m["model"])),
            rate(&m["resolved"]),
            rate(&m["match_rate"]),
            money(&m["cost_usd"]),
            pct(&m["saving_vs_original"]),
            pct(&m["saving_vs_control"]),
            text(&m["attempts"]),
        ]);
    }
    let mut out = String::new();
    for (i, row) in rows.iter().enumerate() {
        if markdown {
            out.push_str(&format!("| {} |\n", row.join(" | ")));
            if i == 0 {
                out.push_str(&format!("|{}\n", "---|".repeat(row.len())));
            }
        } else {
            out.push_str(&format!(
                "{:<28} {:<20} {:<20} {:<10} {:<19} {:<18} {}\n",
                row[0], row[1], row[2], row[3], row[4], row[5], row[6]
            ));
        }
    }
    out.push('\n');
    if let Some(headlines) = report["headlines"].as_object() {
        for line in headlines.values().filter_map(Value::as_str) {
            out.push_str(&format!("{}{line}\n", if markdown { "- " } else { "" }));
        }
    }
    out
}

/// A value for a URL path or query: ids and harness names are plain, but never trust input.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configs_are_found_by_name_or_id() {
        let configs = vec![("id-1".to_string(), "opus".to_string())];
        assert_eq!(config_id(&configs, "opus").unwrap(), "id-1");
        assert_eq!(config_id(&configs, "id-1").unwrap(), "id-1");
        assert!(
            config_id(&configs, "gpt")
                .unwrap_err()
                .to_string()
                .contains("opus")
        );
    }

    #[test]
    fn reports_render_a_row_per_model_and_the_headlines() {
        let report = json!({
            "summary": [{
                "model": "haiku", "is_control": false, "attempts": 4,
                "resolved": {"value": 0.5, "low95": 0.25, "high95": 0.75, "n": 4},
                "match_rate": null, "cost_usd": 0.12, "saving_vs_original": 0.4,
                "saving_vs_control": null
            }],
            "headlines": {"haiku": "haiku matched the original sessions on 50% of tasks."}
        });
        let md = render(&report, true);
        assert!(
            md.contains("| haiku | 50% (25%–75%) | - | $0.1200 | 40% | - | 4 |"),
            "{md}"
        );
        assert!(md.contains("- haiku matched"));
        assert!(render(&report, false).contains("haiku"));
    }

    #[test]
    fn path_values_are_encoded() {
        assert_eq!(encode("a b/c"), "a%20b%2Fc");
        assert_eq!(data(json!({"data": [1]})), json!([1]));
        assert_eq!(data(json!([1])), json!([1]));
    }
}
