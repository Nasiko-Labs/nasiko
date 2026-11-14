//! OpenAI Responses wire format to/from the canonical Chat IR.
//!
//! Cross-provider translation is intentionally lossy. `reasoning` and `include`
//! are advisory and ignored; custom grammars are described to the provider but
//! not enforced; reasoning items and encrypted reasoning are never synthesized.
//! Controls that cannot be represented safely are rejected explicitly.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::error::GatewayError;
use crate::ir::{
    ChatChunk, ChatRequest, ChatResponse, FunctionCall, FunctionDef, Message, ToolCall, ToolDef,
    Usage,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolKind {
    Function,
    Custom,
}

#[derive(Clone, Debug)]
pub struct ResponsesRequest {
    pub chat: ChatRequest,
    pub stream: bool,
    pub tool_kinds: HashMap<String, ToolKind>,
    pub advisory_fields: Vec<&'static str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalOutcome {
    Completed,
    Incomplete,
    Failed,
}

pub fn parse_request(body: &Value) -> Result<ResponsesRequest, GatewayError> {
    let object = body
        .as_object()
        .ok_or_else(|| bad("request must be a JSON object"))?;
    reject_top_level(object)?;

    let stream = optional_bool(object, "stream")?.unwrap_or(false);
    let mut messages = Vec::new();
    if let Some(instructions) = object.get("instructions").filter(|v| !v.is_null()) {
        let text = instructions
            .as_str()
            .ok_or_else(|| bad("instructions must be a string"))?;
        messages.push(message("system", Some(Value::String(text.to_string()))));
    }
    match object.get("input") {
        None | Some(Value::Null) => {}
        Some(Value::String(text)) => {
            messages.push(message("user", Some(Value::String(text.clone()))))
        }
        Some(Value::Array(items)) => parse_input(items, &mut messages)?,
        Some(_) => return Err(bad("input must be a string or an array")),
    }

    let ParsedTools {
        defs: tools,
        kinds: tool_kinds,
        dropped_hosted,
    } = parse_tools(object.get("tools"))?;
    let tool_choice = parse_tool_choice(object.get("tool_choice"))?;
    let temperature = optional_number(object, "temperature")?;
    let max_tokens = optional_i64(object, "max_output_tokens")?;
    let mut advisory_fields = Vec::new();
    for field in ["reasoning", "include"] {
        if object.get(field).is_some_and(|value| !value.is_null()) {
            advisory_fields.push(field);
        }
    }
    if has_custom_grammar(object.get("tools")) {
        advisory_fields.push("custom_tool.format.grammar");
    }
    if dropped_hosted {
        advisory_fields.push(HOSTED_TOOLS_ADVISORY);
    }
    Ok(ResponsesRequest {
        chat: ChatRequest {
            model: object
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string),
            messages,
            tools: (!tools.is_empty()).then_some(tools),
            tool_choice,
            // `reject_top_level` above already rejects `parallel_tool_calls: false`
            // outright (it's unsupported for cross-provider Responses), so by this
            // point the only valid values are absent or `true` — the IR default.
            parallel_tool_calls: None,
            temperature,
            max_tokens,
            stream: Some(stream),
            extra: Map::new(),
        },
        stream,
        tool_kinds,
        advisory_fields,
    })
}

/// Fields the parser knows about. Anything else is rejected rather than dropped, so a control
/// that would silently change the answer can't slip through — but a field that *cannot* reach
/// generation belongs here and is then simply ignored, as `metadata`, `user` and
/// `safety_identifier` already are. `client_metadata` is that kind: Codex attaches its own
/// CLI/terminal telemetry to every request, and rejecting it fails the whole session over a
/// field no provider would have read.
fn reject_top_level(object: &Map<String, Value>) -> Result<(), GatewayError> {
    const ACCEPTED_FIELDS: &[&str] = &[
        "background",
        "client_metadata",
        "compaction",
        "context_management",
        "conversation",
        "include",
        "input",
        "instructions",
        "max_output_tokens",
        "max_tool_calls",
        "metadata",
        "model",
        "parallel_tool_calls",
        "previous_response_id",
        "prompt",
        "prompt_cache_key",
        "prompt_cache_retention",
        "reasoning",
        "safety_identifier",
        "service_tier",
        "store",
        "stream",
        "stream_options",
        "temperature",
        "text",
        "tool_choice",
        "tools",
        "top_logprobs",
        "top_p",
        "truncation",
        "user",
    ];
    if let Some(key) = object
        .keys()
        .find(|key| !ACCEPTED_FIELDS.contains(&key.as_str()))
    {
        return Err(bad(&format!(
            "{key} is not supported for cross-provider Responses"
        )));
    }
    for key in [
        "context_management",
        "compaction",
        "previous_response_id",
        "conversation",
    ] {
        if object.get(key).is_some_and(|v| !v.is_null()) {
            return Err(bad(&format!(
                "{key} is not supported for cross-provider Responses"
            )));
        }
    }
    for (key, rejected) in [
        ("background", true),
        ("parallel_tool_calls", false),
        ("store", true),
    ] {
        let Some(value) = object.get(key).filter(|value| !value.is_null()) else {
            continue;
        };
        let value = value
            .as_bool()
            .ok_or_else(|| bad(&format!("{key} must be a boolean")))?;
        if value == rejected {
            return Err(bad(&format!(
                "{key} is not supported for cross-provider Responses"
            )));
        }
    }
    for key in [
        "logprobs",
        "max_tool_calls",
        "prompt",
        "seed",
        "stream_options",
        "top_logprobs",
        "top_p",
    ] {
        if object.get(key).is_some_and(|v| !v.is_null()) {
            return Err(bad(&format!(
                "{key} is not supported for cross-provider Responses"
            )));
        }
    }
    if object
        .get("truncation")
        .is_some_and(|value| !value.is_null() && value.as_str() != Some("disabled"))
    {
        return Err(bad(
            "truncation is not supported for cross-provider Responses",
        ));
    }
    if object
        .get("text")
        .and_then(|value| value.get("verbosity"))
        .is_some_and(|value| !value.is_null())
    {
        return Err(bad(
            "text.verbosity is not supported for cross-provider Responses",
        ));
    }
    if object
        .get("text")
        .and_then(|v| v.get("format"))
        .is_some_and(|v| !v.is_null() && v.get("type").and_then(Value::as_str) != Some("text"))
    {
        return Err(bad(
            "text.format structured output is not supported for cross-provider Responses",
        ));
    }
    // `reasoning` and `include` are accepted only as Codex compatibility hints. They
    // are deliberately not translated and never synthesize reasoning output or
    // encrypted content for providers that did not return it.
    Ok(())
}

