//! Opt-in live verification. Makes two small paid calls; does not write usage rows.
//! LIVE_STREAM_BASE_URL must name a trusted capture relay forwarding to the configured provider.
use nasiko_react_agent::{
    AgentInfo, DelegationPolicy, Orchestrator, OrchestratorConfig, OrchestratorEvent,
    RegistrySource,
};
use serde_json::json;
use std::sync::Arc;

#[derive(Debug)]
struct OutputCap;
impl DelegationPolicy for OutputCap {
    fn undelegated_max_tokens(&self, _: usize) -> Option<u64> {
        Some(32)
    }
}

#[tokio::test]
#[ignore = "makes paid provider calls; requires explicit live verification authorization"]
async fn provider_stream_reports_usage_before_done() {
    let base = std::env::var("LIVE_STREAM_BASE_URL").expect("trusted capture relay URL");
    let key = std::env::var("OPENAI_API_KEY").expect("provider key");
    let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());
    let mut results = vec![];
    for _ in 0..2 {
        let mut orchestrator = Orchestrator::new(
            OrchestratorConfig {
                model: model.clone(),
                api_key: Some(key.clone()),
                base_url: Some(base.clone()),
                temperature: None,
                max_turns: 1,
                preamble: Some("Verification context only. Do not invoke tools. ".repeat(150)),
                policy: Some(Arc::new(OutputCap)),
                ..Default::default()
            },
            RegistrySource::Static(vec![AgentInfo {
                id: "verification-only".into(),
                name: "unused-verification-tool".into(),
                description: "Not available for this greeting; never invoke.".into(),
                endpoint: "http://127.0.0.1:1".into(),
                skills: vec![],
            }]),
        );
        orchestrator.init().await.unwrap();
        let mut rx = orchestrator.run_stream(
            "Hello. Reply with a brief greeting only. Do not invoke any tools.",
            vec![],
        );
        let mut usages = vec![];
        let mut content_events = 0;
        let mut done = false;
        while let Some(event) = rx.recv().await {
            match event {
                OrchestratorEvent::Usage { usage } => {
                    assert!(!done);
                    assert!(usage.streaming);
                    assert!(!usage.estimated, "provider must report real usage");
                    usages.push(usage);
                }
                OrchestratorEvent::Content { .. } => content_events += 1,
                OrchestratorEvent::Done { .. } => {
                    assert_eq!(usages.len(), 1);
                    done = true;
                }
                OrchestratorEvent::ToolCall { .. } => panic!("verification must not invoke agents"),
                OrchestratorEvent::Error { message } => panic!("provider stream failed: {message}"),
                _ => {}
            }
        }
        assert!(done);
        assert!(content_events > 1, "must observe incremental text");
        let usage = usages.pop().unwrap();
        assert_eq!(
            usage.total_tokens,
            usage.input_tokens
                + usage.output_tokens
                + usage.cache_read_tokens
                + usage.cache_creation_tokens
        );
        results.push(json!({"usage": usage, "content_events": content_events}));
    }
    let path = std::env::var("LIVE_STREAM_RESULTS").expect("result output path");
    std::fs::write(path, serde_json::to_vec_pretty(&results).unwrap()).unwrap();
}
