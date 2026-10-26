//! LangGraph Agent Server (`langgraph dev` / `langgraph up`). It serves one
//! card per assistant at `/.well-known/agent-card.json?assistant_id=<id>`;
//! the bare path answers only when `DEFAULT_A2A_ASSISTANT_ID` is set, so the
//! standard adapter alone misses most LangGraph agents.
//!
//! The server is recognised by `GET /info` (version + feature flags) before
//! anything else is sent, so no other local service ever receives the
//! assistants search request.

use serde_json::{Value, json};

use super::{FoundCard, fetch_card};
use crate::commands::integration::a2a::http::CardHttp;

/// Assistants listed per server. A local dev server holds a handful; the cap
/// only bounds a misbehaving one.
const ASSISTANT_LIMIT: u32 = 100;

pub fn probe(base_url: &str, http: &dyn CardHttp) -> Vec<FoundCard> {
    if !is_langgraph_server(base_url, http) {
        return Vec::new();
    }
    // Sequential on purpose: the server builds each card on request and
    // serialises that work, so parallel fetches finish no sooner and only
    // pile load onto a running agent.
    assistant_ids(base_url, http)
        .into_iter()
        .filter_map(|id| {
            let card_url = format!("{base_url}/.well-known/agent-card.json?assistant_id={id}");
            fetch_card(http, card_url)
        })
        .collect()
}

fn is_langgraph_server(base_url: &str, http: &dyn CardHttp) -> bool {
    http.get(&format!("{base_url}/info"))
        .json()
        .is_some_and(|info| is_langgraph_info(&info))
}

fn assistant_ids(base_url: &str, http: &dyn CardHttp) -> Vec<String> {
    let body = json!({ "limit": ASSISTANT_LIMIT });
    let Some(Value::Array(assistants)) = http
        .post(&format!("{base_url}/assistants/search"), &body)
        .json()
    else {
        return Vec::new();
    };
    assistants
        .iter()
        .filter_map(|assistant| assistant.get("assistant_id")?.as_str())
        .filter(|id| is_query_safe(id))
        .map(str::to_string)
        .collect()
}

/// LangGraph's `GET /info` (`ServerInfo`): a `version` plus a `flags` object,
/// and the library version under `langgraph_py_version` (Python) or
/// `langgraph_js_version` (JS).
fn is_langgraph_info(info: &Value) -> bool {
    let has_flags = info.get("flags").is_some_and(Value::is_object);
    let has_library_version =
        info.get("langgraph_py_version").is_some() || info.get("langgraph_js_version").is_some();
    has_flags && has_library_version
}

/// Assistant IDs are UUIDs or graph names; anything else is skipped rather
/// than escaped, since it cannot be a well-formed LangGraph ID.
fn is_query_safe(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::integration::a2a::http::tests::FakeHttp;

    const BASE: &str = "http://127.0.0.1:2024";

    fn langgraph_server() -> FakeHttp {
        FakeHttp::default().get(
            &format!("{BASE}/info"),
            json!({"version": "0.4.21", "langgraph_py_version": "1.0.1", "flags": {"assistants": true}, "metadata": {}}),
        )
    }

    #[test]
    fn returns_one_card_per_assistant() {
        let http = langgraph_server()
            .post(
                &format!("{BASE}/assistants/search"),
                json!([{"assistant_id": "a1"}, {"assistant_id": "a2"}]),
            )
            .get(
                &format!("{BASE}/.well-known/agent-card.json?assistant_id=a1"),
                json!({"name": "planner", "url": format!("{BASE}/a2a/a1")}),
            )
            .get(
                &format!("{BASE}/.well-known/agent-card.json?assistant_id=a2"),
                json!({"name": "writer", "url": format!("{BASE}/a2a/a2")}),
            );

        let found = probe(BASE, &http);

        let names: Vec<_> = found.iter().map(|f| f.card["name"].clone()).collect();
        assert_eq!(names, vec![json!("planner"), json!("writer")]);
    }

    #[test]
    fn sends_nothing_further_to_other_servers() {
        // Would answer the search, but /info is not LangGraph's.
        let http = FakeHttp::default()
            .get(&format!("{BASE}/info"), json!({"version": "1.0"}))
            .post(
                &format!("{BASE}/assistants/search"),
                json!([{"assistant_id": "a1"}]),
            );
        assert!(probe(BASE, &http).is_empty());
        assert_eq!(http.post_count(), 0);
    }

    #[test]
    fn skips_assistant_ids_that_are_not_query_safe() {
        let http = langgraph_server().post(
            &format!("{BASE}/assistants/search"),
            json!([{"assistant_id": "a1&x=y"}]),
        );
        assert!(probe(BASE, &http).is_empty());
    }
}
