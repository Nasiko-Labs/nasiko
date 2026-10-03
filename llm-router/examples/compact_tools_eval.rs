//! P1 evaluator: EVAL_SET=input.json OUT=output.jsonl cargo run --release
//! -p nasiko-llm-router --example compact_tools_eval. Offline unless BOTH
//! PROVIDER_BASE_URL and MODEL are explicitly configured. ToolScope is never used.

#[path = "compact_tools/dataset.rs"]
mod dataset;
#[path = "compact_tools/live.rs"]
mod live;
#[path = "compact_tools/request.rs"]
mod request;

use anyhow::{Context, Result};
use dataset::{Case, Dataset};
use nasiko_tool_compact::{
    CompactError, StreamDecoder, ToolCall, ToolDef, decode_calls, render_calls,
};
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{BufWriter, Write},
};

#[tokio::main]
async fn main() -> Result<()> {
    let input = std::env::var("EVAL_SET").context("EVAL_SET must name the dataset")?;
    let output = std::env::var("OUT").context("OUT must name the JSONL output")?;
    let dataset: Dataset =
        serde_json::from_reader(File::open(input)?).context("invalid evaluator dataset")?;
    let lookup = dataset.lookup()?;
    let live = live::LiveConfig::from_env()?;
    let mut writer = BufWriter::new(File::create(output)?);
    for case in &dataset.cases {
        let tools = dataset::resolve(&lookup, &case.tools)?;
        let (body, compacted) = request::build(case, &tools)?;
        let rendered = render_calls(&case.expected)?;
        let roundtrip = decode_calls(&rendered, &tools);
        let mut record = json!({"id":case.id,"compact_request":body,"compacted":compacted,"rendered_calls":rendered,"roundtrip_calls":call_result(roundtrip)});
        if let Some(live) = &live {
            let (sent, raw, calls) = live.evaluate(body, compacted, &tools).await?;
            record["compact_request"] = sent;
            record["raw_output"] = raw;
            record["live_calls"] = calls;
        }
        write_record(&mut writer, &record)?;
    }
    for case in &dataset.decoder_cases {
        let tools = dataset::resolve(&lookup, &case.tools)?;
        let decoded = decode_chunks(&case.chunks, &tools);
        write_record(&mut writer, &json!({"id":case.id,"decoded":decoded}))?;
    }
    writer.flush()?;
    Ok(())
}

fn write_record(writer: &mut impl Write, record: &Value) -> Result<()> {
    serde_json::to_writer(&mut *writer, record)?;
    writer.write_all(b"\n")?;
    Ok(())
}

fn call_result(result: nasiko_tool_compact::Result<Vec<ToolCall>>) -> Value {
    match result {
        Ok(calls) => json!(calls),
        Err(error) => json!({"error":error_label(&error)}),
    }
}

fn error_label(error: &CompactError) -> &'static str {
    match error {
        CompactError::UnknownTool(_) => "unknown_tool",
        _ => "invalid_arguments",
    }
}

fn decode_chunks(chunks: &[String], tools: &[ToolDef]) -> Value {
    let result = (|| {
        let mut decoder = StreamDecoder::new(tools)?;
        for chunk in chunks {
            decoder.push(chunk)?;
        }
        decoder.finish()
    })();
    match result {
        Ok(calls) => json!({"calls":calls}),
        Err(error) => json!({"error":error_label(&error)}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case() -> Case {
        serde_json::from_value(json!({"id":"unseen","tools":["work"],"messages":[{"role":"system","content":"Original"},{"role":"user","content":"Do work"}],"expected":[]})).unwrap()
    }
    fn tools() -> Vec<ToolDef> {
        serde_json::from_value(json!([{"type":"function","function":{"name":"work","parameters":{"type":"object","required":["x"],"properties":{"x":{"type":"integer"}}}}}])).unwrap()
    }

    #[test]
    fn preserves_messages_and_uses_whole_request_native_fallback() {
        let case = case();
        let tools = tools();
        let (compact, enabled) = request::build(&case, &tools).unwrap();
        assert!(enabled);
        assert!(compact.get("tools").is_none());
        assert_eq!(compact["messages"][0], case.messages[0]);
        assert_eq!(compact["messages"][2], case.messages[1]);
        let mut unsupported = tools;
        unsupported[0].function.parameters.as_mut().unwrap()["oneOf"] = json!([]);
        let (native, enabled) = request::build(&case, &unsupported).unwrap();
        assert!(!enabled);
        assert_eq!(native["tools"], serde_json::to_value(unsupported).unwrap());
        assert_eq!(native["messages"], json!(case.messages));
    }

    #[test]
    fn decoder_cases_are_streamed_and_fail_atomically() {
        assert_eq!(
            decode_chunks(
                &["<<ca".into(), "ll work {\"x\":1}>".into(), ">".into()],
                &tools()
            ),
            json!({"calls":[{"name":"work","arguments":{"x":1}}]})
        );
        assert_eq!(
            decode_chunks(
                &[
                    "<<call work {\"x\":1}>>".into(),
                    "<<call missing {}>>".into()
                ],
                &tools()
            ),
            json!({"error":"unknown_tool"})
        );
        assert_eq!(
            decode_chunks(&["<<call work {}>>".into()], &tools()),
            json!({"error":"invalid_arguments"})
        );
    }

    #[test]
    fn forced_choice_and_tool_history_use_native_tools() {
        let mut case = case();
        case.extra.insert(
            "tool_choice".into(),
            json!({"type":"function","function":{"name":"work"}}),
        );
        assert!(!request::build(&case, &tools()).unwrap().1);
        case.extra.clear();
        case.extra
            .insert("parallel_tool_calls".into(), json!(false));
        assert!(!request::build(&case, &tools()).unwrap().1);
        case.extra.clear();
        case.messages
            .push(json!({"role":"tool","content":"done","tool_call_id":"existing"}));
        assert!(!request::build(&case, &tools()).unwrap().1);
    }
}