fn parse_input(items: &[Value], messages: &mut Vec<Message>) -> Result<(), GatewayError> {
    for item in items {
        let kind = item
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("message");
        match kind {
            "message" => {
                let role = item
                    .get("role")
                    .and_then(Value::as_str)
                    .ok_or_else(|| bad("message role is required"))?;
                let chat_role = match role {
                    "developer" => "system",
                    "user" | "assistant" => role,
                    _ => return Err(bad(&format!("unsupported message role '{role}'"))),
                };
                let text = parse_content(item.get("content"))?;
                messages.push(message(chat_role, Some(Value::String(text))));
            }
            "function_call" | "custom_tool_call" => {
                let name = required_str(item, "name", kind)?;
                let call_id = item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| bad(&format!("{kind}.call_id is required")))?;
                let arguments = if kind == "function_call" {
                    let args = required_str(item, "arguments", kind)?;
                    let parsed = serde_json::from_str::<Value>(args)
                        .map_err(|_| bad("function_call.arguments must be valid JSON"))?;
                    if !parsed.is_object() {
                        return Err(bad("function_call.arguments must encode a JSON object"));
                    }
                    args.to_string()
                } else {
                    let input = required_str(item, "input", kind)?;
                    json!({"input": input}).to_string()
                };
                let call = ToolCall {
                    id: call_id.to_string(),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: name.to_string(),
                        arguments,
                    },
                    extra: Map::new(),
                };
                if let Some(last) = messages.last_mut().filter(|m| m.role == "assistant") {
                    last.tool_calls.get_or_insert_with(Vec::new).push(call);
                } else {
                    let mut assistant = message("assistant", None);
                    assistant.tool_calls = Some(vec![call]);
                    messages.push(assistant);
                }
            }
            "function_call_output" | "custom_tool_call_output" => {
                let call_id = required_str(item, "call_id", kind)?;
                let output = item
                    .get("output")
                    .ok_or_else(|| bad(&format!("{kind}.output is required")))?;
                let text = value_text(output)?;
                let mut result = message("tool", Some(Value::String(text)));
                result.tool_call_id = Some(call_id.to_string());
                messages.push(result);
            }
            "reasoning" => {}
            "image_generation_call"
            | "computer_call"
            | "computer_call_output"
            | "local_shell_call"
            | "local_shell_call_output"
            | "shell_call"
            | "shell_call_output"
            | "mcp_call"
            | "mcp_list_tools"
            | "tool_search_call"
            | "tool_search_output"
            | "web_search_call" => {
                return Err(bad(&format!(
                    "input item type '{kind}' is not supported for cross-provider Responses"
                )));
            }
            other => {
                return Err(bad(&format!(
                    "input item type '{other}' is not supported for cross-provider Responses"
                )));
            }
        }
    }
    Ok(())
}

fn parse_content(content: Option<&Value>) -> Result<String, GatewayError> {
    let Some(content) = content else {
        return Ok(String::new());
    };
    if let Some(text) = content.as_str() {
        return Ok(text.to_string());
    }
    let parts = content
        .as_array()
        .ok_or_else(|| bad("message content must be a string or array"))?;
    let mut text = String::new();
    for part in parts {
        match part.get("type").and_then(Value::as_str) {
            Some("input_text" | "output_text" | "text") => {
                text.push_str(required_str(part, "text", "content part")?)
            }
            Some("input_image" | "output_image" | "input_audio" | "audio") => {
                return Err(bad(
                    "image and audio content is not supported for cross-provider Responses",
                ));
            }
            Some(kind) => {
                return Err(bad(&format!(
                    "content part type '{kind}' is not supported for cross-provider Responses"
                )));
            }
            None => return Err(bad("content part type is required")),
        }
    }
    Ok(text)
}

fn value_text(value: &Value) -> Result<String, GatewayError> {
    if let Some(text) = value.as_str() {
        Ok(text.to_string())
    } else {
        parse_content(Some(value))
    }
}

/// Tool types the *provider's own backend* executes. Nothing downstream can run them —
/// Bedrock and Anthropic have no equivalent — so they are dropped rather than rejected:
/// a tool that isn't advertised simply never gets called, which strands nothing, whereas
/// a 400 takes the whole turn down over a capability the model can live without. The drop
/// is reported through [`ResponsesRequest::advisory_fields`], the same way `reasoning` and
/// `include` are, so the response still carries the lossy-translation header.
const HOSTED_TOOL_TYPES: &[&str] = &[
    "web_search",
    "web_search_preview",
    "image_generation",
    "computer",
    "computer_use_preview",
    "local_shell",
    "shell",
    "mcp",
    "tool_search",
];

/// Marker pushed onto `advisory_fields` when a hosted tool was dropped.
pub(crate) const HOSTED_TOOLS_ADVISORY: &str = "tools.hosted";

/// The flat tool list a request translates to, plus whether anything was dropped reaching it.
#[derive(Default)]
struct ParsedTools {
    defs: Vec<ToolDef>,
    kinds: HashMap<String, ToolKind>,
    dropped_hosted: bool,
}

/// [`ParsedTools`] under construction. `signatures` is scratch for the duplicate check and
/// does not outlive collection.
#[derive(Default)]
struct ToolAccumulator {
    parsed: ParsedTools,
    signatures: HashMap<String, Value>,
}

fn parse_tools(value: Option<&Value>) -> Result<ParsedTools, GatewayError> {
    let Some(value) = value.filter(|v| !v.is_null()) else {
        return Ok(ParsedTools::default());
    };
    let tools = value
        .as_array()
        .ok_or_else(|| bad("tools must be an array"))?;
    let mut acc = ToolAccumulator::default();
    for tool in tools {
        collect_tool(tool, true, &mut acc)?;
    }
    Ok(acc.parsed)
}

