use super::tokenize::{tokens, words};
use crate::ToolDef;
use std::collections::BTreeSet;

const EXACT_NAME: usize = 24;
const NAME_TOKEN: usize = 8;
const REQUIRED_TOKEN: usize = 4;
const DESCRIPTION_TOKEN: usize = 2;
const OPTIONAL_TOKEN: usize = 1;

pub(super) fn score(
    tool: &ToolDef,
    query_words: &[String],
    query_tokens: &BTreeSet<String>,
) -> usize {
    let name_words = words(&tool.function.name);
    let exact = !name_words.is_empty()
        && query_words
            .windows(name_words.len())
            .any(|window| window == name_words);
    let (required, optional) = property_tokens(tool);
    let mut score = if exact { EXACT_NAME } else { 0 };
    for (feature, weight) in [
        (tokens(&tool.function.name), NAME_TOKEN),
        (required, REQUIRED_TOKEN),
        (
            tokens(tool.function.description.as_deref().unwrap_or("")),
            DESCRIPTION_TOKEN,
        ),
        (optional, OPTIONAL_TOKEN),
    ] {
        score = score.saturating_add(
            feature
                .intersection(query_tokens)
                .count()
                .saturating_mul(weight),
        );
    }
    score
}

fn property_tokens(tool: &ToolDef) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut required_tokens = BTreeSet::new();
    let mut optional_tokens = BTreeSet::new();
    if let Some(schema) = &tool.function.parameters
        && let Some(properties) = schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
    {
        let required: BTreeSet<&str> = schema
            .get("required")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .collect();
        for name in properties.keys() {
            if required.contains(name.as_str()) {
                required_tokens.extend(tokens(name));
            } else {
                optional_tokens.extend(tokens(name));
            }
        }
    }
    (required_tokens, optional_tokens)
}
