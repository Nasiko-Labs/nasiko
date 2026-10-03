//! Request classifier eval — the harness entry point.
//!
//! ```sh
//! EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
//! cargo run --release -p nasiko-llm-router --example classifier_eval
//! ```
//!
//! No required arguments. Reads the `examples` array from `EVAL_SET` and writes one JSONL
//! row per case, in input order, to `OUT` (default `classifier-out.jsonl`):
//! `{"id","request_type","complexity","confidence","latency_us"}`. Scores are not
//! computed here (`classifier_report` does that); exit 0 means the eval ran.
//!
//! Backend selection and every other knob come from the same env vars the router reads
//! (`ClassifierConfig::from_env`): `CLASSIFIER_BACKEND=regex|jev` (default regex, no
//! network), `CLASSIFIER_ENDPOINT`, `CLASSIFIER_MODEL`, `TYPESAFE_API_KEY`,
//! `CLASSIFIER_TIMEOUT_MS`, `CLASSIFIER_MIN_CONFIDENCE`, `CLASSIFIER_MAX_CONCURRENCY`,
//! `CLASSIFIER_RETRIES`. The service is built once before the loop; `latency_us` excludes
//! that initialization, which is reported separately on stderr.
//!
//! Diagnostics (effective backend per row, fallback reason, hosted distributions, token
//! usage, model version) go to a sidecar JSONL: `DIAG_OUT`, default `<OUT>.diagnostics.jsonl`.
//! The sidecar is the only place extra fields live; `OUT` stays on the harness schema.
use std::io::Write;
use std::process::ExitCode;

use nasiko_llm_router::config::ClassifierConfig;
use nasiko_llm_router::routing::ClassifierService;
use nasiko_llm_router::routing::classifier_eval::{describe_summary, read_eval_cases, run_eval};

fn fail(msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("classifier_eval: error: {msg}");
    ExitCode::from(2)
}

#[tokio::main]
async fn main() -> ExitCode {
    let path = match std::env::var("EVAL_SET") {
        Ok(p) if !p.trim().is_empty() => p,
        _ => return fail("set EVAL_SET to the eval JSON path (e.g. /tmp/classifier-eval.json)"),
    };
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "classifier-out.jsonl".into());
    let diag_path =
        std::env::var("DIAG_OUT").unwrap_or_else(|_| format!("{out_path}.diagnostics.jsonl"));

    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        Err(e) => return fail(format!("cannot read EVAL_SET '{path}': {e}")),
    };
    let cases = match read_eval_cases(&raw) {
        Ok(c) => c,
        Err(e) => return fail(format!("EVAL_SET '{path}': {e}")),
    };

    // One service for the whole run, built from the same loader the router uses.
    let cfg = ClassifierConfig::from_env();
    let service = ClassifierService::from_config(&cfg);

    let out_file = match std::fs::File::create(&out_path) {
        Ok(f) => f,
        Err(e) => return fail(format!("cannot create OUT '{out_path}': {e}")),
    };
    let diag_file = match std::fs::File::create(&diag_path) {
        Ok(f) => f,
        Err(e) => return fail(format!("cannot create DIAG_OUT '{diag_path}': {e}")),
    };
    let mut out = std::io::BufWriter::new(out_file);
    let mut diag = std::io::BufWriter::new(diag_file);

    let summary = match run_eval(&service, &cases, &mut out, Some(&mut diag)).await {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    if let Err(e) = out.flush().and_then(|_| diag.flush()) {
        return fail(format!("flush: {e}"));
    }
    eprint!("{}", describe_summary(&summary));
    eprintln!(
        "  wrote {} rows to {out_path}; diagnostics in {diag_path}",
        summary.cases
    );
    ExitCode::SUCCESS
}
