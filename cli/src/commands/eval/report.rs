//! `nasiko eval report <run>`: the headline sentence and the tables, as text, JSON or Markdown.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use nasiko_evals::report::{self, ModelSummary};

use super::run::{RunInfo, attempt_files};
use super::store::Store;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Text,
    Json,
    Markdown,
}

pub fn render(store: &Store, run_id: &str, format: Format) -> Result<String> {
    let run_dir = store.layout().run_dir(run_id);
    let info: RunInfo = serde_json::from_str(
        &fs::read_to_string(run_dir.join("run.json"))
            .with_context(|| format!("no run {run_id}"))?,
    )?;
    let records: Vec<_> = attempt_files(&run_dir)?
        .into_iter()
        .map(|(_, f)| f.record)
        .collect();
    let control = info.control.as_deref();
    let summaries = report::summarise(&records, control.unwrap_or(""));
    let dataset = format!("{}@{}", info.dataset, info.dataset_version);
    let out = match format {
        Format::Json => serde_json::to_string_pretty(&serde_json::json!({
            "run": info,
            "summaries": summaries,
            "attempts": records,
        }))?,
        Format::Text | Format::Markdown => markdown(&info, &summaries, control, &dataset),
    };
    if format == Format::Markdown {
        store.save_text(&run_dir.join("report.md"), &out)?;
    }
    if format == Format::Json {
        store.save_text(&run_dir.join("report.json"), &out)?;
    }
    Ok(out)
}

fn pct(v: f64) -> String {
    format!("{:.0}%", v * 100.0)
}

fn markdown(
    info: &RunInfo,
    summaries: &[ModelSummary],
    control: Option<&str>,
    dataset: &str,
) -> String {
    let mut s = format!("# Replay eval {}\n\n", info.run_id);
    for m in summaries.iter().filter(|m| !m.is_control) {
        s.push_str(&format!(
            "**{}**\n\n",
            report::headline(m, control, dataset)
        ));
    }
    s.push_str("| Model | Resolved (95% range) | Match | Cost (US$) | Per resolved | Saving vs control | Saving vs original | Unpriced |\n");
    s.push_str("|---|---|---|---|---|---|---|---|\n");
    for m in summaries {
        let opt = |v: Option<f64>, f: fn(f64) -> String| v.map(f).unwrap_or_else(|| "—".into());
        s.push_str(&format!(
            "| {}{} | {} ({}–{}), n={} | {} | {:.4} | {} | {} | {} | {} |\n",
            m.model,
            if info.builders_tested.contains(&m.model) {
                " (built the dataset: optimistic)"
            } else if m.is_control {
                " (control)"
            } else {
                ""
            },
            pct(m.resolved.value),
            pct(m.resolved.low95),
            pct(m.resolved.high95),
            m.resolved.n,
            opt(m.match_rate.map(|r| r.value), pct),
            m.cost_usd,
            opt(m.cost_per_resolved, |v| format!("{v:.4}")),
            opt(m.saving_vs_control, pct),
            opt(m.saving_vs_original, pct),
            m.unpriced_attempts,
        ));
    }
    s.push_str("\n## By difficulty\n\n| Model | Level | Resolved | n |\n|---|---|---|---|\n");
    for m in summaries {
        for (level, r) in &m.by_level {
            s.push_str(&format!(
                "| {} | {level} | {} | {} |\n",
                m.model,
                pct(r.value),
                r.n
            ));
        }
    }
    s.push_str("\n## Why attempts failed\n\n| Model | Reason | Count |\n|---|---|---|\n");
    for m in summaries {
        for (reason, n) in &m.failures {
            s.push_str(&format!("| {} | {reason} | {n} |\n", m.model));
        }
    }
    s.push_str(&format!(
        "\nDataset {dataset} · prompt {} · opencode {} · {} attempt(s) per task · costs from the Nasiko router.\n",
        info.prompt_version, info.opencode_version, info.attempts
    ));
    s
}

/// Whether a run folder exists (for friendlier errors).
pub fn run_exists(store: &Store, run_id: &str) -> bool {
    Path::new(&store.layout().run_dir(run_id))
        .join("run.json")
        .exists()
}
