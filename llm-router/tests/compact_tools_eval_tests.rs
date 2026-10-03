use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use serde_json::Value;

// Path to test fixture
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("compact_tools_eval_sample.json")
}

#[test]
fn test_compact_tools_eval_runner_full_cycle() {
    let fixture = fixture_path();
    assert!(fixture.exists(), "fixture must exist at {}", fixture.display());

    let out_dir = std::env::temp_dir().join("nasiko_test_eval");
    std::fs::create_dir_all(&out_dir).unwrap();
    let out_file = out_dir.join("test_out.jsonl");

    // Execute example via cargo run command to test true CLI / environment contract
    let status = std::process::Command::new("cargo")
        .args([
            "run",
            "-p",
            "nasiko-llm-router",
            "--example",
            "compact_tools_eval",
        ])
        .env("EVAL_SET", &fixture)
        .env("OUT", &out_file)
        .status()
        .expect("failed to execute compact_tools_eval example");

    assert!(status.success(), "evaluation example must exit successfully");
    assert!(out_file.exists(), "OUT file must be produced");

    // Inspect OUT JSONL lines
    let file = File::open(&out_file).unwrap();
    let lines: Vec<String> = BufReader::new(file).lines().map(Result::unwrap).collect();

    // 4 regular cases + 3 decoder cases = 7 lines total
    assert_eq!(lines.len(), 7, "expected 7 JSONL lines");

    // 1. Case ct-001 (successful compaction & roundtrip)
    let ct001: Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(ct001["id"], "ct-001");
    assert_eq!(ct001["compacted"], true);
    assert!(ct001["rendered_calls"].as_str().unwrap().contains("<<call create_calendar_event"));
    assert!(ct001.get("raw_output").is_none(), "offline mode must omit raw_output");
    assert!(ct001.get("live_calls").is_none(), "offline mode must omit live_calls");
    let rt001 = ct001["roundtrip_calls"].as_array().unwrap();
    assert_eq!(rt001.len(), 1);
    assert_eq!(rt001[0]["name"], "create_calendar_event");
    assert_eq!(rt001[0]["arguments"]["title"], "Design review");

    // 2. Case ct-002 (multiple calls)
    let ct002: Value = serde_json::from_str(&lines[1]).unwrap();
    assert_eq!(ct002["id"], "ct-002");
    assert_eq!(ct002["compacted"], true);
    let rt002 = ct002["roundtrip_calls"].as_array().unwrap();
    assert_eq!(rt002.len(), 2);
    assert_eq!(rt002[0]["name"], "send_email");
    assert_eq!(rt002[1]["name"], "create_calendar_event");

    // 3. Case ct-003 (no calls expected)
    let ct003: Value = serde_json::from_str(&lines[2]).unwrap();
    assert_eq!(ct003["id"], "ct-003");
    assert_eq!(ct003["compacted"], true);
    assert_eq!(ct003["rendered_calls"], "");
    assert_eq!(ct003["roundtrip_calls"].as_array().unwrap().len(), 0);

    // 4. Case ct-004-unsupported (bypass compaction test)
    let ct004: Value = serde_json::from_str(&lines[3]).unwrap();
    assert_eq!(ct004["id"], "ct-004-unsupported");
    assert_eq!(ct004["compacted"], false, "unsupported oneOf schema must bypass compaction");
    assert!(ct004["compact_request"]["tools"].is_array(), "native tools must be preserved on bypass");

    // 5. Decoder Case dc-002 (successful stream decoding from split chunks)
    let dc002: Value = serde_json::from_str(&lines[4]).unwrap();
    assert_eq!(dc002["id"], "dc-002");
    let dc002_calls = dc002["decoded"]["calls"].as_array().unwrap();
    assert_eq!(dc002_calls.len(), 1);
    assert_eq!(dc002_calls[0]["name"], "create_calendar_event");
    assert_eq!(dc002_calls[0]["arguments"]["title"], "Retro");

    // 6. Decoder Case dc-003-error-unknown (unknown tool error code)
    let dc003: Value = serde_json::from_str(&lines[5]).unwrap();
    assert_eq!(dc003["id"], "dc-003-error-unknown");
    assert_eq!(dc003["decoded"]["error"], "unknown_tool");

    // 7. Decoder Case dc-004-error-missing-arg (missing required field error code)
    let dc004: Value = serde_json::from_str(&lines[6]).unwrap();
    assert_eq!(dc004["id"], "dc-004-error-missing-arg");
    assert_eq!(dc004["decoded"]["error"], "invalid_arguments");
}

#[test]
fn test_compact_tools_eval_runner_partial_config_fails() {
    let fixture = fixture_path();
    let out_dir = std::env::temp_dir().join("nasiko_test_eval");
    std::fs::create_dir_all(&out_dir).unwrap();
    let out_file = out_dir.join("test_partial_config.jsonl");

    // Only set PROVIDER_BASE_URL without MODEL
    let status = std::process::Command::new("cargo")
        .args([
            "run",
            "-p",
            "nasiko-llm-router",
            "--example",
            "compact_tools_eval",
        ])
        .env("EVAL_SET", &fixture)
        .env("OUT", &out_file)
        .env("PROVIDER_BASE_URL", "http://localhost:8000/v1")
        .env_remove("MODEL")
        .status()
        .expect("failed to execute compact_tools_eval example");

    assert!(!status.success(), "runner must fail with error code when partial live config is provided");
}

#[tokio::test]
async fn test_compact_tools_eval_runner_live_mode_with_mock() {
    let mut server = mockito::Server::new_async().await;
    let url = server.url();

    let mock_response = serde_json::json!({
        "id": "chatcmpl-live",
        "object": "chat.completion",
        "choices": [
            {
                "message": {
                    "role": "assistant",
                    "content": "<<call create_calendar_event {\"title\":\"Live Mock Event\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>"
                }
            }
        ]
    });

    let mock = server
        .mock("POST", "/chat/completions")
        .match_body(mockito::Matcher::PartialJson(serde_json::json!({
            "model": "mock-gpt4",
            "temperature": 0.0
        })))
        .expect_at_least(1)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_response.to_string())
        .create_async()
        .await;

    let fixture = fixture_path();
    let out_dir = std::env::temp_dir().join("nasiko_test_eval");
    std::fs::create_dir_all(&out_dir).unwrap();
    let out_file = out_dir.join("test_live_out.jsonl");

    let status = std::process::Command::new("cargo")
        .args([
            "run",
            "-p",
            "nasiko-llm-router",
            "--example",
            "compact_tools_eval",
        ])
        .env("EVAL_SET", &fixture)
        .env("OUT", &out_file)
        .env("PROVIDER_BASE_URL", &url)
        .env("MODEL", "mock-gpt4")
        .status()
        .expect("failed to execute compact_tools_eval example in live mode");

    assert!(status.success(), "runner must succeed in live mode with valid mock server");
    assert!(out_file.exists(), "OUT file must exist");
    mock.assert_async().await;

    let file = File::open(&out_file).unwrap();
    let lines: Vec<String> = BufReader::new(file).lines().map(Result::unwrap).collect();
    assert_eq!(lines.len(), 7);

    // Verify ct-001 has raw_output and live_calls populated
    let ct001: Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(ct001["id"], "ct-001");
    assert!(ct001["raw_output"].as_str().unwrap().contains("<<call create_calendar_event"));
    let live_calls = &ct001["live_calls"];
    assert!(live_calls["calls"].is_array(), "live_calls must contain calls array");
    let calls = live_calls["calls"].as_array().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["name"], "create_calendar_event");
    assert_eq!(calls[0]["arguments"]["title"], "Live Mock Event");
}