/// Translate one entry of the `tools` array into the accumulator.
///
/// `namespaces_allowed` is false while recursing into a namespace's members, so a nested
/// namespace is reported rather than silently flattened — Codex only ever emits one level
/// (`{"type":"namespace","name":…,"description":…,"tools":[…function tools…]}`), and a
/// deeper shape would be a format we have not seen.
fn collect_tool(
    tool: &Value,
    namespaces_allowed: bool,
    acc: &mut ToolAccumulator,
) -> Result<(), GatewayError> {
    let kind = required_str(tool, "type", "tool")?;
    let tool_kind = match kind {
        "function" => ToolKind::Function,
        "custom" => ToolKind::Custom,
        // A grouping wrapper, not a tool: Codex packages a whole MCP server's tools under
        // one namespace. The chat IR and every provider's tool format are flat, so the
        // members are hoisted out and treated as ordinary function tools. Their bare names
        // are kept — that is what the model sees and calls, and the duplicate check below
        // turns a collision between two namespaces into an error rather than a mis-route.
        "namespace" if namespaces_allowed => return collect_namespace(tool, acc),
        "namespace" => {
            return Err(bad(
                "nested tool namespaces are not supported for cross-provider Responses",
            ));
        }
        hosted if HOSTED_TOOL_TYPES.contains(&hosted) => {
            tracing::info!(
                tool_type = %hosted,
                "dropping provider-hosted tool for cross-provider Responses"
            );
            acc.parsed.dropped_hosted = true;
            return Ok(());
        }
        _ => {
            // A type this parser has never seen — a newer client, or a Responses feature
            // added since. Unlike a hosted tool, we can't know it is safe to drop, so it
            // is rejected; the client only learns the name, so log the tool's structure
            // here, since that is what a translation for it has to be written against.
            // Keys only, never values — a tool's description and schema are the caller's
            // content, not ours to log.
            tracing::warn!(
                tool_type = %kind,
                fields = ?object_keys(tool),
                "unknown Responses tool type rejected"
            );
            return Err(bad(&format!(
                "tool type '{kind}' is not supported for cross-provider Responses"
            )));
        }
    };
    if tool_kind == ToolKind::Custom {
        validate_custom_format(tool.get("format"))?;
    } else if let Some(strict) = tool.get("strict").filter(|value| !value.is_null()) {
        let strict = strict
            .as_bool()
            .ok_or_else(|| bad("function tool strict must be a boolean"))?;
        if strict {
            return Err(bad(
                "strict function tools are not supported for cross-provider Responses",
            ));
        }
    }
    let name = required_str(tool, "name", "tool")?.to_string();
    if let Some(previous) = acc.signatures.get(&name) {
        if previous != tool {
            return Err(bad(&format!(
                "duplicate tool '{name}' has conflicting definitions"
            )));
        }
        return Ok(());
    }
    acc.signatures.insert(name.clone(), tool.clone());
    acc.parsed.kinds.insert(name.clone(), tool_kind);
    let parameters = if tool_kind == ToolKind::Custom {
        json!({"type":"object","properties":{"input":{"type":"string"}},"required":["input"],"additionalProperties":false})
    } else {
        tool.get("parameters")
            .cloned()
            .unwrap_or_else(|| json!({"type":"object","properties":{}}))
    };
    acc.parsed.defs.push(ToolDef {
        kind: "function".into(),
        function: FunctionDef {
            name,
            description: custom_tool_description(tool, tool_kind),
            parameters: Some(parameters),
            // `strict: true` is rejected above for function tools; `None` matches
            // every value that can actually reach this point.
            strict: None,
        },
        extra: Map::new(),
    });
    Ok(())
}

/// Hoist a namespace's member tools up to the flat tool list.
fn collect_namespace(tool: &Value, acc: &mut ToolAccumulator) -> Result<(), GatewayError> {
    let members = tool
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("tool namespace must carry a 'tools' array"))?;
    for member in members {
        collect_tool(member, false, acc)?;
    }
    Ok(())
}

fn has_custom_grammar(tools: Option<&Value>) -> bool {
    tools.and_then(Value::as_array).is_some_and(|tools| {
        tools.iter().any(|tool| {
            tool.get("type").and_then(Value::as_str) == Some("custom")
                && tool
                    .get("format")
                    .and_then(|format| format.get("type"))
                    .and_then(Value::as_str)
                    == Some("grammar")
        })
    })
}

fn custom_tool_description(tool: &Value, kind: ToolKind) -> Option<String> {
    let description = tool.get("description").and_then(Value::as_str);
    if kind != ToolKind::Custom
        || tool
            .get("format")
            .and_then(|format| format.get("type"))
            .and_then(Value::as_str)
            != Some("grammar")
    {
        return description.map(str::to_string);
    }
    let format = &tool["format"];
    let syntax = format["syntax"].as_str().expect("grammar was validated");
    let definition = format["definition"]
        .as_str()
        .expect("grammar was validated");
    Some(format!(
        "{}Input must match this {syntax} grammar:\n{definition}",
        description
            .map(|text| format!("{text}\n\n"))
            .unwrap_or_default()
    ))
}

fn validate_custom_format(format: Option<&Value>) -> Result<(), GatewayError> {
    let Some(format) = format.filter(|value| !value.is_null()) else {
        return Ok(());
    };
    match format.get("type").and_then(Value::as_str) {
        Some("text") => Ok(()),
        Some("grammar")
            if format
                .get("definition")
                .and_then(Value::as_str)
                .is_some_and(|definition| !definition.is_empty())
                && matches!(
                    format.get("syntax").and_then(Value::as_str),
                    Some("lark" | "regex")
                ) =>
        {
            Ok(())
        }
        _ => Err(bad(
            "custom tool format must be text or a non-empty lark/regex grammar",
        )),
    }
}

fn parse_tool_choice(value: Option<&Value>) -> Result<Option<Value>, GatewayError> {
    let Some(value) = value.filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    if value.is_string() {
        return match value.as_str() {
            Some("auto" | "required") => Ok(Some(value.clone())),
            Some("none") => Err(bad(
                "tool_choice 'none' is not supported for cross-provider Responses",
            )),
            _ => Err(bad("unsupported tool_choice value")),
        };
    }
    if !matches!(
        value.get("type").and_then(Value::as_str),
        Some("function" | "custom")
    ) {
        return Err(bad(
            "tool_choice type must be function or custom for cross-provider Responses",
        ));
    }
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("tool_choice.name is required"))?;
    Ok(Some(json!({"type":"function","function":{"name":name}})))
}

pub fn render_response(
    resp: ChatResponse,
    model: &str,
    tool_kinds: &HashMap<String, ToolKind>,
) -> Result<Value, GatewayError> {
    let id = response_id();
    let finish_reason = resp
        .choices
        .first()
        .and_then(|choice| choice.finish_reason.as_deref());
    let incomplete = finish_reason.is_some_and(|reason| matches!(reason, "length" | "max_tokens"));
    if finish_reason == Some("content_filter") || finish_reason.is_none() {
        return Ok(failed_response(
            &id,
            model,
            resp.usage.as_ref(),
            "content_filter",
            "provider did not return a successful finish reason",
        ));
    }
    let output = match response_output(&resp, tool_kinds, incomplete) {
        Ok(output) => output,
        Err(error) => {
            return Ok(failed_response(
                &id,
                model,
                resp.usage.as_ref(),
                "invalid_tool_call",
                &error.to_string(),
            ));
        }
    };
    Ok(terminal_response(
        &id,
        model,
        output,
        resp.usage.as_ref(),
        resp.created,
        incomplete,
    ))
}

fn response_output(
    resp: &ChatResponse,
    tool_kinds: &HashMap<String, ToolKind>,
    incomplete: bool,
) -> Result<Vec<Value>, GatewayError> {
    let Some(choice) = resp.choices.first() else {
        return Ok(Vec::new());
    };
    let status = if choice
        .finish_reason
        .as_deref()
        .is_some_and(|reason| matches!(reason, "length" | "max_tokens"))
    {
        "incomplete"
    } else {
        "completed"
    };
    let mut output = Vec::new();
    if let Some(text) = choice.message.text().filter(|t| !t.is_empty()) {
        output.push(message_item(
            &item_id("msg"),
            status,
            vec![json!({"type":"output_text","annotations":[],"logprobs":[],"text":text})],
        ));
    }
    if let Some(calls) = &choice.message.tool_calls {
        for call in calls {
            if let Some(item) = call_item(call, tool_kinds, status, incomplete)? {
                output.push(item);
            }
        }
    }
    Ok(output)
}

