//! Guards accidental leakage between the authored development and held-out splits.

use std::collections::HashSet;

use nasiko_llm_router::routing::classifier::RequestType;
use serde::Deserialize;

#[derive(Deserialize)]
struct Set {
    examples: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    family: String,
    query: String,
    context: String,
    request_type: String,
    complexity: u8,
    rationale: String,
}

#[test]
fn authored_splits_have_valid_labels_unique_scenarios_and_no_text_duplicates() {
    let train: Set = serde_json::from_str(include_str!("data/classifier_train.json")).unwrap();
    let validation: Set =
        serde_json::from_str(include_str!("data/classifier_validation.json")).unwrap();
    let mut ids = HashSet::new();
    let mut families = HashSet::new();
    let mut texts = HashSet::new();
    for cases in [&train.examples, &validation.examples] {
        assert!(cases.len() >= 30);
        let mut labels = HashSet::new();
        for case in cases {
            assert!(ids.insert(&case.id));
            assert!(
                families.insert(&case.family),
                "scenario family crosses split"
            );
            assert!(texts.insert(format!("{} {}", case.query, case.context).to_lowercase()));
            assert!(!case.rationale.is_empty());
            assert!((1..=5).contains(&case.complexity));
            assert!(RequestType::from_wire(&case.request_type).is_some());
            labels.insert(&case.request_type);
        }
        assert_eq!(labels.len(), 7);
    }
}
