//! Closed set of framework adapters and immediate adapter delegation — the
//! same shape as the coding-agent catalog in `integration/agents/`.
//!
//! Each adapter knows one way an A2A card can be served. [`Framework::Standard`]
//! is the A2A well-known path and finds agents from any framework that uses it
//! (CrewAI, Google ADK, a2a-sdk servers, ...). Frameworks that serve cards
//! elsewhere get their own adapter; LangGraph is the first. Add one by adding
//! a module here, a variant, and its place in [`Framework::ALL`].

use serde_json::Value;

use super::http::CardHttp;

pub mod langgraph;
pub mod standard;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Framework {
    LangGraph,
    Standard,
}

impl Framework {
    /// Most specific first: when two adapters find the same agent, the first
    /// one's result is kept.
    pub const ALL: &'static [Self] = &[Self::LangGraph, Self::Standard];

    /// Every card this adapter finds at `base_url`; empty when it recognises
    /// nothing there.
    pub fn probe(self, base_url: &str, http: &dyn CardHttp) -> Vec<FoundCard> {
        match self {
            Self::LangGraph => langgraph::probe(base_url, http),
            Self::Standard => standard::probe(base_url, http),
        }
    }
}

/// A card an adapter found, with where it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundCard {
    pub card_url: String,
    pub card: Value,
}

/// Whether `value` looks like an A2A agent card: a name, plus somewhere to
/// reach the agent (`url` in 0.3.x, `supportedInterfaces` in 1.0). Only
/// recognition — whether the catalog can store it is
/// `nasiko_types::agent_card::read_agent_card`'s call.
pub fn is_agent_card(value: &Value) -> bool {
    let has_name = value
        .get("name")
        .and_then(Value::as_str)
        .is_some_and(|name| !name.trim().is_empty());
    let has_url = value.get("url").and_then(Value::as_str).is_some();
    let has_interfaces = value
        .get("supportedInterfaces")
        .and_then(Value::as_array)
        .is_some_and(|interfaces| !interfaces.is_empty());
    has_name && (has_url || has_interfaces)
}

/// GET `card_url` and keep it if it is a card.
fn fetch_card(http: &dyn CardHttp, card_url: String) -> Option<FoundCard> {
    let card = http.get(&card_url).json().filter(is_agent_card)?;
    Some(FoundCard { card_url, card })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cards_need_a_name_and_an_address() {
        assert!(is_agent_card(&json!({"name": "a", "url": "http://x/"})));
        assert!(is_agent_card(
            &json!({"name": "a", "supportedInterfaces": [{"url": "http://x/"}]})
        ));
        assert!(!is_agent_card(&json!({"name": "a"})));
        assert!(!is_agent_card(&json!({"name": " ", "url": "http://x/"})));
        assert!(!is_agent_card(&json!({"status": "ok"})));
    }

    #[test]
    fn the_most_specific_adapter_comes_first() {
        assert_eq!(Framework::ALL.first(), Some(&Framework::LangGraph));
        assert_eq!(Framework::ALL.last(), Some(&Framework::Standard));
    }
}
