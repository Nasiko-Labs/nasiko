//! Compact tool definitions: evaluation output.
//!
//! Offline (default, no network):
//!   EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//!
//! Live (adds `raw_output` and `live_calls` per case):
//!   PROVIDER_BASE_URL=https://host/v1 MODEL=some-model [PROVIDER_API_KEY=… | BEDROCK_API_KEY=…] \
//!   EVAL_SET=… OUT=… cargo run --release -p nasiko-llm-router --example compact_tools_eval
//!
//! Optional: `MODEL` (also used offline as the request's `model`), `MAX_OUTPUT_TOKENS` (1024),
//! `LIVE_TIMEOUT_SECS` (60). Writes one JSONL line per case to `OUT`; reports outputs, not
//! scores. Exit code 0 means the run completed; per-case failures are recorded in the line.

mod support;

use std::io::Write;

use serde_json::{Value, json};
use support::eval::{self, CaseInputs, EvalSet};
use support::live::{self, LiveConfig, LiveOutcome};

#[tokio::main]
async fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "compact-tools-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let set: EvalSet = serde_json::from_str(&raw).expect("valid eval JSON");
    let live = LiveConfig::from_env();
    let model = live
        .as_ref()
        .map(|l| l.model.clone())
        .or_else(|| std::env::var("MODEL").ok().filter(|m| !m.is_empty()))
        .unwrap_or_else(|| eval::OFFLINE_MODEL_PLACEHOLDER.to_owned());
    let max_output_tokens = live
        .as_ref()
        .map(|l| l.max_output_tokens)
        .unwrap_or_else(|| {
            std::env::var("MAX_OUTPUT_TOKENS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(eval::DEFAULT_MAX_OUTPUT_TOKENS)
        });

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    match live {
        None => eval::run_offline(&set, &model, max_output_tokens, &mut out).expect("write OUT"),
        Some(cfg) => run_live(&set, &cfg, &mut out).await,
    }
    out.flush().expect("flush OUT");
}

/// Offline lines plus `raw_output` / `live_calls` (or `live_error`) per ordinary case. Decoder
/// cases are offline by nature and are written unchanged.
async fn run_live(set: &EvalSet, cfg: &LiveConfig, out: &mut dyn Write) {
    let http = live::live_client(cfg.timeout).expect("http client");
    let mut auth_failed = false;
    for case in &set.cases {
        let mut line = eval::case_line(case, &set.tools, &cfg.model, cfg.max_output_tokens);
        if line.get("error").is_none() {
            let built = eval::build_request(
                CaseInputs {
                    tools: &case.tools,
                    messages: &case.messages,
                },
                &set.tools,
                &cfg.model,
                cfg.max_output_tokens,
            )
            .expect("case_line already built this request");
            if auth_failed {
                line["live_error"] =
                    json!({"status": null, "message": "skipped after an authentication failure"});
            } else {
                match live::call_live(&http, cfg, &built.body).await {
                    LiveOutcome::Completed(reply) => {
                        let judged = live::judge_reply(&reply, &built);
                        line["raw_output"] = judged.raw_output;
                        line["live_calls"] = judged.live_calls;
                    }
                    LiveOutcome::Failed { status, message } => {
                        line["live_error"] = json!({"status": status, "message": message});
                    }
                    LiveOutcome::AuthFailed { status, message } => {
                        auth_failed = true;
                        line["live_error"] = json!({"status": status, "message": message});
                        eprintln!("authentication failed ({status}); remaining live calls skipped");
                    }
                }
            }
        }
        eval::write_line(out, &line).expect("write OUT");
    }
    for case in &set.decoder_cases {
        let line: Value = eval::decoder_line(case, &set.tools);
        eval::write_line(out, &line).expect("write OUT");
    }
}