fn call_item(
    call: &ToolCall,
    kinds: &HashMap<String, ToolKind>,
    status: &str,
    incomplete: bool,
) -> Result<Option<Value>, GatewayError> {
    if kinds.get(&call.function.name) == Some(&ToolKind::Custom) {
        let input = match parse_arguments_object(&call.function.arguments) {
            Ok(arguments) => arguments
                .get("input")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| bad("custom tool arguments must contain a string input"))?,
            Err(_) if incomplete => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok(Some(
            json!({"type":"custom_tool_call","id":item_id("ctc"),"call_id":call.id,"name":call.function.name,"input":input,"status":status}),
        ))
    } else {
        if !incomplete {
            parse_arguments_object(&call.function.arguments)?;
        }
        Ok(Some(
            json!({"type":"function_call","id":item_id("fc"),"call_id":call.id,"name":call.function.name,"arguments":call.function.arguments,"status":status}),
        ))
    }
}

fn failed_response(
    id: &str,
    model: &str,
    usage: Option<&Usage>,
    code: &str,
    message: &str,
) -> Value {
    let mut response = terminal_response(id, model, Vec::new(), usage, None, false);
    response["status"] = json!("failed");
    response["error"] = json!({"code":code,"message":message});
    response
}

fn parse_arguments_object(arguments: &str) -> Result<Value, GatewayError> {
    let value = serde_json::from_str::<Value>(arguments)
        .map_err(|_| bad("function call arguments must be valid JSON"))?;
    if !value.is_object() {
        return Err(bad("function call arguments must encode a JSON object"));
    }
    Ok(value)
}

fn terminal_response(
    id: &str,
    model: &str,
    output: Vec<Value>,
    usage: Option<&Usage>,
    created: Option<i64>,
    incomplete: bool,
) -> Value {
    let input = usage.and_then(|u| u.prompt_tokens).unwrap_or(0);
    let output_tokens = usage.and_then(|u| u.completion_tokens).unwrap_or(0);
    let total = usage
        .and_then(|u| u.total_tokens)
        .unwrap_or(input + output_tokens);
    json!({
        "id":id,"object":"response","created_at":created.unwrap_or_else(crate::providers::now_unix),
        "status":if incomplete {"incomplete"} else {"completed"},"error":Value::Null,
        "incomplete_details":if incomplete {json!({"reason":"max_output_tokens"})} else {Value::Null},"instructions":Value::Null,
        "model":model,"output":output,"parallel_tool_calls":true,"previous_response_id":Value::Null,
        "reasoning":{"effort":Value::Null,"summary":Value::Null},"store":false,
        "text":{"format":{"type":"text"}},"tool_choice":"auto","tools":[],"truncation":"disabled",
        "usage":{"input_tokens":input,"input_tokens_details":{"cached_tokens":0},"output_tokens":output_tokens,
            "output_tokens_details":{"reasoning_tokens":0},"total_tokens":total}
    })
}

fn message_item(id: &str, status: &str, content: Vec<Value>) -> Value {
    json!({"type":"message","id":id,"status":status,"role":"assistant","content":content})
}

#[derive(Debug)]
struct StreamCall {
    id: String,
    call_id: String,
    name: String,
    kind: ToolKind,
    arguments: String,
    output_index: usize,
}

pub struct ResponsesStreamRenderer {
    response_id: String,
    model: String,
    sequence: u64,
    output: Vec<Value>,
    text: String,
    text_item_id: Option<String>,
    text_output_index: Option<usize>,
    calls: BTreeMap<i64, StreamCall>,
    usage: Option<Usage>,
    finish_reason: Option<String>,
    terminal: bool,
    outcome: Option<TerminalOutcome>,
    tool_kinds: HashMap<String, ToolKind>,
    next_output_index: usize,
}

impl ResponsesStreamRenderer {
    pub fn new(model: String, tool_kinds: HashMap<String, ToolKind>) -> Self {
        Self {
            response_id: response_id(),
            model,
            sequence: 0,
            output: Vec::new(),
            text: String::new(),
            text_item_id: None,
            text_output_index: None,
            calls: BTreeMap::new(),
            usage: None,
            finish_reason: None,
            terminal: false,
            outcome: None,
            tool_kinds,
            next_output_index: 0,
        }
    }

    pub fn start(&mut self) -> Vec<String> {
        let response = terminal_response(&self.response_id, &self.model, vec![], None, None, false);
        let mut in_progress = response;
        in_progress["status"] = json!("in_progress");
        in_progress["usage"] = Value::Null;
        vec![
            self.event("response.created", json!({"response":in_progress})),
            self.event("response.in_progress", json!({"response":in_progress})),
        ]
    }

    pub fn render(&mut self, chunk: ChatChunk) -> Vec<String> {
        if self.terminal {
            return Vec::new();
        }
        if chunk.usage.is_some() {
            self.usage = chunk.usage.clone();
        }
        let mut frames = Vec::new();
        for choice in chunk.choices {
            if choice.finish_reason.is_some() {
                self.finish_reason = choice.finish_reason.clone();
            }
            if let Some(text) = choice.delta.content.filter(|s| !s.is_empty()) {
                self.open_text(&mut frames);
                self.text.push_str(&text);
                frames.push(self.event("response.output_text.delta", json!({"item_id":self.text_item_id,"output_index":self.text_output_index,"content_index":0,"delta":text,"logprobs":[]})));
            }
            for delta in choice.delta.tool_calls.unwrap_or_default() {
                let function = delta.function.unwrap_or_default();
                if !self.calls.contains_key(&delta.index) {
                    let name = function.name.clone().unwrap_or_default();
                    let kind = self
                        .tool_kinds
                        .get(&name)
                        .copied()
                        .unwrap_or(ToolKind::Function);
                    let output_index = if kind == ToolKind::Custom {
                        usize::MAX
                    } else {
                        let index = self.next_output_index;
                        self.next_output_index += 1;
                        index
                    };
                    let call = StreamCall {
                        id: item_id(if kind == ToolKind::Custom {
                            "ctc"
                        } else {
                            "fc"
                        }),
                        call_id: delta.id.clone().unwrap_or_else(|| item_id("call")),
                        name,
                        kind,
                        arguments: String::new(),
                        output_index,
                    };
                    if kind == ToolKind::Function {
                        let item = stream_call_item(&call, "in_progress", "");
                        frames.push(self.event(
                            "response.output_item.added",
                            json!({"output_index":output_index,"item":item}),
                        ));
                    }
                    self.calls.insert(delta.index, call);
                }
                let fragment = function.arguments.unwrap_or_default();
                if !fragment.is_empty() {
                    let call = self.calls.get_mut(&delta.index).expect("inserted above");
                    call.arguments.push_str(&fragment);
                    if call.kind == ToolKind::Function {
                        let item_id = call.id.clone();
                        let output_index = call.output_index;
                        frames.push(self.event(
                            "response.function_call_arguments.delta",
                            json!({"item_id":item_id,"output_index":output_index,"delta":fragment}),
                        ));
                    }
                }
            }
        }
        frames
    }

    pub fn finish(&mut self) -> Vec<String> {
        let incomplete = self
            .finish_reason
            .as_deref()
            .is_some_and(|reason| matches!(reason, "length" | "max_tokens"));
        if self.finish_reason.as_deref() == Some("content_filter") || self.finish_reason.is_none() {
            return self.terminal(
                true,
                false,
                Some("provider did not return a successful finish reason".into()),
                Some("content_filter"),
            );
        }
        self.terminal(false, incomplete, None, None)
    }

