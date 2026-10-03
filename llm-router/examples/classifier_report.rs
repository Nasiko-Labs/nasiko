//! Offline evaluation of the Level 3 request classifiers: regex vs. Laya.
//!
//! ```sh
//! laya-serve &   # or: docker compose --profile laya up -d laya
//! cargo run -p nasiko-llm-router --example classifier_report -- [dev|test|all]
//! ```
//!
//! Reads `llm-router/eval/requests.jsonl` (`{id, split, query, context, type, complexity}`),
//! classifies every row with both backends through the router's own code
//! ([`classify_input`] → [`RequestClassifier`]), then replays routing: each classification
//! is fed to [`pick_tier`] under 200 fixed seeds with no learned cells — the cold-start
//! situation where the classifier decides alone. Prints a markdown report and writes it to
//! `llm-router/eval/report-<split>.md`.
//!
//! Env: `LAYA_URL` (default `http://localhost:8000`), `LAYA_TIMEOUT_MS` (default 30000 —
//! offline CPU runs are slower than the production budget; the fallback path is covered by
//! unit tests, not by this report).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use nasiko_llm_router::ir::Message;
use nasiko_llm_router::routing::classifier::{
    COMPLEXITY_PRIOR_WEIGHT, Cell, CellMap, RequestType, Tier, pick_tier, tier_cost, tier_prior,
    update_cell,
};
use nasiko_llm_router::routing::laya::LayaClassifier;
use nasiko_llm_router::routing::request_classifier::classify_input;
use nasiko_llm_router::routing::{
    Classification, ClassifierInput, ClassifierSource, RegexClassifier, RequestClassifier,
};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::Deserialize;
use serde_json::json;

const SEEDS: u64 = 200;

#[derive(Deserialize)]
struct Turn {
    role: String,
    text: String,
}

#[derive(Deserialize)]
struct Row {
    id: String,
    split: String,
    query: String,
    context: Vec<Turn>,
    #[serde(rename = "type")]
    label: String,
    complexity: u8,
}

struct Scored {
    classification: Classification,
    latency_ms: f64,
}

fn messages(row: &Row) -> Vec<Message> {
    row.context
        .iter()
        .map(|t| (t.role.as_str(), t.text.as_str()))
        .chain(std::iter::once(("user", row.query.as_str())))
        .map(|(role, text)| {
            serde_json::from_value(json!({ "role": role, "content": text })).expect("message")
        })
        .collect()
}

