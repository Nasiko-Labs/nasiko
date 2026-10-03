use super::*;
use serde_json::json;

fn request() -> ChatRequest {
    serde_json::from_value(json!({
        "model":"test-model","messages":[{"role":"user","content":"Find the calendar event"}],
        "tools":[
            {"type":"function","function":{"name":"calendar","description":"Find calendar events","parameters":{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}}},
            {"type":"function","function":{"name":"email","description":"Email messages","parameters":{"type":"object","properties":{}}}},
            {"type":"function","function":{"name":"lookup","description":"Resolve identifiers","parameters":{"type":"object","properties":{}}}}
        ]
    })).unwrap()
}

fn candidates() -> Vec<ToolCandidate> {
    build_candidates(request().tools.as_ref().unwrap()).unwrap()
}
fn result(scores: &[f64]) -> SelectionResult {
    SelectionResult {
        probabilities: scores
            .iter()
            .enumerate()
            .map(|(i, &p)| (format!("tool_{i:04}"), p))
            .collect(),
        ..Default::default()
    }
}
fn config() -> SelectionConfig {
    SelectionConfig {
        mode: SelectionMode::Jev,
        uncertainty_floor: None,
        api_key: SelectionApiKey::new("test-key".into()),
        ..Default::default()
    }
}

struct Fake(Result<SelectionResult, SelectionError>);
#[async_trait]
impl ToolSelector for Fake {
    async fn select(
        &self,
        _: &SelectionContext,
        _: &[ToolCandidate],
    ) -> Result<SelectionResult, SelectionError> {
        self.0.clone()
    }
}

#[test]
fn threshold_and_uncertainty_are_inclusive() {
    let c = candidates();
    assert_eq!(
        apply_policy(&c, &result(&[0.5, 0.49, 0.1]), &config())
            .unwrap()
            .indices,
        vec![0]
    );
    let cfg = SelectionConfig {
        uncertainty_floor: Some(0.4),
        ..config()
    };
    let selected = apply_policy(&c, &result(&[0.5, 0.4, 0.39]), &cfg).unwrap();
    assert_eq!(selected.indices, vec![0, 1]);
    assert_eq!(selected.uncertain_count, 1);
}

#[test]
fn mandatory_closure_can_overflow_but_is_never_dropped() {
    let mut c = candidates();
    c[0].mandatory = true;
    let cfg = SelectionConfig {
        max_tools: Some(1),
        max_tool_tokens: Some(0),
        dependencies: BTreeMap::from([("calendar".into(), vec!["lookup".into()])]),
        ..config()
    };
    let selected = apply_policy(&c, &result(&[0.0, 0.9, 0.0]), &cfg).unwrap();
    assert_eq!(selected.indices, vec![0, 2]);
    assert!(selected.budget_overflow);
    assert_eq!(selected.dependency_count, 1);
}

#[test]
fn budgets_rank_then_admit_whole_dependency_bundles() {
    let c = candidates();
    let cfg = SelectionConfig {
        max_tools: Some(1),
        dependencies: BTreeMap::from([("calendar".into(), vec!["lookup".into()])]),
        ..config()
    };
    assert_eq!(
        apply_policy(&c, &result(&[0.9, 0.8, 0.0]), &cfg)
            .unwrap()
            .indices,
        vec![1]
    );
    let cfg = SelectionConfig {
        max_tool_tokens: Some(c[1].estimated_tokens),
        ..config()
    };
    assert_eq!(
        apply_policy(&c, &result(&[0.9, 0.8, 0.0]), &cfg)
            .unwrap()
            .indices,
        vec![1]
    );
    let cfg = SelectionConfig {
        max_tools: Some(1),
        ..config()
    };
    assert_eq!(
        apply_policy(&c, &result(&[0.9, 0.9, 0.0]), &cfg)
            .unwrap()
            .indices,
        vec![0]
    );
}

#[test]
fn closure_handles_cycles_and_rejects_unknown_endpoints() {
    let c = candidates();
    let graph = BTreeMap::from([
        ("calendar".into(), vec!["lookup".into()]),
        ("lookup".into(), vec!["calendar".into()]),
    ]);
    let seeds = BTreeSet::from([0]);
    let closed = dependency_closure(&c, &seeds, &graph).unwrap();
    assert_eq!(closed, BTreeSet::from([0, 2]));
    assert_eq!(dependency_closure(&c, &closed, &graph).unwrap(), closed);
    assert!(
        dependency_closure(
            &c,
            &seeds,
            &BTreeMap::from([("calendar".into(), vec!["invented".into()])])
        )
        .is_err()
    );
}

