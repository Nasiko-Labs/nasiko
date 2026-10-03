//! Tool list → compact prompt → tool list, and every bypass path.

mod common;

use common::{nested_tool, sample_tools, sample_tools_json, tool};
use nasiko_tool_compact::{
    BypassReason, CALL_INSTRUCTION, CompactTools, EncodeOptions, Instructions, MAX_DEPTH, ToolDef,
    decode_calls, decode_tools, encode_tools, encode_tools_with, encode_tools_with_choice,
    tool_choice_allows_compact,
};
use serde_json::{Value, json};

const SAMPLE_PROMPT: &str = "\
create_calendar_event(title:str \"Event title\", start:datetime \"Start time, ISO 8601\", attendees?:[str] \"Attendee emails\", duration_min?:int \"Duration in minutes\", visibility?:enum public|private) - Create an event in the user's calendar.
send_email(to:[str] \"Recipient emails\", subject:str \"Subject line\", body:str \"Plain-text body\", cc?:[str] \"CC emails\") - Send an email from the user's account.

To call a tool, emit: <<call NAME {json args}>>
? = optional; args are one JSON object; datetime = RFC 3339 with offset, e.g. 2026-01-31T09:30:00+01:00";

fn bypass_label(c: &CompactTools) -> &'static str {
    c.bypass_reason()
        .map_or("compacted", BypassReason::as_label)
}

#[test]
fn calendar_example_renders_exactly_and_round_trips() {
    let tools = sample_tools();
    let compact = encode_tools(&tools).unwrap();
    assert!(compact.compacted());
    assert_eq!(compact.prompt().unwrap(), SAMPLE_PROMPT);
    assert_eq!(decode_tools(&compact).unwrap(), tools);
}

#[test]
fn round_trip_restores_openai_json() {
    let tools = sample_tools();
    let back: Vec<Value> = decode_tools(&encode_tools(&tools).unwrap())
        .unwrap()
        .iter()
        .map(ToolDef::to_openai)
        .collect();
    assert_eq!(Value::Array(back), sample_tools_json());
}

#[test]
fn compact_prompt_is_smaller_than_native_json() {
    let native = sample_tools_json().to_string();
    let compact = encode_tools(&sample_tools()).unwrap();
    let prompt = compact.prompt().unwrap();
    // Under two thirds of the native bytes, notes line included.
    assert!(
        prompt.len() * 3 < native.len() * 2,
        "compact {} bytes vs native {} bytes",
        prompt.len(),
        native.len()
    );
}

#[test]
fn encoding_is_deterministic_regardless_of_property_order() {
    let a = tool(
        "t",
        json!({"type":"object","properties":{"b":{"type":"integer"},"a":{"type":"string"}}}),
    );
    let b = tool(
        "t",
        json!({"type":"object","properties":{"a":{"type":"string"},"b":{"type":"integer"}}}),
    );
    assert_eq!(encode_tools(&[a]).unwrap(), encode_tools(&[b]).unwrap());
}

#[test]
fn nested_object_and_array_round_trip() {
    let tools = vec![nested_tool()];
    let compact = encode_tools(&tools).unwrap();
    let line = compact.prompt().unwrap().lines().next().unwrap();
    assert_eq!(
        line,
        "create_ticket(title:str, reporter:{email:str, team?:enum infra|web} \"Who filed it\", \
         labels?:[str], subtasks?:[{name:str, done?:bool, due?:date, estimate?:num}]) - create_ticket tool"
    );
    assert_eq!(decode_tools(&compact).unwrap(), tools);
}

#[test]
fn awkward_descriptions_are_quoted_and_survive() {
    for desc in [
        "",
        "  leading space",
        "\"starts with a quote",
        "line one\nline two",
        "has ) and , and >> and | inside",
        "unicode — नमस्ते",
    ] {
        let tools = vec![ToolDef {
            name: "t".into(),
            description: Some(desc.into()),
            parameters: Some(json!({
                "type": "object",
                "properties": { "x": { "type": "string", "description": desc } }
            })),
        }];
        let compact = encode_tools(&tools).unwrap();
        assert!(compact.compacted(), "{desc:?}");
        assert_eq!(decode_tools(&compact).unwrap(), tools, "{desc:?}");
    }
}

