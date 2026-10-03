//! Request-classifier benchmark, run by `cargo test` — regex vs. the in-process local model
//! (vs. Laya, when `LAYA_URL` points at a reachable `laya-serve`, and optimized/raw
//! Strands when `STRANDS_URL` points at a ready sidecar), through the router's own code
//! paths, on three labelled sets:
//!
//! - `eval/h4-validation.json` — 154 held-out requests, 22 per type, each tagged with the
//!   failure mode it exercises (keyword traps, no keywords, multilingual, typos, ...).
//! - `eval/h4-public-sample.json` — the hackathon's 10 public examples
//!   (https://registry.nasiko.dev/r/nasiko/classifier-eval).
//! - `eval/requests.jsonl` — 100 hand-labelled requests with real chat history, classified
//!   from `[Message]`s exactly as a live request is (`classify_input`).
//!
//! For every backend it measures what routing depends on: type accuracy / macro-F1 / per-type
//! F1; complexity exact, ±1, MAE and hard-request recall; confidence calibration (ECE, Brier);
//! latency p50/p95/p99; and the routing outcome — each classification goes through
//! `pick_tier` over fixed seeds at cold start, giving the mean tier cost and how often hard /
//! easy requests land on the cheapest tier. Accuracy is then broken down by failure-mode tag.
//!
//! The quality gates at the bottom fail the build if a retrained model or a feature change
//! makes the local classifier regress. The report prints with `-- --nocapture`, and is
//! written to `BENCH_REPORT` when that is set (CI points it at the job summary):
//!
//! ```sh
//! cargo test --release -p nasiko-llm-router --test classifier_benchmark -- --nocapture
//! LAYA_URL=http://localhost:8000 cargo test --release ... # adds the Laya column
//! STRANDS_URL=http://localhost:18099 cargo test --release ... # adds both Strands columns
//! BENCH_PREDICTIONS=/tmp/bench-predictions cargo test --release ... # saves JSONL + SHA manifest
//! ```

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nasiko_llm_router::ir::Message;
use nasiko_llm_router::routing::classifier::{CellMap, Tier, pick_tier, tier_cost};
use nasiko_llm_router::routing::laya::{LayaClassifier, probe_health};
use nasiko_llm_router::routing::local_classifier::{LocalClassifier, eval_state};
use nasiko_llm_router::routing::request_classifier::{LOW_CONFIDENCE_FLOOR, classify_input};
use nasiko_llm_router::routing::{
    Classification, ClassifierInput, ClassifierSource, RegexClassifier, RequestClassifier,
    RequestType,
};
use nasiko_llm_router::{GatewayConfig, build_request_classifier};
use rand::SeedableRng;
use rand::rngs::StdRng;
use serde_json::{Value, json};

const TYPES: [RequestType; 7] = [
    RequestType::CodeGeneration,
    RequestType::CodeUnderstanding,
    RequestType::TechnicalDesign,
    RequestType::AnalyticalReasoning,
    RequestType::Writing,
    RequestType::FactualLookup,
    RequestType::General,
];
/// Cold-start Thompson draws per request for the routing columns.
const SEEDS: u64 = 50;

struct Case {
    id: String,
    query: String,
    /// The exact classifier state the router would build for this request.
    state: String,
    gold_type: RequestType,
    gold_complexity: u8,
    tags: Vec<String>,
}

struct Scored {
    case_idx: usize,
    c: Classification,
    latency: Duration,
}

fn eval_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("eval")
        .join(name)
}

fn gold_type(label: &str) -> RequestType {
    RequestType::from_wire(label).unwrap_or_else(|| panic!("unknown request type {label}"))
}

