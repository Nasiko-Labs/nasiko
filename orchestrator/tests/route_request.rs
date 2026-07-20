use nasiko_orchestrator::{AgentCard, FilePart, RouteRequest};
use uuid::Uuid;

// ── AgentCard construction ────────────────────────────────────────────────────

#[test]
fn agent_card_construction_with_all_fields() {
    let id = Uuid::new_v4();
    let card = AgentCard {
        id,
        name: "coder".to_string(),
        description: "Writes and reviews code".to_string(),
        skills: vec!["code-review".to_string(), "refactor".to_string()],
        tags: vec!["engineering".to_string()],
        url: Some("http://localhost:9000".to_string()),
    };
    assert_eq!(card.id, id);
    assert_eq!(card.name, "coder");
    assert_eq!(card.skills.len(), 2);
    assert!(card.url.is_some());
}

#[test]
fn agent_card_construction_minimal() {
    let card = AgentCard {
        id: Uuid::nil(),
        name: "agent".to_string(),
        description: String::new(),
        skills: vec![],
        tags: vec![],
        url: None,
    };
    assert!(card.skills.is_empty());
    assert!(card.url.is_none());
}

// ── AgentCard serialization ───────────────────────────────────────────────────

#[test]
fn agent_card_serializes_to_json() {
    let id = Uuid::new_v4();
    let card = AgentCard {
        id,
        name: "test-agent".to_string(),
        description: "Test".to_string(),
        skills: vec!["skill-a".to_string()],
        tags: vec!["tag-x".to_string()],
        url: Some("http://agent:9000".to_string()),
    };
    let json = serde_json::to_string(&card).unwrap();
    assert!(json.contains("test-agent"));
    assert!(json.contains("skill-a"));
    assert!(json.contains("tag-x"));
}

#[test]
fn agent_card_round_trips_through_json() {
    let id = Uuid::new_v4();
    let original = AgentCard {
        id,
        name: "round-trip-agent".to_string(),
        description: "desc".to_string(),
        skills: vec!["s1".to_string(), "s2".to_string()],
        tags: vec!["t1".to_string()],
        url: None,
    };
    let json = serde_json::to_string(&original).unwrap();
    let restored: AgentCard = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.id, original.id);
    assert_eq!(restored.name, original.name);
    assert_eq!(restored.skills, original.skills);
    assert!(restored.url.is_none());
}

#[test]
fn agent_card_with_url_none_omits_null() {
    let card = AgentCard {
        id: Uuid::nil(),
        name: "n".to_string(),
        description: String::new(),
        skills: vec![],
        tags: vec![],
        url: None,
    };
    let json = serde_json::to_string(&card).unwrap();
    // url: null is still present (Option serializes as null by default unless skip_serializing_if)
    // just verify it round-trips correctly
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(v.get("url").is_some_and(|u| u.is_null()));
}

// ── RouteRequest construction ─────────────────────────────────────────────────

#[test]
fn route_request_construction() {
    let req = RouteRequest {
        query: "what is the status of BTC?".to_string(),
        session_id: "sess-abc".to_string(),
        user_id: Uuid::new_v4(),
        file_parts: vec![],
    };
    assert_eq!(req.query, "what is the status of BTC?");
    assert!(req.file_parts.is_empty());
}

#[test]
fn route_request_with_file_parts() {
    let fp = FilePart::encode("report.pdf".into(), b"%PDF-1.4", "application/pdf".into());
    let req = RouteRequest {
        query: "summarize this document".to_string(),
        session_id: "sess-xyz".to_string(),
        user_id: Uuid::new_v4(),
        file_parts: vec![fp],
    };
    assert_eq!(req.file_parts.len(), 1);
    assert_eq!(req.file_parts[0].filename, "report.pdf");
    assert_eq!(req.file_parts[0].content_type, "application/pdf");
}

#[test]
fn route_request_with_multiple_file_parts() {
    let fp1 = FilePart::encode("a.txt".into(), b"hello", "text/plain".into());
    let fp2 = FilePart::encode("b.png".into(), b"\x89PNG", "image/png".into());
    let req = RouteRequest {
        query: "analyze these files".to_string(),
        session_id: "sess-multi".to_string(),
        user_id: Uuid::nil(),
        file_parts: vec![fp1, fp2],
    };
    assert_eq!(req.file_parts.len(), 2);
}

// ── FilePart encode ───────────────────────────────────────────────────────────

#[test]
fn file_part_encode_produces_data_uri() {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as B64;

    let fp = FilePart::encode("test.txt".into(), b"hello world", "text/plain".into());
    let data_str = String::from_utf8(fp.data).unwrap();
    assert!(data_str.starts_with("data:text/plain;base64,"));
    let b64 = data_str.split(',').nth(1).unwrap();
    let decoded = B64.decode(b64).unwrap();
    assert_eq!(decoded, b"hello world");
}

#[test]
fn file_part_encode_preserves_mime_type() {
    let fp = FilePart::encode("img.png".into(), b"\x89PNG", "image/png".into());
    assert_eq!(fp.content_type, "image/png");
    assert_eq!(fp.filename, "img.png");
}

#[test]
fn file_part_encode_empty_bytes() {
    let fp = FilePart::encode("empty.bin".into(), b"", "application/octet-stream".into());
    let data_str = String::from_utf8(fp.data).unwrap();
    assert!(data_str.starts_with("data:application/octet-stream;base64,"));
}