    pub fn fail(&mut self, message: String) -> Vec<String> {
        self.terminal(true, false, Some(message), Some("upstream_error"))
    }

    pub fn outcome(&self) -> Option<TerminalOutcome> {
        self.outcome
    }

    fn open_text(&mut self, frames: &mut Vec<String>) {
        if self.text_item_id.is_some() {
            return;
        }
        let id = item_id("msg");
        let output_index = self.next_output_index;
        self.next_output_index += 1;
        self.text_item_id = Some(id.clone());
        self.text_output_index = Some(output_index);
        frames.push(self.event(
            "response.output_item.added",
            json!({"output_index":output_index,"item":message_item(&id,"in_progress",vec![])}),
        ));
        frames.push(self.event("response.content_part.added", json!({"item_id":id,"output_index":output_index,"content_index":0,"part":{"type":"output_text","annotations":[],"logprobs":[],"text":""}})));
    }

    fn terminal(
        &mut self,
        failed: bool,
        incomplete: bool,
        error: Option<String>,
        error_code: Option<&str>,
    ) -> Vec<String> {
        if self.terminal {
            return Vec::new();
        }
        self.terminal = true;
        self.outcome = Some(if failed {
            TerminalOutcome::Failed
        } else if incomplete {
            TerminalOutcome::Incomplete
        } else {
            TerminalOutcome::Completed
        });
        let mut frames = Vec::new();
        if failed {
            let mut response = terminal_response(
                &self.response_id,
                &self.model,
                Vec::new(),
                self.usage.as_ref(),
                None,
                false,
            );
            response["status"] = json!("failed");
            response["error"] = json!({"code":error_code.unwrap_or("upstream_error"),"message":error.unwrap_or_else(|| "provider stream failed".into())});
            frames.push(self.event("response.failed", json!({"response":response})));
            return frames;
        }
        let call_values = match self.validated_call_values(incomplete) {
            Ok(values) => values,
            Err(error) => {
                let mut response = terminal_response(
                    &self.response_id,
                    &self.model,
                    Vec::new(),
                    self.usage.as_ref(),
                    None,
                    false,
                );
                response["status"] = json!("failed");
                response["error"] = json!({"code":"invalid_tool_call","message":error.to_string()});
                self.outcome = Some(TerminalOutcome::Failed);
                frames.push(self.event("response.failed", json!({"response":response})));
                return frames;
            }
        };
        let mut completed_items = Vec::new();
        if let Some(id) = self.text_item_id.clone() {
            let output_index = self.text_output_index.expect("text item has an index");
            let text = self.text.clone();
            frames.push(self.event(
                "response.output_text.done",
                json!({"item_id":id,"output_index":output_index,"content_index":0,"text":text,"logprobs":[]}),
            ));
            let part = json!({"type":"output_text","annotations":[],"logprobs":[],"text":text});
            frames.push(self.event(
                "response.content_part.done",
                json!({"item_id":id,"output_index":output_index,"content_index":0,"part":part}),
            ));
            let item = message_item(
                &id,
                if incomplete {
                    "incomplete"
                } else {
                    "completed"
                },
                vec![part],
            );
            frames.push(self.event(
                "response.output_item.done",
                json!({"output_index":output_index,"item":item}),
            ));
            completed_items.push((output_index, item));
        }
        let indexes: Vec<i64> = self.calls.keys().copied().collect();
        for (index, value) in indexes.into_iter().zip(call_values) {
            let mut call = self.calls.remove(&index).expect("known call");
            let Some(value) = value else {
                continue;
            };
            if call.kind == ToolKind::Custom {
                call.output_index = self.next_output_index;
                self.next_output_index += 1;
                let item = stream_call_item(&call, "in_progress", "");
                frames.push(self.event(
                    "response.output_item.added",
                    json!({"output_index":call.output_index,"item":item}),
                ));
            }
            let (event, value) = if call.kind == ToolKind::Custom {
                let input = value;
                if !input.is_empty() {
                    frames.push(self.event(
                        "response.custom_tool_call_input.delta",
                        json!({"item_id":call.id,"output_index":call.output_index,"delta":input}),
                    ));
                }
                ("response.custom_tool_call_input.done", input)
            } else {
                (
                    "response.function_call_arguments.done",
                    call.arguments.clone(),
                )
            };
            let done = if call.kind == ToolKind::Custom {
                json!({"item_id":call.id,"output_index":call.output_index,"input":value})
            } else {
                json!({"item_id":call.id,"output_index":call.output_index,"arguments":value})
            };
            frames.push(self.event(event, done));
            let item = stream_call_item(
                &call,
                if incomplete {
                    "incomplete"
                } else {
                    "completed"
                },
                &value,
            );
            frames.push(self.event(
                "response.output_item.done",
                json!({"output_index":call.output_index,"item":item}),
            ));
            completed_items.push((call.output_index, item));
        }
        completed_items.sort_by_key(|(index, _)| *index);
        self.output = completed_items.into_iter().map(|(_, item)| item).collect();
        let response = terminal_response(
            &self.response_id,
            &self.model,
            self.output.clone(),
            self.usage.as_ref(),
            None,
            incomplete,
        );
        if incomplete {
            frames.push(self.event("response.incomplete", json!({"response":response})));
        } else {
            frames.push(self.event("response.completed", json!({"response":response})));
        }
        frames
    }

    fn validated_call_values(&self, incomplete: bool) -> Result<Vec<Option<String>>, GatewayError> {
        self.calls
            .values()
            .map(|call| {
                let arguments = match parse_arguments_object(&call.arguments) {
                    Ok(arguments) => arguments,
                    Err(_) if incomplete && call.kind == ToolKind::Custom => return Ok(None),
                    Err(_) if incomplete => return Ok(Some(call.arguments.clone())),
                    Err(error) => return Err(error),
                };
                if call.kind == ToolKind::Custom {
                    return arguments
                        .get("input")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .map(Some)
                        .ok_or_else(|| bad("custom tool arguments must contain a string input"))
                        .or_else(|error| if incomplete { Ok(None) } else { Err(error) });
                }
                Ok(Some(call.arguments.clone()))
            })
            .collect()
    }

    fn event(&mut self, kind: &str, fields: Value) -> String {
        let mut value = fields.as_object().cloned().unwrap_or_default();
        value.insert("type".into(), json!(kind));
        value.insert("sequence_number".into(), json!(self.sequence));
        self.sequence += 1;
        format!("event: {kind}\ndata: {}\n\n", Value::Object(value))
    }
}

fn stream_call_item(call: &StreamCall, status: &str, value: &str) -> Value {
    if call.kind == ToolKind::Custom {
        json!({"type":"custom_tool_call","id":call.id,"call_id":call.call_id,"name":call.name,"input":value,"status":status})
    } else {
        json!({"type":"function_call","id":call.id,"call_id":call.call_id,"name":call.name,"arguments":value,"status":status})
    }
}

