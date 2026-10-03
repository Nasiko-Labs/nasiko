mod common;

use common::*;
use nasiko_tool_compact::{Error, decode_tools, encode_tools};
use serde_json::json;

fn sig(tools: &[nasiko_tool_compact::ToolDef]) -> String {
    let c = encode_tools(tools).expect("encode");
    assert!(c.native.is_empty(), "unexpected bypass: {:?}", c.bypassed);
    c.signatures
}

#[test]
fn basic_tool_is_one_exact_line() {
    let t = tool("ping", Some("Echo"), Some(msg_params()));
    assert_eq!(sig(&[t]), "ping(msg:str) - Echo");
}

#[test]
fn tool_without_description_or_parameters() {
    assert_eq!(sig(&[tool("ping", None, None)]), "ping()");
}

#[test]
fn multiple_tools_one_line_each_in_input_order() {
    let s = sig(&[email(), calendar()]);
    let lines: Vec<&str> = s.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with("send_email("));
    assert!(lines[1].starts_with("create_calendar_event("));
}

#[test]
fn required_and_optional_are_distinguished() {
    let p = json!({"type":"object","properties":{"a":{"type":"string"},"b":{"type":"integer"}},"required":["a"]});
    assert_eq!(sig(&[tool("t", None, Some(p))]), "t(a:str,b?:int)");
}

#[test]
fn primitive_types() {
    let p = json!({"type":"object","properties":{
        "s":{"type":"string"},"i":{"type":"integer"},"n":{"type":"number"},"b":{"type":"boolean"},
        "d":{"type":"string","format":"date-time"}},
        "required":["s","i","n","b","d"]});
    // Required fields keep the schema's `required` order; optional ones are sorted.
    assert_eq!(
        sig(&[tool("t", None, Some(p))]),
        "t(s:str,i:int,n:num,b:bool,d:datetime)"
    );
}

#[test]
fn enum_values_keep_declared_order() {
    let p =
        json!({"type":"object","properties":{"v":{"type":"string","enum":["public","private"]}}});
    assert_eq!(sig(&[tool("t", None, Some(p))]), "t(v?:public|private)");
}

#[test]
fn arrays_including_nested() {
    let p = json!({"type":"object","properties":{
        "a":{"type":"array","items":{"type":"string"}},
        "m":{"type":"array","items":{"type":"array","items":{"type":"integer"}}},
        "o":{"type":"array","items":{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}}}});
    assert_eq!(
        sig(&[tool("t", None, Some(p))]),
        "t(a?:[str],m?:[[int]],o?:[{id:int}])"
    );
}

#[test]
fn nested_object_keeps_its_own_required() {
    let p = json!({"type":"object","properties":{"addr":{"type":"object","properties":{
        "city":{"type":"string"},"zip":{"type":"string"}},"required":["city"]}},"required":["addr"]});
    assert_eq!(
        sig(&[tool("t", None, Some(p))]),
        "t(addr:{city:str,zip?:str})"
    );
}