/// An H4-schema file: `{"examples": [{id, query, context, request_type, complexity, tests}]}`.
fn load_h4(name: &str) -> Vec<Case> {
    let raw = std::fs::read_to_string(eval_path(name)).expect("read eval set");
    let data: Value = serde_json::from_str(&raw).expect("valid eval JSON");
    data["examples"]
        .as_array()
        .expect("examples")
        .iter()
        .map(|e| {
            let query = e["query"].as_str().expect("query").to_string();
            Case {
                id: e["id"].as_str().expect("id").into(),
                state: eval_state(&query, e["context"].as_str()),
                query,
                gold_type: gold_type(e["request_type"].as_str().expect("request_type")),
                gold_complexity: e["complexity"].as_u64().expect("complexity") as u8,
                tags: e["tests"]
                    .as_array()
                    .map(|t| {
                        t.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
            }
        })
        .collect()
}

/// `requests.jsonl`: chat history as messages, classified through `classify_input`.
fn load_requests() -> Vec<Case> {
    let raw = std::fs::read_to_string(eval_path("requests.jsonl")).expect("read requests");
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let r: Value = serde_json::from_str(line).expect("valid row");
            let history = r["context"].as_array().expect("context");
            let mut turns: Vec<Value> = history
                .iter()
                .map(|m| json!({ "role": m["role"], "content": m["text"] }))
                .collect();
            turns.push(json!({ "role": "user", "content": r["query"] }));
            let messages: Vec<Message> =
                serde_json::from_value(Value::Array(turns)).expect("messages");
            Case {
                id: r["id"].as_str().expect("id").into(),
                query: r["query"].as_str().expect("query").into(),
                state: classify_input(&messages).expect("a user turn"),
                gold_type: gold_type(r["type"].as_str().expect("type")),
                gold_complexity: r["complexity"].as_u64().expect("complexity") as u8,
                tags: vec![
                    if history.is_empty() {
                        "plain"
                    } else {
                        "context_dependent"
                    }
                    .into(),
                ],
            }
        })
        .collect()
}

fn assert_backend_source(name: &str, id: &str, c: &Classification, expected: ClassifierSource) {
    assert_eq!(
        c.source.fallback_reason(),
        None,
        "{name} {id}: unexpected fallback must not be scored as a model answer"
    );
    assert_eq!(
        c.source, expected,
        "{name} {id}: classifier factory selected an unexpected backend"
    );
}

async fn run(
    name: &str,
    classifier: &dyn RequestClassifier,
    expected: ClassifierSource,
    cases: &[Case],
) -> Vec<Scored> {
    // Warm-up: lazily compiled regexes / model load / connection setup stay out of latency.
    let warmup = classifier
        .classify(&ClassifierInput {
            query: "hello",
            state: "Latest request:\nhello",
        })
        .await;
    assert_backend_source(name, "warmup", &warmup, expected);
    let mut out = Vec::with_capacity(cases.len());
    for (case_idx, case) in cases.iter().enumerate() {
        let started = Instant::now();
        let c = classifier
            .classify(&ClassifierInput {
                query: &case.query,
                state: &case.state,
            })
            .await;
        assert_backend_source(name, &case.id, &c, expected);
        out.push(Scored {
            case_idx,
            c,
            latency: started.elapsed(),
        });
    }
    out
}