/// Labelled complexity band: what a well-routed request in it should mostly get.
fn band(complexity: u8) -> &'static str {
    match complexity {
        1 | 2 => "low (1-2)",
        3 => "mid (3)",
        _ => "high (4-5)",
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

#[derive(Default)]
struct Replay {
    picks: BTreeMap<&'static str, [usize; 3]>,
    cost: f64,
    total: usize,
    under: usize,
    under_of: usize,
    over: usize,
    over_of: usize,
}

fn tier_idx(t: Tier) -> usize {
    match t {
        Tier::Tier1 => 0,
        Tier::Tier2 => 1,
        Tier::Tier3 => 2,
    }
}

fn replay(rows: &[&Row], scored: &[Scored]) -> Replay {
    let cells = CellMap::new();
    let mut r = Replay::default();
    for (row, s) in rows.iter().zip(scored) {
        let b = band(row.complexity);
        for seed in 0..SEEDS {
            let (tier, _) = pick_tier(&s.classification, &cells, &mut StdRng::seed_from_u64(seed));
            r.picks.entry(b).or_default()[tier_idx(tier)] += 1;
            r.cost += tier_cost(tier);
            r.total += 1;
            if row.complexity >= 4 {
                r.under_of += 1;
                r.under += usize::from(tier == Tier::Tier3);
            }
            if row.complexity <= 2 {
                r.over_of += 1;
                r.over += usize::from(tier == Tier::Tier1);
            }
        }
    }
    r
}

/// A Laya classification as a variant of the decision rule would see it: `argmax` drops the
/// type distribution (decide on the top label only); `k` rescales the prior shift to weight
/// `k` instead of the shipped `COMPLEXITY_PRIOR_WEIGHT` (the shift is linear in
/// `complexity_confidence * K`, so scaling the confidence is exact).
fn variant(c: &Classification, argmax: bool, k: f64) -> Classification {
    Classification {
        type_probabilities: if argmax {
            None
        } else {
            c.type_probabilities.clone()
        },
        complexity_confidence: c.complexity_confidence * k / COMPLEXITY_PRIOR_WEIGHT,
        ..c.clone()
    }
}

fn high_tier1(r: &Replay) -> String {
    match r.picks.get("high (4-5)") {
        Some([t1, t2, t3]) => pct(*t1, t1 + t2 + t3),
        None => "-".into(),
    }
}

// ---------------------------------------------------------------------------------------
// Stage 3 — online learning simulation
// ---------------------------------------------------------------------------------------

/// Requests in one simulated stream, and independent streams averaged.
const SIM_ROUNDS: usize = 3_000;
const SIM_RUNS: u64 = 20;
/// Share of turns whose next user message carries a feedback signal (`classifier::signal`
/// is deliberately conservative, so most turns teach nothing).
const FEEDBACK_RATE: f64 = 0.3;
/// Report windows over the stream: cold start, warming up, warm.
const WINDOWS: [(usize, usize); 3] = [(0, 300), (300, 1_500), (1_500, 3_000)];

/// SYNTHETIC reward model: probability that a tier's answer satisfies a request of a given
/// labelled complexity. Tier1 handles everything; weaker tiers degrade with difficulty.
/// This is an assumption for the simulation, not a measurement.
fn p_success(tier: Tier, complexity: u8) -> f64 {
    let c = usize::from(complexity.clamp(1, 5)) - 1;
    match tier {
        Tier::Tier1 => [0.95, 0.95, 0.95, 0.95, 0.95][c],
        Tier::Tier2 => [0.92, 0.90, 0.80, 0.50, 0.30][c],
        Tier::Tier3 => [0.90, 0.75, 0.45, 0.20, 0.10][c],
    }
}

/// How the learned cells are keyed: as in production `(tier, request_type)`, or additionally
/// by the classifier's *predicted* complexity band (one `CellMap` per band).
#[derive(Clone, Copy)]
enum CellKey {
    Type,
    TypeAndPredictedBand,
}

fn predicted_band(c: &Classification) -> usize {
    match c.complexity {
        0..=2 => 0,
        3 => 1,
        _ => 2,
    }
}

#[derive(Default, Clone, Copy)]
struct Window {
    n: usize,
    success: usize,
    cost: f64,
    hard: usize,
    hard_t3: usize,
}

/// Stream requests drawn uniformly from `rows` through `pick_tier`, with real learning:
/// the outcome is drawn from [`p_success`] for the request's *labelled* complexity, and
/// (with probability [`FEEDBACK_RATE`]) folded into the cell of the tier and the type the
/// decision was made under — exactly what the router's cache + `CellStore::observe` do.
fn simulate(rows: &[&Row], scored: &[Scored], key: CellKey) -> [Window; 3] {
    let mut windows = [Window::default(); 3];
    for run in 0..SIM_RUNS {
        let mut rng = StdRng::seed_from_u64(10_000 + run);
        let mut cells: [CellMap; 3] = Default::default();
        for t in 0..SIM_ROUNDS {
            let i = rng.random_range(0..rows.len());
            let (row, c) = (rows[i], &scored[i].classification);
            let map = match key {
                CellKey::Type => 0,
                CellKey::TypeAndPredictedBand => predicted_band(c),
            };
            let (tier, rt) = pick_tier(c, &cells[map], &mut rng);
            let ok = rng.random::<f64>() < p_success(tier, row.complexity);
            if rng.random::<f64>() < FEEDBACK_RATE {
                let cell = cells[map].entry((tier, rt)).or_insert(Cell {
                    quality_mean: tier_prior(tier, rt),
                    samples: 0,
                });
                *cell = update_cell(*cell, if ok { 1.0 } else { 0.0 });
            }
            if let Some(w) = WINDOWS.iter().position(|(a, b)| (*a..*b).contains(&t)) {
                let w = &mut windows[w];
                w.n += 1;
                w.success += usize::from(ok);
                w.cost += tier_cost(tier);
                if row.complexity >= 4 {
                    w.hard += 1;
                    w.hard_t3 += usize::from(tier == Tier::Tier3);
                }
            }
        }
    }
    windows
}

fn learning(out: &mut String, rows: &[&Row], regex: &[Scored], layas: &[Scored]) {
    let _ = writeln!(
        out,
        "\n## Stage 3 — online learning simulation (Thompson sampling + learned cells)\n"
    );
    let _ = writeln!(
        out,
        "{SIM_RUNS} streams × {SIM_ROUNDS} requests drawn from this split; feedback on \
         {:.0}% of turns, credited to the (tier, decision type) cell as the router does. \
         **The reward model is synthetic** (P(success | tier, labelled complexity): Tier1 0.95 \
         flat; Tier2 0.92→0.30; Tier3 0.90→0.10 from complexity 1→5) — it tests the \
         *mechanism*, not real answer quality.\n",
        FEEDBACK_RATE * 100.0
    );
    let _ = writeln!(
        out,
        "| variant | requests | success rate | mean tier cost | hard → Tier3 |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|");
    let rescored = |f: &dyn Fn(&Classification) -> Classification| -> Vec<Scored> {
        layas
            .iter()
            .map(|s| Scored {
                classification: f(&s.classification),
                latency_ms: s.latency_ms,
            })
            .collect()
    };
    let argmax = rescored(&|c| Classification {
        type_probabilities: None,
        ..c.clone()
    });
    // Type ignored entirely: the bandit learns per predicted complexity band only.
    let band_only = rescored(&|c| Classification {
        request_type: RequestType::General,
        type_probabilities: None,
        ..c.clone()
    });
    let variants: [(&str, &[Scored], CellKey); 5] = [
        ("regex, cells by type (main)", regex, CellKey::Type),
        ("laya sampled type, cells by type", layas, CellKey::Type),
        ("laya argmax type, cells by type", &argmax, CellKey::Type),
        (
            "laya sampled type, cells by type + predicted band",
            layas,
            CellKey::TypeAndPredictedBand,
        ),
        (
            "laya, cells by predicted band only",
            &band_only,
            CellKey::TypeAndPredictedBand,
        ),
    ];
    for (name, scored, key) in variants {
        for ((a, b), w) in WINDOWS.iter().zip(simulate(rows, scored, key)) {
            let _ = writeln!(
                out,
                "| {name} | {a}–{b} | {} | {:.2} | {} |",
                pct(w.success, w.n),
                w.cost / w.n as f64,
                pct(w.hard_t3, w.hard)
            );
        }
    }
}

fn pct(n: usize, of: usize) -> String {
    if of == 0 {
        "-".into()
    } else {
        format!("{:.1}%", 100.0 * n as f64 / of as f64)
    }
}

fn summary(out: &mut String, name: &str, rows: &[&Row], scored: &[Scored]) {
    let n = rows.len();
    let correct = |(r, s): (&&Row, &Scored)| s.classification.request_type.as_str() == r.label;
    let acc = rows.iter().zip(scored).filter(|p| correct(*p)).count();
    let err: Vec<f64> = rows
        .iter()
        .zip(scored)
        .map(|(r, s)| (s.classification.complexity as f64 - r.complexity as f64).abs())
        .collect();
    let mae = err.iter().sum::<f64>() / n as f64;
    let within1 = err.iter().filter(|e| **e <= 1.0).count();
    let fallbacks = scored
        .iter()
        .filter(|s| matches!(s.classification.source, ClassifierSource::Fallback(_)))
        .count();
    let mut lat: Vec<f64> = scored.iter().map(|s| s.latency_ms).collect();
    lat.sort_by(f64::total_cmp);
    let _ = writeln!(
        out,
        "| {name} | {} ({acc}/{n}) | {mae:.2} | {} | {fallbacks} | {:.1} | {:.1} |",
        pct(acc, n),
        pct(within1, n),
        percentile(&lat, 0.5),
        percentile(&lat, 0.95),
    );
}

fn calibration(out: &mut String, name: &str, rows: &[&Row], scored: &[Scored]) {
    for (lo, hi, label) in [
        (0.0, 0.5, "< 0.5"),
        (0.5, 0.8, "0.5-0.8"),
        (0.8, 1.01, ">= 0.8"),
    ] {
        let bucket: Vec<_> = rows
            .iter()
            .zip(scored)
            .filter(|(_, s)| (lo..hi).contains(&s.classification.confidence))
            .collect();
        let ok = bucket
            .iter()
            .filter(|(r, s)| s.classification.request_type.as_str() == r.label)
            .count();
        let _ = writeln!(
            out,
            "| {name} | {label} | {} | {} |",
            bucket.len(),
            pct(ok, bucket.len())
        );
    }
}

fn replay_rows(out: &mut String, name: &str, r: &Replay) {
    for (b, [t1, t2, t3]) in &r.picks {
        let total = t1 + t2 + t3;
        let _ = writeln!(
            out,
            "| {name} | {b} | {} | {} | {} |",
            pct(*t1, total),
            pct(*t2, total),
            pct(*t3, total)
        );
    }
}

#[tokio::main]
async fn main() {
    let split = std::env::args().nth(1).unwrap_or_else(|| "dev".into());
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("eval");
    let data = std::fs::read_to_string(dir.join("requests.jsonl")).expect("eval/requests.jsonl");
    let all: Vec<Row> = data
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("row"))
        .collect();
    let rows: Vec<&Row> = all
        .iter()
        .filter(|r| split == "all" || r.split == split)
        .collect();
    for r in &rows {
        assert!(
            RequestType::from_wire(&r.label).is_some() && (1..=5).contains(&r.complexity),
            "bad label on {}",
            r.id
        );
    }

    let url = std::env::var("LAYA_URL").unwrap_or_else(|_| "http://localhost:8000".into());
    let timeout_ms = std::env::var("LAYA_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30_000);
    let laya = LayaClassifier::new(
        reqwest::Client::new(),
        &url,
        &std::env::var("LAYA_API_KEY").unwrap_or_default(),
        Duration::from_millis(timeout_ms),
    );
    // Warm-up: the first call pays model load / graph compilation.
    laya.classify(&ClassifierInput {
        query: "hello",
        state: "Latest request:\nhello",
    })
    .await;

    let (mut regex, mut layas) = (Vec::new(), Vec::new());
    for row in &rows {
        let msgs = messages(row);
        let state = classify_input(&msgs).expect("has a user turn");
        let input = ClassifierInput {
            query: &row.query,
            state: &state,
        };
        let t = Instant::now();
        let c = RegexClassifier.classify(&input).await;
        regex.push(Scored {
            classification: c,
            latency_ms: t.elapsed().as_secs_f64() * 1e3,
        });
        let t = Instant::now();
        let c = laya.classify(&input).await;
        layas.push(Scored {
            classification: c,
            latency_ms: t.elapsed().as_secs_f64() * 1e3,
        });
        eprint!(".");
    }
    eprintln!();

    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Request classifier evaluation — `{split}` split ({} requests)\n",
        rows.len()
    );
    let _ = writeln!(out, "Laya: `{url}`, checkpoint `multilingual`.\n");
    let _ = writeln!(out, "## Stage 1 — classification\n");
    let _ = writeln!(
        out,
        "| backend | type accuracy | complexity MAE | complexity ±1 | fallbacks | p50 ms | p95 ms |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|---|---|");
    summary(&mut out, "regex", &rows, &regex);
    summary(&mut out, "laya", &rows, &layas);
    let _ = writeln!(
        out,
        "\nRegex has no complexity signal: it always reports 3 (neutral), so its MAE is the \
         cost of not knowing.\n"
    );

    let _ = writeln!(out, "### Accuracy by labelled type\n");
    let _ = writeln!(out, "| type | n | regex | laya |");
    let _ = writeln!(out, "|---|---|---|---|");
    let mut types: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
    types.sort();
    types.dedup();
    for t in types {
        let idx: Vec<usize> = (0..rows.len()).filter(|i| rows[*i].label == t).collect();
        let hit = |s: &[Scored]| {
            idx.iter()
                .filter(|i| s[**i].classification.request_type.as_str() == t)
                .count()
        };
        let _ = writeln!(
            out,
            "| {t} | {} | {} | {} |",
            idx.len(),
            pct(hit(&regex), idx.len()),
            pct(hit(&layas), idx.len())
        );
    }

    let _ = writeln!(
        out,
        "\n### Confidence calibration (type accuracy per confidence bucket)\n"
    );
    let _ = writeln!(out, "| backend | confidence | n | accuracy |");
    let _ = writeln!(out, "|---|---|---|---|");
    calibration(&mut out, "regex", &rows, &regex);
    calibration(&mut out, "laya", &rows, &layas);

    let (rr, lr) = (replay(&rows, &regex), replay(&rows, &layas));
    let _ = writeln!(
        out,
        "\n## Stage 2 — routing replay ({SEEDS} seeds per request, cold start, tier costs 15 / 3 / 0.8)\n"
    );
    let _ = writeln!(
        out,
        "| backend | mean tier cost | hard (4-5) sent to Tier3 | easy (1-2) sent to Tier1 |"
    );
    let _ = writeln!(out, "|---|---|---|---|");
    for (name, r) in [("regex", &rr), ("laya", &lr)] {
        let _ = writeln!(
            out,
            "| {name} | {:.2} | {} | {} |",
            r.cost / r.total as f64,
            pct(r.under, r.under_of),
            pct(r.over, r.over_of)
        );
    }
    let _ = writeln!(out, "\n### Tier mix by labelled complexity\n");
    let _ = writeln!(out, "| backend | band | Tier1 | Tier2 | Tier3 |");
    let _ = writeln!(out, "|---|---|---|---|---|");
    replay_rows(&mut out, "regex", &rr);
    replay_rows(&mut out, "laya", &lr);

    let _ = writeln!(
        out,
        "\n### Decision-rule sweep (same Laya answers, different mapping onto tiers)\n"
    );
    let _ = writeln!(
        out,
        "| type used | K | mean tier cost | hard -> Tier3 | easy -> Tier1 | high band -> Tier1 |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|---|");
    let _ = writeln!(
        out,
        "| regex (baseline) | - | {:.2} | {} | {} | {} |",
        rr.cost / rr.total as f64,
        pct(rr.under, rr.under_of),
        pct(rr.over, rr.over_of),
        high_tier1(&rr)
    );
    for argmax in [true, false] {
        for k in [0.0, 0.3, 0.5, 0.7, 1.0, 1.5] {
            let scored: Vec<Scored> = layas
                .iter()
                .map(|s| Scored {
                    classification: variant(&s.classification, argmax, k),
                    latency_ms: s.latency_ms,
                })
                .collect();
            let r = replay(&rows, &scored);
            let _ = writeln!(
                out,
                "| {} | {k:.1} | {:.2} | {} | {} | {} |",
                if argmax {
                    "argmax"
                } else {
                    "sampled from p(type)"
                },
                r.cost / r.total as f64,
                pct(r.under, r.under_of),
                pct(r.over, r.over_of),
                high_tier1(&r)
            );
        }
    }

    learning(&mut out, &rows, &regex, &layas);

    let _ = writeln!(out, "\n## Per request\n");
    let _ = writeln!(
        out,
        "| id | label | regex | laya | laya complexity (level, conf) |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|");
    for ((row, r), l) in rows.iter().zip(&regex).zip(&layas) {
        let mark = |c: &Classification| {
            let ok = if c.request_type.as_str() == row.label {
                "✓"
            } else {
                "✗"
            };
            format!("{ok} {}", c.request_type.as_str())
        };
        let lc = &l.classification;
        let _ = writeln!(
            out,
            "| {} | {} / {} | {} | {} | {} ({:.2}, {:.2}){} |",
            row.id,
            row.label,
            row.complexity,
            mark(&r.classification),
            mark(lc),
            lc.complexity,
            lc.complexity_level,
            lc.complexity_confidence,
            lc.source
                .fallback_reason()
                .map(|r| format!(" fallback: {r}"))
                .unwrap_or_default(),
        );
    }

    print!("{out}");
    let path = dir.join(format!("report-{split}.md"));
    std::fs::write(&path, &out).expect("write report");
    eprintln!("wrote {}", path.display());
}
