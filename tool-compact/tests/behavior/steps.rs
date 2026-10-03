use cucumber::{given, gherkin::Step, then, when};
use nasiko_tool_compact::{
    ArgumentFault, CompactError, StreamDecoder, ToolDef, decode_calls, decode_tools, encode_tools,
};
use serde_json::Value;

use crate::sample::{
    assert_decoder, assert_named_calls, case_by_id, load_sample, reject_invalid_success,
    render_calls, sample_tool, tools_for_case,
};
use crate::world::CompactWorld;

fn doc(step: &Step) -> String {
    step.docstring
        .clone()
        .expect("this step needs a docstring")
}

fn tool(name: &str, description: Option<&str>, parameters: Option<Value>) -> ToolDef {
    ToolDef {
        name: name.to_string(),
        description: description.map(str::to_string),
        parameters,
    }
}

fn ensure_sample(world: &mut CompactWorld) {
    if world.sample.is_none() {
        world.sample = Some(load_sample());
    }
}

fn signature<'a>(world: &'a CompactWorld) -> &'a str {
    world
        .compact
        .as_ref()
        .expect("encode the tools first")
        .text
        .as_str()
}

fn decoded_tool(world: &CompactWorld, index: usize) -> &nasiko_tool_compact::ToolDef {
    let schemas = world
        .schemas
        .as_ref()
        .expect("decode the compact text first")
        .as_ref()
        .expect("schema decoding failed");
    schemas
        .get(index)
        .unwrap_or_else(|| panic!("no decoded tool at {}", index + 1))
}

fn params<'a>(world: &'a CompactWorld, index: usize) -> &'a Value {
    decoded_tool(world, index)
        .parameters
        .as_ref()
        .expect("decoded tool has parameters")
}

fn outcome<'a>(
    world: &'a CompactWorld,
) -> &'a Result<Vec<nasiko_tool_compact::ToolCall>, CompactError> {
    world
        .stream
        .as_ref()
        .or(world.calls.as_ref())
        .expect("decode a reply or finish the stream first")
}

fn replay(world: &mut CompactWorld) {
    let chunks = world.chunks.clone();
    let mut emitted = Vec::new();
    let result = {
        let mut decoder = StreamDecoder::new(&world.tools);
        let mut calls = Vec::new();
        let mut failed = None;
        for chunk in &chunks {
            match decoder.push(chunk) {
                Ok(found) => {
                    emitted.push(found.len());
                    calls.extend(found);
                }
                Err(err) => {
                    emitted.push(0);
                    failed = Some(err);
                    break;
                }
            }
        }
        match failed {
            Some(err) => Err(err),
            None => match decoder.finish() {
                Ok(_) => Ok(calls),
                Err(err) => Err(err),
            },
        }
    };
    world.chunk_emitted = emitted;
    world.stream = Some(result);
}

#[given("the public sample")]
fn public_sample(world: &mut CompactWorld) {
    ensure_sample(world);
}

#[given("no tools")]
fn no_tools(world: &mut CompactWorld) {
    world.tools.clear();
}

#[given(expr = "the sample tool {string}")]
fn sample_tool_step(world: &mut CompactWorld, name: String) {
    ensure_sample(world);
    let sample = world.sample.clone().expect("sample");
    world.tools.push(sample_tool(&sample, &name));
}

#[given(expr = "a tool named {string} described as {string} with this schema:")]
fn tool_with_schema(world: &mut CompactWorld, name: String, description: String, #[step] step: &Step) {
    let schema: Value = serde_json::from_str(doc(step).trim()).expect("schema json");
    world.tools.push(tool(&name, Some(&description), Some(schema)));
}

#[given(expr = "a tool named {string} with no parameters")]
fn tool_without_parameters(world: &mut CompactWorld, name: String) {
    world.tools.push(tool(&name, Some("Check liveness"), None));
}