#[derive(Default)]
struct Metrics {
    accuracy: f64,
    macro_f1: f64,
    f1: BTreeMap<&'static str, f64>,
    cx_exact: f64,
    cx_within1: f64,
    cx_mae: f64,
    hard_recall: f64,
    ece: f64,
    brier: f64,
    p50_us: f64,
    p95_us: f64,
    p99_us: f64,
    mean_cost: f64,
    hard_to_cheapest: f64,
    easy_to_cheapest: f64,
    by_tag: BTreeMap<String, (usize, usize)>,
    fallbacks: usize,
    /// Scalar confidence <0.4, before any backend-specific admission guard (regex exempt).
    below_floor: f64,
    /// Share rejected by the full admission policy (configured model, unpinned).
    low_confidence: f64,
    /// Type accuracy on requests admitted by the full classifier policy.
    confident_accuracy: f64,
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    let i = ((q * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len()) - 1;
    sorted[i]
}

fn metrics(cases: &[Case], scored: &[Scored]) -> Metrics {
    let n = scored.len() as f64;
    let mut m = Metrics::default();
    let correct: Vec<bool> = scored
        .iter()
        .map(|s| s.c.request_type == cases[s.case_idx].gold_type)
        .collect();
    m.accuracy = correct.iter().filter(|c| **c).count() as f64 / n;

    for t in TYPES {
        let tp = scored
            .iter()
            .filter(|s| s.c.request_type == t && cases[s.case_idx].gold_type == t)
            .count() as f64;
        let predicted = scored.iter().filter(|s| s.c.request_type == t).count() as f64;
        let actual = scored
            .iter()
            .filter(|s| cases[s.case_idx].gold_type == t)
            .count() as f64;
        let (p, r) = (
            if predicted > 0.0 { tp / predicted } else { 0.0 },
            if actual > 0.0 { tp / actual } else { 0.0 },
        );
        m.f1.insert(
            t.as_str(),
            if p + r > 0.0 {
                2.0 * p * r / (p + r)
            } else {
                0.0
            },
        );
    }
    m.macro_f1 = m.f1.values().sum::<f64>() / TYPES.len() as f64;

    let diffs: Vec<i32> = scored
        .iter()
        .map(|s| i32::from(s.c.complexity) - i32::from(cases[s.case_idx].gold_complexity))
        .collect();
    m.cx_exact = diffs.iter().filter(|d| **d == 0).count() as f64 / n;
    m.cx_within1 = diffs.iter().filter(|d| d.abs() <= 1).count() as f64 / n;
    m.cx_mae = diffs.iter().map(|d| f64::from(d.abs())).sum::<f64>() / n;
    let hard: Vec<&Scored> = scored
        .iter()
        .filter(|s| cases[s.case_idx].gold_complexity >= 4)
        .collect();
    m.hard_recall = if hard.is_empty() {
        f64::NAN
    } else {
        hard.iter().filter(|s| s.c.complexity >= 4).count() as f64 / hard.len() as f64
    };

    // Calibration of the type confidence: 10-bin ECE and Brier score.
    let mut bins = [(0usize, 0.0f64, 0.0f64); 10];
    for (s, ok) in scored.iter().zip(&correct) {
        let b = ((s.c.confidence * 10.0) as usize).min(9);
        bins[b].0 += 1;
        bins[b].1 += s.c.confidence;
        bins[b].2 += f64::from(u8::from(*ok));
    }
    m.ece = bins
        .iter()
        .filter(|b| b.0 > 0)
        .map(|(k, conf, acc)| (*k as f64 / n) * ((acc - conf) / *k as f64).abs())
        .sum();
    m.brier = scored
        .iter()
        .zip(&correct)
        .map(|(s, ok)| (s.c.confidence - f64::from(u8::from(*ok))).powi(2))
        .sum::<f64>()
        / n;

    let mut lat: Vec<f64> = scored
        .iter()
        .map(|s| s.latency.as_secs_f64() * 1e6)
        .collect();
    lat.sort_by(f64::total_cmp);
    (m.p50_us, m.p95_us, m.p99_us) = (
        quantile(&lat, 0.50),
        quantile(&lat, 0.95),
        quantile(&lat, 0.99),
    );

    // Routing outcome: the router's own tier sampler at cold start.
    let cells = CellMap::new();
    let (mut cost, mut hard_cheap, mut hard_n, mut easy_cheap, mut easy_n) = (0.0, 0, 0, 0, 0);
    for s in scored {
        let gold = cases[s.case_idx].gold_complexity;
        for seed in 0..SEEDS {
            let (tier, _) = pick_tier(&s.c, &cells, &mut StdRng::seed_from_u64(seed));
            cost += tier_cost(tier);
            if gold >= 4 {
                hard_n += 1;
                hard_cheap += usize::from(tier == Tier::Tier3);
            } else if gold <= 2 {
                easy_n += 1;
                easy_cheap += usize::from(tier == Tier::Tier3);
            }
        }
    }
    m.mean_cost = cost / (n * SEEDS as f64);
    m.hard_to_cheapest = hard_cheap as f64 / hard_n.max(1) as f64;
    m.easy_to_cheapest = easy_cheap as f64 / easy_n.max(1) as f64;

    for (s, ok) in scored.iter().zip(&correct) {
        for tag in &cases[s.case_idx].tags {
            let e = m.by_tag.entry(tag.clone()).or_default();
            e.0 += 1;
            e.1 += usize::from(*ok);
        }
    }
    m.fallbacks = scored
        .iter()
        .filter(|s| s.c.source.fallback_reason().is_some())
        .count();
    m.below_floor = scored
        .iter()
        .filter(|s| s.c.type_probabilities.is_some() && s.c.confidence < LOW_CONFIDENCE_FLOOR)
        .count() as f64
        / n;
    let confident: Vec<bool> = scored
        .iter()
        .zip(&correct)
        .filter(|(s, _)| !s.c.is_low_confidence())
        .map(|(_, ok)| *ok)
        .collect();
    m.low_confidence = 1.0 - confident.len() as f64 / n;
    m.confident_accuracy =
        confident.iter().filter(|ok| **ok).count() as f64 / confident.len().max(1) as f64;
    m
}

fn pct(x: f64) -> String {
    format!("{:.1}%", 100.0 * x)
}

fn us(x: f64) -> String {
    if x < 10_000.0 {
        format!("{x:.0} µs")
    } else {
        format!("{:.0} ms", x / 1000.0)
    }
}

fn table(out: &mut String, header: &[String], rows: &[Vec<String>]) {
    let _ = writeln!(out, "| {} |", header.join(" | "));
    let _ = writeln!(out, "|{}|", vec!["---"; header.len()].join("|"));
    for r in rows {
        let _ = writeln!(out, "| {} |", r.join(" | "));
    }
    out.push('\n');
}

fn report(out: &mut String, title: &str, cases: &[Case], results: &[(&str, Vec<Scored>, Metrics)]) {
    let names: Vec<String> = results.iter().map(|(n, ..)| n.to_string()).collect();
    let header = |first: &str| {
        std::iter::once(first.to_string())
            .chain(names.iter().cloned())
            .collect::<Vec<_>>()
    };
    let row = |label: &str, f: &dyn Fn(&Metrics) -> String| {
        std::iter::once(label.to_string())
            .chain(results.iter().map(|(_, _, m)| f(m)))
            .collect::<Vec<_>>()
    };
    let _ = writeln!(out, "## {title} ({} requests)\n", cases.len());
    table(
        out,
        &header("metric"),
        &[
            row("type accuracy", &|m| pct(m.accuracy)),
            row("type macro-F1", &|m| format!("{:.3}", m.macro_f1)),
            row("scalar confidence <0.4 (regex exempt; diagnostic)", &|m| {
                pct(m.below_floor)
            }),
            row("low confidence → configured model, unpinned", &|m| {
                pct(m.low_confidence)
            }),
            row("type accuracy when confident", &|m| {
                pct(m.confident_accuracy)
            }),
            row("complexity exact", &|m| pct(m.cx_exact)),
            row("complexity within ±1", &|m| pct(m.cx_within1)),
            row("complexity MAE", &|m| format!("{:.2}", m.cx_mae)),
            row("hard (4–5) recognised as hard", &|m| pct(m.hard_recall)),
            row("confidence ECE (lower is better)", &|m| {
                format!("{:.3}", m.ece)
            }),
            row("Brier score (lower is better)", &|m| {
                format!("{:.3}", m.brier)
            }),
            row("latency p50", &|m| us(m.p50_us)),
            row("latency p95", &|m| us(m.p95_us)),
            row("latency p99", &|m| us(m.p99_us)),
            row("routing: mean tier cost", &|m| {
                format!("{:.2}", m.mean_cost)
            }),
            row("routing: hard → cheapest tier (lower is better)", &|m| {
                pct(m.hard_to_cheapest)
            }),
            row("routing: easy → cheapest tier (higher is better)", &|m| {
                pct(m.easy_to_cheapest)
            }),
            row("fell back to regex", &|m| m.fallbacks.to_string()),
        ],
    );

    let _ = writeln!(out, "### Accuracy by failure mode\n");
    let tags: Vec<&String> = results[0].2.by_tag.keys().collect();
    let mut rows: Vec<Vec<String>> = tags
        .iter()
        .map(|tag| {
            let (n, _) = results[0].2.by_tag[*tag];
            std::iter::once(format!("{tag} (n={n})"))
                .chain(results.iter().map(|(_, _, m)| {
                    let (n, ok) = m.by_tag[*tag];
                    pct(ok as f64 / n as f64)
                }))
                .collect()
        })
        .collect();
    rows.sort();
    table(out, &header("tag"), &rows);

    let _ = writeln!(out, "### F1 by request type\n");
    let rows: Vec<Vec<String>> = TYPES
        .iter()
        .map(|t| row(t.as_str(), &|m| format!("{:.2}", m.f1[t.as_str()])))
        .collect();
    table(out, &header("type"), &rows);

    // The concrete requests each backend fixes and breaks relative to regex.
    let base = &results[0].1;
    for (name, scored, _) in &results[1..] {
        let ok = |s: &Scored| s.c.request_type == cases[s.case_idx].gold_type;
        let fixed: Vec<String> = base
            .iter()
            .zip(scored)
            .filter(|(b, s)| !ok(b) && ok(s))
            .map(|(b, _)| {
                let case = &cases[b.case_idx];
                format!(
                    "`{}` {} → regex said {}",
                    case.id,
                    case.gold_type.as_str(),
                    b.c.request_type.as_str()
                )
            })
            .collect();
        let broken: Vec<String> = base
            .iter()
            .zip(scored)
            .filter(|(b, s)| ok(b) && !ok(s))
            .map(|(_, s)| {
                let case = &cases[s.case_idx];
                format!(
                    "`{}` {} → {name} said {}",
                    case.id,
                    case.gold_type.as_str(),
                    s.c.request_type.as_str()
                )
            })
            .collect();
        let _ = writeln!(
            out,
            "### {name} vs regex: {} fixed, {} broken\n\nBroken: {}\n",
            fixed.len(),
            broken.len(),
            if broken.is_empty() {
                "none".into()
            } else {
                broken.join("; ")
            }
        );
    }
}

async fn laya_backend() -> Option<LayaClassifier> {
    let url = std::env::var("LAYA_URL").ok()?;
    let http = reqwest::Client::new();
    match probe_health(&http, &url).await {
        Ok(_) => Some(LayaClassifier::new(http, &url, "", Duration::from_secs(30))),
        Err(error) => {
            eprintln!("LAYA_URL={url} not reachable ({error}); benchmarking regex and local only");
            None
        }
    }
}

/// Use the production factory for both Strands modes, with the configured endpoint,
/// authentication and deadline. A health response alone is not enough when a sidecar
/// explicitly reports that its startup warm-up is incomplete.
struct StrandsBackends {
    optimized: Arc<dyn RequestClassifier>,
    raw: Arc<dyn RequestClassifier>,
    health: Value,
    timeout_ms: u64,
}

/// SHA utilities are required only when the optional prediction export is requested.
/// Argument arrays avoid shell interpolation, including for paths containing spaces.
fn sha256(path: &Path) -> String {
    for (program, args) in [("shasum", vec!["-a", "256"]), ("sha256sum", vec![])] {
        let result = std::process::Command::new(program)
            .args(args)
            .arg(path)
            .output();
        if let Ok(result) = result
            && result.status.success()
            && let Ok(stdout) = String::from_utf8(result.stdout)
            && let Some(hash) = stdout.split_whitespace().next()
            && hash.len() == 64
            && hash.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return hash.to_ascii_lowercase();
        }
    }
    panic!(
        "BENCH_PREDICTIONS needs shasum or sha256sum to hash {}",
        path.display()
    );
}

