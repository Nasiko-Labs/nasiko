//! Request classifier scorer — turns an `OUT` file (and its diagnostics sidecar) into the
//! report the PR cites. Separate from `classifier_eval`, which only writes predictions.
//!
//! ```sh
//! EVAL_SET=/tmp/classifier-eval.json PRED=/tmp/classifier-out.jsonl \
//! cargo run --release -p nasiko-llm-router --example classifier_report
//! ```
//!
//! Optional: `DIAG=<sidecar>` (default `<PRED>.diagnostics.jsonl` if it exists) adds
//! fallback counts by cause and a "deployed policy" view; `PRED2=<second run>` adds a
//! repeatability check on the semantic fields (`latency_us` is expected to differ);
//! `ECE_BINS` (default 10); `PRICE_PER_M_INPUT` (USD per million input tokens, for the
//! cost-per-decision line — the source and date must be stated alongside the figure).
//!
//! Reads labels (this is the only place labels are read). Prints Markdown to stdout.
use std::collections::BTreeMap;
use std::process::ExitCode;

use nasiko_llm_router::routing::classifier_eval::metrics::{
    Prediction, Report, score, semantic_diff,
};
use nasiko_llm_router::routing::classifier_eval::{
    read_diagnostics, read_labeled_cases, read_predictions,
};
use nasiko_llm_router::routing::{Disposition, RequestType};

fn fail(msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("classifier_report: error: {msg}");
    ExitCode::from(2)
}

fn pct(x: f64) -> String {
    format!("{:.1}%", x * 100.0)
}

fn print_report(title: &str, r: &Report) {
    println!("### {title}\n");
    println!("| metric | value |\n|---|---|");
    println!("| cases / scored | {} / {} |", r.cases, r.scored);
    println!("| request-type accuracy | {} |", pct(r.accuracy));
    println!("| macro F1 | {:.3} |", r.macro_f1);
    match r.ece {
        Some(e) => println!(
            "| ECE ({} equal-width bins) | {:.3} (over {} rows with confidence) |",
            r.ece_bins,
            e,
            r.scored - r.without_confidence
        ),
        None => println!("| ECE | n/a (no row carried a confidence) |"),
    }
    match (
        r.complexity_exact,
        r.complexity_mae,
        r.complexity_within_one,
    ) {
        (Some(e), Some(m), Some(w)) => println!(
            "| complexity exact / within ±1 / MAE | {} / {} / {:.2} (over {} rows) |",
            pct(e),
            pct(w),
            m,
            r.complexity_scored
        ),
        _ => println!("| complexity | n/a (no row carried a complexity) |"),
    }
    println!(
        "| latency p50 / p95 (µs) | {} / {} |",
        r.latency_us_p50, r.latency_us_p95
    );
    println!();
    println!("Per class:\n");
    println!(
        "| class | support | predicted | correct | P | R | F1 |\n|---|---|---|---|---|---|---|"
    );
    for c in &r.per_class {
        println!(
            "| {} | {} | {} | {} | {:.2} | {:.2} | {:.2} |",
            c.request_type.as_str(),
            c.support,
            c.predicted,
            c.correct,
            c.precision,
            c.recall,
            c.f1
        );
    }
    println!();
    println!(
        "Confusion (rows = truth, cols = predicted; order {}):\n",
        RequestType::ALL
            .iter()
            .map(|r| r.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "| truth \\ pred | {} |",
        RequestType::ALL
            .iter()
            .map(|r| short(*r))
            .collect::<Vec<_>>()
            .join(" | ")
    );
    println!("|---|{}|", "---|".repeat(RequestType::ALL.len()));
    for (i, rt) in RequestType::ALL.iter().enumerate() {
        println!(
            "| {} | {} |",
            short(*rt),
            r.confusion[i]
                .iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(" | ")
        );
    }
    println!();
    if r.ece.is_some() {
        println!("Calibration bins:\n");
        println!("| bin | n | mean conf | accuracy |\n|---|---|---|---|");
        for b in r.calibration.iter().filter(|b| b.count > 0) {
            println!(
                "| [{:.1}, {:.1}) | {} | {:.3} | {:.3} |",
                b.lo, b.hi, b.count, b.mean_confidence, b.accuracy
            );
        }
        println!();
        println!("Selective accuracy (keep rows with confidence ≥ t):\n");
        println!("| t | coverage | accuracy on covered |\n|---|---|---|");
        for s in &r.selective {
            println!(
                "| {:.1} | {} | {} |",
                s.threshold,
                pct(s.coverage),
                pct(s.accuracy_on_covered)
            );
        }
        println!();
    }
}

fn short(rt: RequestType) -> &'static str {
    match rt {
        RequestType::CodeGeneration => "code_gen",
        RequestType::CodeUnderstanding => "code_und",
        RequestType::TechnicalDesign => "design",
        RequestType::AnalyticalReasoning => "analysis",
        RequestType::Writing => "writing",
        RequestType::FactualLookup => "factual",
        RequestType::General => "general",
    }
}