#[given(expr = "a tool named {string} with no description and no parameters")]
fn tool_without_description(world: &mut CompactWorld, name: String) {
    world.tools.push(tool(&name, None, None));
}

#[when("the tools are encoded")]
fn encode(world: &mut CompactWorld) {
    match encode_tools(&world.tools) {
        Ok(compact) => {
            world.encode_error = None;
            world.compact = Some(compact);
        }
        Err(err) => {
            world.compact = None;
            world.encode_error = Some(err);
        }
    }
}

#[when("the compact text is decoded back into schemas")]
fn decode_schemas(world: &mut CompactWorld) {
    let compact = world.compact.as_ref().expect("encode the tools first");
    world.schemas = Some(decode_tools(compact));
}

#[when("the compact text is replaced with:")]
fn replace_text(world: &mut CompactWorld, #[step] step: &Step) {
    world
        .compact
        .as_mut()
        .expect("encode the tools first")
        .text = doc(step).trim().to_string();
}

#[when("the model replies:")]
fn model_replies(world: &mut CompactWorld, #[step] step: &Step) {
    world.reply = doc(step).trim().to_string();
    world.calls = Some(decode_calls(&world.reply, &world.tools));
    world.stream = None;
}

#[when(expr = "the stream receives chunk {string}")]
fn receive_chunk(world: &mut CompactWorld, chunk: String) {
    world.chunks.push(chunk);
}

#[when("the stream receives this chunk:")]
fn receive_chunk_doc(world: &mut CompactWorld, #[step] step: &Step) {
    world.chunks.push(doc(step).trim().to_string());
}

#[when("the stream is finished")]
fn finish_stream(world: &mut CompactWorld) {
    replay(world);
}

#[when("that reply is pushed as one stream chunk")]
fn reply_as_chunk(world: &mut CompactWorld) {
    world.chunks = vec![world.reply.clone()];
    replay(world);
}

#[when(expr = "case {string} is round-tripped")]
fn round_trip_case(world: &mut CompactWorld, id: String) {
    let sample = world.sample.clone().expect("load the public sample first");
    let case = case_by_id(&sample, "cases", &id);
    let tools = tools_for_case(&sample, case);
    let compact = encode_tools(&tools).expect("sample tools encode");
    let expected = case["expected"].as_array().cloned().unwrap_or_default();
    world.calls = Some(decode_calls(&render_calls(&expected), &tools));
    world.compact = Some(compact);
    world.tools = tools;
    world.stream = None;
}

#[when(expr = "decoder case {string} is fed as published chunks")]
fn feed_decoder_case(world: &mut CompactWorld, id: String) {
    let sample = world.sample.clone().expect("load the public sample first");
    let case = case_by_id(&sample, "decoder_cases", &id);
    world.tools = tools_for_case(&sample, case);
    world.chunks = case["chunks"]
        .as_array()
        .expect("chunks")
        .iter()
        .map(|chunk| chunk.as_str().unwrap_or("").to_string())
        .collect();
    replay(world);
}

#[when("invalid replies are decoded against the calendar tool")]
fn invalid_replies(world: &mut CompactWorld) {
    reject_invalid_success();
    world.property_held = true;
}

#[then(expr = "encoding fails because of {string}")]
fn encoding_fails(world: &mut CompactWorld, feature: String) {
    let err = world.encode_error.as_ref().expect("encoding should fail");
    match err {
        CompactError::UnsupportedSchema { feature: got, .. } => assert_eq!(got, &feature),
        other => panic!("expected unsupported schema, got {other:?}"),
    }
    let text = err.to_string();
    assert!(text.contains("unsupported schema"), "{text}");
    assert!(text.contains(&feature), "{text}");
}

#[then(expr = "the signature contains {string}")]
fn signature_contains(world: &mut CompactWorld, needle: String) {
    let text = signature(world);
    assert!(text.contains(&needle), "{text}");
}