#[test]
fn descriptions_are_preserved_and_quoted_safely() {
    let s = sig(&[calendar()]);
    assert!(s.contains("title:str \"Event title\""), "{s}");
    assert!(s.contains("start:datetime \"Start time, ISO 8601\""), "{s}");
    assert!(
        s.ends_with("- Create an event in the user's calendar."),
        "{s}"
    );
    let p =
        json!({"type":"object","properties":{"x":{"type":"string","description":"say \"hi\" )"}}});
    let s = sig(&[tool("t", None, Some(p))]);
    assert!(s.contains(r#"x?:str "say \"hi\" )""#), "{s}");
}

#[test]
fn compact_signature_is_much_shorter_than_native_json() {
    let native: usize = [calendar(), email()]
        .iter()
        .map(|t| {
            t.parameters.as_ref().unwrap().to_string().len() + t.description.as_ref().unwrap().len()
        })
        .sum();
    let compact = sig(&[calendar(), email()]).len();
    assert!(
        compact * 10 < native * 7,
        "compact={compact} native={native}"
    );
}

#[test]
fn prompt_carries_call_format_and_signatures() {
    let c = encode_tools(&[calendar()]).unwrap();
    let p = c.prompt();
    assert!(p.contains("<<call "), "{p}");
    assert!(p.contains(&c.signatures));
    assert!(c.is_compacted());
}

#[test]
fn unsupported_schema_is_bypassed_unchanged() {
    let cases = [
        json!({"type":"object","properties":{"x":{"type":"string","minLength":2}}}),
        json!({"type":"object","properties":{"x":{"oneOf":[{"type":"string"},{"type":"integer"}]}}}),
        json!({"type":"object","properties":{"x":{"$ref":"#/$defs/X"}}}),
        json!({"type":"object","properties":{"x":{"type":["string","null"]}}}),
        json!({"type":"object","properties":{"x":{"type":"string","default":"a"}}}),
        json!({"type":"object","properties":{"x":{"type":"string","pattern":"^a"}}}),
        json!({"type":"object","properties":{"x":{"type":"string","format":"email"}}}),
        json!({"type":"object","properties":{"x":{"type":"array"}}}),
        json!({"type":"object","properties":{"x":{"type":"array","items":{"type":"string"},"minItems":1}}}),
        json!({"type":"object","properties":{"x":{"type":"string","enum":["a b","c"]}}}),
        json!({"type":"object","properties":{"x":{"type":"string","enum":["only"]}}}),
        json!({"type":"object","properties":{"x":{"type":"integer","enum":[1,2]}}}),
        json!({"type":"object","properties":{"bad name":{"type":"string"}}}),
        json!({"type":"object","properties":{"x":{"type":"string"}},"required":["ghost"]}),
        json!({"type":"object","properties":{"x":{"type":"string"}},"additionalProperties":true}),
        json!({"type":"object","properties":{"x":{"type":"string"}},"$schema":"http://json-schema.org/draft-07/schema#"}),
        json!({"type":"string"}),
    ];
    for p in cases {
        let t = tool("t", Some("d"), Some(p.clone()));
        let c = encode_tools(std::slice::from_ref(&t)).unwrap();
        assert!(c.signatures.is_empty(), "should bypass {p}");
        assert_eq!(
            c.native,
            vec![t],
            "bypassed tool must be byte-identical: {p}"
        );
        assert_eq!(c.bypassed.len(), 1);
        assert_eq!(c.bypassed[0].name, "t");
        assert!(!c.bypassed[0].reason.is_empty());
        assert!(!c.is_compacted());
    }
}

#[test]
fn unrepresentable_names_and_descriptions_are_bypassed() {
    for t in [
        tool("has space", None, None),
        tool("", None, None),
        tool("t", Some("two\nlines"), None),
        tool("t", Some(" padded "), None),
        tool("t", Some(""), None),
    ] {
        let c = encode_tools(std::slice::from_ref(&t)).unwrap();
        assert_eq!(c.native, vec![t.clone()], "{t:?}");
    }
}

#[test]
fn mixed_set_compacts_what_it_can_and_bypasses_the_rest() {
    let odd = tool(
        "odd",
        None,
        Some(json!({"type":"object","properties":{"x":{"type":"string","minLength":2}}})),
    );
    let c = encode_tools(&[calendar(), odd.clone(), email()]).unwrap();
    assert_eq!(c.signatures.lines().count(), 2);
    assert_eq!(c.native, vec![odd]);
    assert!(c.is_compacted());
}

#[test]
fn duplicate_tool_names_are_an_error() {
    let err = encode_tools(&[calendar(), calendar()]).unwrap_err();
    assert_eq!(err, Error::DuplicateTool("create_calendar_event".into()));
}

#[test]
fn schema_information_survives_decode_tools() {
    let odd = tool(
        "odd",
        None,
        Some(json!({"type":"object","properties":{"x":{"type":"string","minLength":2}}})),
    );
    let originals = vec![calendar(), email(), tool("ping", None, None), odd];
    let c = encode_tools(&originals).unwrap();
    let mut back = decode_tools(&c).unwrap();
    back.sort_by(|a, b| a.name.cmp(&b.name));
    let mut want = originals.clone();
    want.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(back, want);
}

#[test]
fn instruction_always_carries_what_the_validator_enforces() {
    let p = encode_tools(&[calendar()]).unwrap().prompt();
    for needle in [
        "<<call name {json args}>>",     // the call syntax itself
        "several allowed",               // multiple calls
        "?=optional",                    // optional vs required
        "datetime=RFC 3339 with offset", // the decoder rejects datetimes without an offset
        "Only listed tools and args",    // undeclared tools/args are rejected
        "reply in plain text",           // the no-call case
    ] {
        assert!(p.contains(needle), "instruction lost `{needle}`:\n{p}");
    }
}
