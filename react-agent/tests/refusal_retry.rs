//! A policy refusal reaches the model the way the policy words it: guided once, then "do not
//! retry", then cut off. Driven through the streaming loop, which production uses.
use nasiko_react_agent::{
    AgentInfo, DelegationPolicy, Orchestrator, OrchestratorConfig, OrchestratorEvent,
    RegistrySource,
};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug)]
struct RefusesEverything;

impl DelegationPolicy for RefusesEverything {
    fn check_tool_call(&self, _arguments: &serde_json::Value) -> Result<(), String> {
        Err("confidence 70% is below the 80% required".to_string())
    }
    fn refusal_guidance(&self, prior_refusals: usize) -> Option<String> {
        (prior_refusals == 0).then(|| "Call it once more if it fits.".to_string())
    }
}

/// The same call, as a buffered completion.
fn buffered_tool_call() -> String {
    json!({"id":"c","model":"m","choices":[{"index":0,"message":{"role":"assistant",
        "tool_calls":[{"id":"call_1","type":"function",
        "function":{"name":"call_agent_helper","arguments":"{\"message\":\"hi\"}"}}]},
        "finish_reason":"tool_calls"}]})
    .to_string()
}

/// The model's streamed answer: one call to the `helper` agent, every turn.
fn streamed_tool_call() -> String {
    let call = json!({"id":"c","model":"m","choices":[{"index":0,"delta":{"tool_calls":[{
        "index":0,"id":"call_1","type":"function",
        "function":{"name":"call_agent_helper","arguments":"{\"message\":\"hi\"}"}}]},
        "finish_reason":null}]});
    let stop = json!({"id":"c","model":"m","choices":[{"index":0,"delta":{},
        "finish_reason":"tool_calls"}]});
    format!("data: {call}\n\ndata: {stop}\n\ndata: [DONE]\n\n")
}

#[tokio::test]
async fn a_refused_agent_gets_one_reassessment_then_is_closed_while_streaming() {
    let guided = Arc::new(AtomicBool::new(false));
    let told_not_to_retry = Arc::new(AtomicBool::new(false));
    let cut_off = Arc::new(AtomicBool::new(false));

    let mut llm = mockito::Server::new_async().await;
    // The loop streams a turn, or buffers it when a policy refused the one before.
    for (streaming, reply, content_type) in [
        (true, streamed_tool_call(), "text/event-stream"),
        (false, buffered_tool_call(), "application/json"),
    ] {
        let seen = (guided.clone(), told_not_to_retry.clone(), cut_off.clone());
        llm.mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(json!({"stream": streaming})))
            .with_status(200)
            .with_header("content-type", content_type)
            .with_body(reply)
            .match_request(move |request| {
                let body = request.utf8_lossy_body().unwrap_or_default();
                if body.contains("Call it once more if it fits.") {
                    seen.0.store(true, Ordering::SeqCst);
                }
                if body.contains("Do NOT retry this agent") {
                    seen.1.store(true, Ordering::SeqCst);
                }
                if body.contains("already failed 2 times") {
                    seen.2.store(true, Ordering::SeqCst);
                }
                true
            })
            .expect_at_least(0)
            .create_async()
            .await;
    }

    let mut orchestrator = Orchestrator::new(
        OrchestratorConfig {
            api_key: Some("test-only".into()),
            base_url: Some(llm.url()),
            model: "m".into(),
            max_turns: 5,
            policy: Some(Arc::new(RefusesEverything)),
            ..Default::default()
        },
        RegistrySource::Static(vec![AgentInfo {
            id: "agent".into(),
            name: "helper".into(),
            description: "test".into(),
            endpoint: "http://unused.invalid".into(),
            auth_header: None,
            skills: vec![],
        }]),
    );
    orchestrator.init().await.unwrap();

    let mut rejections = 0;
    let mut events = orchestrator.run_stream("question", vec![]);
    while let Some(event) = events.recv().await {
        if matches!(event, OrchestratorEvent::PolicyRejected { .. }) {
            rejections += 1;
        }
    }

    assert!(
        guided.load(Ordering::SeqCst),
        "first refusal should invite a re-assessment"
    );
    assert!(
        told_not_to_retry.load(Ordering::SeqCst),
        "second refusal should say not to retry"
    );
    assert!(
        cut_off.load(Ordering::SeqCst),
        "third ask should hit the retry limit"
    );
    // Two policy refusals, then every later ask is stopped by the retry limit.
    assert!(rejections >= 3, "got {rejections} rejections");
}
