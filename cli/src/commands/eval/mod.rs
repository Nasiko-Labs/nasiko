//! `nasiko eval`: build a private SWE-bench-style benchmark from finished coding sessions and
//! replay it on other models. The logic lives in the `nasiko-evals` crate; this module does the
//! I/O (files, git, Docker, the Nasiko router) and the printing.

pub mod build;
pub mod containers;
pub mod dataset;
pub mod gateway;
pub mod git;
pub mod harvest;
pub mod llm;
pub mod remote;
pub mod report;
pub mod router;
pub mod run;
pub mod screen;
pub mod store;

use anyhow::Result;
pub use nasiko_evals::images;
use nasiko_evals::{EvalLayout, TaskStatus};
use nasiko_utils::display::trunc;
use tabled::settings::{Alignment, Style};
use tabled::{Table, Tabled};

pub use build::BuildArgs;
pub use harvest::{HarvestArgs, Redo};
use store::{EvalConfig, Store, eval_root};

fn open_store() -> Store {
    Store::new(EvalLayout::new(eval_root()))
}

/// `nasiko eval init`: creates the eval folder and a default `config.json`.
pub fn init() -> Result<()> {
    let store = open_store();
    let path = store.layout().config_file();
    if store.load_config()?.is_some() {
        println!(
            "Eval folder already set up: {}",
            store.layout().root().display()
        );
        return Ok(());
    }
    // Built from `{}` so every field takes its documented default.
    let config: EvalConfig = serde_json::from_str("{}")?;
    store.save_config(&config)?;
    nasiko_utils::term::print_success(&format!("Created {}", path.display()));
    println!("Add the models to compare under \"models\" in that file.");
    Ok(())
}

/// `nasiko eval harvest`.
pub fn harvest(args: &HarvestArgs) -> Result<()> {
    let store = open_store();
    let summary = harvest::harvest(&store, args)?;
    println!(
        "{}: {} session logs found, {} already harvested",
        summary.repo, summary.logs_found, summary.skipped_seen
    );
    for (outcome, count) in &summary.outcomes {
        println!("  {:<18} {count}", format!("{outcome:?}"));
    }
    if summary.new_tasks.is_empty() {
        println!("No new tasks.");
    } else {
        nasiko_utils::term::print_success(&format!(
            "{} new candidate tasks",
            summary.new_tasks.len()
        ));
        println!("Run `nasiko eval tasks` to see them.");
    }
    Ok(())
}

#[derive(Tabled)]
struct TaskRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "STATUS")]
    status: String,
    #[tabled(rename = "TESTS")]
    tests: String,
    #[tabled(rename = "LINK")]
    link: String,
    #[tabled(rename = "TURNS")]
    turns: u32,
    #[tabled(rename = "REQUEST")]
    request: String,
}

/// `nasiko eval tasks`: lists tasks, optionally one status only.
pub fn tasks(status: Option<TaskStatus>, json: bool) -> Result<()> {
    let tasks: Vec<_> = open_store()
        .tasks()?
        .into_iter()
        .filter(|t| status.is_none_or(|s| t.x_nasiko.status == s))
        .collect();
    if json {
        println!("{}", serde_json::to_string_pretty(&tasks)?);
        return Ok(());
    }
    if tasks.is_empty() {
        println!("No tasks yet. Run `nasiko eval harvest --repo <path>`.");
        return Ok(());
    }
    let rows: Vec<TaskRow> = tasks
        .iter()
        .map(|t| TaskRow {
            id: t.instance_id.clone(),
            status: format!("{:?}", t.x_nasiko.status).to_lowercase(),
            tests: format!("{:?}", t.x_nasiko.test_source).to_lowercase(),
            link: format!("{:.2}", t.x_nasiko.link_score),
            turns: t.x_nasiko.orig.user_turns,
            request: trunc(&t.problem_statement.replace('\n', " "), 60),
        })
        .collect();
    println!(
        "{}",
        Table::new(rows)
            .with(Style::blank())
            .with(Alignment::left())
    );
    Ok(())
}

/// `nasiko eval spec detect`.
pub fn spec_detect(repo: &std::path::Path) -> Result<()> {
    let store = open_store();
    let spec = build::spec_detect(&store, repo)?;
    println!("{}", serde_json::to_string_pretty(&spec)?);
    nasiko_utils::term::print_success("Saved. Edit the file under specs/ to change it.");
    Ok(())
}

/// `nasiko eval build`.
pub fn build(args: &BuildArgs) -> Result<()> {
    let store = open_store();
    let reports = build::build(&store, &containers::DockerCli::new(), args)?;
    if reports.is_empty() {
        println!("No candidate tasks to build.");
    }
    for r in reports {
        match r.verdict {
            Ok(nasiko_evals::gate::GateVerdict::Reproducible(l)) => println!(
                "  {:<34} reproducible: {} fail-to-pass, {} pass-to-pass, {} flaky",
                r.instance_id,
                l.fail_to_pass.len(),
                l.pass_to_pass.len(),
                l.flaky.len()
            ),
            Ok(nasiko_evals::gate::GateVerdict::Rejected { reason }) => {
                println!("  {:<34} rejected: {reason:?}", r.instance_id)
            }
            Err(e) => println!("  {:<34} error: {e}", r.instance_id),
        }
    }
    Ok(())
}

