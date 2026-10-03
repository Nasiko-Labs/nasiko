use nasiko_tool_compact::{OptimizationContext, OptimizationPlan, ToolDef, optimize_tools};
use serde_json::json;

fn tools() -> Vec<ToolDef> {
    (0..48)
        .map(|index| {
            serde_json::from_value(json!({"function":{"name":format!("action_{index}")}})).unwrap()
        })
        .collect()
}

#[test]
fn explicit_opt_in_forced_choices_and_unsupported_schemas_are_native() {
    let mut tools = tools();
    for (enabled, forced) in [(false, false), (true, true)] {
        let result = optimize_tools(OptimizationContext {
            tools: &tools,
            query: Some("action_2"),
            compact_enabled: enabled,
            scope_enabled: true,
            forced_tool_choice: forced,
        });
        assert_eq!(result.report.plan, OptimizationPlan::Native);
        assert!(result.compact.is_none());
        assert_eq!(result.selected_indices.len(), 48);
    }
    tools[0].function.parameters = Some(json!({"type":"object","oneOf":[]}));
    let result = optimize_tools(OptimizationContext {
        tools: &tools,
        query: None,
        compact_enabled: true,
        scope_enabled: false,
        forced_tool_choice: false,
    });
    assert_eq!(result.report.plan, OptimizationPlan::Native);
    assert_eq!(result.report.schemas_bypassed, 48);
}

#[test]
fn optional_selection_compacts_all_retained_tools_or_falls_back_to_all_originals() {
    let mut tools = tools();
    // Every action shares a name token: an ambiguous query must keep all.
    let full = optimize_tools(OptimizationContext {
        tools: &tools,
        query: None,
        compact_enabled: true,
        scope_enabled: true,
        forced_tool_choice: false,
    });
    assert_eq!(full.report.plan, OptimizationPlan::CompactAll);
    tools[0].function.name = "send_email".into();
    let subset = optimize_tools(OptimizationContext {
        tools: &tools,
        query: Some("send_email"),
        compact_enabled: true,
        scope_enabled: true,
        forced_tool_choice: false,
    });
    assert_eq!(subset.report.plan, OptimizationPlan::SelectAndCompact);
    assert_eq!(subset.selected_indices, vec![0]);
    tools[0].function.parameters = Some(json!({"type":"object","oneOf":[]}));
    let native = optimize_tools(OptimizationContext {
        tools: &tools,
        query: Some("send_email"),
        compact_enabled: true,
        scope_enabled: true,
        forced_tool_choice: false,
    });
    assert_eq!(native.report.plan, OptimizationPlan::Native);
    assert_eq!(native.selected_indices.len(), 48);
}