#[then(expr = "the signature ends with {string}")]
fn signature_ends(world: &mut CompactWorld, suffix: String) {
    let line = signature(world).lines().next().expect("signature");
    assert!(line.ends_with(&suffix), "{line}");
}

#[then("the signature is the call instruction")]
fn signature_is_instruction(world: &mut CompactWorld) {
    assert_eq!(
        signature(world).trim(),
        "To call a tool, emit: <<call name {json args}>>"
    );
}

#[then(expr = "the encoded tools accessor lists {string}")]
fn accessor_lists(world: &mut CompactWorld, name: String) {
    let names: Vec<_> = world
        .compact
        .as_ref()
        .expect("encoded")
        .tools()
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert!(names.contains(&name.as_str()), "{names:?}");
}

#[then("schema decoding fails as malformed")]
fn schemas_malformed(world: &mut CompactWorld) {
    let err = world
        .schemas
        .as_ref()
        .expect("decode schemas")
        .as_ref()
        .expect_err("damaged signature should fail");
    assert!(matches!(
        err,
        CompactError::InvalidArguments { reason: ArgumentFault::Malformed, .. }
    ));
    assert!(err.to_string().contains("malformed"), "{err}");
}

#[then(expr = "schema decoding returns {int} tools")]
fn schema_count(world: &mut CompactWorld, count: usize) {
    let schemas = world.schemas.as_ref().expect("decode").as_ref().expect("ok");
    assert_eq!(schemas.len(), count);
}

#[then(expr = "decoded tool {int} is named {string}")]
fn decoded_name(world: &mut CompactWorld, index: usize, name: String) {
    assert_eq!(decoded_tool(world, index - 1).name, name);
}

#[then(expr = "decoded tool {int} description is {string}")]
fn decoded_description(world: &mut CompactWorld, index: usize, description: String) {
    assert_eq!(
        decoded_tool(world, index - 1).description.as_deref(),
        Some(description.as_str())
    );
}

#[then(expr = "decoded tool {int} has no description")]
fn decoded_no_description(world: &mut CompactWorld, index: usize) {
    assert_eq!(decoded_tool(world, index - 1).description, None);
}

#[then(expr = "decoded tool {int} requires {string}")]
fn decoded_requires(world: &mut CompactWorld, index: usize, field: String) {
    let required = params(world, index - 1)["required"]
        .as_array()
        .expect("required");
    assert!(required.iter().any(|item| item.as_str() == Some(field.as_str())));
}

#[then(expr = "decoded tool {int} has no required fields")]
fn decoded_no_required(world: &mut CompactWorld, index: usize) {
    let parameters = params(world, index - 1);
    let required = parameters.get("required").and_then(Value::as_array);
    assert!(required.map(Vec::is_empty).unwrap_or(true));
}

#[then(expr = "decoded tool {int} property {string} has type {string}")]
fn property_type(world: &mut CompactWorld, index: usize, property: String, type_name: String) {
    assert_eq!(
        params(world, index - 1)["properties"][&property]["type"],
        type_name
    );
}

#[then(expr = "decoded tool {int} property {string} format is {string}")]
fn property_format(world: &mut CompactWorld, index: usize, property: String, format: String) {
    assert_eq!(
        params(world, index - 1)["properties"][&property]["format"],
        format
    );
}

#[then(expr = "decoded tool {int} property {string} enum is {string}")]
fn property_enum(world: &mut CompactWorld, index: usize, property: String, values: String) {
    let want: Vec<Value> = values
        .split(',')
        .map(|item| Value::String(item.to_string()))
        .collect();
    assert_eq!(
        params(world, index - 1)["properties"][&property]["enum"],
        Value::Array(want)
    );
}

#[then(expr = "decoded tool {int} property {string} items are {string}")]
fn property_items(world: &mut CompactWorld, index: usize, property: String, type_name: String) {
    assert_eq!(
        params(world, index - 1)["properties"][&property]["items"]["type"],
        type_name
    );
}

