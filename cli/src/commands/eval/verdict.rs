//! `nasiko eval verdict <run>`: is it safe to switch from the control model to a cheaper one?
//!
//! Prints one paired verdict per candidate and exits 0 only when every candidate is SAFE, so a
//! CI job can block a bad model switch: 2 means NOT SAFE, 3 NOT ENOUGH DATA (1 stays for tool
//! errors, e.g. an unreachable cluster).

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;

use anyhow::{Context, Result, bail};
use nasiko_evals::verdict::{self, Decision, Outcome, Verdict};
use serde_json::Value;

use super::open_store;
use super::run::{RunInfo, attempt_files};

/// What to compare and how strict to be.
#[derive(Debug, Clone)]
pub struct Options {
    /// The model to switch to; every non-control model when `None`.
    pub candidate: Option<String>,
    /// The model to switch from; the run's control when `None`.
    pub control: Option<String>,
    /// How many points worse (0-100) the candidate may be.
    pub margin_points: f64,
    /// Let NOT ENOUGH DATA pass (exit 0) instead of blocking.
    pub allow_inconclusive: bool,
    pub json: bool,
}

/// The verdict for a run of `nasiko eval local run`.
pub fn local(run_id: &str, opts: &Options) -> Result<()> {
    let store = open_store();
    let run_dir = store.layout().run_dir(run_id);
    let info: RunInfo =
        serde_json::from_str(&fs::read_to_string(run_dir.join("run.json")).with_context(
            || format!("no run {run_id} under {}", store.layout().root().display()),
        )?)?;
    let outcomes: Vec<Outcome> = attempt_files(&run_dir)?
        .iter()
        .map(|(_, f)| Outcome::from(&f.record))
        .collect();
    finish(&outcomes, &info.models, info.control.as_deref(), opts)
}

/// The verdict for a run on the cluster.
pub fn remote(run: &Value, opts: &Options) -> Result<()> {
    let status = run["status"].as_str().unwrap_or("-");
    if status != "done" {
        eprintln!("run is {status}: the verdict uses only the attempts that have ended");
    }
    let (outcomes, models) = outcomes_of_run(run);
    finish(&outcomes, &models, run["report"]["control"].as_str(), opts)
}

/// The ended attempts of a cluster run (`GET /evals/runs/{id}`), and its model names.
pub fn outcomes_of_run(run: &Value) -> (Vec<Outcome>, Vec<String>) {
    let names: BTreeMap<String, String> = run["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            Some((
                m["id"].as_str()?.to_string(),
                m["name"].as_str()?.to_string(),
            ))
        })
        .collect();
    let outcomes = run["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| matches!(r["status"].as_str(), Some("done" | "failed")))
        .filter_map(|r| {
            let model_id = r["model_id"].as_str()?;
            let resolved = r["resolved"].as_bool();
            Some(Outcome {
                task: r["task_id"].as_str()?.to_string(),
                model: names
                    .get(model_id)
                    .cloned()
                    .unwrap_or_else(|| model_id.into()),
                attempt: r["attempt"].as_u64().unwrap_or(1) as u32,
                resolved: resolved.unwrap_or(false),
                // No graded record means the sandbox failed before the model was judged.
                infra: resolved.is_none() || r["failure"].as_str() == Some("infra_error"),
                cost_usd: r["cost_usd"].as_f64(),
            })
        })
        .collect();
    (outcomes, names.into_values().collect())
}