/// Persist model outputs without changing labels, so probability/admission diagnostics
/// can be recomputed without another expensive inference pass.
fn save_predictions(
    directory: &Path,
    set: usize,
    name: &str,
    cases: &[Case],
    scored: &[Scored],
) -> PathBuf {
    use std::io::Write as _;
    let path = directory.join(format!("{set}-{name}.jsonl"));
    let mut file =
        std::io::BufWriter::new(std::fs::File::create(&path).expect("create predictions"));
    for s in scored {
        let probabilities = s.c.type_probabilities.as_ref().map(|probabilities| {
            probabilities
                .iter()
                .map(|(label, p)| (label.as_str().to_string(), json!(p)))
                .collect::<serde_json::Map<String, Value>>()
        });
        let row = json!({
            "id": cases[s.case_idx].id,
            "backend": name,
            "request_type": s.c.request_type.as_str(),
            "type_probabilities": probabilities,
            "complexity": s.c.complexity,
            "complexity_level": s.c.complexity_level,
            "complexity_confidence": s.c.complexity_confidence,
            "confidence": s.c.confidence,
            "low_confidence": s.c.is_low_confidence(),
            "source": s.c.source.as_str(),
            "fallback_reason": s.c.source.fallback_reason(),
            "latency_us": s.latency.as_micros() as u64,
        });
        writeln!(file, "{row}").expect("write predictions");
    }
    file.flush().expect("flush predictions");
    path
}

