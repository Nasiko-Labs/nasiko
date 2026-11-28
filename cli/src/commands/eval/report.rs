//! `nasiko eval report <run>`: the headline sentence and the tables, as text, JSON or Markdown.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use nasiko_evals::report::{self, MetricRange, ModelSummary, ParetoStatus, Rate};

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

fn usd_range(r: Option<MetricRange>) -> String {
    r.map(|r| format!("${:.4} (${:.4}–${:.4})", r.value, r.low95, r.high95))
        .unwrap_or_else(|| "-".into())
}

fn pct_range(r: Option<MetricRange>) -> String {
    r.map(|r| format!("{} ({}–{})", pct(r.value), pct(r.low95), pct(r.high95)))
        .unwrap_or_else(|| "-".into())
}

fn rate_range(r: Option<Rate>) -> String {
    r.map(|r| format!("{} ({}–{})", pct(r.value), pct(r.low95), pct(r.high95)))
        .unwrap_or_else(|| "-".into())
}

fn pareto_label(status: ParetoStatus) -> &'static str {
    match status {
        ParetoStatus::ParetoOptimal => "Optimal",
        ParetoStatus::Dominated => "Dominated",
        ParetoStatus::InsufficientData => "-",
    }
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
    s.push_str("\n## Cost vs. Confidence\n\n| Model | Mean Cost / Attempt (95% CI) | Cost / Resolved (95% CI) | Saving vs Control (95% CI) | Resolved / $ | Consistency | Pareto Status |\n|---|---|---|---|---|---|---|\n");
    for m in summaries {
        let cc = m.cost_confidence.as_ref();
        let mean_cost = usd_range(cc.and_then(|c| c.mean_cost_per_attempt));
        let cost_per_resolved = usd_range(cc.and_then(|c| c.cost_per_resolved));
        let saving_ci = pct_range(cc.and_then(|c| c.saving_vs_control_ci));
        let resolved_per_dollar = cc
            .and_then(|c| c.resolved_per_dollar)
            .map(|v| format!("{v:.2}"))
            .unwrap_or_else(|| "-".into());
        let consistency = rate_range(cc.and_then(|c| c.consistency));
        let pareto = cc.map(|c| pareto_label(c.pareto_status)).unwrap_or("-");
        s.push_str(&format!(
            "| {} | {mean_cost} | {cost_per_resolved} | {saving_ci} | {resolved_per_dollar} | {consistency} | {pareto} |\n",
            m.model
        ));
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

#[cfg(test)]
mod tests {
    use super::*;
    use nasiko_evals::cost::PriceQuality;
    use nasiko_evals::grade::{FailureClass, Grade, Resolution};
    use nasiko_evals::instance::Difficulty;
    use nasiko_evals::report::AttemptRecord;

    fn attempt(
        id: &str,
        model: &str,
        attempt: u32,
        resolved: bool,
        cost: Option<f64>,
    ) -> AttemptRecord {
        AttemptRecord {
            instance_id: id.into(),
            model: model.into(),
            attempt,
            difficulty: Some(Difficulty::Under15Min),
            grade: Grade {
                resolution: if resolved {
                    Resolution::Full
                } else {
                    Resolution::No
                },
                resolved,
                f2p_passed: u32::from(resolved),
                f2p_total: 1,
                p2p_passed: 0,
                p2p_total: 0,
                failure: (!resolved).then_some(FailureClass::TestsFailed),
            },
            usage: None,
            cost_usd: cost,
            worst_price: cost.map(|_| PriceQuality::Exact),
            wall_secs: 1,
            original_cost_usd: None,
        }
    }

    #[test]
    fn markdown_renders_cost_vs_confidence_table_and_missing_values() {
        let records = vec![
            attempt("t1", "flagship", 1, true, Some(1.0)),
            attempt("t2", "flagship", 1, true, Some(1.0)),
            attempt("t1", "flash", 1, true, Some(0.2)),
            attempt("t1", "flash", 2, true, Some(0.2)),
            attempt("t2", "flash", 1, false, Some(0.2)),
            attempt("t2", "flash", 2, true, Some(0.2)),
            attempt("t1", "unpriced", 1, false, None),
        ];
        let summaries = report::summarise(&records, "flagship");
        let info = RunInfo {
            run_id: "run-1".into(),
            dataset: "ds".into(),
            dataset_version: "1".into(),
            models: vec!["flagship".into(), "flash".into(), "unpriced".into()],
            control: Some("flagship".into()),
            builders_tested: vec![],
            attempts: 2,
            prompt_version: "p1".into(),
            opencode_version: "oc1".into(),
            created_at: chrono::Utc::now(),
        };
        let md = markdown(&info, &summaries, Some("flagship"), "ds@1");
        assert!(md.contains("## Cost vs. Confidence"), "{md}");
        assert!(md.contains("| Model | Mean Cost / Attempt (95% CI) | Cost / Resolved (95% CI) | Saving vs Control (95% CI) | Resolved / $ | Consistency | Pareto Status |"), "{md}");
        assert!(md.contains("| flash | $0.2000 ($0.2000–$0.2000) |"), "{md}");
        assert!(md.contains("50% (0%–100%) | Optimal |"), "{md}");
        assert!(md.contains("| unpriced | - | - | - | - | - | - |"), "{md}");
    }
}