fn finish(
    outcomes: &[Outcome],
    models: &[String],
    run_control: Option<&str>,
    opts: &Options,
) -> Result<()> {
    let verdicts = verdicts(outcomes, models, run_control, opts)?;
    let code = exit_code(&verdicts, opts.allow_inconclusive);
    if opts.json {
        println!("{}", serde_json::to_string_pretty(&verdicts)?);
    } else {
        for v in &verdicts {
            println!("{}", verdict::sentence(v));
        }
        if code != 0 {
            println!("Blocked: exit {code}.");
        }
    }
    std::io::stdout().flush()?;
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

fn verdicts(
    outcomes: &[Outcome],
    models: &[String],
    run_control: Option<&str>,
    opts: &Options,
) -> Result<Vec<Verdict>> {
    let Some(control) = opts.control.as_deref().or(run_control) else {
        bail!("the run has no control model: name the model to switch from with --control");
    };
    if !(0.0..=100.0).contains(&opts.margin_points) {
        bail!("--margin is in points, 0-100");
    }
    let candidates: Vec<&str> = match &opts.candidate {
        Some(c) => vec![c.as_str()],
        None => models
            .iter()
            .map(String::as_str)
            .filter(|m| *m != control)
            .collect(),
    };
    if candidates.is_empty() {
        bail!("no model to compare with {control}: name one with --candidate");
    }
    Ok(candidates
        .into_iter()
        .map(|c| verdict::verdict(outcomes, control, c, opts.margin_points / 100.0))
        .collect())
}

/// The strictest verdict's code: one unsafe candidate blocks the switch.
fn exit_code(verdicts: &[Verdict], allow_inconclusive: bool) -> i32 {
    verdicts
        .iter()
        .map(|v| match v.decision {
            Decision::NotEnoughData if allow_inconclusive => 0,
            d => d.exit_code(),
        })
        .max_by_key(|&c| match c {
            2 => 2,
            3 => 1,
            _ => 0,
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn opts() -> Options {
        Options {
            candidate: None,
            control: None,
            margin_points: 10.0,
            allow_inconclusive: false,
            json: false,
        }
    }

    #[test]
    fn cluster_results_become_paired_outcomes() {
        let run = json!({
            "status": "done",
            "models": [{"id": "m1", "name": "big"}, {"id": "m2", "name": "small"}],
            "results": [
                {"task_id": "t1", "model_id": "m1", "attempt": 1, "status": "done", "resolved": true, "cost_usd": 1.0},
                {"task_id": "t1", "model_id": "m2", "attempt": 1, "status": "done", "resolved": false, "failure": "tests_failed", "cost_usd": 0.1},
                {"task_id": "t2", "model_id": "m2", "attempt": 1, "status": "failed", "resolved": null},
                {"task_id": "t3", "model_id": "m2", "attempt": 1, "status": "pending"}
            ],
            "report": {"control": "big"}
        });
        let (outcomes, models) = outcomes_of_run(&run);
        assert_eq!(models, vec!["big", "small"]);
        assert_eq!(outcomes.len(), 3);
        assert!(!outcomes[1].resolved && !outcomes[1].infra);
        assert!(outcomes[2].infra);
        let v = verdicts(&outcomes, &models, Some("big"), &opts()).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(
            (v[0].candidate.as_str(), v[0].pairs.control_only),
            ("small", 1)
        );
    }

    #[test]
    fn one_unsafe_candidate_blocks_and_inconclusive_can_be_allowed() {
        let mk = |d: Decision| Verdict {
            decision: d,
            ..verdict::verdict(&[], "a", "b", 0.1)
        };
        let all = [
            mk(Decision::Safe),
            mk(Decision::NotEnoughData),
            mk(Decision::NotSafe),
        ];
        assert_eq!(exit_code(&all, false), 2);
        assert_eq!(exit_code(&all[..2], false), 3);
        assert_eq!(exit_code(&all[..2], true), 0);
        assert_eq!(exit_code(&all[..1], false), 0);
    }

    #[test]
    fn a_control_is_required() {
        let err = verdicts(&[], &["a".into()], None, &opts()).unwrap_err();
        assert!(err.to_string().contains("--control"));
        let err = verdicts(&[], &["a".into()], Some("a"), &opts()).unwrap_err();
        assert!(err.to_string().contains("--candidate"));
    }
}
