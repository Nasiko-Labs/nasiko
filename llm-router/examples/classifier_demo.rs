//! Before/after demo of the Level 3 request classifier: today's regex vs. the in-process
//! local model (and Laya, when a `laya-serve` is up), through the router's own code — what
//! each classifier says, and which tier the router then picks.
//!
//! ```sh
//! LAYA_MODELS=multilingual laya-serve &
//! cargo run -p nasiko-llm-router --example classifier_demo                 # built-in script
//! cargo run -p nasiko-llm-router --example classifier_demo -- "your query"  # your own
//! ```
//!
//! Four acts: (1) the same requests through every classifier, with the tier mix the router
//! samples over 50 seeds; (2) the failure modes that break regex — keyword traps, no
//! keywords, other languages, typos; (3) a follow-up that only makes sense with
//! conversation context; (4) Laya unreachable — the request is still routed, with the reason
//! logged. Numbers over whole datasets: `cargo test --test classifier_benchmark`.
//! Env: `LAYA_URL` (default `http://localhost:8000`); Laya rows are skipped when it is down.

use std::time::{Duration, Instant};

use nasiko_llm_router::ir::Message;
use nasiko_llm_router::routing::classifier::{CellMap, Tier, pick_tier, tier_cost};
use nasiko_llm_router::routing::laya::LayaClassifier;
use nasiko_llm_router::routing::local_classifier::LocalClassifier;
use nasiko_llm_router::routing::request_classifier::classify_input;
use nasiko_llm_router::routing::{
    Classification, ClassifierInput, RegexClassifier, RequestClassifier,
};
use rand::SeedableRng;
use rand::rngs::StdRng;
use serde_json::json;

const SEEDS: u64 = 50;

const SCRIPT: [&str; 6] = [
    "hi!",
    "What is the capital of France?",
    "Implement an LRU cache in Rust with O(1) get and put, generic over key and value types.",
    "Design a sharded, lock-free hash map in C++ with linearizable resize.",
    "How would you architect a real-time collaborative text editor like Google Docs?",
    "mujhe ek Python script likh ke do jo CSV file padhe aur har column ka average nikale",
];

/// One request per failure mode the regex vote-counter has: a keyword that means something
/// else, no keyword at all, a language it has no patterns for, and informal spelling.
const FAILURE_MODES: [(&str, &str); 6] = [
    (
        "keyword trap",
        "Refactor my resume so the summary reads stronger.",
    ),
    (
        "keyword trap",
        "Write a short story about a robot debugging its own feelings.",
    ),
    (
        "no keyword",
        "Why does my tomato plant keep dropping its flowers?",
    ),
    (
        "multilingual",
        "explica la diferencia entre un proceso y un hilo",
    ),
    (
        "multilingual",
        "ek email likho manager ko ki main kal chutti le raha hoon",
    ),
    (
        "typo / informal",
        "y does my pythn loop nvr end lol, its just while x>0: print(x)",
    ),
];

fn messages(turns: &[(&str, &str)]) -> Vec<Message> {
    turns
        .iter()
        .map(|(role, text)| {
            serde_json::from_value(json!({ "role": role, "content": text })).expect("message")
        })
        .collect()
}

/// Share of each tier over `SEEDS` cold-start Thompson draws, plus the mean tier cost.
fn tier_mix(c: &Classification) -> ([usize; 3], f64) {
    let cells = CellMap::new();
    let mut mix = [0usize; 3];
    let mut cost = 0.0;
    for seed in 0..SEEDS {
        let (tier, _) = pick_tier(c, &cells, &mut StdRng::seed_from_u64(seed));
        mix[match tier {
            Tier::Tier1 => 0,
            Tier::Tier2 => 1,
            Tier::Tier3 => 2,
        }] += 1;
        cost += tier_cost(tier);
    }
    (mix, cost / SEEDS as f64)
}

fn bar(mix: [usize; 3]) -> String {
    let pct = |n: usize| n * 100 / SEEDS as usize;
    format!(
        "T1 {:>3}% {:<10} T3 {:>3}%",
        pct(mix[0]),
        "█".repeat(mix[0] / 5),
        pct(mix[2])
    )
}

