use super::*;

#[test]
fn file_encode_produces_data_uri() {
    let part = FilePart::encode("test.txt".into(), b"hello", "text/plain".into());
    let data = String::from_utf8(part.data).unwrap();
    assert!(data.starts_with("data:text/plain;base64,"));
    let b64_part = data.split(',').nth(1).unwrap();
    let decoded = B64.decode(b64_part).unwrap();
    assert_eq!(decoded, b"hello");
}

#[test]
fn build_payload_text_only() {
    let payload = build_payload("what is BTC?", &[], "ctx-123");
    assert_eq!(payload.method, "message/stream");
    assert_eq!(payload.jsonrpc, "2.0");
    assert_eq!(payload.params.context_id, "ctx-123");
    assert_eq!(payload.params.message.parts.len(), 1);
}

#[test]
fn build_payload_with_file() {
    let fp = FilePart::encode("img.png".into(), b"\x89PNG", "image/png".into());
    let payload = build_payload("describe this", &[fp], "ctx-456");
    assert_eq!(payload.params.message.parts.len(), 2);
}

#[test]
fn payload_is_valid_json() {
    let fp = FilePart::encode("doc.pdf".into(), b"%PDF", "application/pdf".into());
    let payload = build_payload("summarize", &[fp], "ctx-789");
    let json = serde_json::to_string(&payload).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["method"], "message/stream");
    assert_eq!(parsed["params"]["contextId"], "ctx-789");
}

#[test]
fn agent_client_new_does_not_panic() {
    let _ = AgentClient::new(60);
}
