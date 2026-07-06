use nasiko_orchestrator::agent_client::AgentClient;
use nasiko_orchestrator::FilePart;

// ── AgentClient construction ──────────────────────────────────────────────────

#[test]
fn agent_client_new_does_not_panic() {
    let _ = AgentClient::new(60);
}

#[test]
fn agent_client_new_with_zero_timeout_does_not_panic() {
    // timeout=0 is unusual but should not panic at construction time
    let _ = AgentClient::new(0);
}

#[test]
fn agent_client_new_with_large_timeout_does_not_panic() {
    let _ = AgentClient::new(3600);
}

// ── FilePart::encode ──────────────────────────────────────────────────────────

#[test]
fn file_encode_produces_data_uri() {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as B64;

    let fp = FilePart::encode("test.txt".into(), b"hello", "text/plain".into());
    let data_str = String::from_utf8(fp.data).unwrap();
    assert!(data_str.starts_with("data:text/plain;base64,"));
    let b64_part = data_str.split(',').nth(1).unwrap();
    let decoded = B64.decode(b64_part).unwrap();
    assert_eq!(decoded, b"hello");
}

#[test]
fn file_encode_png_produces_correct_mime() {
    let fp = FilePart::encode("img.png".into(), b"\x89PNG", "image/png".into());
    let data_str = String::from_utf8(fp.data).unwrap();
    assert!(data_str.starts_with("data:image/png;base64,"));
    assert_eq!(fp.content_type, "image/png");
    assert_eq!(fp.filename, "img.png");
}

#[test]
fn file_encode_pdf_bytes_round_trip() {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as B64;

    let bytes = b"%PDF-1.4 binary content";
    let fp = FilePart::encode("doc.pdf".into(), bytes, "application/pdf".into());
    let data_str = String::from_utf8(fp.data).unwrap();
    let b64 = data_str.split(',').nth(1).unwrap();
    let decoded = B64.decode(b64).unwrap();
    assert_eq!(decoded.as_slice(), bytes);
}

#[test]
fn file_encode_empty_bytes_produces_valid_uri() {
    let fp = FilePart::encode("empty.bin".into(), b"", "application/octet-stream".into());
    let data_str = String::from_utf8(fp.data).unwrap();
    assert!(data_str.starts_with("data:application/octet-stream;base64,"));
    // base64 of empty bytes is an empty string after the comma
    let b64_part = data_str.split(',').nth(1).unwrap();
    assert!(b64_part.is_empty());
}

// ── send: live HTTP tests ─────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires a live A2A agent endpoint"]
async fn send_to_live_agent_streams_events() {
    use futures::StreamExt;

    let agent_url = std::env::var("TEST_AGENT_URL")
        .unwrap_or_else(|_| "http://localhost:9000".into());

    let client = AgentClient::new(30);
    let stream = client.send(
        agent_url,
        "hello world".to_string(),
        vec![],
        "test-context-id".to_string(),
        None,
    );

    futures::pin_mut!(stream);
    // Just check we can get at least one event without panicking
    let first = stream.next().await;
    assert!(first.is_some());
}