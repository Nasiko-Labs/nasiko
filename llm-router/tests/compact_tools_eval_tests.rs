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