fn line(name: &str, c: &Classification, ms: f64) {
    let (mix, cost) = tier_mix(c);
    let extra = match c.source.fallback_reason() {
        Some(reason) => format!("  ← fallback: {reason}"),
        None => String::new(),
    };
    println!(
        "    {name:<6} {:<21} complexity {} (conf {:.2})  type conf {:.2}  | {}  cost {:>5.2}  [{:>6.0} ms]{extra}",
        c.request_type.as_str(),
        c.complexity,
        c.complexity_confidence,
        c.confidence,
        bar(mix),
        cost,
        ms,
    );
}

async fn compare(local: &LocalClassifier, laya: Option<&LayaClassifier>, turns: &[(&str, &str)]) {
    let msgs = messages(turns);
    let state = classify_input(&msgs).expect("a user turn");
    let query = turns.last().expect("turns").1;
    let input = ClassifierInput {
        query,
        state: &state,
    };
    for (role, text) in &turns[..turns.len() - 1] {
        println!("    [{role}] {text}");
    }
    println!("  ▶ {query}");
    let t = Instant::now();
    let before = RegexClassifier.classify(&input).await;
    line("regex", &before, t.elapsed().as_secs_f64() * 1e3);
    let t = Instant::now();
    let after = local.classify(&input).await;
    line("local", &after, t.elapsed().as_secs_f64() * 1e3);
    if let Some(laya) = laya {
        let t = Instant::now();
        let after = laya.classify(&input).await;
        line("laya", &after, t.elapsed().as_secs_f64() * 1e3);
    }
    println!();
}

fn heading(title: &str) {
    println!(
        "\n━━ {title} {}\n",
        "━".repeat(70usize.saturating_sub(title.len()))
    );
}

#[tokio::main]
async fn main() {
    let url = std::env::var("LAYA_URL").unwrap_or_else(|_| "http://localhost:8000".into());
    let http = reqwest::Client::new();
    let local = LocalClassifier::embedded().expect("embedded local model");
    let laya = LayaClassifier::new(http.clone(), &url, "", Duration::from_secs(30));
    // Warm-up: the first call pays model load. A fallback means no sidecar: skip Laya rows.
    let warm = laya
        .classify(&ClassifierInput {
            query: "hello",
            state: "Latest request:\nhello",
        })
        .await;
    let laya_up = warm.source.fallback_reason().is_none();
    if !laya_up {
        println!("(laya-serve not reachable at {url}: showing regex and local only)");
    }
    let laya_opt = laya_up.then_some(&laya);

    let custom: Vec<String> = std::env::args().skip(1).collect();
    if !custom.is_empty() {
        heading("Your requests");
        for q in &custom {
            compare(&local, laya_opt, &[("user", q)]).await;
        }
        return;
    }

    println!(
        "Nasiko LLM router — Level 3 request classifier, before (regex) vs after (local, Laya)"
    );
    println!("Tier mix = what the router's Thompson sampler picks over {SEEDS} seeds, cold start.");
    println!("Tier costs 15 / 3 / 0.8 (Tier1 strongest).");

    heading("1. Same request, every classifier");
    for q in SCRIPT {
        compare(&local, laya_opt, &[("user", q)]).await;
    }

    heading("2. Where regex fails: keyword traps, no keywords, languages, typos");
    for (mode, q) in FAILURE_MODES {
        println!("  [{mode}]");
        compare(&local, laya_opt, &[("user", q)]).await;
    }

    heading("3. Context: a follow-up the regex cannot read");
    compare(
        &local,
        laya_opt,
        &[
            ("user", "can you draft a toast for my sister's wedding"),
            (
                "assistant",
                "Of course! Here's a draft: Friends and family, tonight we celebrate...",
            ),
            ("user", "make it funnier and shorter"),
        ],
    )
    .await;

    heading("4. Laya down: the request is still routed");
    let dead = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let dead_url = format!("http://{}", dead.local_addr().expect("addr"));
    drop(dead);
    let down = LayaClassifier::new(http, &dead_url, "", Duration::from_millis(300));
    compare(&local, Some(&down), &[("user", SCRIPT[3])]).await;
    println!("  Laya unreachable → regex answer, no prior shift, request never fails.");
}