fn main() -> ExitCode {
    let set_path = match std::env::var("EVAL_SET") {
        Ok(p) if !p.trim().is_empty() => p,
        _ => return fail("set EVAL_SET to the labelled eval JSON path"),
    };
    let pred_path = match std::env::var("PRED") {
        Ok(p) if !p.trim().is_empty() => p,
        _ => return fail("set PRED to the predictions JSONL written by classifier_eval"),
    };
    let ece_bins: usize = std::env::var("ECE_BINS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);

    let labels = match std::fs::read_to_string(&set_path)
        .map_err(|e| e.to_string())
        .and_then(|r| read_labeled_cases(&r))
    {
        Ok(l) => l,
        Err(e) => return fail(format!("EVAL_SET '{set_path}': {e}")),
    };
    let preds = match std::fs::read_to_string(&pred_path)
        .map_err(|e| e.to_string())
        .and_then(|r| read_predictions(&r))
    {
        Ok(p) => p,
        Err(e) => return fail(format!("PRED '{pred_path}': {e}")),
    };
    let diag_path =
        std::env::var("DIAG").unwrap_or_else(|_| format!("{pred_path}.diagnostics.jsonl"));
    let diag = std::fs::read_to_string(&diag_path)
        .ok()
        .map(|r| read_diagnostics(&r));
    let diag = match diag {
        Some(Ok(d)) => Some(d),
        Some(Err(e)) => return fail(format!("DIAG '{diag_path}': {e}")),
        None => None,
    };

    println!("## Classifier report\n");
    println!("- labels: `{set_path}` ({} cases)", labels.len());
    println!("- predictions: `{pred_path}` ({} rows)", preds.len());
    match &diag {
        Some(d) => println!("- diagnostics: `{diag_path}` ({} rows)", d.len()),
        None => println!("- diagnostics: none found (fallback breakdown unavailable)"),
    }
    println!();

    // Raw quality: every row scored as written.
    let raw = score(&labels, &preds, ece_bins);
    print_report("Raw classifier output (every row as written to OUT)", &raw);

    if let Some(d) = &diag {
        let by_id: BTreeMap<&str, _> = d.iter().map(|r| (r.id.as_str(), r)).collect();
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        let mut versions: BTreeMap<String, usize> = BTreeMap::new();
        let mut input_tokens = 0u64;
        let mut hosted_rows = 0usize;
        for r in d {
            let key = match &r.disposition {
                Disposition::Regex => "regex (configured)".to_string(),
                Disposition::Primary => format!("{} (accepted)", r.answered_by),
                Disposition::Abstained => format!("{} (abstained: low confidence)", r.answered_by),
                Disposition::Fallback { reason, .. } => {
                    format!("regex fallback: {}", reason.as_str())
                }
            };
            *counts.entry(key).or_default() += 1;
            if let Some(x) = &r.diagnostics {
                hosted_rows += 1;
                input_tokens += x.input_tokens.unwrap_or(0);
                if let Some(v) = &x.model_version {
                    *versions.entry(v.clone()).or_default() += 1;
                }
            }
        }
        println!("### Dispositions (from the diagnostics sidecar)\n");
        println!("| disposition | rows | share |\n|---|---|---|");
        for (k, n) in &counts {
            println!("| {k} | {n} | {} |", pct(*n as f64 / d.len().max(1) as f64));
        }
        let fallbacks: usize = d
            .iter()
            .filter(|r| matches!(r.disposition, Disposition::Fallback { .. }))
            .count();
        println!(
            "| **fallback rate** | {fallbacks} | {} |",
            pct(fallbacks as f64 / d.len().max(1) as f64)
        );
        println!();
        if !versions.is_empty() {
            println!(
                "Model versions that answered: {}\n",
                versions
                    .iter()
                    .map(|(k, n)| format!("`{k}`×{n}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        if hosted_rows > 0 {
            let per = input_tokens as f64 / hosted_rows as f64;
            println!(
                "Billed input tokens: {input_tokens} over {hosted_rows} hosted answers ({per:.0} per decision; instructions and criteria included)."
            );
            if let Some(price) = std::env::var("PRICE_PER_M_INPUT")
                .ok()
                .and_then(|p| p.parse::<f64>().ok())
            {
                println!(
                    "Estimated cost per decision at ${price}/M input tokens: ${:.6} (output tokens priced at $0 per vendor pricing; state the price source and date next to this figure).",
                    per * price / 1_000_000.0
                );
            } else {
                println!(
                    "Cost per decision: set `PRICE_PER_M_INPUT` to compute (no price assumed)."
                );
            }
            println!();
        }

        // Deployed-policy quality: what the router would act on. Abstentions are not
        // scored as classifications (they route to the safe default); fallbacks are
        // scored as the regex labels they are.
        let acted: Vec<Prediction> = preds
            .iter()
            .filter(|p| {
                by_id
                    .get(p.id.as_str())
                    .is_some_and(|r| r.disposition != Disposition::Abstained)
            })
            .cloned()
            .collect();
        let policy = score(&labels, &acted, ece_bins);
        print_report(
            "Deployed-policy view (abstentions excluded as 'routed to safe default'; regex fallbacks scored as regex)",
            &policy,
        );
        let hosted_only: Vec<Prediction> = preds
            .iter()
            .filter(|p| {
                by_id.get(p.id.as_str()).is_some_and(|r| {
                    r.disposition == Disposition::Primary || r.disposition == Disposition::Abstained
                })
            })
            .cloned()
            .collect();
        if !hosted_only.is_empty() && hosted_only.len() != preds.len() {
            let h = score(&labels, &hosted_only, ece_bins);
            print_report("Hosted-only rows (fallbacks excluded)", &h);
        }
    }

    if let Ok(p2) = std::env::var("PRED2") {
        match std::fs::read_to_string(&p2)
            .map_err(|e| e.to_string())
            .and_then(|r| read_predictions(&r))
        {
            Ok(second) => {
                let (compared, differing) = semantic_diff(&preds, &second);
                println!("### Repeatability vs `{p2}`\n");
                println!(
                    "- compared {compared} rows on request_type/complexity/confidence (latency_us ignored): {} differ ({})",
                    differing.len(),
                    pct(differing.len() as f64 / compared.max(1) as f64)
                );
                if !differing.is_empty() {
                    println!("- differing ids: {}", differing.join(", "));
                }
                println!();
            }
            Err(e) => return fail(format!("PRED2 '{p2}': {e}")),
        }
    }
    ExitCode::SUCCESS
}