#[test]
fn reject_corrupt_results_atomically() {
    let c = candidates();
    for p in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
        assert_eq!(
            apply_policy(&c, &result(&[0.9, p, 0.1]), &config()).unwrap_err(),
            SelectionError::Probability
        );
    }
    assert_eq!(
        apply_policy(&c, &result(&[0.9]), &config()).unwrap_err(),
        SelectionError::Incomplete
    );
    let mut r = result(&[0.9, 0.1, 0.0]);
    r.probabilities.insert("foreign".into(), 1.0);
    assert_eq!(
        apply_policy(&c, &r, &config()).unwrap_err(),
        SelectionError::UnknownId
    );
    let mut c = c;
    c[1].id = c[0].id.clone();
    assert_eq!(
        apply_policy(&c, &result(&[0.9, 0.1, 0.0]), &config()).unwrap_err(),
        SelectionError::InvalidPolicy
    );
}

#[test]
fn property_mandatory_additions_and_registry_safety_without_budgets() {
    // Exhaustively enumerate score/mandatory sets on this tiny registry.
    // With hard caps, adding mandatory tools may displace optional tools by design.
    for scores in 0..8 {
        let r = result(
            &(0..3)
                .map(|i| if scores & (1 << i) != 0 { 1.0 } else { 0.0 })
                .collect::<Vec<_>>(),
        );
        for mandatory in 0..8 {
            let mut c = candidates();
            for (i, c) in c.iter_mut().enumerate() {
                c.mandatory = mandatory & (1 << i) != 0;
            }
            let cfg = SelectionConfig {
                min_selected_tools: 0,
                ..config()
            };
            let before = apply_policy(&c, &r, &cfg)
                .unwrap()
                .indices
                .into_iter()
                .collect::<BTreeSet<_>>();
            assert!(before.iter().all(|&i| i < c.len()));
            for i in 0..3 {
                let mut more = c.clone();
                more[i].mandatory = true;
                let after = apply_policy(&more, &r, &cfg)
                    .unwrap()
                    .indices
                    .into_iter()
                    .collect::<BTreeSet<_>>();
                assert!(before.is_subset(&after));
            }
        }
    }
}

#[test]
fn candidate_metadata_is_bounded_and_deterministic() {
    let mut req = request();
    req.tools.as_mut().unwrap()[0].function.description = Some("é \n".repeat(5000));
    let a = build_candidates(req.tools.as_ref().unwrap()).unwrap();
    let b = build_candidates(req.tools.as_ref().unwrap()).unwrap();
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap()
    );
    assert!(a[0].description.as_ref().unwrap().chars().count() <= 384);
    assert!(a[0].input_summary.contains("id!:string"));
    req.tools.as_mut().unwrap()[1].function.name = "calendar".into();
    assert!(build_candidates(req.tools.as_ref().unwrap()).is_err());
}

#[test]
fn credential_debug_is_redacted() {
    let cfg = config();
    assert!(!format!("{cfg:?}").contains("test-key"));
}

#[tokio::test]
async fn incomplete_request_context_and_invalid_dependency_config_preserve_all() {
    let mut req = request();
    req.messages[0].content = Some(json!("x".repeat(4097)));
    let fake = Fake(Ok(result(&[0.9, 0.0, 0.0])));
    let outcome = select_request(&req, &config(), &reqwest::Client::new(), Some(&fake))
        .await
        .unwrap();
    assert_eq!(outcome.indices, vec![0, 1, 2]);
    assert!(!outcome.telemetry.selector_invoked);
    assert_eq!(outcome.telemetry.fallback, Some(SelectionError::InputLimit));
    let cfg = SelectionConfig {
        fallback: FallbackMode::DeterministicSubset,
        dependencies: BTreeMap::from([("calendar".into(), vec!["foreign".into()])]),
        ..config()
    };
    let outcome = select_request(&request(), &cfg, &reqwest::Client::new(), Some(&fake))
        .await
        .unwrap();
    assert_eq!(outcome.indices, vec![0, 1, 2]);
    assert!(!outcome.telemetry.selector_invoked);
    assert_eq!(
        outcome.telemetry.fallback,
        Some(SelectionError::InvalidPolicy)
    );
    let cfg = SelectionConfig {
        configuration_error: Some(SelectionError::InvalidPolicy),
        ..config()
    };
    let outcome = select_request(&request(), &cfg, &reqwest::Client::new(), Some(&fake))
        .await
        .unwrap();
    assert_eq!(outcome.indices, vec![0, 1, 2]);
    assert!(!outcome.telemetry.selector_invoked);
    let cfg = SelectionConfig {
        max_tools: Some(1),
        ..config()
    };
    let outcome = select_request(
        &request(),
        &cfg,
        &reqwest::Client::new(),
        Some(&Fake(Err(SelectionError::Auth))),
    )
    .await
    .unwrap();
    assert_eq!(outcome.indices, vec![0, 1, 2]);
    assert!(outcome.telemetry.budget_overflow);
}