#[test]
fn no_parameters_and_no_description() {
    let tools = vec![ToolDef {
        name: "ping".into(),
        description: None,
        parameters: Some(json!({"type":"object","properties":{}})),
    }];
    let compact = encode_tools(&tools).unwrap();
    assert!(compact.prompt().unwrap().starts_with("ping()\n"));
    assert_eq!(decode_tools(&compact).unwrap(), tools);
}

#[test]
fn unsupported_schemas_bypass_with_the_original_tools() {
    let cases = [
        json!({"type":"object","properties":{"x":{"oneOf":[{"type":"string"},{"type":"integer"}]}}}),
        json!({"type":"object","properties":{"x":{"anyOf":[{"type":"string"}]}}}),
        json!({"type":"object","properties":{"x":{"allOf":[{"type":"string"}]}}}),
        json!({"type":"object","properties":{"x":{"$ref":"#/$defs/X"}}}),
        json!({"type":"object","properties":{"x":{"const":"a"}}}),
        json!({"type":"object","properties":{"x":{"type":"object","properties":{},"patternProperties":{"^a":{"type":"string"}}}}}),
        json!({"type":"object","properties":{"x":{"type":"string"}},"additionalProperties":true}),
        json!({"type":"object","properties":{"x":{"type":"string"}},"additionalProperties":{"type":"string"}}),
        json!({"type":"object","properties":{"x":{"type":"string","format":"not a name"}}}),
        json!({"type":"object","properties":{"x":{"type":["string","null"]}}}),
        json!({"type":"object","properties":{"x":{"type":"integer","multipleOf":2}}}),
        json!({"type":"object","properties":{"x":{"type":"integer","exclusiveMinimum":true}}}),
        json!({"type":"object","properties":{"x":{"type":"integer","minLength":1}}}),
        json!({"type":"object","properties":{"x":{"type":"string","minimum":1}}}),
        json!({"type":"object","properties":{"x":{"type":"string","maxLength":-1}}}),
        json!({"type":"object","properties":{"x":{"type":"array","items":{"type":"string"},"uniqueItems":false}}}),
        json!({"type":"object","properties":{"x":{"type":"string","enum":["a"],"maxLength":3}}}),
        json!({"type":"object","properties":{"x":{"type":"string","default":"a"}}}),
        // Not validated (no regex engine), so not compacted: fail closed.
        json!({"type":"object","properties":{"x":{"type":"string","pattern":"^a+$"}}}),
        json!({"type":"object","properties":{"x":{"type":"array","items":{"type":"string","pattern":"^a"}}}}),
        json!({"type":"object","properties":{"x":{"type":"string","enum":["has space"]}}}),
        json!({"type":"object","properties":{"x":{"type":"integer","enum":[1,2]}}}),
        json!({"type":"object","properties":{"x":{"type":"object"}}}),
        json!({"type":"object"}),
        json!({"type":"object","properties":{"x":{"type":"string"}},"required":["y"]}),
    ];
    for params in cases {
        let tools = vec![sample_tools().remove(0), tool("weird", params.clone())];
        let compact = encode_tools(&tools).unwrap();
        assert!(!compact.compacted(), "{params}");
        assert_eq!(bypass_label(&compact), "unsupported_schema", "{params}");
        assert_eq!(
            decode_tools(&compact).unwrap(),
            tools,
            "bypass carries tools: {params}"
        );
    }
}

#[test]
fn bypass_reason_names_tool_and_path() {
    let compact = encode_tools(&[tool(
        "weird",
        json!({"type":"object","properties":{"a":{"type":"array","items":{"oneOf":[]}}}}),
    )])
    .unwrap();
    assert_eq!(
        compact.bypass_reason().unwrap().to_string(),
        "unsupported schema in weird at $.a[]: keyword 'oneOf'"
    );
}