/// `nasiko eval clean`.
pub fn clean(label: Option<&str>) -> Result<()> {
    use containers::Containers as _;
    let removed = containers::DockerCli::new().remove_labelled(label)?;
    println!("Removed {removed} containers and networks.");
    Ok(())
}

fn print_steps(results: Vec<screen::StepResult>) {
    if results.is_empty() {
        println!("Nothing to do.");
    }
    for r in results {
        println!("  {:<34} {}", r.instance_id, r.outcome);
    }
}

/// `nasiko eval rewrite`.
pub fn rewrite(only: &[String]) -> Result<()> {
    let store = open_store();
    let router = router::RouterClient::from_login()?;
    print_steps(screen::rewrite(&store, &router, only)?);
    Ok(())
}

/// `nasiko eval screen`.
pub fn screen_tasks() -> Result<()> {
    let store = open_store();
    let router = router::RouterClient::from_login()?;
    print_steps(screen::screen(&store, &router)?);
    Ok(())
}

/// `nasiko eval review`: asks a person, only for tasks no reviewer could judge.
pub fn review() -> Result<()> {
    use dialoguer::{Input, Select};
    use nasiko_evals::Difficulty;
    let store = open_store();
    let tasks = screen::needing_people(&store)?;
    if tasks.is_empty() {
        println!("No task needs a person: every reproducible task has a reviewer verdict.");
        return Ok(());
    }
    let reviewer = std::env::var("USER").unwrap_or_else(|_| "reviewer".into());
    for task in tasks {
        println!(
            "\n=== {} ===\n{}\n",
            task.instance_id, task.problem_statement
        );
        println!("--- gold patch ---\n{}", trunc(&task.patch, 4000));
        println!(
            "--- tests that must go from failing to passing ---\n{}",
            task.fail_to_pass.join("\n")
        );
        let score = |q: &str| -> Result<u8> {
            Ok(Input::<u8>::new()
                .with_prompt(format!("{q} (0 best … 3 worst)"))
                .validate_with(|v: &u8| if *v <= 3 { Ok(()) } else { Err("0-3") })
                .interact_text()?)
        };
        let clarity = score("Is the request clear enough to solve without seeing the fix?")?;
        let test_validity = score("Would the tests accept every reasonable correct fix?")?;
        let efforts = [
            Difficulty::Under15Min,
            Difficulty::Min15To1H,
            Difficulty::H1To4,
            Difficulty::Over4H,
        ];
        let pick = Select::new()
            .with_prompt("How long would an engineer who knows the repo need?")
            .items(&["<15 min", "15 min - 1 hour", "1-4 hours", ">4 hours"])
            .default(1)
            .interact()?;
        let problems: String = Input::new()
            .with_prompt("Other problems (empty for none)")
            .allow_empty(true)
            .interact_text()?;
        let outcome = screen::record_human_review(
            &store,
            task.clone(),
            screen::HumanScores {
                reviewer: reviewer.clone(),
                clarity,
                test_validity,
                effort: efforts[pick],
                problems: if problems.trim().is_empty() {
                    vec![]
                } else {
                    vec![format!("other: {}", problems.trim())]
                },
            },
        )?;
        println!("{} → {outcome}", task.instance_id);
    }
    Ok(())
}

/// `nasiko eval freeze`.
pub fn freeze(name: &str) -> Result<()> {
    let m = dataset::freeze(&open_store(), name)?;
    nasiko_utils::term::print_success(&format!(
        "Froze {} tasks as {}@{}",
        m.instance_ids.len(),
        m.name,
        m.version
    ));
    Ok(())
}

/// `nasiko eval export`: the dataset as SWE-bench JSONL on stdout.
pub fn export(reference: &str) -> Result<()> {
    let (_, tasks) = dataset::load(&open_store(), reference)?;
    for t in tasks {
        println!("{}", t.to_swebench_line()?);
    }
    Ok(())
}

/// `nasiko eval run`.
pub fn run(args: &run::RunArgs) -> Result<()> {
    let store = open_store();
    let router = router::RouterClient::from_login()?;
    let info = run::run(&store, &containers::DockerCli::new(), &router, args)?;
    nasiko_utils::term::print_success(&format!("Run {} finished.", info.run_id));
    println!(
        "{}",
        report::render(&store, &info.run_id, report::Format::Text)?
    );
    Ok(())
}

/// `nasiko eval report`.
pub fn report_run(run_id: &str, format: report::Format) -> Result<()> {
    let store = open_store();
    if !report::run_exists(&store, run_id) {
        anyhow::bail!("no run {run_id} under {}", store.layout().root().display());
    }
    println!("{}", report::render(&store, run_id, format)?);
    Ok(())
}

/// `nasiko eval costs`: re-reads the router's costs for a run (e.g. after adding a price).
pub fn costs(run_id: &str) -> Result<()> {
    let store = open_store();
    let router = router::RouterClient::from_login()?;
    run::attach_costs(&store, &router, &store.layout().run_dir(run_id), run_id)?;
    println!("{}", report::render(&store, run_id, report::Format::Text)?);
    Ok(())
}
