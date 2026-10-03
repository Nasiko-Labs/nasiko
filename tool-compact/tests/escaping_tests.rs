use nasiko_tool_compact::{decode_call, encode_call};
use serde_json::json;

#[test]
fn test_string_escaping_roundtrip() {
    let args = json!({
        "query": "hello \"world\" \n line2 \t tabbed \\ backslash",
        "unicode": "Rust 🦀 rocket 🚀"
    });

    let encoded = encode_call("search_tool", &args).unwrap();
    println!("Encoded: {}", encoded);

    let (tool_name, decoded_args) = decode_call(&encoded).unwrap();
    assert_eq!(tool_name, "search_tool");
    assert_eq!(decoded_args["query"], "hello \"world\" \n line2 \t tabbed \\ backslash");
    assert_eq!(decoded_args["unicode"], "Rust 🦀 rocket 🚀");
}

#[test]
fn test_unicode_escape_sequence_decoding() {
    let input = r#"greet(message="Hello \u0026 Welcome")"#;
    let (tool_name, args) = decode_call(input).unwrap();
    assert_eq!(tool_name, "greet");
    assert_eq!(args["message"], "Hello & Welcome");
}
