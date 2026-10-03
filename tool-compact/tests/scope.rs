use nasiko_tool_compact::{ScopeInput, ScopeReason, ToolDef, select_tools};
use serde_json::json;

fn tools() -> Vec<ToolDef> {
    let mut names = vec![
        "sendEmail".to_owned(),
        "CreateCalendarEvent".to_owned(),
        "HTTPServerStatus".to_owned(),
    ];
    names.extend((0..45).map(|index| format!("task_{index}")));
    names
        .into_iter()
        .map(|name| serde_json::from_value(json!({"function":{"name":name}})).unwrap())
        .collect()
}

#[test]
fn exact_multi_tool_selection_is_deterministic_and_ordered() {
    let tools = tools();
    let query = "Create-calendar-event and send_email";
    let result = select_tools(ScopeInput {
        query,
        tools: &tools,
    });
    assert_eq!(result.selected_indices, vec![0, 1]);
    assert_eq!(result.reason, ScopeReason::ConfidentSubset);
    assert_eq!(
        result,
        select_tools(ScopeInput {
            query,
            tools: &tools
        })
    );
    assert_eq!(
        select_tools(ScopeInput {
            query: "http_server_status",
            tools: &tools
        })
        .selected_indices,
        vec![2]
    );
}

#[test]
fn no_signal_small_sets_weak_and_ambiguous_requests_fall_back() {
    let mut tools = tools();
    for query in ["", "quantum bananas", "task"] {
        let result = select_tools(ScopeInput {
            query,
            tools: &tools,
        });
        assert_eq!(result.selected_indices.len(), tools.len());
    }
    tools[0].function.description = Some("banana".into());
    assert_eq!(
        select_tools(ScopeInput {
            query: "banana",
            tools: &tools
        })
        .reason,
        ScopeReason::LowConfidenceFullFallback
    );
    assert_eq!(
        select_tools(ScopeInput {
            query: "send_email",
            tools: &tools[..8]
        })
        .reason,
        ScopeReason::NotNeededSmallToolSet
    );
}

#[test]
fn weights_are_distinct_and_safety_tail_is_bounded() {
    let mut tools = tools();
    for index in [3, 4] {
        tools[index].function.parameters = Some(
            json!({"type":"object","properties":{"recipient":{"type":"string"}},"required":["recipient"]}),
        );
    }
    let result = select_tools(ScopeInput {
        query: "send_email recipient recipient",
        tools: &tools,
    });
    assert_eq!(result.scores[0], 40);
    assert_eq!(result.scores[3], 4);
    assert_eq!(result.scores[4], 4);
    assert_eq!(result.selected_indices, vec![0, 3]);
}
