//! Compact tool definitions: measurement report (development aid, not part of the eval contract).
//!
//!   OUT=/tmp/compact-tools-measure.md [EVAL_SET=/tmp/compact-tools-eval.json] \
//!   [FIXTURES_DIR=llm-router/examples/fixtures/compact-tools] \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_measure
//!
//! Counts `o200k_base` tokens of the full request body for native tools, TOON-encoded tools
//! and the compact notation, reports bypasses, round-trip fidelity and encode/decode time, and
//! (when `PROVIDER_BASE_URL` and `MODEL` are set) runs each variant live, one request at a time,
//! judging replies with the router's own finalization rules. `LIVE_VARIANTS` (default
//! `native,compact`) selects which variants are sent live. Public-sample results and custom
//! fixtures are reported in separate sections.

mod support;

use std::path::PathBuf;

use serde_json::Value;
use support::eval::{self, BuiltRequest, CaseInputs, EvalSet};
use support::live::{self, LiveConfig, LiveOutcome};
use support::measure::{self, CaseMeasurement, Report, TokenCounter, VariantLive};

struct Tiktoken(tiktoken_rs::CoreBPE);

impl TokenCounter for Tiktoken {
    fn count(&self, text: &str) -> usize {
        self.0.encode_ordinary(text).len()
    }
}

fn toon(value: &Value) -> Result<String, String> {
    toon_format::encode_default(value).map_err(|e| e.to_string())
}

fn load(path: &PathBuf) -> EvalSet {
    let raw =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

#[tokio::main]
async fn main() {
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "compact-tools-measure.md".into());
    let fixtures_dir = std::env::var("FIXTURES_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/fixtures/compact-tools")
        });

    let mut sources: Vec<(String, EvalSet)> = Vec::new();
    if let Ok(path) = std::env::var("EVAL_SET") {
        sources.push((
            "public sample (EVAL_SET)".into(),
            load(&PathBuf::from(path)),
        ));
    }
    for name in ["dev", "heldout"] {
        let path = fixtures_dir.join(format!("{name}.json"));
        if path.exists() {
            sources.push((format!("fixtures/{name}"), load(&path)));
        }
    }
    assert!(
        !sources.is_empty(),
        "no EVAL_SET and no fixtures found under {}",
        fixtures_dir.display()
    );

    let counter = Tiktoken(tiktoken_rs::o200k_base().expect("o200k_base"));
    let live_cfg = LiveConfig::from_env();
    let model = live_cfg
        .as_ref()
        .map(|l| l.model.clone())
        .or_else(|| std::env::var("MODEL").ok())
        .unwrap_or_else(|| eval::OFFLINE_MODEL_PLACEHOLDER.to_owned());
    let max_output_tokens = live_cfg
        .as_ref()
        .map_or(eval::DEFAULT_MAX_OUTPUT_TOKENS, |l| l.max_output_tokens);
    let live_variants: Vec<String> = std::env::var("LIVE_VARIANTS")
        .unwrap_or_else(|_| "native,compact".into())
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();

    let mut cases: Vec<CaseMeasurement> = Vec::new();
    let http = live::live_client(std::time::Duration::from_secs(
        live_cfg.as_ref().map_or(60, |l| l.timeout.as_secs()),
    ))
    .expect("http client");
    let mut auth_failed = false;
    for (source, set) in &sources {
        for case in &set.cases {
            let mut m = match measure::measure_case(
                case,
                source,
                &set.tools,
                &model,
                max_output_tokens,
                &counter,
                Some(&toon),
            ) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("{source} {}: {e}", case.id);
                    continue;
                }
            };
            if let Some(cfg) = &live_cfg
                && m.bypass.is_none()
            {
                let inputs = CaseInputs {
                    tools: &case.tools,
                    messages: &case.messages,
                };
                let variants = measure::build_variants(
                    inputs,
                    &set.tools,
                    &model,
                    max_output_tokens,
                    Some(&toon),
                )
                .expect("variants");
                let compact_built = eval::build_request(
                    CaseInputs {
                        tools: &case.tools,
                        messages: &case.messages,
                    },
                    &set.tools,
                    &model,
                    max_output_tokens,
                )
                .expect("request");
                let native_built = BuiltRequest {
                    body: variants.native.clone(),
                    compacted: false,
                    bypass: None,
                    compiled: None,
                    tool_defs: compact_built.tool_defs.clone(),
                };
                let bodies: Vec<(&str, &Value, &BuiltRequest)> = [
                    ("native", Some(&variants.native), &native_built),
                    ("toon", variants.toon.as_ref(), &compact_built),
                    ("compact", variants.compact.as_ref(), &compact_built),
                ]
                .into_iter()
                .filter(|(name, body, _)| body.is_some() && live_variants.iter().any(|v| v == name))
                .map(|(name, body, built)| (name, body.expect("filtered"), built))
                .collect();
                for (variant, body, built) in bodies {
                    if auth_failed {
                        // Reported as skipped, never as a failure of the model or the format.
                        measure::record_live(&mut m, variant, VariantLive::Skipped);
                        continue;
                    }
                    match live::call_live(&http, cfg, body).await {
                        LiveOutcome::Completed(reply) => {
                            let judged = live::judge_reply(&reply, built);
                            let result = judged.finalized.map(|f| {
                                f.calls
                                    .into_iter()
                                    .map(|c| (c.name, c.arguments))
                                    .collect::<Vec<_>>()
                            });
                            measure::record_live(
                                &mut m,
                                variant,
                                measure::verdict(result, &case.expected, case.match_rules.as_ref()),
                            );
                        }
                        LiveOutcome::Failed { status, message } => {
                            measure::record_live(
                                &mut m,
                                variant,
                                VariantLive::TransportError(format!(
                                    "http {}: {message}",
                                    status.map_or("none".to_owned(), |s| s.to_string())
                                )),
                            );
                        }
                        LiveOutcome::AuthFailed { status, message } => {
                            auth_failed = true;
                            eprintln!(
                                "authentication failed ({status}): {message}; remaining live calls skipped"
                            );
                            measure::record_live(
                                &mut m,
                                variant,
                                VariantLive::TransportError(format!("auth {status}")),
                            );
                        }
                    }
                }
            }
            cases.push(m);
        }
    }

    let report = Report {
        cases,
        tokenizer: "tiktoken-rs 0.12.1 / o200k_base".into(),
        toon_note: "encoded with toon-format 0.5.0 (implements TOON spec v3.0; the current spec is v4.1), one TOON document per tool from its {name, description, parameters} object, with the TOON variant's own one-sentence preamble plus the shared call protocol. These figures describe that crate and version, not every TOON implementation.".into(),
        live_model: live_cfg.as_ref().map(|l| l.model.clone()),
        live_endpoint: live_cfg.as_ref().map(|l| l.base_url.clone()),
    };
    let markdown = measure::render_markdown(&report);
    std::fs::write(&out_path, &markdown).expect("write OUT");
    println!("{markdown}");
}
