//! Opt-in compact tool schemas at the egress seam (CompTrust, `nasiko-tool-compact`).
//!
//! Runs on the normalized [`ChatRequest`] / [`ChatResponse`] / [`ChatChunk`] IR, so one
//! implementation covers the OpenAI, Anthropic and Gemini inbound surfaces. `nasiko-tool-compact`
//! stays a pure library with its own types; this module is the seam that converts to and from
//! the router's IR.
//!
//! # What it does
//!
//! Request: tools with an exact compact form are removed from `tools` and described, with the
//! call format, in one trailing `system` message (the form [`crate::brevity`] uses, so author
//! text stays byte-identical and the cached prefix is untouched). Tools that cannot be
//! represented exactly stay in `tools` unchanged, so they keep working natively.
//!
//! Response: `<<call name {json}>>` markers in the assistant text are decoded and validated
//! against the **original** schemas, then returned as ordinary `tool_calls` with router-minted
//! ids. An invalid call is an error — never a repaired or guessed one.
//!
//! # Default off, and when it steps aside
//!
//! Nothing here runs unless `GatewayConfig::compact_tools_enabled` is set, and with it off the
//! request is not touched at all. Even when on, a request is left alone if compaction cannot be
//! guaranteed safe: a forced/disabled `tool_choice`, any prior tool call or tool result in the
//! transcript (the earlier calls would reference tools that are no longer declared), duplicate
//! tool names, a tool that is not a plain function or carries extra provider fields, or nothing
//! compactable.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use futures::StreamExt;
use futures::stream::BoxStream;
use nasiko_tool_compact as tc;
use serde_json::Value;

use crate::config::GatewayConfig;
use crate::ir::{
    ChatChunk, ChatRequest, ChatResponse, ChunkChoice, Delta, FunctionCall, FunctionCallDelta,
    Message, ToolCall, ToolCallDelta, ToolDef,
};
use crate::providers::ProviderError;

/// The tools that were offered in compact form: what the response must be decoded against.
#[derive(Debug, Clone)]
pub(crate) struct Plan {
    tools: Vec<tc::ToolDef>,
}

/// Why a request was left untouched, for logs and tests.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum Skipped {
    Disabled,
    NoTools,
    /// `tool_choice` other than absent/`auto`: a forced call cannot be guaranteed in compact form.
    ToolChoice,
    /// The transcript already contains tool calls/results, which reference declared tools.
    ToolHistory,
    DuplicateNames,
    NothingCompactable,
}

fn is_plain_function(t: &ToolDef) -> bool {
    t.kind == "function" && t.extra.is_empty()
}

fn compact_error(e: &tc::Error) -> String {
    format!("compact tool call rejected ({}): {e}", e.code())
}

/// Compact the request's tools in place. `Err` means the request was left byte-identical.
pub(crate) fn apply(req: &mut ChatRequest, cfg: &GatewayConfig) -> Result<Plan, Skipped> {
    if !cfg.compact_tools_enabled {
        return Err(Skipped::Disabled);
    }
    let tools = match req.tools.as_deref() {
        Some(t) if !t.is_empty() => t,
        _ => return Err(Skipped::NoTools),
    };
    match &req.tool_choice {
        None | Some(Value::Null) => {}
        Some(Value::String(s)) if s == "auto" => {}
        Some(_) => return Err(Skipped::ToolChoice),
    }
    if req
        .messages
        .iter()
        .any(|m| m.tool_calls.is_some() || m.role == "tool")
    {
        return Err(Skipped::ToolHistory);
    }

    let mut names = HashSet::new();
    if !tools.iter().all(|t| names.insert(t.function.name.as_str())) {
        return Err(Skipped::DuplicateNames);
    }
    let defs: Vec<tc::ToolDef> = tools
        .iter()
        .filter(|t| is_plain_function(t))
        .map(|t| tc::ToolDef {
            name: t.function.name.clone(),
            description: t.function.description.clone(),
            parameters: t.function.parameters.clone(),
        })
        .collect();
    let compact = tc::encode_tools(&defs).map_err(|_| Skipped::DuplicateNames)?;
    if !compact.is_compacted() {
        return Err(Skipped::NothingCompactable);
    }

    // Everything that did not become a compact signature is sent exactly as it came in.
    let bypassed: HashSet<&str> = compact.native.iter().map(|t| t.name.as_str()).collect();
    let native: Vec<ToolDef> = tools
        .iter()
        .filter(|t| !is_plain_function(t) || bypassed.contains(t.function.name.as_str()))
        .cloned()
        .collect();
    let plan = Plan {
        tools: defs
            .into_iter()
            .filter(|d| !bypassed.contains(d.name.as_str()))
            .collect(),
    };

    if native.is_empty() {
        // `tool_choice: auto` without `tools` is rejected by OpenAI-shaped providers.
        req.tools = None;
        req.tool_choice = None;
    } else {
        req.tools = Some(native);
    }
    req.messages.push(Message {
        role: "system".into(),
        content: Some(Value::String(compact.prompt())),
        name: None,
        tool_calls: None,
        tool_call_id: None,
        extra: Default::default(),
    });
    Ok(plan)
}