#[then(expr = "decoded tool {int} nested property {string} {string} has type {string}")]
fn nested_property(
    world: &mut CompactWorld,
    index: usize,
    parent: String,
    child: String,
    type_name: String,
) {
    assert_eq!(
        params(world, index - 1)["properties"][&parent]["properties"][&child]["type"],
        type_name
    );
}

#[then(expr = "there are {int} calls")]
fn call_count(world: &mut CompactWorld, count: usize) {
    let calls = outcome(world).as_ref().expect("calls should succeed");
    assert_eq!(calls.len(), count);
}

#[then(expr = "call {int} is named {string}")]
fn call_name(world: &mut CompactWorld, index: usize, name: String) {
    let calls = outcome(world).as_ref().expect("calls");
    assert_eq!(calls[index - 1].name, name);
}

#[then(expr = "call {int} argument {string} is {string}")]
fn call_argument_string(world: &mut CompactWorld, index: usize, field: String, value: String) {
    assert_argument_string(world, index, &field, &value);
}

#[then(expr = "call {int} argument {string} is:")]
fn call_argument_string_doc(world: &mut CompactWorld, index: usize, field: String, #[step] step: &Step) {
    assert_argument_string(world, index, &field, doc(step).trim());
}

#[then(expr = "call {int} argument {string} json is {string}")]
fn call_argument_json(world: &mut CompactWorld, index: usize, field: String, json: String) {
    assert_argument_json(world, index, &field, &json);
}

#[then(expr = "call {int} argument {string} json is:")]
fn call_argument_json_doc(world: &mut CompactWorld, index: usize, field: String, #[step] step: &Step) {
    assert_argument_json(world, index, &field, doc(step).trim());
}

fn assert_argument_string(world: &CompactWorld, index: usize, field: &str, value: &str) {
    let calls = outcome(world).as_ref().expect("calls");
    let args: Value = serde_json::from_str(&calls[index - 1].arguments).expect("json");
    assert_eq!(args[field], value);
}

fn assert_argument_json(world: &CompactWorld, index: usize, field: &str, json: &str) {
    let calls = outcome(world).as_ref().expect("calls");
    let args: Value = serde_json::from_str(&calls[index - 1].arguments).expect("json");
    let want: Value = serde_json::from_str(json).unwrap_or_else(|err| panic!("expected json: {err}"));
    assert_eq!(args[field], want);
}

#[then(expr = "decoding fails with unknown tool {string}")]
fn unknown_tool(world: &mut CompactWorld, name: String) {
    let err = outcome(world).as_ref().expect_err("unknown tool");
    match err {
        CompactError::UnknownTool { name: got } => assert_eq!(got, &name),
        other => panic!("{other:?}"),
    }
    let text = err.to_string();
    assert!(text.contains("unknown tool"), "{text}");
    assert!(text.contains(&name), "{text}");
}

#[then(expr = "decoding fails with missing field {string}")]
fn missing_field(world: &mut CompactWorld, field: String) {
    let err = outcome(world).as_ref().expect_err("missing field");
    match err {
        CompactError::InvalidArguments {
            reason: ArgumentFault::MissingField(got),
            ..
        } => assert_eq!(got, &field),
        other => panic!("{other:?}"),
    }
    assert!(err.to_string().contains("missing field"), "{err}");
}

#[then(expr = "decoding fails with wrong type for {string}")]
fn wrong_type(world: &mut CompactWorld, field: String) {
    let err = outcome(world).as_ref().expect_err("wrong type");
    match err {
        CompactError::InvalidArguments {
            reason: ArgumentFault::WrongType { field: got },
            ..
        } => assert_eq!(got, &field),
        other => panic!("{other:?}"),
    }
    assert!(err.to_string().contains("wrong type"), "{err}");
}

