//! Train the local request classifier and report held-out metrics against the regex baseline.
//!
//! ```sh
//! cargo run --release -p nasiko-llm-router --example classifier_train
//! ```
//!
//! | env         | default                                    |
//! |-------------|--------------------------------------------|
//! | `DATA`      | `llm-router/assets/classifier/labelled.jsonl` |
//! | `OUT_MODEL` | `llm-router/assets/classifier/linear-v1.json` |
//!
//! Deterministic: the split, the SGD order and the output are fixed, so re-running reproduces the
//! committed model byte for byte. The model is trained on the train split only; every number
//! printed for `val` comes from examples (and their near-duplicates) the model never saw.

use std::collections::BTreeMap;

use nasiko_llm_router::routing::classify_request_type;
use nasiko_llm_router::routing::linear_classifier::{
    Example, LABELS, LinearModel, TrainConfig, jaccard, split, token_set, train,
};

const NEAR_DUP: f64 = 0.6;

fn main() {
    let root = env!("CARGO_MANIFEST_DIR");
    let data = std::env::var("DATA")
        .unwrap_or_else(|_| format!("{root}/assets/classifier/labelled.jsonl"));
    let out = std::env::var("OUT_MODEL")
        .unwrap_or_else(|_| format!("{root}/assets/classifier/linear-v1.json"));

    let examples: Vec<Example> = std::fs::read_to_string(&data)
        .expect("read DATA")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("valid JSONL"))
        .collect();
    let (tr, val) = split(&examples, NEAR_DUP);

    let leak = val
        .iter()
        .flat_map(|v| {
            tr.iter()
                .map(move |t| jaccard(&token_set(&v.query), &token_set(&t.query)))
        })
        .fold(0.0, f64::max);
    assert!(
        leak < NEAR_DUP,
        "near-duplicate leaked across splits: {leak}"
    );
    println!(
        "examples {} → train {} / val {} (max cross-split query Jaccard {leak:.2} < {NEAR_DUP})",
        examples.len(),
        tr.len(),
        val.len()
    );

    let model = train(&tr, &TrainConfig::default()).expect("train");
    std::fs::write(
        &out,
        serde_json::to_string(&model).expect("serialize") + "\n",
    )
    .expect("write model");
    println!(
        "model → {out} ({} features, T={}, Tc={})",
        model.features.len(),
        model.temperature,
        model.complexity_temperature
    );

    report("train", &model, &tr);
    report("val", &model, &val);
}

fn report(name: &str, model: &LinearModel, set: &[Example]) {
    let n = set.len() as f64;
    let (mut correct, mut regex_correct, mut cx_exact, mut cx_within1, mut cx_abs) =
        (0, 0, 0, 0, 0u32);
    let mut bins = [(0usize, 0.0f64, 0usize); 10]; // (count, sum confidence, correct)
    let mut per_class: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let mut confident: Vec<(f32, bool)> = Vec::new();
    for e in set {
        let ctx = (!e.context.is_empty()).then_some(e.context.as_str());
        let p = model.predict(&e.query, ctx);
        let ok = p.request_type.as_str() == e.request_type;
        correct += ok as usize;
        regex_correct += (classify_request_type(&e.query).as_str() == e.request_type) as usize;
        let d = (p.complexity as i32 - e.complexity as i32).unsigned_abs();
        cx_exact += (d == 0) as usize;
        cx_within1 += (d <= 1) as usize;
        cx_abs += d;
        let b = ((p.confidence * 10.0) as usize).min(9);
        bins[b].0 += 1;
        bins[b].1 += p.confidence as f64;
        bins[b].2 += ok as usize;
        let entry = per_class
            .entry(
                LABELS
                    .iter()
                    .find(|l| l.as_str() == e.request_type)
                    .unwrap()
                    .as_str(),
            )
            .or_default();
        entry.0 += 1;
        entry.1 += ok as usize;
        confident.push((p.confidence, ok));
    }
    let ece: f64 = bins
        .iter()
        .filter(|b| b.0 > 0)
        .map(|(c, s, k)| (*c as f64 / n) * (s / *c as f64 - *k as f64 / *c as f64).abs())
        .sum();
    println!("\n[{name}] n={}", set.len());
    println!(
        "  request_type accuracy: local {:.1}%  vs regex {:.1}%",
        100.0 * correct as f64 / n,
        100.0 * regex_correct as f64 / n
    );
    println!("  ECE (10 bins): {ece:.3}");
    println!(
        "  complexity: exact {:.1}%, within ±1 {:.1}%, MAE {:.2}",
        100.0 * cx_exact as f64 / n,
        100.0 * cx_within1 as f64 / n,
        cx_abs as f64 / n
    );
    for (label, (total, ok)) in &per_class {
        println!("    {label:<22} {ok}/{total}");
    }
    for t in [0.3f32, 0.4, 0.5, 0.6] {
        let (above, below): (Vec<_>, Vec<_>) = confident.iter().partition(|(c, _)| *c >= t);
        let acc = |v: &[&(f32, bool)]| {
            if v.is_empty() {
                f64::NAN
            } else {
                100.0 * v.iter().filter(|x| x.1).count() as f64 / v.len() as f64
            }
        };
        println!(
            "  min_confidence {t:.1}: {:>3} kept ({:.1}% correct), {:>3} to regex fallback ({:.1}% of those correct before fallback)",
            above.len(),
            acc(&above),
            below.len(),
            acc(&below)
        );
    }
}
