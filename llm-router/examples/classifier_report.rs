//! Routing simulation for the request classifier: what tier mix would the router pick?
//!
//! Run:
//!   EVAL_SET=llm-router/training/request_classifier/data/eval_test.json \
//!   cargo run --release -p nasiko-llm-router --example classifier_report
//!
//! For every case it classifies with the configured backend (same factory as the router;
//! `CLASSIFIER_BACKEND`, default `local`) and the regex, then runs the router's real tier
//! selector (`routing::select_tier`, empty learned cells = cold start) over `SEEDS` seeded
//! RNGs (default 200), with complexity-aware routing off and on. It prints, per system:
//!
//! - the tier distribution (variance across seeds, aggregated over cases),
//! - the share of gold complexity ≥ 4 cases sent to Tier 3 and of complexity ≤ 2 cases sent to
//!   Tier 1,
//! - a relative cost proxy using the bandit's tier cost gradient (Tier1 15, Tier2 3, Tier3 0.8),
//! - a determinism check (the same seed always yields the same tier).
//!
//! This is a **cost proxy, not answer quality**: it says nothing about whether the cheaper tier
//! answers well. Claims of cheaper or better routing need a downstream quality study.
use std::collections::HashMap;

use nasiko_llm_router::routing::classifier::CellMap;
use nasiko_llm_router::routing::{
    Classification, ClassifyInput, ComplexityRouting, RegexClassifier, RequestClassifier, Tier,
    select_tier,
};
use nasiko_llm_router::{GatewayConfig, build_request_classifier};
use rand::SeedableRng;
use rand::rngs::StdRng;

const TIER_COST: [f64; 3] = [15.0, 3.0, 0.8];

fn idx(t: Tier) -> usize {
    match t {
        Tier::Tier1 => 0,
        Tier::Tier2 => 1,
        Tier::Tier3 => 2,
    }
}

struct Sim {
    counts: [u64; 3],
    hard_t3: (u64, u64),
    easy_t1: (u64, u64),
    cost: f64,
    deterministic: bool,
}

fn simulate(cases: &[(Classification, u8)], cx: ComplexityRouting, seeds: u64) -> Sim {
    let cells: CellMap = HashMap::new();
    let mut sim = Sim {
        counts: [0; 3],
        hard_t3: (0, 0),
        easy_t1: (0, 0),
        cost: 0.0,
        deterministic: true,
    };
    for (n, (c, gold_cx)) in cases.iter().enumerate() {
        for seed in 0..seeds {
            let s = seed.wrapping_mul(1_000_003).wrapping_add(n as u64);
            let tier = select_tier(&cells, c, cx, &mut StdRng::seed_from_u64(s));
            if seed == 0 && select_tier(&cells, c, cx, &mut StdRng::seed_from_u64(s)) != tier {
                sim.deterministic = false;
            }
            let i = idx(tier);
            sim.counts[i] += 1;
            sim.cost += TIER_COST[i];
            if *gold_cx >= 4 {
                sim.hard_t3.1 += 1;
                sim.hard_t3.0 += u64::from(i == 2);
            }
            if *gold_cx <= 2 {
                sim.easy_t1.1 += 1;
                sim.easy_t1.0 += u64::from(i == 0);
            }
        }
    }
    sim
}

fn pct(a: u64, b: u64) -> f64 {
    if b == 0 {
        0.0
    } else {
        100.0 * a as f64 / b as f64
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to an eval JSON path");
    let seeds: u64 = std::env::var("SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read EVAL_SET"))
            .expect("valid eval JSON");
    let examples = data["examples"].as_array().expect("examples array");

    let mut cfg = GatewayConfig::from_env();
    if std::env::var("CLASSIFIER_BACKEND").map_or(true, |v| v.trim().is_empty()) {
        cfg.classifier.backend = "local".into();
    }
    let model = build_request_classifier(&cfg.classifier, &reqwest::Client::new());
    let mut model_cases = Vec::new();
    let mut regex_cases = Vec::new();
    for e in examples {
        let query = e["query"].as_str().expect("query");
        let context = e["context"].as_str();
        let gold_cx = e["complexity"].as_u64().unwrap_or(3) as u8;
        let c = model
            .classify(&ClassifyInput { query, context })
            .await
            .unwrap_or_else(|_| RegexClassifier::classify_sync(query));
        model_cases.push((c, gold_cx));
        regex_cases.push((RegexClassifier::classify_sync(query), gold_cx));
    }

    let on = ComplexityRouting {
        enabled: true,
        guard_confidence: cfg.classifier.complexity_guard_confidence,
    };
    println!(
        "# Routing simulation ({} cases × {seeds} seeds, cold start; cost proxy, not answer quality)\n",
        examples.len()
    );
    println!(
        "| system | T1 / T2 / T3 share | gold cx≥4 → T3 | gold cx≤2 → T1 | relative cost | deterministic |"
    );
    println!("|---|---|---|---|---|---|");
    let baseline = simulate(&regex_cases, ComplexityRouting::OFF, seeds).cost;
    for (name, cases, cx) in [
        ("regex (today)", &regex_cases, ComplexityRouting::OFF),
        (
            "model, complexity routing off",
            &model_cases,
            ComplexityRouting::OFF,
        ),
        ("model, complexity routing on", &model_cases, on),
    ] {
        let s = simulate(cases, cx, seeds);
        let total: u64 = s.counts.iter().sum();
        println!(
            "| {name} | {:.1}% / {:.1}% / {:.1}% | {:.1}% | {:.1}% | {:.3} | {} |",
            pct(s.counts[0], total),
            pct(s.counts[1], total),
            pct(s.counts[2], total),
            pct(s.hard_t3.0, s.hard_t3.1),
            pct(s.easy_t1.0, s.easy_t1.1),
            s.cost / baseline,
            if s.deterministic { "yes" } else { "NO" }
        );
    }
    eprintln!(
        "classifier_report: backend={} (relative cost = model / regex-today)",
        model.name()
    );
}
