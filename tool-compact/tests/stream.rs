use nasiko_tool_compact::{
    CompactError, StreamDecoder, ToolCall, ToolDef, decode_calls, render_calls,
};
use serde_json::json;

fn tools() -> Vec<ToolDef> {
    vec![serde_json::from_value(json!({"function":{"name":"foo","parameters":{"type":"object","required":["x"],"properties":{"x":{"type":"string"}}}}})).unwrap()]
}

#[test]
fn every_character_split_matches_batch_and_finish() {
    let calls = vec![
        ToolCall {
            name: "foo".into(),
            arguments: json!({"x":"हेलो >> { } \" \\","nested":[{"items":[]}]})
        };
        2
    ];
    let text = format!("Before {} After", render_calls(&calls).unwrap());
    assert_eq!(decode_calls(&text, &tools()).unwrap(), calls);
    for boundary in (0..=text.len()).filter(|&index| text.is_char_boundary(index)) {
        let mut decoder = StreamDecoder::new(&tools()).unwrap();
        let mut provisional = decoder.push(&text[..boundary]).unwrap();
        provisional.extend(decoder.push(&text[boundary..]).unwrap());
        assert_eq!(provisional, calls);
        assert_eq!(decoder.finish().unwrap(), calls);
    }
    let mut decoder = StreamDecoder::new(&tools()).unwrap();
    for character in text.chars() {
        decoder.push(&character.to_string()).unwrap();
    }
    assert_eq!(decoder.finish().unwrap(), calls);
}

#[test]
fn calls_are_emitted_only_after_closing_marker() {
    let mut decoder = StreamDecoder::new(&tools()).unwrap();
    for chunk in ["<<ca", "ll f", "oo {\"x\":", "\"yes\"}>"] {
        assert!(decoder.push(chunk).unwrap().is_empty());
    }
    assert_eq!(decoder.push(">").unwrap().len(), 1);
    assert_eq!(decoder.finish().unwrap().len(), 1);
}

#[test]
fn no_calls_and_text_around_calls() {
    assert_eq!(decode_calls("The answer is 42.", &tools()).unwrap(), vec![]);
    assert_eq!(
        decode_calls("text <<call \n foo \t {\"x\":\"yes\"} >> text", &tools())
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        decode_calls("<<<call foo {\"x\":\"yes\"}>>", &tools())
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn errors_poison_the_entire_response_and_cannot_recover() {
    let valid = "<<call foo {\"x\":\"yes\"}>>";
    for invalid in [
        "<<call missing {}>>",
        "<<call foo {}>>",
        "<<callfoo {}>>",
        "<<call foo {bad}>>",
        "<<call foo {\"x\":\"yes\",\"x\":\"no\"}>>",
        "<<call foo {\"x\":\"yes\"}>x",
        "<<call foo [1]>>",
    ] {
        assert!(
            decode_calls(&format!("{valid}{invalid}"), &tools()).is_err(),
            "{invalid}"
        );
        let mut decoder = StreamDecoder::new(&tools()).unwrap();
        decoder.push(valid).unwrap();
        assert!(decoder.push(invalid).is_err());
        assert!(decoder.push(valid).is_err());
        assert!(decoder.finish().is_err());
    }
}

#[test]
fn incomplete_stream_and_depth_limit_fail_closed() {
    for text in [
        "<<ca",
        "<<call",
        "<<call foo",
        "<<call foo {\"x\":\"",
        "<<call foo {\"x\":\"yes\"}>",
    ] {
        assert!(
            matches!(
                decode_calls(text, &tools()),
                Err(CompactError::IncompleteCall)
            ),
            "{text}"
        );
    }
    let deep = format!(
        "<<call foo {{\"x\":\"yes\",\"deep\":{}0{}}}>>",
        "[".repeat(70),
        "]".repeat(70)
    );
    assert!(matches!(
        decode_calls(&deep, &tools()),
        Err(CompactError::LimitExceeded(_))
    ));
}
