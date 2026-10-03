//! Synthetic SCOPE → ZIP → GUARD demo; separate from official P1 results.
#[path = "compact_tools/dataset.rs"]
mod dataset;
#[path = "compact_tools/request.rs"]
mod request;

use anyhow::Result;
use dataset::Case;
use nasiko_tool_compact::{
    ScopeInput, StreamDecoder, ToolCall, ToolDef, analyze_tools, decode_calls, decode_tools,
    encode_tools, render_calls, select_tools,
};
use serde_json::{Value, json};

fn main() -> Result<()> {
    let tools = demo_tools()?;
    let query = "mail_send and calendar_create";
    let scope = select_tools(ScopeInput {
        query,
        tools: &tools,
    });
    let selected: Vec<ToolDef> = scope
        .selected_indices
        .iter()
        .map(|&index| tools[index].clone())
        .collect();
    let schema = encode_tools(&selected)?;
    let equality = analyze_tools(&selected)? == analyze_tools(&decode_tools(&schema)?)?;
    let calls = vec![ToolCall {
        name: "mail_send".into(),
        arguments: json!({"text":"Build >> deployed","mode":"public"}),
    }];
    let rendered = render_calls(&calls)?;
    let split = rendered
        .find("mail_send")
        .map(|index| index + 4)
        .unwrap_or(4);
    let chunks = [
        &rendered[..4],
        &rendered[4..split],
        &rendered[split..rendered.len() - 1],
        &rendered[rendered.len() - 1..],
    ];
    let mut stream = StreamDecoder::new(&selected)?;
    let mut emitted = Vec::new();
    for chunk in chunks {
        emitted.push(stream.push(chunk)?.len());
    }
    let decoded = stream.finish()?;
    let openai: Vec<Value> = decoded.iter().enumerate().map(|(index,call)| {
        Ok(json!({"id":format!("call_demo_{index}"),"type":"function","function":{"name":call.name,"arguments":serde_json::to_string(&call.arguments)?}}))
    }).collect::<Result<_>>()?;
    let case = Case {
        id: "synthetic-demo".into(),
        tools: tools
            .iter()
            .map(|tool| tool.function.name.clone())
            .collect(),
        messages: vec![json!({"role":"user","content":query})],
        expected: calls,
        extra: Default::default(),
    };
    let native = json!({"messages":case.messages,"tools":tools});
    let (zip, _) = request::build(&case, &tools)?;
    let (scoped, _) = request::build(&case, &selected)?;
    let tokenizer = tiktoken_rs::o200k_base()?;
    let count = |body: &Value| -> Result<usize> {
        Ok(tokenizer
            .encode_with_special_tokens(&serde_json::to_string(body)?)
            .len())
    };
    let native_tokens = count(&native)?;
    let zip_tokens = count(&zip)?;
    let scoped_tokens = count(&scoped)?;
    let no_signal = select_tools(ScopeInput {
        query: "quantum bananas",
        tools: &tools,
    });
    let ambiguous = select_tools(ScopeInput {
        query: "record",
        tools: &tools,
    });
    let unknown = decode_calls("<<call missing {}>>", &selected).is_err();
    let invalid_enum = decode_calls(
        "<<call mail_send {\"text\":\"x\",\"mode\":\"secret\"}>>",
        &selected,
    )
    .is_err();
    let mut unsupported = tools.clone();
    unsupported[0]
        .function
        .parameters
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("demo schema missing"))?["oneOf"] = json!([]);
    let (fallback, compacted) = request::build(&case, &unsupported)?;
    // Also exercise generic dataset resolution used by the official evaluator.
    let dataset: dataset::Dataset =
        serde_json::from_value(json!({"tools":tools,"cases":[],"decoder_cases":[]}))?;
    let resolved = dataset::resolve(&dataset.lookup()?, &case.tools)?;
    let roundtrip_matches_expected = decoded == case.expected;
    println!(
        "Demo case {}: roundtrip matches expected = {roundtrip_matches_expected}",
        case.id
    );
    let output = json!({"fixture":"synthetic 48-tool demo; not organizer scores","native_tokens":native_tokens,"zip_only_tokens":zip_tokens,"zip_only_reduction":1.0-zip_tokens as f64/native_tokens as f64,"scope_and_zip_tokens":scoped_tokens,"scope_and_zip_reduction":1.0-scoped_tokens as f64/native_tokens as f64,"candidate_count":resolved.len(),"selected_count":selected.len(),"selected_tools":selected.iter().map(|tool|&tool.function.name).collect::<Vec<_>>(),"scope_confidence":scope.confidence,"no_signal_retained":no_signal.selected_indices.len(),"ambiguous_retained":ambiguous.selected_indices.len(),"compact_grammar":schema.rendered,"rendered_call":rendered,"standard_tool_calls":openai,"stream_emissions":emitted,"schema_roundtrip_equal":equality,"unknown_tool_rejected":unknown,"invalid_enum_rejected":invalid_enum,"unsupported_schema_native_fallback":!compacted && fallback.get("tools").is_some(),"official_dataset_case_count":dataset.cases.len(),"official_decoder_cases":dataset.decoder_cases.iter().map(|case|json!({"id":case.id,"tools":case.tools,"chunk_count":case.chunks.len()})).collect::<Vec<_>>()});
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn demo_tools() -> Result<Vec<ToolDef>> {
    let names = ["mail_send".to_owned(), "calendar_create".to_owned()]
        .into_iter()
        .chain((0..46).map(|index| format!("record_{index}")));
    names.map(|name| {
        Ok(serde_json::from_value(json!({"type":"function","function":{"description":format!("Run the {name} operation on the supplied text."),"name":name,"parameters":{"type":"object","required":["text"],"additionalProperties":false,"properties":{"text":{"type":"string","description":"Text to process"},"mode":{"type":"string","enum":["public","private"],"description":"Visibility of the result"},"metadata":{"type":"object","additionalProperties":false,"properties":{"labels":{"type":"array","items":{"type":"string"},"description":"Optional labels"}}}}}}}))?)
    }).collect()
}
