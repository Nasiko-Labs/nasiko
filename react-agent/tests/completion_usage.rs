//! Mock HTTP integration tests for the public orchestrator usage event contract.
use nasiko_react_agent::{
    AgentInfo, DelegationPolicy, Orchestrator, OrchestratorConfig, OrchestratorEvent,
    RegistrySource,
};
use serde_json::json;
use std::sync::Arc;

#[derive(Debug)]
struct Buffered;
impl DelegationPolicy for Buffered {
    fn buffer_every_turn(&self) -> bool {
        true
    }
    fn undelegated_max_tokens(&self, _: usize) -> Option<u64> {
        Some(64)
    }
}

#[tokio::test]
async fn buffered_and_streamed_calls_emit_one_cache_aware_usage_before_done() {
    for streaming in [false, true] {
        let mut server = mockito::Server::new_async().await;
        let usage = json!({"prompt_tokens":4732,"completion_tokens":110,"total_tokens":4842,
            "prompt_tokens_details":{"cached_tokens":3968}});
        let body = if streaming {
            format!(
                "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                json!({"id":"c","model":"model-test","choices":[{"index":0,"delta":{"content":"answer"},"finish_reason":"stop"}]}),
                json!({"id":"c","model":"model-test","choices":[],"usage":usage})
            )
        } else {
            json!({"id":"c","model":"model-test","choices":[{"index":0,"message":{"role":"assistant","content":"answer"}}],"usage":usage}).to_string()
        };
        let expected = if streaming {
            json!({"stream":true,"stream_options":{"include_usage":true}})
        } else {
            json!({"stream":false,"max_completion_tokens":64})
        };
        let mock = server
            .mock("POST", "/chat/completions")
            .match_header("authorization", "Bearer test-only")
            .match_body(mockito::Matcher::PartialJson(expected))
            .with_status(200)
            .with_header(
                "content-type",
                if streaming {
                    "text/event-stream"
                } else {
                    "application/json"
                },
            )
            .with_body(body)
            .expect(1)
            .create_async()
            .await;
        let mut orchestrator = Orchestrator::new(
            OrchestratorConfig {
                api_key: Some("test-only".into()),
                base_url: Some(server.url()),
                model: "model-test".into(),
                policy: (!streaming).then(|| Arc::new(Buffered) as Arc<dyn DelegationPolicy>),
                ..Default::default()
            },
            RegistrySource::Static(vec![AgentInfo {
                id: "agent".into(),
                name: "helper".into(),
                description: "test".into(),
                endpoint: "http://unused.invalid".into(),
                skills: vec![],
            }]),
        );
        orchestrator.init().await.unwrap();
        let mut events = orchestrator.run_stream("question", vec![]);
        let mut usages = Vec::new();
        let mut done = false;
        while let Some(event) = events.recv().await {
            match event {
                OrchestratorEvent::Usage { usage } => {
                    assert!(!done);
                    usages.push(usage);
                }
                OrchestratorEvent::Done { .. } => {
                    assert_eq!(usages.len(), 1);
                    done = true;
                }
                OrchestratorEvent::Error { message } => panic!("{message}"),
                _ => {}
            }
        }
        assert!(done);
        assert_eq!(usages.len(), 1);
        let usage = &usages[0];
        assert_eq!(
            (
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_read_tokens,
                usage.cache_creation_tokens
            ),
            (764, 110, 3968, 0)
        );
        assert_eq!(usage.total_tokens, 4842);
        assert_eq!(usage.streaming, streaming);
        assert_eq!(
            usage.provider, None,
            "custom URL is not evidence of OpenAI hosting"
        );
        assert!(!usage.estimated);
        mock.assert_async().await;
    }
}