#[tokio::test]
async fn economic_gate_preserves_catalog_and_explicit_budgets_take_precedence() {
    let cfg = SelectionConfig {
        min_jev_catalog_tokens: u32::MAX,
        ..config()
    };
    let outcome = select_request(
        &request(),
        &cfg,
        &reqwest::Client::new(),
        Some(&Fake(Err(SelectionError::Auth))),
    )
    .await
    .unwrap();
    assert_eq!(outcome.indices, vec![0, 1, 2]);
    assert!(!outcome.telemetry.selector_invoked);
    assert_eq!(
        outcome.telemetry.selector_skip_reason,
        Some("below_cost_floor")
    );
    let cfg = SelectionConfig {
        max_tools: Some(1),
        ..cfg
    };
    let outcome = select_request(
        &request(),
        &cfg,
        &reqwest::Client::new(),
        Some(&Fake(Ok(result(&[0.9, 0.1, 0.0])))),
    )
    .await
    .unwrap();
    assert!(outcome.telemetry.selector_invoked);
    assert_eq!(outcome.indices, vec![0]);
}

#[tokio::test]
async fn single_catalog_avoids_external_selector_cost() {
    let mut req = request();
    req.tools.as_mut().unwrap().truncate(1);
    let outcome = select_request(
        &req,
        &config(),
        &reqwest::Client::new(),
        Some(&Fake(Err(SelectionError::Auth))),
    )
    .await
    .unwrap();
    assert_eq!(outcome.indices, vec![0]);
    assert!(!outcome.telemetry.selector_invoked);
    assert!(outcome.telemetry.fallback.is_none());
    assert_eq!(outcome.telemetry.selection_usage.unwrap().input_tokens, 0);
}

#[tokio::test]
async fn hybrid_sends_only_remaining_optional_candidates() {
    let mut req = request();
    req.messages[0].content = Some(json!("Use calendar"));
    let cfg = SelectionConfig {
        mode: SelectionMode::Hybrid,
        ..config()
    };
    let r = SelectionResult {
        probabilities: BTreeMap::from([("tool_0001".into(), 0.0), ("tool_0002".into(), 0.0)]),
        ..Default::default()
    };
    let outcome = select_request(&req, &cfg, &reqwest::Client::new(), Some(&Fake(Ok(r))))
        .await
        .unwrap();
    assert_eq!(outcome.indices, vec![0]);
    assert!(outcome.telemetry.selector_invoked);
    assert_eq!(outcome.telemetry.jev_selected_tools, 0);
}

struct Slow;
#[async_trait]
impl ToolSelector for Slow {
    async fn select(
        &self,
        _: &SelectionContext,
        _: &[ToolCandidate],
    ) -> Result<SelectionResult, SelectionError> {
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(result(&[0.9, 0.0, 0.0]))
    }
}

#[tokio::test]
async fn total_selector_timeout_falls_back() {
    let cfg = SelectionConfig {
        timeout_ms: 1,
        ..config()
    };
    let outcome = select_request(&request(), &cfg, &reqwest::Client::new(), Some(&Slow))
        .await
        .unwrap();
    assert_eq!(outcome.indices, vec![0, 1, 2]);
    assert_eq!(outcome.telemetry.fallback, Some(SelectionError::Timeout));
}