#[then("decoding fails with wrong type")]
fn wrong_type_any(world: &mut CompactWorld) {
    let err = outcome(world).as_ref().expect_err("wrong type");
    assert!(matches!(
        err,
        CompactError::InvalidArguments { reason: ArgumentFault::WrongType { .. }, .. }
    ));
    assert!(err.to_string().contains("wrong type"), "{err}");
}

#[then(expr = "decoding fails with a bad enum for {string}")]
fn bad_enum(world: &mut CompactWorld, field: String) {
    let err = outcome(world).as_ref().expect_err("bad enum");
    match err {
        CompactError::InvalidArguments {
            reason: ArgumentFault::BadEnum { field: got },
            ..
        } => assert_eq!(got, &field),
        other => panic!("{other:?}"),
    }
    assert!(err.to_string().contains("enum"), "{err}");
}

#[then("decoding fails as malformed")]
fn decode_malformed(world: &mut CompactWorld) {
    let err = outcome(world).as_ref().expect_err("malformed");
    assert!(matches!(
        err,
        CompactError::InvalidArguments { reason: ArgumentFault::Malformed, .. }
    ));
    assert!(err.to_string().contains("malformed"), "{err}");
}

#[then(regex = r"^chunk (\d+) emits (\d+) calls?$")]
fn chunk_emits(world: &mut CompactWorld, chunk: usize, count: usize) {
    assert_eq!(world.chunk_emitted[chunk - 1], count);
}

#[then(expr = "the stream fails with a bad enum for {string}")]
fn stream_bad_enum(world: &mut CompactWorld, field: String) {
    let err = world.stream.as_ref().expect("stream").as_ref().expect_err("bad enum");
    match err {
        CompactError::InvalidArguments {
            reason: ArgumentFault::BadEnum { field: got },
            ..
        } => assert_eq!(got, &field),
        other => panic!("{other:?}"),
    }
}

#[then("the stream fails as malformed")]
fn stream_malformed(world: &mut CompactWorld) {
    let err = world
        .stream
        .as_ref()
        .expect("stream")
        .as_ref()
        .expect_err("malformed");
    assert!(matches!(
        err,
        CompactError::InvalidArguments { reason: ArgumentFault::Malformed, .. }
    ));
    assert!(world.chunk_emitted.iter().all(|count| *count == 0));
}

#[then("the stream calls match the decoded calls")]
fn stream_matches_decode(world: &mut CompactWorld) {
    let direct = world.calls.as_ref().expect("decode").as_ref().expect("ok");
    let streamed = world.stream.as_ref().expect("stream").as_ref().expect("ok");
    assert_eq!(streamed, direct);
}

#[then(expr = "the round trip matches case {string}")]
fn round_trip_matches(world: &mut CompactWorld, id: String) {
    let sample = world.sample.as_ref().expect("sample");
    let case = case_by_id(sample, "cases", &id);
    let expected = case["expected"].as_array().expect("expected");
    let calls = world.calls.as_ref().expect("round trip").as_ref().expect("decoded");
    assert_named_calls(calls, expected);
    let names: Vec<_> = case["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let stored: Vec<_> = world
        .compact
        .as_ref()
        .expect("encoded")
        .tools()
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert_eq!(stored, names);
}

#[then(expr = "the decoded chunks match case {string}")]
fn chunks_match(world: &mut CompactWorld, id: String) {
    let sample = world.sample.as_ref().expect("sample");
    let case = case_by_id(sample, "decoder_cases", &id);
    let outcome = world.stream.as_ref().expect("stream");
    assert_decoder(outcome, &case["expected"]);
}

#[then(expr = "the sample includes a tool named {string}")]
fn sample_includes(world: &mut CompactWorld, name: String) {
    let sample = world.sample.as_ref().expect("sample");
    let found = sample["tools"].as_array().expect("tools").iter().any(|tool| {
        tool.get("function")
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str)
            == Some(name.as_str())
    });
    assert!(found, "missing {name}");
}

#[then("every invalid reply is rejected")]
fn property_held(world: &mut CompactWorld) {
    assert!(world.property_held);
}