#[test]
fn excessive_nesting_bypasses() {
    let mut schema = json!({"type":"string"});
    for _ in 0..40 {
        schema = json!({"type":"array","items":schema});
    }
    let params = json!({"type":"object","properties":{"deep":schema}});
    assert_eq!(
        bypass_label(&encode_tools(&[tool("deep", params)]).unwrap()),
        "unsupported_schema"
    );
}

#[test]
fn tool_choice_rules() {
    let tools = sample_tools();
    for choice in [None, Some(json!("auto")), Some(Value::Null)] {
        assert!(tool_choice_allows_compact(choice.as_ref()));
        assert!(
            encode_tools_with_choice(&tools, choice.as_ref())
                .unwrap()
                .compacted()
        );
    }
    for choice in [
        json!("none"),
        json!("required"),
        json!({"type":"function","function":{"name":"send_email"}}),
        json!("something-new"),
    ] {
        let c = encode_tools_with_choice(&tools, Some(&choice)).unwrap();
        assert_eq!(bypass_label(&c), "tool_choice", "{choice}");
        assert_eq!(decode_tools(&c).unwrap(), tools);
    }
}

#[test]
fn empty_list_bypasses_and_duplicates_error() {
    assert_eq!(bypass_label(&encode_tools(&[]).unwrap()), "no_tools");
    let dup = vec![sample_tools().remove(0), sample_tools().remove(0)];
    assert_eq!(encode_tools(&dup).unwrap_err().code(), "invalid_tools");
}

#[test]
fn from_openai_refuses_what_it_cannot_carry() {
    let mut strict = sample_tools_json()[0].clone();
    strict["function"]["strict"] = json!(true);
    assert_eq!(
        ToolDef::from_openai(&strict).unwrap_err().code(),
        "invalid_tools"
    );
    assert!(ToolDef::from_openai(&json!({"type":"web_search"})).is_err());
}

#[test]
fn decode_tools_rejects_foreign_text() {
    for prompt in [
        "not a tool list".to_string(),
        format!("bad line(\n\n{CALL_INSTRUCTION}"),
        format!("t(x:wat)\n\n{CALL_INSTRUCTION}"),
        format!("t(x:str, x:int)\n\n{CALL_INSTRUCTION}"),
        format!("t()\nt()\n\n{CALL_INSTRUCTION}"),
    ] {
        let c = CompactTools::Compact { prompt };
        assert_eq!(decode_tools(&c).unwrap_err().code(), "malformed_compact");
    }
}

fn annotated_tool() -> ToolDef {
    tool(
        "book_room",
        json!({
            "type": "object",
            "properties": {
                "email": { "type": "string", "format": "email", "maxLength": 64 },
                "site": { "type": "string", "format": "uri" },
                "code": { "type": "string", "minLength": 5 },
                "seats": { "type": "integer", "minimum": 1, "maximum": 20 },
                "budget": { "type": "number", "exclusiveMinimum": 0, "exclusiveMaximum": 1.5e3 },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 1, "maxItems": 3, "uniqueItems": true
                },
                "when": { "type": "string", "format": "date-time" },
                "meta": {
                    "type": "object",
                    "properties": { "note": { "type": "string" } },
                    "additionalProperties": false
                }
            },
            "required": ["email", "seats"],
            "additionalProperties": false
        }),
    )
}

#[test]
fn closed_objects_formats_and_limits_are_annotated_and_round_trip() {
    let tools = vec![annotated_tool()];
    let compact = encode_tools(&tools).unwrap();
    assert!(compact.compacted(), "{:?}", compact.bypass_reason());
    assert_eq!(
        compact.prompt().unwrap().lines().next().unwrap(),
        "book_room(email:str<email,maxlen=64>, seats:int<min=1,max=20>, \
         budget?:num<gt=0,lt=1500.0>, code?:str<minlen=5>, \
         meta?:{note?:str}!, site?:str<uri>, tags?:[str]<minitems=1,maxitems=3,unique>, \
         when?:datetime)! - book_room tool"
    );
    assert_eq!(decode_tools(&compact).unwrap(), tools);
}

