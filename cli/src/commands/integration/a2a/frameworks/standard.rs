//! The A2A standard: a card at the spec's well-known path. Finds agents from
//! any framework that serves it there.

use super::{FoundCard, fetch_card};
use crate::commands::integration::a2a::http::CardHttp;

/// Card paths, current spec first, then the pre-0.3 name.
const CARD_PATHS: [&str; 2] = ["/.well-known/agent-card.json", "/.well-known/agent.json"];

pub fn probe(base_url: &str, http: &dyn CardHttp) -> Vec<FoundCard> {
    CARD_PATHS
        .iter()
        .find_map(|path| fetch_card(http, format!("{base_url}{path}")))
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::integration::a2a::http::tests::FakeHttp;
    use serde_json::json;

    const BASE: &str = "http://127.0.0.1:9000";

    #[test]
    fn falls_back_to_the_legacy_card_path() {
        let http = FakeHttp::default().get(
            &format!("{BASE}/.well-known/agent.json"),
            json!({"name": "legacy", "url": "http://x/"}),
        );
        let found = probe(BASE, &http);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].card_url, format!("{BASE}/.well-known/agent.json"));
    }

    #[test]
    fn ignores_json_that_is_not_a_card() {
        let http = FakeHttp::default().get(
            &format!("{BASE}/.well-known/agent-card.json"),
            json!({"detail": "Not Found"}),
        );
        assert!(probe(BASE, &http).is_empty());
    }
}