#[test]
fn duplicate_json_answer_ids_are_rejected() {
    let raw = br#"{"model":"version","answers":{"tool_0000":{"type":"noul","noul":0.9},"tool_0000":{"type":"noul","noul":0.1}}}"#;
    assert_eq!(
        JevSelector::parse_response(raw, &candidates()[..1]).unwrap_err(),
        SelectionError::Malformed
    );
}

#[tokio::test]
async fn fake_selector_subset_and_all_error_fallbacks() {
    let req = request();
    let http = reqwest::Client::new();
    let subset = select_request(
        &req,
        &config(),
        &http,
        Some(&Fake(Ok(result(&[0.9, 0.1, 0.1])))),
    )
    .await
    .unwrap();
    assert_eq!(subset.indices, vec![0]);
    assert_eq!(subset.telemetry.jev_selected_tools, 1);
    assert_eq!(
        serde_json::to_value(&req).unwrap()["tools"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    for error in [
        SelectionError::Auth,
        SelectionError::Configuration,
        SelectionError::Timeout,
        SelectionError::RateLimit,
        SelectionError::Service,
        SelectionError::Transport,
        SelectionError::Malformed,
        SelectionError::UnknownId,
        SelectionError::Incomplete,
        SelectionError::Probability,
    ] {
        for mode in [
            FallbackMode::Native,
            FallbackMode::CompactAll,
            FallbackMode::DeterministicSubset,
        ] {
            let cfg = SelectionConfig {
                fallback: mode,
                ..config()
            };
            let fallback = select_request(&req, &cfg, &http, Some(&Fake(Err(error))))
                .await
                .unwrap();
            assert!(!fallback.indices.is_empty());
            assert_eq!(fallback.telemetry.fallback, Some(error));
            assert_eq!(fallback.native_fallback, mode == FallbackMode::Native);
            assert!(fallback.indices.iter().all(|&i| i < 3));
        }
    }
}

#[tokio::test]
async fn invalid_partial_and_empty_results_retain_all() {
    let req = request();
    let http = reqwest::Client::new();
    for r in [result(&[0.9]), result(&[0.0, 0.0, 0.0])] {
        let fallback = select_request(&req, &config(), &http, Some(&Fake(Ok(r))))
            .await
            .unwrap();
        assert_eq!(fallback.indices, vec![0, 1, 2]);
        assert!(fallback.telemetry.fallback.is_some());
    }
}

#[tokio::test]
async fn forced_and_configured_mandatory_tools_skip_jev_exclusion() {
    let mut req = request();
    req.tool_choice = Some(json!({"type":"function","function":{"name":"calendar"}}));
    let cfg = SelectionConfig {
        mandatory_tools: vec!["lookup".into()],
        ..config()
    };
    // Only email is optional and answered; forced/mandatory candidates are never asked.
    let r = SelectionResult {
        probabilities: BTreeMap::from([("tool_0001".into(), 0.0)]),
        ..Default::default()
    };
    let selected = select_request(&req, &cfg, &reqwest::Client::new(), Some(&Fake(Ok(r))))
        .await
        .unwrap();
    assert_eq!(selected.indices, vec![0, 2]);
    assert_eq!(selected.telemetry.mandatory_tools, 2);
    let mut gateway = crate::GatewayConfig {
        tool_compaction_enabled: true,
        ..Default::default()
    };
    gateway.tool_selection = SelectionConfig {
        mode: SelectionMode::Deterministic,
        ..cfg
    };
    let original_choice = req.tool_choice.clone();
    assert!(
        crate::tool_compaction::transform(&mut req, &gateway, &reqwest::Client::new())
            .await
            .is_none()
    );
    assert_eq!(req.tool_choice, original_choice);
    assert!(
        req.tools
            .as_ref()
            .unwrap()
            .iter()
            .any(|t| t.function.name == "calendar")
    );
}

#[tokio::test]
async fn off_master_switch_and_phase1_identity() {
    let http = reqwest::Client::new();
    let mut req = request();
    let before = serde_json::to_vec(&req).unwrap();
    let cfg = crate::GatewayConfig {
        tool_selection: config(),
        ..Default::default()
    };
    assert!(
        crate::tool_compaction::transform(&mut req, &cfg, &http)
            .await
            .is_none()
    );
    assert_eq!(serde_json::to_vec(&req).unwrap(), before);
    let cfg = crate::GatewayConfig {
        tool_compaction_enabled: true,
        ..Default::default()
    };
    let mut phase1 = req.clone();
    crate::tool_compaction::apply(&mut phase1, &cfg).unwrap();
    crate::tool_compaction::transform(&mut req, &cfg, &http)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_vec(&req).unwrap(),
        serde_json::to_vec(&phase1).unwrap()
    );
}

#[tokio::test]
async fn continuations_are_not_pruned_and_streaming_stays_native() {
    let http = reqwest::Client::new();
    let cfg = crate::GatewayConfig {
        tool_compaction_enabled: true,
        tool_selection: SelectionConfig {
            mode: SelectionMode::Deterministic,
            ..config()
        },
        ..Default::default()
    };
    let mut req = request();
    req.stream = Some(true);
    assert!(
        crate::tool_compaction::transform(&mut req, &cfg, &http)
            .await
            .is_none()
    );
    assert_eq!(req.tools.as_ref().unwrap().len(), 1);
    assert!(
        req.messages
            .iter()
            .all(|m| !m.text().unwrap_or_default().contains("<<call"))
    );
    let mut req = request();
    req.messages.push(
        serde_json::from_value(json!({"role":"tool","content":"result","tool_call_id":"id"}))
            .unwrap(),
    );
    let before = serde_json::to_vec(&req).unwrap();
    assert!(
        crate::tool_compaction::transform(&mut req, &cfg, &http)
            .await
            .is_none()
    );
    assert_eq!(serde_json::to_vec(&req).unwrap(), before);
}

#[tokio::test]
async fn subset_compaction_preserves_decoder_authority() {
    let http = reqwest::Client::new();
    let cfg = crate::GatewayConfig {
        tool_compaction_enabled: true,
        tool_selection: SelectionConfig {
            mode: SelectionMode::Deterministic,
            ..config()
        },
        ..Default::default()
    };
    let mut req = request();
    let applied = crate::tool_compaction::transform(&mut req, &cfg, &http)
        .await
        .unwrap();
    let catalog = req.messages.last().unwrap().text().unwrap();
    assert!(catalog.contains("calendar("));
    assert!(!catalog.contains("email("));
    let response = |text: &str| {
        serde_json::from_value::<crate::ir::ChatResponse>(json!({"id":"id","model":"test","choices":[{"index":0,"message":{"role":"assistant","content":text},"finish_reason":"stop"}]})).unwrap()
    };
    let mut resp = response("<<call calendar {\"id\":\"event-1\"}>>");
    crate::tool_compaction::decode_response(&mut resp, &applied).unwrap();
    assert!(resp.choices[0].message.content.is_none());
    assert_eq!(
        resp.choices[0].message.tool_calls.as_ref().unwrap()[0]
            .function
            .name,
        "calendar"
    );
    assert!(
        crate::tool_compaction::decode_response(&mut response("<<call email {}>>"), &applied)
            .is_err()
    );
    assert!(
        crate::tool_compaction::decode_response(&mut response("<<call calendar {}>>"), &applied)
            .is_err()
    );
}

#[tokio::test]
async fn unsupported_selected_schema_bypasses_without_loss() {
    let mut req = request();
    req.tools.as_mut().unwrap()[0]
        .function
        .parameters
        .as_mut()
        .unwrap()["oneOf"] = json!([]);
    let cfg = crate::GatewayConfig {
        tool_compaction_enabled: true,
        tool_selection: SelectionConfig {
            mode: SelectionMode::Deterministic,
            ..config()
        },
        ..Default::default()
    };
    assert!(
        crate::tool_compaction::transform(&mut req, &cfg, &reqwest::Client::new())
            .await
            .is_none()
    );
    assert_eq!(req.tools.as_ref().unwrap().len(), 1);
    assert_eq!(req.tools.as_ref().unwrap()[0].function.name, "calendar");
    assert!(
        req.messages
            .iter()
            .all(|m| !m.text().unwrap_or_default().contains("<<call"))
    );
}

#[test]
fn jev_contract_embeds_candidate_per_question_and_validates_typed_answers() {
    let c = candidates();
    let cfg = config();
    let adapter = JevSelector::new(reqwest::Client::new(), &cfg);
    let body = adapter
        .request_body(
            &SelectionContext {
                request_text: "calendar".into(),
            },
            &c,
        )
        .unwrap();
    assert_eq!(
        body["questions"]["tool_0000"]["instructions"]["candidate"]["name"],
        "calendar"
    );
    assert!(!body.to_string().contains("parameters"));
    assert!(!body.to_string().contains("test-key"));
    for (body, error) in [
        (
            json!({"model":"jev-version","answers":{}}),
            SelectionError::Incomplete,
        ),
        (
            json!({"model":"jev-version","answers":{"foreign":{"type":"noul","noul":0.9}}}),
            SelectionError::UnknownId,
        ),
        (
            json!({"model":"jev-version","answers":{"tool_0000":{"type":"score","noul":0.9}}}),
            SelectionError::Malformed,
        ),
        (
            json!({"model":"jev-version","answers":{"tool_0000":{"type":"noul","noul":1.2}}}),
            SelectionError::Probability,
        ),
    ] {
        assert_eq!(
            JevSelector::parse_response(body.to_string().as_bytes(), &c[..1]).unwrap_err(),
            error
        );
    }
}

#[tokio::test]
async fn http_adapter_maps_status_and_usage_and_reuses_client() {
    let mut server = mockito::Server::new_async().await;
    let c = candidates();
    let context = SelectionContext {
        request_text: "calendar".into(),
    };
    let cfg = SelectionConfig {
        jev_base_url: server.url(),
        ..config()
    };
    let adapter = JevSelector::new(reqwest::Client::new(), &cfg);
    let mock = server.mock("POST","/v1/systemone").match_header("authorization","Bearer test-key")
        .match_body(mockito::Matcher::PartialJson(json!({"questions":{"tool_0000":{"type":"noul","instructions":{"candidate":{"name":"calendar"}}}}})))
        .with_status(200).with_body(json!({"model":"jev-resolved","answers":{"tool_0000":{"type":"noul","noul":0.8}},"usage":{"input_tokens":21,"output_tokens":4}}).to_string()).create_async().await;
    let r = adapter.select(&context, &c[..1]).await.unwrap();
    assert_eq!(r.resolved_model.as_deref(), Some("jev-resolved"));
    assert_eq!(r.usage.unwrap().input_tokens, 21);
    mock.assert_async().await;
    mock.remove_async().await;
    for (status, error) in [
        (401, SelectionError::Auth),
        (403, SelectionError::Auth),
        (429, SelectionError::RateLimit),
        (529, SelectionError::Service),
        (422, SelectionError::Malformed),
    ] {
        let mock = server
            .mock("POST", "/v1/systemone")
            .with_status(status)
            .with_body("must not log this body")
            .create_async()
            .await;
        assert_eq!(adapter.select(&context, &c[..1]).await.unwrap_err(), error);
        mock.assert_async().await;
        mock.remove_async().await;
    }
}

#[tokio::test]
async fn native_and_compact_all_fallback_transform_paths() {
    let http = reqwest::Client::new();
    for fallback in [FallbackMode::Native, FallbackMode::CompactAll] {
        let mut req = request();
        let before = serde_json::to_vec(&req).unwrap();
        let cfg = crate::GatewayConfig {
            tool_compaction_enabled: true,
            tool_selection: SelectionConfig {
                fallback,
                api_key: SelectionApiKey::default(),
                ..config()
            },
            ..Default::default()
        };
        let compacted = crate::tool_compaction::transform(&mut req, &cfg, &http).await;
        if fallback == FallbackMode::Native {
            assert!(compacted.is_none());
            assert_eq!(serde_json::to_vec(&req).unwrap(), before);
        } else {
            assert!(compacted.is_some());
            let catalog = req.messages.last().unwrap().text().unwrap();
            for name in ["calendar(", "email(", "lookup("] {
                assert!(catalog.contains(name));
            }
        }
    }
}

#[tokio::test]
#[ignore = "requires explicitly supplied TYPESAFE_API_KEY and live API access"]
async fn live_jev_smoke() {
    let cfg = crate::GatewayConfig::from_env().tool_selection;
    let c = candidates();
    let result = JevSelector::new(reqwest::Client::new(), &cfg)
        .select(&selection_context(&request()), &c)
        .await
        .unwrap();
    super::policy::validate_result(&c, &result).unwrap();
    eprintln!(
        "resolved_model={:?}, usage={:?}",
        result.resolved_model, result.usage
    );
}