async fn strands_backends() -> Option<StrandsBackends> {
    let url = std::env::var("STRANDS_URL").ok()?;
    let http = reqwest::Client::new();
    let health = match probe_health(&http, &url).await {
        Ok(health) if health["ready"].as_bool() != Some(false) => health,
        Ok(_) => {
            eprintln!("STRANDS_URL={url} is not ready; omitting both Strands backends");
            return None;
        }
        Err(error) => {
            eprintln!("STRANDS_URL={url} not reachable ({error}); omitting both Strands backends");
            return None;
        }
    };
    let mut config = GatewayConfig::from_env();
    config.strands_url = url;
    config.request_classifier = "strands".into();
    let optimized = build_request_classifier(&config, &http);
    config.request_classifier = "strands_raw".into();
    let raw = build_request_classifier(&config, &http);
    Some(StrandsBackends {
        optimized,
        raw,
        health,
        timeout_ms: config.strands_timeout_ms,
    })
}

#[tokio::test]
async fn classifier_benchmark() {
    let local = LocalClassifier::embedded().expect("embedded local model");
    let laya = laya_backend().await;
    let strands = strands_backends().await;
    let predictions_dir = std::env::var("BENCH_PREDICTIONS").ok().map(PathBuf::from);
    if let Some(directory) = &predictions_dir {
        std::fs::create_dir_all(directory).expect("create BENCH_PREDICTIONS directory");
    }
    let mut backends: Vec<(&str, &dyn RequestClassifier, ClassifierSource)> = vec![
        ("regex", &RegexClassifier, ClassifierSource::Regex),
        ("local", &local, ClassifierSource::Local),
    ];
    if let Some(laya) = &laya {
        backends.push(("laya", laya, ClassifierSource::Laya));
    }
    if let Some(strands) = &strands {
        backends.push((
            "strands_raw",
            strands.raw.as_ref(),
            ClassifierSource::Strands,
        ));
        backends.push((
            "strands",
            strands.optimized.as_ref(),
            ClassifierSource::Strands,
        ));
    }

    let sets = [
        ("Held-out validation", load_h4("h4-validation.json")),
        ("Hackathon public sample", load_h4("h4-public-sample.json")),
        ("Labelled requests with chat history", load_requests()),
    ];
    let mut out = format!(
        "# Request-classifier benchmark\n\n{} build; routing columns are `pick_tier` over {SEEDS} \
         cold-start seeds (tier costs 15 / 3 / 0.8).\n\n",
        if cfg!(debug_assertions) {
            "Debug"
        } else {
            "Release"
        }
    );
    out.push_str(
        "Routing columns simulate the tier sampler for every classification, including \
         low-confidence cases. The configured-model abstention rate is reported separately; \
         configured-model cost and downstream answer quality are not measured here.\n\n",
    );
    if let Some(strands) = &strands {
        let health = &strands.health;
        let _ = writeln!(
            out,
            "Strands endpoint model: `{}`; device: `{}`; ready: `{}`. \
             Both backends use the runtime factory with a {} ms timeout. \
             `strands_raw` asks both questions and reports native concentration confidence. \
             `strands` asks only type, reports selected-label probability using the \
             checked-in temperature asset (identity when training-only sharpening is \
             rejected), and takes complexity from the embedded local model. \
             Confidence semantics differ, so their ECE/Brier values are not a comparison \
             of two probabilities with the same meaning.\n",
            health["model"]
                .as_str()
                .or_else(|| health["checkpoint"].as_str())
                .unwrap_or("unspecified"),
            health["device"].as_str().unwrap_or("unspecified"),
            health["ready"],
            strands.timeout_ms,
        );
    }
    let mut all = Vec::new();
    let mut prediction_files = Vec::new();
    for (set, (title, cases)) in sets.iter().enumerate() {
        let mut results = Vec::new();
        for (name, backend, source) in &backends {
            let scored = run(name, *backend, *source, cases).await;
            let m = metrics(cases, &scored);
            if let Some(directory) = &predictions_dir {
                let path = save_predictions(directory, set, name, cases, &scored);
                prediction_files.push(json!({
                    "set": title,
                    "backend": name,
                    "file": path.file_name().expect("prediction filename").to_string_lossy(),
                    "sha256": sha256(&path),
                    "cases": scored.len(),
                }));
            }
            eprintln!(
                "{title}: {name} {} cases; accuracy {}; p95 {}; no inference fallbacks",
                scored.len(),
                pct(m.accuracy),
                us(m.p95_us),
            );
            results.push((*name, scored, m));
        }
        report(&mut out, title, cases, &results);
        all.push(results);
    }
    if let Some(directory) = &predictions_dir {
        let calibration = directory.join("strands-calibration.json");
        std::fs::write(
            &calibration,
            include_str!("../assets/strands_calibration.json"),
        )
        .expect("write compiled calibration asset");
        let binary = std::env::current_exe().expect("benchmark binary path");
        let mut fixtures = serde_json::Map::new();
        for name in [
            "h4-validation.json",
            "h4-public-sample.json",
            "requests.jsonl",
        ] {
            fixtures.insert(name.into(), json!(sha256(&eval_path(name))));
        }
        let source =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/classifier_benchmark.rs");
        let manifest = json!({
            "schema": "classifier-benchmark-predictions@v1",
            "routing_seeds": SEEDS,
            "tier_cost_units": [15.0, 3.0, 0.8],
            "routing_scope": "cold-start pick_tier simulation on every classification; configured-model abstention measured separately",
            "binary": {"path": binary, "sha256": sha256(&binary)},
            "benchmark_source_sha256": sha256(&source),
            "fixtures_sha256": fixtures,
            "compiled_calibration_sha256": sha256(&calibration),
            "strands_timeout_ms": strands.as_ref().map(|s| s.timeout_ms),
            "strands_health": strands.as_ref().map(|s| &s.health),
            "prediction_files": prediction_files,
        });
        std::fs::write(
            directory.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).expect("serialize prediction manifest"),
        )
        .expect("write prediction manifest");
    }
    println!("{out}");
    if let Ok(path) = std::env::var("BENCH_REPORT") {
        std::fs::write(&path, &out).expect("write BENCH_REPORT");
    }

    // ── Quality gates: the local model must keep beating regex where routing needs it. ──
    let get = |set: usize, name: &str| -> &Metrics {
        &all[set]
            .iter()
            .find(|(n, ..)| *n == name)
            .expect("backend")
            .2
    };
    let (regex, local) = (get(0, "regex"), get(0, "local"));
    assert!(
        local.accuracy >= 0.84,
        "local accuracy {:.3}",
        local.accuracy
    );
    assert!(
        local.accuracy >= regex.accuracy + 0.50,
        "local {:.3} must beat regex {:.3} by 50 pts",
        local.accuracy,
        regex.accuracy
    );
    assert!(
        local.macro_f1 >= 0.84,
        "local macro-F1 {:.3}",
        local.macro_f1
    );
    assert!(
        local.cx_within1 >= 0.90,
        "local complexity ±1 {:.3}",
        local.cx_within1
    );
    assert!(
        local.hard_recall >= 0.75,
        "local hard recall {:.3}",
        local.hard_recall
    );
    assert!(local.ece <= 0.10, "local ECE {:.3}", local.ece);
    assert!(
        local.hard_to_cheapest < regex.hard_to_cheapest,
        "local must send fewer hard requests to the cheapest tier than regex"
    );
    assert!(
        local.low_confidence <= 0.05,
        "local low-confidence rate {:.3}",
        local.low_confidence
    );
    assert!(
        get(2, "local").accuracy >= 0.90,
        "chat-history accuracy regressed"
    );
    for (tag, (n, ok)) in &local.by_tag {
        let (_, regex_ok) = regex.by_tag[tag];
        assert!(
            *ok >= regex_ok,
            "local is worse than regex on {tag} ({ok} vs {regex_ok} of {n})"
        );
    }
    for (set, (title, _)) in sets.iter().enumerate().skip(1) {
        assert!(
            get(set, "local").accuracy > get(set, "regex").accuracy,
            "local must beat regex on {title}"
        );
    }
    let p99_budget_us = if cfg!(debug_assertions) {
        20_000.0
    } else {
        2_000.0
    };
    assert!(
        local.p99_us <= p99_budget_us,
        "local p99 {:.0} µs over the {p99_budget_us} µs budget",
        local.p99_us
    );
}