fn mint_id() -> String {
    format!("call_{}", uuid::Uuid::new_v4().simple())
}

/// Decode compact calls in a non-streaming response into `tool_calls`.
///
/// A message without a marker is left untouched. On the first invalid call the whole response
/// is rejected: the caller must not forward any part of it.
pub(crate) fn decode_response(resp: &mut ChatResponse, plan: &Plan) -> Result<(), String> {
    for choice in &mut resp.choices {
        let Some(text) = choice.message.text() else {
            continue;
        };
        if !text.contains("<<call") {
            continue;
        }
        let mut decoder = tc::StreamDecoder::new(&plan.tools).map_err(|e| compact_error(&e))?;
        let calls = decoder.push(&text).map_err(|e| compact_error(&e))?;
        let mut prose = decoder.take_text();
        prose.push_str(&decoder.finish().map_err(|e| compact_error(&e))?);
        if calls.is_empty() {
            continue; // marker lookalikes in prose: nothing to decode, nothing to change
        }
        let msg = &mut choice.message;
        let prose = prose.trim();
        msg.content = (!prose.is_empty()).then(|| Value::String(prose.to_string()));
        msg.tool_calls
            .get_or_insert_with(Vec::new)
            .extend(calls.into_iter().map(|c| ToolCall {
                id: mint_id(),
                kind: "function".into(),
                function: FunctionCall {
                    name: c.name,
                    arguments: c.arguments,
                },
                extra: Default::default(),
            }));
        if matches!(choice.finish_reason.as_deref(), None | Some("stop")) {
            choice.finish_reason = Some("tool_calls".into());
        }
    }
    Ok(())
}

struct ChoiceState {
    /// `None` once the choice has finished.
    decoder: Option<tc::StreamDecoder>,
    /// Next `tool_calls` index; starts after any native call deltas already seen.
    next_index: i64,
    emitted_calls: bool,
}

/// The envelope of `chunk` (id, model, …) with no choices and no usage.
fn template_of(chunk: &ChatChunk) -> ChatChunk {
    ChatChunk {
        id: chunk.id.clone(),
        object: chunk.object.clone(),
        created: chunk.created,
        model: chunk.model.clone(),
        choices: vec![],
        usage: None,
        extra: chunk.extra.clone(),
    }
}

fn choice_chunk(
    template: &ChatChunk,
    index: i64,
    delta: Delta,
    finish: Option<String>,
) -> ChatChunk {
    ChatChunk {
        choices: vec![ChunkChoice {
            index,
            delta,
            finish_reason: finish,
        }],
        ..template.clone()
    }
}

