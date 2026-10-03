//! Guards on the contribution's labelled classifier data: schema, coverage, split
//! hygiene and a leakage check. Hermetic — reads the JSON files in `tests/data/classifier`.
use std::collections::{BTreeMap, BTreeSet, HashSet};

use nasiko_llm_router::routing::RequestType;
use nasiko_llm_router::routing::classifier_eval::{
    LabeledCase, read_eval_cases, read_labeled_cases,
};

const SPLITS: [&str; 3] = ["dev", "calibration", "heldout"];

fn load(split: &str) -> Vec<LabeledCase> {
    let path = format!(
        "{}/tests/data/classifier/{split}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    // The inference-side reader must accept the file too (same schema as the public set).
    read_eval_cases(&raw).unwrap_or_else(|e| panic!("{split}: inference reader rejected: {e}"));
    read_labeled_cases(&raw).unwrap_or_else(|e| panic!("{split}: {e}"))
}

fn tokens(s: &str) -> BTreeSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 2)
        .map(str::to_string)
        .collect()
}

fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let inter = a.intersection(b).count() as f64;
    let union = a.union(b).count() as f64;
    inter / union
}

#[test]
fn every_split_is_well_formed_with_unique_ids_families_and_valid_labels() {
    for split in SPLITS {
        let cases = load(split);
        assert!(
            cases.len() >= 20,
            "{split}: expected a usable split, got {}",
            cases.len()
        );
        let mut ids = HashSet::new();
        for c in &cases {
            assert!(ids.insert(c.id.clone()), "{split}: duplicate id {}", c.id);
            assert!(c.family.is_some(), "{split}/{}: missing family", c.id);
            assert!(
                (1..=5).contains(&c.complexity),
                "{split}/{}: complexity {}",
                c.id,
                c.complexity
            );
            assert!(!c.query.trim().is_empty(), "{split}/{}: empty query", c.id);
            assert!(!c.tests.is_empty(), "{split}/{}: no test tags", c.id);
        }
    }
}

#[test]
fn dev_and_heldout_cover_every_type_and_every_complexity_level() {
    for split in ["dev", "heldout"] {
        let cases = load(split);
        let mut by_type: BTreeMap<&str, usize> = BTreeMap::new();
        let mut by_cx: BTreeMap<u8, usize> = BTreeMap::new();
        for c in &cases {
            *by_type.entry(c.request_type.as_str()).or_default() += 1;
            *by_cx.entry(c.complexity).or_default() += 1;
        }
        for rt in RequestType::ALL {
            assert!(
                by_type.get(rt.as_str()).copied().unwrap_or(0) >= 4,
                "{split}: too few {} cases: {by_type:?}",
                rt.as_str()
            );
        }
        for level in 1..=5u8 {
            assert!(
                by_cx.get(&level).copied().unwrap_or(0) >= 3,
                "{split}: too few complexity-{level} cases: {by_cx:?}"
            );
        }
    }
}

#[test]
fn phenomenon_tags_are_represented_in_the_heldout_split() {
    let cases = load("heldout");
    let tags: HashSet<&str> = cases
        .iter()
        .flat_map(|c| c.tests.iter().map(String::as_str))
        .collect();
    for required in [
        "ambiguity",
        "mixed_intent",
        "paraphrase",
        "unseen_topic",
        "negation",
        "keyword_trap",
        "context_dependent",
        "noisy_padded",
        "concise_hard",
        "verbose_easy",
        "spelling_error",
        "missing_context",
        "adversarial_quoted",
        "unicode",
    ] {
        assert!(
            tags.contains(required),
            "heldout: no case tagged {required}"
        );
    }
}

#[test]
fn no_family_and_no_near_duplicate_crosses_a_split() {
    let all: Vec<(&str, LabeledCase)> = SPLITS
        .iter()
        .flat_map(|s| load(s).into_iter().map(move |c| (*s, c)))
        .collect();
    // Families are split-exclusive.
    let mut family_split: BTreeMap<String, &str> = BTreeMap::new();
    for (split, c) in &all {
        let fam = c.family.clone().unwrap();
        match family_split.get(&fam) {
            Some(prev) if prev != split => {
                panic!("family '{fam}' appears in both {prev} and {split}")
            }
            _ => {
                family_split.insert(fam, split);
            }
        }
    }
    // Near-duplicate queries (token Jaccard ≥ 0.6) never cross splits; within a split
    // they must share a family (so the manifest explains them).
    let toks: Vec<BTreeSet<String>> = all.iter().map(|(_, c)| tokens(&c.query)).collect();
    for i in 0..all.len() {
        for j in (i + 1)..all.len() {
            let sim = jaccard(&toks[i], &toks[j]);
            if sim >= 0.6 {
                let (si, ci) = &all[i];
                let (sj, cj) = &all[j];
                assert_eq!(
                    si, sj,
                    "near-duplicate across splits: {} ({si}) ~ {} ({sj}) jaccard={sim:.2}",
                    ci.id, cj.id
                );
                assert_eq!(
                    ci.family, cj.family,
                    "near-duplicate within {si} not in one family: {} ~ {} jaccard={sim:.2}",
                    ci.id, cj.id
                );
            }
        }
    }
    // Nothing in our splits duplicates the public smoke sample's ids.
    for (_, c) in &all {
        assert!(
            !c.id.starts_with("pub-"),
            "{}: reserved public-sample id prefix",
            c.id
        );
    }
}

#[test]
fn split_manifest_matches_the_files() {
    let path = format!(
        "{}/tests/data/classifier/split-manifest.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(&path).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&raw).unwrap();
    for split in SPLITS {
        let cases = load(split);
        let m = &manifest["splits"][split];
        assert_eq!(
            m["cases"].as_u64().unwrap() as usize,
            cases.len(),
            "{split}: manifest case count"
        );
        let families: BTreeSet<String> = cases.iter().map(|c| c.family.clone().unwrap()).collect();
        let listed: BTreeSet<String> = m["families"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(families, listed, "{split}: manifest families");
    }
}