fn response_id() -> String {
    item_id("resp")
}
fn item_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}
fn bad(message: &str) -> GatewayError {
    GatewayError::BadRequest(message.to_string())
}
/// The field names of a JSON object, for diagnostics that must describe a payload's
/// shape without logging its contents. Empty for anything that isn't an object.
fn object_keys(value: &Value) -> Vec<&str> {
    value
        .as_object()
        .map(|object| object.keys().map(String::as_str).collect())
        .unwrap_or_default()
}
fn required_str<'a>(value: &'a Value, key: &str, context: &str) -> Result<&'a str, GatewayError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| bad(&format!("{context}.{key} must be a string")))
}
fn optional_bool(object: &Map<String, Value>, key: &str) -> Result<Option<bool>, GatewayError> {
    object
        .get(key)
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_bool()
                .ok_or_else(|| bad(&format!("{key} must be a boolean")))
        })
        .transpose()
}
fn optional_number(object: &Map<String, Value>, key: &str) -> Result<Option<f64>, GatewayError> {
    object
        .get(key)
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_f64()
                .ok_or_else(|| bad(&format!("{key} must be a number")))
        })
        .transpose()
}
fn optional_i64(object: &Map<String, Value>, key: &str) -> Result<Option<i64>, GatewayError> {
    object
        .get(key)
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_i64()
                .ok_or_else(|| bad(&format!("{key} must be an integer")))
        })
        .transpose()
}
fn message(role: &str, content: Option<Value>) -> Message {
    Message {
        role: role.into(),
        content,
        name: None,
        tool_calls: None,
        tool_call_id: None,
        extra: Map::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codex_history_tools_and_adjacent_calls() {
        let parsed = parse_request(&json!({
            "instructions":"be concise","input":[
                {"role":"developer","content":[{"type":"input_text","text":"rules"}]},
                {"role":"user","content":"go"},{"type":"reasoning","summary":[]},
                {"type":"function_call","call_id":"call_1","name":"f","arguments":"{\"x\":1}"},
                {"type":"custom_tool_call","call_id":"call_2","name":"patch","input":"*** Begin"},
                {"type":"function_call_output","call_id":"call_1","output":"one"},
                {"type":"custom_tool_call_output","call_id":"call_2","output":"two"}
            ],"tools":[{"type":"function","name":"f","parameters":{"type":"object"}},{"type":"custom","name":"patch"}]
        })).unwrap();
        assert_eq!(parsed.chat.messages.len(), 6);
        assert_eq!(
            parsed.chat.messages[3].tool_calls.as_ref().unwrap().len(),
            2
        );
        assert_eq!(
            parsed.chat.messages[3].tool_calls.as_ref().unwrap()[1].id,
            "call_2"
        );
        assert_eq!(
            parsed.chat.messages[4].tool_call_id.as_deref(),
            Some("call_1")
        );
        assert_eq!(parsed.tool_kinds["patch"], ToolKind::Custom);
    }

    #[test]
    fn attaches_multiple_calls_to_preceding_assistant_text_until_result_boundary() {
        let parsed = parse_request(&json!({
            "input":[
                {"role":"user","content":"go"},
                {"role":"assistant","content":"I will call both."},
                {"type":"function_call","call_id":"call_1","name":"one","arguments":"{}"},
                {"type":"function_call","call_id":"call_2","name":"two","arguments":"{}"},
                {"type":"function_call_output","call_id":"call_1","output":"done"}
            ]
        }))
        .unwrap();
        assert_eq!(parsed.chat.messages.len(), 3);
        assert_eq!(
            parsed.chat.messages[1].text().as_deref(),
            Some("I will call both.")
        );
        assert_eq!(
            parsed.chat.messages[1].tool_calls.as_ref().unwrap().len(),
            2
        );
        assert_eq!(parsed.chat.messages[2].role, "tool");
    }

    #[test]
    fn rejects_unsupported_content_tools_and_malformed_arguments() {
        for body in [
            json!({"input":[{"role":"user","content":[{"type":"input_image","image_url":"x"}]}]}),
            json!({"input":[{"type":"function_call","call_id":"c","name":"f","arguments":"{"}]}),
            json!({"input":[],"text":{"format":{"type":"json_schema"}}}),
        ] {
            assert!(parse_request(&body).is_err(), "accepted {body}");
        }
        // `{"type":"web_search"}` used to belong on that list. It is now dropped instead —
        // Codex sends it on every request, and a provider-hosted tool nothing downstream can
        // execute is not worth failing a turn over. See
        // `codex_namespaces_flatten_and_hosted_tools_drop_without_failing_the_turn`.
    }

    #[test]
    fn nonstream_pure_tools_has_no_empty_message_and_preserves_call_id() {
        let resp: ChatResponse = serde_json::from_value(json!({"id":"x","model":"m","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_x","type":"function","function":{"name":"patch","arguments":"{\"input\":\"abc\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}})).unwrap();
        let value = render_response(
            resp,
            "effective",
            &HashMap::from([("patch".into(), ToolKind::Custom)]),
        )
        .unwrap();
        assert_eq!(value["output"].as_array().unwrap().len(), 1);
        assert_eq!(value["output"][0]["type"], "custom_tool_call");
        assert_eq!(value["output"][0]["call_id"], "call_x");
        assert_eq!(value["output"][0]["input"], "abc");
        assert_eq!(value["usage"]["total_tokens"], 5);
    }

    #[test]
    fn nonstream_length_preserves_partial_calls_and_normal_finish_is_strict() {
        let length: ChatResponse = serde_json::from_value(json!({
            "id":"x","model":"m","choices":[{"index":0,"message":{"role":"assistant","content":"partial"},"finish_reason":"length"}]
        }))
        .unwrap();
        let value = render_response(length, "m", &HashMap::new()).unwrap();
        assert_eq!(value["status"], "incomplete");
        assert_eq!(value["incomplete_details"]["reason"], "max_output_tokens");
        assert_eq!(value["output"][0]["status"], "incomplete");

        let partial: ChatResponse = serde_json::from_value(json!({
            "id":"x","model":"m","choices":[{"index":0,"message":{"role":"assistant","tool_calls":[{"id":"c","type":"function","function":{"name":"f","arguments":"{\"input\":\"partial"}}]},"finish_reason":"length"}]
        }))
        .unwrap();
        let value = render_response(partial, "m", &HashMap::new()).unwrap();
        assert_eq!(value["status"], "incomplete");
        assert_eq!(value["output"][0]["status"], "incomplete");
        assert_eq!(value["output"][0]["arguments"], "{\"input\":\"partial");

        let partial_custom: ChatResponse = serde_json::from_value(json!({
            "id":"x","model":"m","choices":[{"index":0,"message":{"role":"assistant","tool_calls":[{"id":"c","type":"function","function":{"name":"custom","arguments":"{\"input\":\"partial"}}]},"finish_reason":"length"}]
        }))
        .unwrap();
        let value = render_response(
            partial_custom,
            "m",
            &HashMap::from([("custom".into(), ToolKind::Custom)]),
        )
        .unwrap();
        assert_eq!(value["status"], "incomplete");
        assert!(value["output"].as_array().unwrap().is_empty());
        assert!(!value.to_string().contains("{\\\"input\\\""));

        for arguments in ["{", "[]"] {
            let malformed: ChatResponse = serde_json::from_value(json!({
                "id":"x","model":"m","choices":[{"index":0,"message":{"role":"assistant","tool_calls":[{"id":"c","type":"function","function":{"name":"f","arguments":arguments}}]},"finish_reason":"tool_calls"}]
            }))
            .unwrap();
            let value = render_response(malformed, "m", &HashMap::new()).unwrap();
            assert_eq!(value["status"], "failed");
            assert_eq!(value["error"]["code"], "invalid_tool_call");
        }
    }

    #[test]
    fn rejects_semantics_that_cross_provider_translation_drops() {
        for body in [
            json!({"background":true}),
            json!({"parallel_tool_calls":false}),
            json!({"store":true}),
            json!({"truncation":"auto"}),
            json!({"top_p":0.5}),
            json!({"text":{"verbosity":"low"}}),
            json!({"tool_choice":"none"}),
            json!({"unknown_control":true}),
            json!({"tools":[{"type":"function","name":"x","strict":true}]}),
            json!({"tools":[{"type":"custom","name":"x","format":{"type":"json_schema"}}]}),
            json!({"tools":[{"type":"custom","name":"x","format":{"type":"grammar","syntax":"cfg","definition":"x"}}]}),
        ] {
            assert!(parse_request(&body).is_err(), "accepted {body}");
        }
        let parsed = parse_request(&json!({
            "background":false,"parallel_tool_calls":true,"store":false,
            "truncation":"disabled","reasoning":{"effort":"high"},
            "include":["reasoning.encrypted_content"],"service_tier":"auto",
            "metadata":{"client":"codex"},"prompt_cache_key":"key",
            "tools":[{"type":"custom","name":"x","format":{"type":"grammar","syntax":"lark","definition":"start: /.+/"}}]
        })).unwrap();
        assert!(parsed.chat.extra.is_empty());
        assert_eq!(
            parsed.advisory_fields,
            ["reasoning", "include", "custom_tool.format.grammar"]
        );
        assert!(
            parsed.chat.tools.as_ref().unwrap()[0]
                .function
                .description
                .as_deref()
                .unwrap()
                .contains("lark grammar")
        );

        let response: ChatResponse = serde_json::from_value(json!({
            "id":"x","model":"m","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]
        })).unwrap();
        let rendered = render_response(response, "m", &HashMap::new()).unwrap();
        assert!(
            rendered["output"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["type"] != "reasoning")
        );
        assert!(!rendered.to_string().contains("encrypted_content"));
    }

    /// Shaped exactly like a real Codex 0.160 request (captured off the wire): one
    /// namespace per MCP server plus Codex's own built-in groups, members carrying
    /// `strict: false`, and a hosted `web_search` with no name at all.
    #[test]
    fn codex_namespaces_flatten_and_hosted_tools_drop_without_failing_the_turn() {
        let parsed = parse_request(&json!({
            "input": "hi",
            "tools": [
                {"type":"function","name":"exec_command","description":"run",
                 "parameters":{"type":"object"},"strict":false},
                {"type":"namespace","name":"mcp__nasiko","description":"Nasiko gateway","tools":[
                    {"type":"function","name":"nasiko_search_tools","description":"search",
                     "parameters":{"type":"object"},"strict":false},
                    {"type":"function","name":"nasiko_call_tool","description":"call",
                     "parameters":{"type":"object"},"strict":false}
                ]},
                {"type":"namespace","name":"multi_agent_v1","description":"agents","tools":[
                    {"type":"function","name":"spawn_agent","description":"spawn",
                     "parameters":{"type":"object"},"strict":false}
                ]},
                {"type":"web_search"}
            ]
        }))
        .unwrap();

        // Members are hoisted under their bare names — what the model sees and calls —
        // and the namespace wrapper itself never reaches the provider.
        let names: Vec<&str> = parsed
            .chat
            .tools
            .as_ref()
            .unwrap()
            .iter()
            .map(|tool| tool.function.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "exec_command",
                "nasiko_search_tools",
                "nasiko_call_tool",
                "spawn_agent"
            ]
        );
        assert_eq!(parsed.tool_kinds["nasiko_call_tool"], ToolKind::Function);
        assert!(!parsed.tool_kinds.contains_key("mcp__nasiko"));

        // The hosted tool is dropped, not rejected — but the turn is marked lossy.
        assert_eq!(parsed.advisory_fields, [HOSTED_TOOLS_ADVISORY]);
    }

    #[test]
    fn a_namespace_collision_is_an_error_rather_than_a_silent_mis_route() {
        // Two namespaces exposing the same member name would flatten onto one tool, and a
        // call could not be attributed back. Identical definitions are harmless (the
        // existing duplicate rule); conflicting ones must fail.
        let conflicting = json!({
            "input": "hi",
            "tools": [
                {"type":"namespace","name":"a","tools":[
                    {"type":"function","name":"send","description":"one","parameters":{"type":"object"}}
                ]},
                {"type":"namespace","name":"b","tools":[
                    {"type":"function","name":"send","description":"two","parameters":{"type":"object"}}
                ]}
            ]
        });
        let error = parse_request(&conflicting).unwrap_err();
        assert!(
            error.to_string().contains("duplicate tool 'send'"),
            "{error}"
        );

        // A namespace must actually carry members, and may not nest.
        assert!(
            parse_request(&json!({"input":"hi","tools":[{"type":"namespace","name":"a"}]}))
                .is_err()
        );
        assert!(
            parse_request(&json!({"input":"hi","tools":[
                {"type":"namespace","name":"a","tools":[{"type":"namespace","name":"b","tools":[]}]}
            ]}))
            .is_err()
        );
    }

    #[test]
    fn client_telemetry_is_ignored_while_unknown_controls_still_fail_loud() {
        // Codex attaches `client_metadata` to every request. It cannot reach generation, so
        // it is dropped rather than 400'd — the same treatment `metadata` already gets.
        let parsed = parse_request(&json!({
            "input": "hi",
            "client_metadata": {"cli_version": "0.48.0", "terminal_type": "iTerm.app"},
        }))
        .unwrap();
        assert_eq!(parsed.chat.messages.len(), 1);
        assert!(parsed.chat.extra.is_empty());
        assert!(parsed.advisory_fields.is_empty());

        // Widening the allowlist must not turn the parser permissive: a field that could
        // change the answer is still a hard error.
        assert!(parse_request(&json!({"input": "hi", "unknown_control": true})).is_err());
    }

    #[test]
    fn stream_lifecycle_is_monotonic_complete_and_has_no_done_sentinel() {
        let mut renderer = ResponsesStreamRenderer::new("m".into(), HashMap::new());
        let mut frames = renderer.start();
        let chunk: ChatChunk = serde_json::from_value(
            json!({"id":"x","model":"m","choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":"stop"}]}),
        )
        .unwrap();
        frames.extend(renderer.render(chunk));
        frames.extend(renderer.finish());
        let joined = frames.join("");
        assert!(joined.contains("response.created"));
        assert!(joined.contains("response.output_text.done"));
        assert!(joined.contains("response.completed"));
        assert!(!joined.contains("[DONE]"));
        for (index, frame) in frames.iter().enumerate() {
            assert!(frame.contains(&format!("\"sequence_number\":{index}")));
        }
    }

    #[test]
    fn stream_length_is_incomplete_and_malformed_call_fails_once() {
        let mut renderer = ResponsesStreamRenderer::new("m".into(), HashMap::new());
        let chunk: ChatChunk = serde_json::from_value(json!({
            "id":"x","model":"m","choices":[{"index":0,"delta":{"content":"partial"},"finish_reason":"length"}]
        }))
        .unwrap();
        renderer.start();
        renderer.render(chunk);
        let terminal = renderer.finish().join("");
        assert!(terminal.contains("event: response.incomplete"));
        assert!(!terminal.contains("event: response.completed"));
        assert!(terminal.contains("\"reason\":\"max_output_tokens\""));

        let mut renderer = ResponsesStreamRenderer::new("m".into(), HashMap::new());
        renderer.start();
        renderer.render(serde_json::from_value(json!({
            "id":"x","model":"m","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"f","arguments":"{\"input\":\"partial"}}]},"finish_reason":"length"}]
        })).unwrap());
        let terminal = renderer.finish().join("");
        assert!(terminal.contains("event: response.incomplete"));
        assert!(terminal.contains("\"type\":\"function_call\""));
        assert!(terminal.contains("{\\\"input\\\":\\\"partial"));
        assert_eq!(renderer.outcome(), Some(TerminalOutcome::Incomplete));

        let mut renderer = ResponsesStreamRenderer::new(
            "m".into(),
            HashMap::from([("custom".into(), ToolKind::Custom)]),
        );
        renderer.start();
        let rendered = renderer.render(serde_json::from_value(json!({
            "id":"x","model":"m","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"custom","arguments":"{\"input\":\"partial"}}]},"finish_reason":"length"}]
        })).unwrap());
        assert!(rendered.is_empty());
        let terminal = renderer.finish().join("");
        assert!(terminal.contains("event: response.incomplete"));
        assert!(!terminal.contains("custom_tool_call"));
        assert!(!terminal.contains("{\\\"input\\\""));
        assert_eq!(renderer.outcome(), Some(TerminalOutcome::Incomplete));

        let mut renderer = ResponsesStreamRenderer::new("m".into(), HashMap::new());
        renderer.start();
        let call: ChatChunk = serde_json::from_value(json!({
            "id":"x","model":"m","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"f","arguments":"{"}}]},"finish_reason":"tool_calls"}]
        }))
        .unwrap();
        renderer.render(call);
        let terminal = renderer.finish().join("");
        assert_eq!(terminal.matches("event: response.failed").count(), 1);
        assert!(!terminal.contains("response.output_item.done"));
        assert!(!terminal.contains("response.completed"));
        assert_eq!(renderer.outcome(), Some(TerminalOutcome::Failed));
    }

    #[test]
    fn complete_stream_custom_call_decodes_escaped_input_before_lifecycle() {
        let mut renderer = ResponsesStreamRenderer::new(
            "m".into(),
            HashMap::from([("custom".into(), ToolKind::Custom)]),
        );
        renderer.start();
        let rendered = renderer.render(serde_json::from_value(json!({
            "id":"x","model":"m","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"custom","arguments":"{\"input\":\"line\\n\\\"quote\\\"\\\\tail\"}"}}]},"finish_reason":"tool_calls"}]
        })).unwrap());
        assert!(rendered.is_empty());

        let terminal = renderer.finish().join("");
        assert!(terminal.contains("event: response.output_item.added"));
        assert!(terminal.contains("event: response.custom_tool_call_input.delta"));
        assert!(terminal.contains("event: response.custom_tool_call_input.done"));
        assert!(terminal.contains("line\\n\\\"quote\\\"\\\\tail"));
        assert!(!terminal.contains("{\\\"input\\\""));
    }

    #[test]
    fn content_filter_never_completes_or_exposes_an_executable_call() {
        let response: ChatResponse = serde_json::from_value(json!({
            "id":"x","model":"m","choices":[{"index":0,"message":{"role":"assistant","tool_calls":[{"id":"c","type":"function","function":{"name":"f","arguments":"{}"}}]},"finish_reason":"content_filter"}]
        }))
        .unwrap();
        let rendered = render_response(response, "m", &HashMap::new()).unwrap();
        assert_eq!(rendered["status"], "failed");
        assert_eq!(rendered["error"]["code"], "content_filter");
        assert!(rendered["output"].as_array().unwrap().is_empty());

        let mut renderer = ResponsesStreamRenderer::new("m".into(), HashMap::new());
        renderer.start();
        renderer.render(serde_json::from_value(json!({
            "id":"x","model":"m","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"f","arguments":"{}"}}]},"finish_reason":"content_filter"}]
        })).unwrap());
        let terminal = renderer.finish().join("");
        assert!(terminal.contains("event: response.failed"));
        assert!(terminal.contains("\"code\":\"content_filter\""));
        assert!(!terminal.contains("response.output_item.done"));
        assert!(!terminal.contains("response.completed"));
        assert_eq!(renderer.outcome(), Some(TerminalOutcome::Failed));
    }

    #[test]
    fn stream_item_ids_indexes_and_sequence_are_stable() {
        let mut renderer = ResponsesStreamRenderer::new("m".into(), HashMap::new());
        let mut frames = renderer.start();
        for chunk in [
            json!({"id":"x","model":"m","choices":[{"index":0,"delta":{"content":"text"}}]}),
            json!({"id":"x","model":"m","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"one","arguments":"{}"}}]}}]}),
            json!({"id":"x","model":"m","choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"c2","function":{"name":"two","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}),
        ] {
            frames.extend(renderer.render(serde_json::from_value(chunk).unwrap()));
        }
        frames.extend(renderer.finish());
        let events: Vec<Value> = frames
            .iter()
            .map(|frame| {
                serde_json::from_str(frame.split_once("data: ").unwrap().1.trim()).unwrap()
            })
            .collect();
        for (sequence, event) in events.iter().enumerate() {
            assert_eq!(event["sequence_number"], sequence);
        }
        let added: Vec<_> = events
            .iter()
            .filter(|event| event["type"] == "response.output_item.added")
            .collect();
        assert_eq!(added.len(), 3);
        assert_eq!(added[0]["output_index"], 0);
        assert_eq!(added[1]["output_index"], 1);
        assert_eq!(added[2]["output_index"], 2);
        let ids: Vec<_> = added
            .iter()
            .map(|event| event["item"]["id"].as_str().unwrap())
            .collect();
        assert_ne!(ids[0], ids[1]);
        assert_ne!(ids[1], ids[2]);
        for event in events
            .iter()
            .filter(|event| event["type"] == "response.output_item.done")
        {
            let index = event["output_index"].as_u64().unwrap() as usize;
            assert_eq!(event["item"]["id"], ids[index]);
        }
    }
}