/// Wrap a provider stream so compact calls in the text become `tool_calls` deltas.
///
/// Text outside markers is forwarded as it arrives (a possible marker start is held back until
/// resolved). Each call is emitted whole, as its own chunk, once its marker closes and
/// validates. An invalid or unterminated call ends the stream with an error item and nothing
/// after it is forwarded.
pub(crate) fn decode_stream(
    inner: BoxStream<'static, Result<ChatChunk, ProviderError>>,
    plan: Plan,
) -> BoxStream<'static, Result<ChatChunk, ProviderError>> {
    let reject = |e: &tc::Error| ProviderError::Parse(compact_error(e));
    Box::pin(async_stream::stream! {
        futures::pin_mut!(inner);
        let mut states: HashMap<i64, ChoiceState> = HashMap::new();
        let mut last: Option<ChatChunk> = None;
        while let Some(item) = inner.next().await {
            let mut chunk = match item {
                Ok(c) => c,
                Err(e) => { yield Err(e); return; }
            };
            let template = template_of(&chunk);
            let mut extra_chunks: Vec<ChatChunk> = Vec::new();
            for choice in &mut chunk.choices {
                let st = match states.entry(choice.index) {
                    Entry::Occupied(o) => o.into_mut(),
                    Entry::Vacant(v) => match tc::StreamDecoder::new(&plan.tools) {
                        Ok(d) => v.insert(ChoiceState { decoder: Some(d), next_index: 0, emitted_calls: false }),
                        Err(e) => { yield Err(reject(&e)); return; }
                    },
                };
                if let Some(native) = &choice.delta.tool_calls {
                    st.next_index = native.iter().map(|t| t.index + 1).fold(st.next_index, i64::max);
                }
                let mut calls = Vec::new();
                if let (Some(content), Some(decoder)) = (choice.delta.content.take(), st.decoder.as_mut()) {
                    match decoder.push(&content) {
                        Ok(done) => calls = done,
                        Err(e) => { yield Err(reject(&e)); return; }
                    }
                    let prose = decoder.take_text();
                    choice.delta.content = (!prose.is_empty()).then_some(prose);
                }
                let mut finish = choice.finish_reason.take();
                if finish.is_some() && let Some(decoder) = st.decoder.take() {
                    match decoder.finish() {
                        Ok(tail) if !tail.is_empty() => {
                            choice.delta.content.get_or_insert_with(String::new).push_str(&tail);
                        }
                        Ok(_) => {}
                        Err(e) => { yield Err(reject(&e)); return; }
                    }
                }
                st.emitted_calls |= !calls.is_empty();
                for c in calls {
                    let delta = Delta {
                        tool_calls: Some(vec![ToolCallDelta {
                            index: st.next_index,
                            id: Some(mint_id()),
                            kind: Some("function".into()),
                            function: Some(FunctionCallDelta { name: Some(c.name), arguments: Some(c.arguments) }),
                        }]),
                        ..Default::default()
                    };
                    st.next_index += 1;
                    extra_chunks.push(choice_chunk(&template, choice.index, delta, None));
                }
                if st.emitted_calls && matches!(finish.as_deref(), Some("stop")) {
                    finish = Some("tool_calls".into());
                }
                if finish.is_some() {
                    if extra_chunks.is_empty() {
                        choice.finish_reason = finish;
                    } else {
                        // The finish must come after the calls it describes.
                        extra_chunks.push(choice_chunk(&template, choice.index, Delta::default(), finish));
                    }
                }
            }
            last = Some(template);
            yield Ok(chunk);
            for extra in extra_chunks {
                yield Ok(extra);
            }
        }
        // A stream that ends without a finish_reason must still not hide an unterminated call
        // or swallow a held-back text fragment.
        let mut unfinished: Vec<i64> = states.iter().filter(|(_, s)| s.decoder.is_some()).map(|(i, _)| *i).collect();
        unfinished.sort_unstable();
        for index in unfinished {
            let Some(decoder) = states.get_mut(&index).and_then(|s| s.decoder.take()) else { continue };
            match decoder.finish() {
                Ok(tail) if !tail.is_empty() => {
                    if let Some(like) = &last {
                        yield Ok(choice_chunk(like, index, Delta { content: Some(tail), ..Default::default() }, None));
                    }
                }
                Ok(_) => {}
                Err(e) => { yield Err(reject(&e)); return; }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn on() -> GatewayConfig {
        GatewayConfig {
            compact_tools_enabled: true,
            ..Default::default()
        }
    }

    fn calendar() -> Value {
        json!({"type": "function", "function": {
            "name": "create_calendar_event",
            "description": "Create an event in the user's calendar.",
            "parameters": {"type": "object", "properties": {
                "title": {"type": "string", "description": "Event title"},
                "start": {"type": "string", "format": "date-time"},
                "duration_min": {"type": "integer"},
                "visibility": {"type": "string", "enum": ["public", "private"]}},
                "required": ["title", "start"]}}})
    }

    /// A tool whose schema has no exact compact form (`minLength`).
    fn odd() -> Value {
        json!({"type": "function", "function": {"name": "odd", "parameters": {
            "type": "object", "properties": {"x": {"type": "string", "minLength": 2}}}}})
    }

    fn request(tools: Vec<Value>, extra: Value) -> ChatRequest {
        let mut body = json!({
            "model": "m",
            "messages": [{"role": "user", "content": "book it"}],
            "tools": tools,
        });
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().cloned().unwrap_or_default());
        serde_json::from_value(body).unwrap()
    }

    fn response(content: Option<&str>) -> ChatResponse {
        serde_json::from_value(json!({
            "id": "r", "model": "m",
            "choices": [{"index": 0, "finish_reason": "stop",
                "message": {"role": "assistant", "content": content}}]
        }))
        .unwrap()
    }

    fn plan_for(tools: Vec<Value>) -> Plan {
        apply(&mut request(tools, json!({})), &on()).expect("compactable")
    }

    // ── request side ────────────────────────────────────────────────────────────────────

    #[test]
    fn disabled_leaves_the_request_byte_identical() {
        let mut req = request(vec![calendar()], json!({"tool_choice": "auto"}));
        let before = serde_json::to_string(&req).unwrap();
        assert_eq!(
            apply(&mut req, &GatewayConfig::default()).unwrap_err(),
            Skipped::Disabled
        );
        assert_eq!(serde_json::to_string(&req).unwrap(), before);
        assert!(
            !GatewayConfig::default().compact_tools_enabled,
            "default must be off"
        );
    }

    #[test]
    fn enabled_replaces_tools_with_a_compact_system_message() {
        let mut req = request(vec![calendar()], json!({"tool_choice": "auto"}));
        let plan = apply(&mut req, &on()).unwrap();
        assert_eq!(plan.tools.len(), 1);
        assert!(req.tools.is_none(), "compacted tools must leave `tools`");
        assert!(
            req.tool_choice.is_none(),
            "`auto` without tools would be rejected upstream"
        );
        assert_eq!(req.messages.len(), 2);
        let last = req.messages.last().unwrap();
        assert_eq!(last.role, "system");
        let text = last.text().unwrap();
        assert!(
            text.starts_with("create_calendar_event(title:str"),
            "{text}"
        );
        assert!(text.contains("<<call name {json args}>>"));
        assert_eq!(
            req.messages[0].text().as_deref(),
            Some("book it"),
            "author text untouched"
        );
        let wire = serde_json::to_string(&req).unwrap();
        assert!(
            !wire.contains("\"parameters\""),
            "the JSON schema must not be sent: {wire}"
        );
    }

    #[test]
    fn schemas_without_an_exact_compact_form_stay_native() {
        let mut req = request(vec![calendar(), odd()], json!({}));
        let plan = apply(&mut req, &on()).unwrap();
        assert_eq!(
            plan.tools.len(),
            1,
            "only the compactable tool is decodable"
        );
        let native = req.tools.as_ref().unwrap();
        assert_eq!(native.len(), 1);
        assert_eq!(native[0].function.name, "odd");
        assert_eq!(
            serde_json::to_value(&native[0]).unwrap(),
            odd(),
            "a bypassed tool must be forwarded exactly as received"
        );
        assert!(
            !req.messages
                .last()
                .unwrap()
                .text()
                .unwrap()
                .contains("odd(")
        );
    }

    #[test]
    fn tools_carrying_extra_fields_or_other_kinds_are_not_compacted() {
        // Extras live on the `ToolDef` itself (`FunctionDef` has none: the IR already drops
        // unknown `function.*` fields at parse time, with or without compaction).
        let mut with_extra = calendar();
        with_extra["cache_control"] = json!({"type": "ephemeral"});
        let mut strict = calendar();
        strict["strict"] = json!(true);
        let mut other = calendar();
        other["type"] = json!("custom");
        for t in [with_extra, strict, other] {
            let mut req = request(vec![t], json!({}));
            let before = serde_json::to_string(&req).unwrap();
            assert_eq!(
                apply(&mut req, &on()).unwrap_err(),
                Skipped::NothingCompactable
            );
            assert_eq!(serde_json::to_string(&req).unwrap(), before);
        }
    }

    #[test]
    fn requests_that_cannot_be_compacted_safely_are_left_untouched() {
        let history = json!({"messages": [
            {"role": "user", "content": "go"},
            {"role": "assistant", "content": null, "tool_calls": [
                {"id": "call_1", "type": "function", "function": {"name": "create_calendar_event", "arguments": "{}"}}]},
            {"role": "tool", "tool_call_id": "call_1", "content": "done"}]});
        let cases = [
            (request(vec![], json!({})), Skipped::NoTools),
            (
                request(vec![calendar()], json!({"tool_choice": "required"})),
                Skipped::ToolChoice,
            ),
            (
                request(vec![calendar()], json!({"tool_choice": "none"})),
                Skipped::ToolChoice,
            ),
            (
                request(
                    vec![calendar()],
                    json!({"tool_choice": {"type": "function", "function": {"name": "create_calendar_event"}}}),
                ),
                Skipped::ToolChoice,
            ),
            (request(vec![calendar()], history), Skipped::ToolHistory),
            (
                request(vec![calendar(), calendar()], json!({})),
                Skipped::DuplicateNames,
            ),
            (request(vec![odd()], json!({})), Skipped::NothingCompactable),
        ];
        for (mut req, want) in cases {
            let before = serde_json::to_string(&req).unwrap();
            assert_eq!(apply(&mut req, &on()).unwrap_err(), want);
            assert_eq!(
                serde_json::to_string(&req).unwrap(),
                before,
                "{want:?} must not mutate"
            );
        }
    }

    // ── response side ───────────────────────────────────────────────────────────────────

    const CALL: &str = r#"<<call create_calendar_event {"title":"Review","start":"2026-10-05T15:00:00+05:30","visibility":"public"}>>"#;

    #[test]
    fn a_compact_call_becomes_a_normal_tool_call() {
        let plan = plan_for(vec![calendar()]);
        let mut resp = response(Some(&format!("Booking it.\n{CALL}\n")));
        decode_response(&mut resp, &plan).unwrap();
        let choice = &resp.choices[0];
        let calls = choice.message.tool_calls.as_ref().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].id.starts_with("call_") && calls[0].id.len() > 10);
        assert_eq!(calls[0].kind, "function");
        assert_eq!(calls[0].function.name, "create_calendar_event");
        assert_eq!(
            serde_json::from_str::<Value>(&calls[0].function.arguments).unwrap(),
            json!({"title": "Review", "start": "2026-10-05T15:00:00+05:30", "visibility": "public"})
        );
        assert_eq!(
            choice.message.text().as_deref(),
            Some("Booking it."),
            "markers must not leak"
        );
        assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn a_call_only_reply_has_no_content() {
        let plan = plan_for(vec![calendar()]);
        let mut resp = response(Some(CALL));
        decode_response(&mut resp, &plan).unwrap();
        assert!(resp.choices[0].message.content.is_none());
    }

    #[test]
    fn multiple_calls_get_distinct_ids() {
        let plan = plan_for(vec![calendar()]);
        let mut resp = response(Some(&format!("{CALL} and {CALL}")));
        decode_response(&mut resp, &plan).unwrap();
        let calls = resp.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(calls.len(), 2);
        assert_ne!(calls[0].id, calls[1].id);
    }

    #[test]
    fn plain_answers_are_left_untouched() {
        let plan = plan_for(vec![calendar()]);
        for content in [Some("It is sunny."), Some("a << b >> c <<callous>>"), None] {
            let mut resp = response(content);
            let before = serde_json::to_string(&resp).unwrap();
            decode_response(&mut resp, &plan).unwrap();
            assert_eq!(serde_json::to_string(&resp).unwrap(), before);
        }
    }

    #[test]
    fn invalid_compact_calls_are_rejected_not_repaired() {
        let plan = plan_for(vec![calendar()]);
        let cases = [
            (r#"<<call delete_everything {}>>"#, "unknown_tool"),
            (
                r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00Z"}>>"#,
                "invalid_arguments",
            ),
            (
                r#"<<call create_calendar_event {"title":"t","start":"2026-10-05T15:00:00Z","duration_min":"30"}>>"#,
                "invalid_arguments",
            ),
            (
                r#"<<call create_calendar_event {"title":"t","start":"2026-10-05T15:00:00Z","visibility":"secret"}>>"#,
                "invalid_arguments",
            ),
            (
                r#"<<call create_calendar_event {"title":"t","start":"2026-10-05T15:00:00Z","extra":1}>>"#,
                "invalid_arguments",
            ),
            (
                r#"<<call create_calendar_event {"title":"t","start":"tomorrow"}>>"#,
                "invalid_arguments",
            ),
            (
                r#"<<call create_calendar_event {"title":"t","start":"2026-10-05T15:00:00Z""#,
                "malformed_call",
            ),
        ];
        for (text, code) in cases {
            let mut resp = response(Some(&format!("ok {text}")));
            let err = decode_response(&mut resp, &plan).unwrap_err();
            assert!(err.contains(code), "{text}: {err}");
            assert!(
                resp.choices[0].message.tool_calls.is_none(),
                "no partial result may survive"
            );
        }
    }

    #[test]
    fn a_compact_call_to_a_native_only_tool_is_unknown() {
        let plan = plan_for(vec![calendar(), odd()]);
        let mut resp = response(Some(r#"<<call odd {"x":"ab"}>>"#));
        assert!(
            decode_response(&mut resp, &plan)
                .unwrap_err()
                .contains("unknown_tool")
        );
    }

    #[test]
    fn native_tool_calls_from_the_provider_are_kept() {
        let plan = plan_for(vec![calendar(), odd()]);
        let mut resp: ChatResponse = serde_json::from_value(json!({
            "id": "r", "model": "m",
            "choices": [{"index": 0, "finish_reason": "tool_calls", "message": {
                "role": "assistant", "content": CALL,
                "tool_calls": [{"id": "call_native", "type": "function",
                    "function": {"name": "odd", "arguments": "{\"x\":\"ab\"}"}}]}}]
        }))
        .unwrap();
        decode_response(&mut resp, &plan).unwrap();
        let calls = resp.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call_native");
        assert_eq!(calls[1].function.name, "create_calendar_event");
    }

    // ── streaming ───────────────────────────────────────────────────────────────────────

    fn chunk(content: Option<&str>, finish: Option<&str>) -> ChatChunk {
        serde_json::from_value(json!({
            "id": "c", "model": "m",
            "choices": [{"index": 0, "finish_reason": finish,
                "delta": content.map(|c| json!({"content": c})).unwrap_or(json!({}))}]
        }))
        .unwrap()
    }

    async fn run(chunks: Vec<ChatChunk>) -> Vec<Result<ChatChunk, ProviderError>> {
        let inner = futures::stream::iter(chunks.into_iter().map(Ok)).boxed();
        decode_stream(inner, plan_for(vec![calendar()]))
            .collect()
            .await
    }

    fn text_of(items: &[Result<ChatChunk, ProviderError>]) -> String {
        items
            .iter()
            .filter_map(|i| i.as_ref().ok())
            .flat_map(|c| c.choices.iter())
            .filter_map(|c| c.delta.content.clone())
            .collect()
    }

    fn calls_of(items: &[Result<ChatChunk, ProviderError>]) -> Vec<ToolCallDelta> {
        items
            .iter()
            .filter_map(|i| i.as_ref().ok())
            .flat_map(|c| c.choices.iter())
            .flat_map(|c| c.delta.tool_calls.clone().unwrap_or_default())
            .collect()
    }

    #[tokio::test]
    async fn a_marker_split_across_chunks_streams_as_one_tool_call() {
        let parts = [
            "Sure. <<ca",
            "ll create_calendar_event {\"title\":\"Rev",
            "iew\",\"start\":\"2026-10-05T15:00:00+05:30\"}>",
            ">",
        ];
        let mut chunks: Vec<_> = parts.iter().map(|p| chunk(Some(p), None)).collect();
        chunks.push(chunk(None, Some("stop")));
        let items = run(chunks).await;
        assert!(items.iter().all(Result::is_ok), "{items:?}");
        assert_eq!(
            text_of(&items),
            "Sure. ",
            "no marker text may leak, and no text may be lost"
        );
        let calls = calls_of(&items);
        assert_eq!(calls.len(), 1);
        let f = calls[0].function.as_ref().unwrap();
        assert_eq!(f.name.as_deref(), Some("create_calendar_event"));
        assert_eq!(
            serde_json::from_str::<Value>(f.arguments.as_deref().unwrap()).unwrap(),
            json!({"title": "Review", "start": "2026-10-05T15:00:00+05:30"})
        );
        assert!(calls[0].id.as_deref().unwrap().starts_with("call_"));
        let last = items.last().unwrap().as_ref().unwrap();
        assert_eq!(last.choices[0].finish_reason.as_deref(), Some("tool_calls"));
        let finishes = items
            .iter()
            .filter_map(|i| i.as_ref().ok())
            .filter(|c| c.choices.iter().any(|c| c.finish_reason.is_some()))
            .count();
        assert_eq!(finishes, 1, "exactly one finish chunk, after the call");
    }

    #[tokio::test]
    async fn plain_streams_pass_through_unchanged() {
        let items = run(vec![
            chunk(Some("It is "), None),
            chunk(Some("sunny."), None),
            chunk(None, Some("stop")),
        ])
        .await;
        assert_eq!(text_of(&items), "It is sunny.");
        assert!(calls_of(&items).is_empty());
        let last = items.last().unwrap().as_ref().unwrap();
        assert_eq!(last.choices[0].finish_reason.as_deref(), Some("stop"));
    }

    #[tokio::test]
    async fn a_held_back_partial_marker_is_flushed_as_text() {
        let items = run(vec![chunk(Some("a <<ca"), None), chunk(None, Some("stop"))]).await;
        assert_eq!(text_of(&items), "a <<ca");
        assert!(calls_of(&items).is_empty());
    }

    #[tokio::test]
    async fn an_invalid_streamed_call_ends_the_stream_with_an_error() {
        let items = run(vec![
            chunk(Some("<<call delete_everything {}>>"), None),
            chunk(Some("more text"), None),
            chunk(None, Some("stop")),
        ])
        .await;
        assert!(
            matches!(items.last(), Some(Err(ProviderError::Parse(m))) if m.contains("unknown_tool")),
            "{items:?}"
        );
        assert!(calls_of(&items).is_empty());
        assert!(
            !text_of(&items).contains("more text"),
            "nothing after the rejection is forwarded"
        );
    }

    #[tokio::test]
    async fn an_unterminated_streamed_call_is_an_error_not_silence() {
        let items = run(vec![
            chunk(Some("<<call create_calendar_event {\"title\":\"x\""), None),
            chunk(None, Some("stop")),
        ])
        .await;
        assert!(
            matches!(items.last(), Some(Err(ProviderError::Parse(m))) if m.contains("malformed_call")),
            "{items:?}"
        );
        assert!(calls_of(&items).is_empty());
    }

    #[tokio::test]
    async fn provider_errors_pass_straight_through() {
        let inner = futures::stream::iter(vec![
            Ok(chunk(Some("hi"), None)),
            Err(ProviderError::Transport("boom".into())),
        ])
        .boxed();
        let items: Vec<_> = decode_stream(inner, plan_for(vec![calendar()]))
            .collect()
            .await;
        assert!(matches!(
            items.last(),
            Some(Err(ProviderError::Transport(_)))
        ));
    }
}