#[test]
fn absent_and_false_additional_properties_stay_distinct() {
    let open = tool(
        "t",
        json!({"type":"object","properties":{"a":{"type":"string"}}}),
    );
    let closed = tool(
        "t",
        json!({"type":"object","properties":{"a":{"type":"string"}},"additionalProperties":false}),
    );
    let (o, c) = (
        encode_tools(std::slice::from_ref(&open)).unwrap(),
        encode_tools(std::slice::from_ref(&closed)).unwrap(),
    );
    assert_ne!(o.prompt(), c.prompt());
    assert_eq!(decode_tools(&o).unwrap(), vec![open]);
    assert_eq!(decode_tools(&c).unwrap(), vec![closed]);
}

#[test]
fn never_larger_than_native() {
    // One tiny tool (46 B of native JSON): the fixed call instruction outweighs what it replaces.
    let tiny = ToolDef {
        name: "f".into(),
        description: None,
        parameters: None,
    };
    let c = encode_tools(std::slice::from_ref(&tiny)).unwrap();
    assert_eq!(bypass_label(&c), "not_smaller");
    assert_eq!(decode_tools(&c).unwrap(), vec![tiny]);
}

#[test]
fn example_instructions_add_one_valid_sample_call() {
    let tools = sample_tools();
    let c = encode_tools_with(
        &tools,
        &EncodeOptions {
            instructions: Instructions::Example,
            ..Default::default()
        },
    )
    .unwrap();
    let prompt = c.prompt().unwrap();
    let last = prompt.lines().last().unwrap();
    assert_eq!(
        last,
        format!(
            "Example: <<call create_calendar_event {}>>",
            json!({"title":"text","start":"2026-01-01T09:00:00Z"})
        )
    );
    // The sample is itself a valid call, and the prompt still decodes to the same tools.
    let sample = last.strip_prefix("Example: ").unwrap();
    assert_eq!(decode_calls(sample, &tools).unwrap().calls.len(), 1);
    assert_eq!(decode_tools(&c).unwrap(), tools);
    assert!(prompt.len() > encode_tools(&tools).unwrap().prompt().unwrap().len());
    assert_eq!(Instructions::parse("example"), Some(Instructions::Example));
    assert_eq!(Instructions::parse("loud"), None);
}

/// `{"deep": [[...[ {} ]...]]}` whose innermost empty object sits at node depth `depth`.
fn nested_to(depth: usize) -> ToolDef {
    let mut schema = json!({"type":"object","properties":{}});
    for _ in 1..depth {
        schema = json!({"type":"array","items":schema});
    }
    tool(
        "deep",
        json!({"type":"object","properties":{"deep":schema}}),
    )
}

#[test]
fn max_depth_is_the_same_rule_on_both_sides() {
    // An empty object at exactly MAX_DEPTH: compacted, and the signature parser accepts it back.
    let at_max = vec![nested_to(MAX_DEPTH)];
    let c = encode_tools(&at_max).unwrap();
    assert!(c.compacted(), "{:?}", c.bypass_reason());
    assert!(
        c.prompt()
            .unwrap()
            .contains(&format!("{}{{}}", "[".repeat(MAX_DEPTH - 1)))
    );
    assert_eq!(decode_tools(&c).unwrap(), at_max);

    // One deeper: bypassed by the encoder, and refused by the parser if written by hand.
    let too_deep = nested_to(MAX_DEPTH + 1);
    assert_eq!(
        bypass_label(&encode_tools(std::slice::from_ref(&too_deep)).unwrap()),
        "unsupported_schema"
    );
    let prompt = c
        .prompt()
        .unwrap()
        .replacen("deep:[", "deep:[[", 1)
        .replacen("{}]", "{}]]", 1);
    let forged = CompactTools::Compact { prompt };
    assert_eq!(
        decode_tools(&forged).unwrap_err().code(),
        "malformed_compact"
    );
}

#[test]
fn notes_line_mentions_datetime_only_when_needed() {
    let with_dt = encode_tools(&sample_tools()).unwrap();
    assert!(with_dt.prompt().unwrap().contains("datetime = RFC 3339"));
    let without = encode_tools(&[sample_tools().remove(1)]).unwrap();
    let p = without.prompt().unwrap();
    assert!(
        p.ends_with("\n? = optional; args are one JSON object"),
        "{p}"
    );
    // A prompt whose trailer was edited is not ours.
    let edited = CompactTools::Compact {
        prompt: p.replace("args are one JSON object", "args are JSON"),
    };
    assert_eq!(
        decode_tools(&edited).unwrap_err().code(),
        "malformed_compact"
    );
}

#[test]
fn terse_instructions_are_shortest_and_round_trip() {
    let tools = sample_tools();
    let with = |instructions| {
        encode_tools_with(
            &tools,
            &EncodeOptions {
                instructions,
                ..Default::default()
            },
        )
        .unwrap()
    };
    let terse = with(Instructions::Terse);
    let prompt = terse.prompt().unwrap();
    assert!(
        prompt.ends_with(
            "\n\nCall tools ONLY as <<call NAME {json args}>>, each ending >>\n?=omit unless given; datetime=RFC3339+offset"
        ),
        "{prompt}"
    );
    assert!(prompt.len() < with(Instructions::Minimal).prompt().unwrap().len());
    assert_eq!(decode_tools(&terse).unwrap(), tools);
    assert_eq!(Instructions::parse("terse"), Some(Instructions::Terse));

    // No datetime field: the datetime note is left out here too.
    let email_only = encode_tools_with(
        &[sample_tools().remove(1)],
        &EncodeOptions {
            instructions: Instructions::Terse,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        email_only
            .prompt()
            .unwrap()
            .ends_with("\n?=omit unless given")
    );
    assert_eq!(
        decode_tools(&email_only).unwrap(),
        vec![sample_tools().remove(1)]
    );
}

#[test]
fn balanced_is_one_line_with_paren_descriptions_and_round_trips() {
    let opts = EncodeOptions {
        instructions: Instructions::Balanced,
        ..Default::default()
    };
    let tools = sample_tools();
    let c = encode_tools_with(&tools, &opts).unwrap();
    let prompt = c.prompt().unwrap();
    assert!(
        prompt.starts_with(
            "create_calendar_event(title:str(Event title), start:datetime(Start time, ISO 8601), "
        ),
        "{prompt}"
    );
    assert!(prompt.ends_with(
        "\n\nAlways call tools directly, never ask: <<call NAME ARGS>> (ARGS: JSON object). \
         ?=optional. datetime=RFC3339+offset"
    ));
    assert!(
        !prompt.contains('"') && !prompt.contains('\\'),
        "nothing to escape inside a JSON request body: {prompt}"
    );
    assert_eq!(decode_tools(&c).unwrap(), tools);
    assert_eq!(
        Instructions::parse("balanced"),
        Some(Instructions::Balanced)
    );

    // Descriptions that cannot be written as (text) fall back to JSON strings and still
    // round-trip, in the same prompt.
    let awkward = vec![ToolDef {
        name: "t".into(),
        description: Some("x".into()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "a": { "type": "string", "description": "has (parens)" },
                "b": { "type": "string", "description": "two\nlines" },
                "c": { "type": "string", "description": "" },
                "d": { "type": "string", "description": " spaced \"quote\" " },
                "e": { "type": "array", "items": { "type": "integer", "description": "inner" },
                       "description": "outer" }
            }
        })),
    }];
    let c = encode_tools_with(&awkward, &opts).unwrap();
    let p = c.prompt().unwrap();
    assert!(p.contains("a?:str \"has (parens)\""), "{p}");
    assert!(p.contains("d?:str( spaced \"quote\" )"), "{p}");
    assert!(p.contains("e?:[int(inner)](outer)"), "{p}");
    assert_eq!(decode_tools(&c).unwrap(), awkward);
}
