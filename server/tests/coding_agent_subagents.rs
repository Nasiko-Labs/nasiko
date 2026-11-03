//! Subagent sessions (R15): the parent link a coding-agent turn carries, how
//! `session/list` and `session/{id}` present it, and that a child's spend is
//! counted once — on the child, under the same harness agent.

mod common;

use chrono::{Duration, TimeZone, Utc};
use nasiko_types::coding_agent::CodingAgentSessionParent;
use nasiko_types::{
    CODING_AGENT_EVENT_VERSION, CapturePolicy, CodingAgentEventV1, CodingAgentLlmCall,
    CodingAgentSession, CodingAgentSource, CodingAgentTurn, coding_agent_event_id,
    coding_agent_session_id,
};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

/// The window the fixed-time fixtures fall in; the list defaults to 7 days.
const SINCE: &str = "limit=100&start_time=2020-01-01T00:00:00Z";
const WINDOW: &str = "start_time=2023-01-01T00%3A00%3A00Z&end_time=2024-01-01T00%3A00%3A00Z";
/// Every fixture turn's tokens: 10 input + 5 output + 2 cache read + 3 cache write.
const TURN_TOKENS: i64 = 20;

/// An admin user owning a `claude` coding-agent registration, and that agent.
async fn setup(server: &common::TestServer) -> (Uuid, Uuid) {
    let admin: Value = server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let user_id = Uuid::parse_str(admin["user_id"].as_str().unwrap()).unwrap();
    let agent_id = register_agent(server, user_id).await;
    (user_id, agent_id)
}

async fn register_agent(server: &common::TestServer, user_id: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, coding_agent_integration_id) \
         VALUES ('coding-agent', $1, 'claude') RETURNING id",
    )
    .bind(user_id)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn second_user(server: &common::TestServer) -> Uuid {
    let user = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, username, email) VALUES ($1, 'second', 'second@test.local')",
    )
    .bind(user)
    .execute(&server.db)
    .await
    .unwrap();
    register_agent(server, user).await;
    user
}

/// Turn `index` of `session`, `index` minutes after the base time, 20 tokens.
fn turn(session: &str, index: i64) -> CodingAgentEventV1 {
    let started_at = Utc.timestamp_opt(1_700_000_000, 0).unwrap() + Duration::minutes(index);
    let ended_at = started_at + Duration::seconds(10);
    let turn_id = format!("turn-{index}");
    CodingAgentEventV1 {
        version: CODING_AGENT_EVENT_VERSION,
        event_id: coding_agent_event_id("claude", session, &turn_id),
        captured_at: ended_at,
        source: CodingAgentSource {
            agent_id: "claude".into(),
            agent_name: "coding-agent".into(),
        },
        session: CodingAgentSession {
            id: coding_agent_session_id("claude", session),
            source_id: session.into(),
            title: None,
            parent: None,
        },
        turn: CodingAgentTurn {
            id: turn_id.clone(),
            prompt: Some("question".into()),
            response: Some("answer".into()),
            started_at,
            ended_at,
            llm_calls: vec![CodingAgentLlmCall {
                id: format!("call-{turn_id}"),
                provider: "anthropic".into(),
                model: "claude-test".into(),
                input_tokens: 10,
                output_tokens: 5,
                cache_read_tokens: 2,
                cache_creation_tokens: 3,
                accounting: None,
                started_at,
                ended_at,
            }],
            tool_calls: vec![],
        },
        capture_policy: CapturePolicy::Content,
    }
}

fn child_turn(
    session: &str,
    index: i64,
    parent: &str,
    tool_call_id: Option<&str>,
    agent_type: Option<&str>,
) -> CodingAgentEventV1 {
    let mut event = turn(session, index);
    event.session.parent = Some(CodingAgentSessionParent {
        source_id: parent.into(),
        tool_call_id: tool_call_id.map(Into::into),
        agent_type: agent_type.map(Into::into),
    });
    event
}

/// Posts `events` as `user_id` and returns each event's status.
async fn post(
    server: &common::TestServer,
    user_id: Uuid,
    events: &[CodingAgentEventV1],
) -> Vec<String> {
    let body: Value = common::as_member(
        server
            .client
            .post(server.url("/api/telemetry/coding-agent/events/batch")),
        &user_id.to_string(),
        "admin",
    )
    .json(&json!({"events": events}))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    body["data"]["results"]
        .as_array()
        .unwrap_or_else(|| panic!("no results: {body}"))
        .iter()
        .map(|r| r["status"].as_str().unwrap().to_owned())
        .collect()
}

async fn accept(server: &common::TestServer, user_id: Uuid, events: &[CodingAgentEventV1]) {
    for status in post(server, user_id, events).await {
        assert_eq!(status, "accepted");
    }
}

/// The server-scoped id of `user_id`'s session with this agent-supplied id.
async fn scoped_id(server: &common::TestServer, user_id: Uuid, source_session: &str) -> String {
    sqlx::query_scalar(
        "SELECT DISTINCT session_id FROM coding_agent_telemetry_events \
          WHERE user_id = $1 AND source_session_id = $2",
    )
    .bind(user_id)
    .bind(source_session)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

type Link = (Option<String>, Option<String>, Option<String>);

async fn link_of(server: &common::TestServer, session_id: &str) -> Link {
    sqlx::query_as(
        "SELECT parent_session_id, spawn_tool_call_id, subagent_type \
           FROM chat_sessions WHERE session_id = $1",
    )
    .bind(session_id)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn get_as(server: &common::TestServer, user_id: Uuid, path: &str) -> Value {
    let res = common::as_member(
        server.client.get(server.url(path)),
        &user_id.to_string(),
        "u",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200, "{path}");
    res.json().await.unwrap()
}

async fn list(server: &common::TestServer, user_id: Uuid, extra: &str) -> Value {
    get_as(
        server,
        user_id,
        &format!("/api/observability/session/list?{SINCE}{extra}"),
    )
    .await
}

fn rows(body: &Value) -> &Vec<Value> {
    body["data"]["sessions"].as_array().unwrap()
}

fn row<'a>(body: &'a Value, session_id: &str) -> &'a Value {
    rows(body)
        .iter()
        .find(|s| s["session_id"] == session_id)
        .unwrap_or_else(|| panic!("{session_id} not listed: {body}"))
}

fn listed(body: &Value, session_id: &str) -> bool {
    rows(body).iter().any(|s| s["session_id"] == session_id)
}

/// Pins every imported turn's cost, so totals do not depend on the pricing table.
async fn pin_turn_cost(server: &common::TestServer, cost: f64) {
    sqlx::query("UPDATE chat_messages SET cost_usd = $1 WHERE role = 'assistant'")
        .bind(cost)
        .execute(&server.db)
        .await
        .unwrap();
}

/// A parent with two turns and an "Explore" child with three, ingested child
/// first: the child is recorded and linked before its parent exists.
#[tokio::test]
#[serial]
async fn child_before_parent_links_and_the_list_nests_it() {
    let server = common::TestServer::start().await;
    let (user_id, _) = setup(&server).await;
    let child_turns: Vec<_> = (0..3)
        .map(|i| child_turn("child", i, "parent", Some("toolu_1"), Some("Explore")))
        .collect();
    accept(&server, user_id, &child_turns).await;
    let child = scoped_id(&server, user_id, "child").await;
    let (parent_link, _, _) = link_of(&server, &child).await;
    let parent_link = parent_link.expect("linked before the parent arrives");

    accept(&server, user_id, &[turn("parent", 0), turn("parent", 1)]).await;
    let parent = scoped_id(&server, user_id, "parent").await;
    assert_eq!(
        parent_link, parent,
        "the parent id is scoped like the child's"
    );
    assert_eq!(
        link_of(&server, &child).await,
        (
            Some(parent.clone()),
            Some("toolu_1".into()),
            Some("Explore".into())
        )
    );
    pin_turn_cost(&server, 0.25).await;

    let top = list(&server, user_id, "").await;
    assert!(
        !listed(&top, &child),
        "children are hidden by default: {top}"
    );
    let parent_row = row(&top, &parent);
    assert_eq!(parent_row["parent_session_id"], Value::Null);
    assert_eq!(parent_row["num_traces"], 2, "the parent's own turns only");
    assert_eq!(parent_row["token_usage"]["total"], 2 * TURN_TOKENS);
    assert_eq!(parent_row["cost_summary"]["total"]["cost"], 0.5);
    assert_eq!(parent_row["subagent_count"], 1);
    assert_eq!(
        parent_row["subagents"],
        json!({"turns": 3, "tokens": 3 * TURN_TOKENS, "cost": 0.75})
    );
    assert_eq!(
        top["data"]["matching_count"], 1,
        "the count matches the page"
    );

    let children = list(&server, user_id, &format!("&parent={parent}")).await;
    assert_eq!(rows(&children).len(), 1, "{children}");
    let child_row = row(&children, &child);
    assert_eq!(child_row["parent_session_id"], parent.as_str());
    assert_eq!(child_row["spawn_tool_call_id"], "toolu_1");
    assert_eq!(child_row["subagent_type"], "Explore");
    assert_eq!(child_row["num_traces"], 3);
    assert_eq!(child_row["subagent_count"], 0);
    assert_eq!(
        child_row["subagents"],
        json!({"turns": 0, "tokens": null, "cost": null})
    );
    assert_eq!(children["data"]["matching_count"], 1);
    let past_the_end = list(&server, user_id, &format!("&parent={parent}&offset=1")).await;
    assert!(
        rows(&past_the_end).is_empty(),
        "offset pages the children: {past_the_end}"
    );

    server.cleanup().await;
}

/// Parent first, then a child whose first turn lacks the spawn details: a later
/// turn fills them, and nothing repoints the child at another parent.
#[tokio::test]
#[serial]
async fn child_after_parent_fills_details_first_non_null_wins() {
    let server = common::TestServer::start().await;
    let (user_id, _) = setup(&server).await;
    accept(&server, user_id, &[turn("parent", 0)]).await;
    let parent = scoped_id(&server, user_id, "parent").await;

    accept(
        &server,
        user_id,
        &[child_turn("child", 0, "parent", None, None)],
    )
    .await;
    let child = scoped_id(&server, user_id, "child").await;
    assert_eq!(
        link_of(&server, &child).await,
        (Some(parent.clone()), None, None)
    );

    accept(
        &server,
        user_id,
        &[child_turn(
            "child",
            1,
            "parent",
            Some("toolu_9"),
            Some("general-purpose"),
        )],
    )
    .await;
    let filled = (
        Some(parent.clone()),
        Some("toolu_9".into()),
        Some("general-purpose".into()),
    );
    assert_eq!(link_of(&server, &child).await, filled);

    accept(
        &server,
        user_id,
        &[
            child_turn("child", 2, "parent", Some("toolu_other"), Some("Plan")),
            child_turn("child", 3, "someone-else", Some("toolu_x"), Some("X")),
        ],
    )
    .await;
    assert_eq!(
        link_of(&server, &child).await,
        filled,
        "first non-null wins"
    );

    server.cleanup().await;
}

/// A child whose parent never arrives stays reachable from the top level.
#[tokio::test]
#[serial]
async fn orphan_child_is_listed_at_the_top_level() {
    let server = common::TestServer::start().await;
    let (user_id, _) = setup(&server).await;
    accept(
        &server,
        user_id,
        &[child_turn("orphan", 0, "gone", None, None)],
    )
    .await;
    let orphan = scoped_id(&server, user_id, "orphan").await;

    let top = list(&server, user_id, "").await;
    assert!(row(&top, &orphan)["parent_session_id"].is_string());
    server.cleanup().await;
}

/// History imported before subagents were linked is re-sent with the link,
/// under the same event ids: a duplicate that records the link, not a conflict.
#[tokio::test]
#[serial]
async fn reimport_links_existing_history_without_duplicating_it() {
    let server = common::TestServer::start().await;
    let (user_id, _) = setup(&server).await;
    accept(&server, user_id, &[turn("parent", 0), turn("child", 0)]).await;
    let child = scoped_id(&server, user_id, "child").await;

    let relinked = child_turn("child", 0, "parent", Some("toolu_1"), Some("Explore"));
    assert_eq!(post(&server, user_id, &[relinked]).await, ["duplicate"]);
    let parent = scoped_id(&server, user_id, "parent").await;
    assert_eq!(link_of(&server, &child).await.0, Some(parent));
    let receipts: i64 = sqlx::query_scalar("SELECT count(*) FROM coding_agent_telemetry_events")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(receipts, 2);

    let mut conflicting = child_turn("child", 0, "parent", None, None);
    conflicting.turn.response = Some("a different answer".into());
    assert_eq!(post(&server, user_id, &[conflicting]).await, ["rejected"]);
    server.cleanup().await;
}

/// A child's turns are ordinary turns of the same harness agent: every total
/// counts them, once.
#[tokio::test]
#[serial]
async fn spend_totals_include_child_turns_exactly_once() {
    let server = common::TestServer::start().await;
    let (user_id, agent_id) = setup(&server).await;
    accept(&server, user_id, &[turn("parent", 0), turn("parent", 1)]).await;
    let child_turns: Vec<_> = (0..3)
        .map(|i| child_turn("child", i, "parent", Some("toolu_1"), Some("Explore")))
        .collect();
    accept(&server, user_id, &child_turns).await;

    let (rows, agents, tokens, cost): (i64, i64, i64, f64) = sqlx::query_as(
        "SELECT count(*), count(DISTINCT agent_id), \
                sum(input_tokens + output_tokens + cache_read_tokens + cache_creation_tokens)::bigint, \
                sum(cost_usd)::float8 FROM trace_usage",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!((rows, agents, tokens), (5, 1, 5 * TURN_TOKENS));
    let attributed: Uuid = sqlx::query_scalar("SELECT DISTINCT agent_id FROM trace_usage")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(
        attributed, agent_id,
        "children spend under the same harness agent"
    );

    let dashboard = get_as(
        &server,
        user_id,
        &format!("/api/observability/finops/dashboard?{WINDOW}"),
    )
    .await;
    assert_eq!(dashboard["data"]["token_usage"]["total_tokens"], tokens);
    let dashboard_cost = dashboard["data"]["summary"]["total_cost"].as_f64().unwrap();
    assert!((dashboard_cost - cost).abs() < 1e-9, "{dashboard}");

    let usage = get_as(
        &server,
        user_id,
        &format!("/api/observability/coding-agents/usage?{WINDOW}"),
    )
    .await;
    let totals = &usage["data"]["totals"];
    assert_eq!(totals["turns"], 5, "{usage}");
    assert_eq!(totals["tokens"], tokens, "{usage}");
    assert!((totals["cost_usd"].as_f64().unwrap() - cost).abs() < 1e-9);
    server.cleanup().await;
}

/// Another user sees neither someone else's children nor their totals.
#[tokio::test]
#[serial]
async fn another_users_children_are_not_visible() {
    let server = common::TestServer::start().await;
    let (owner, _) = setup(&server).await;
    accept(&server, owner, &[turn("parent", 0)]).await;
    accept(
        &server,
        owner,
        &[child_turn("child", 0, "parent", None, None)],
    )
    .await;
    let parent = scoped_id(&server, owner, "parent").await;

    let other = second_user(&server).await;
    let theirs = list(&server, other, &format!("&parent={parent}")).await;
    assert!(rows(&theirs).is_empty(), "{theirs}");
    assert_eq!(theirs["data"]["matching_count"], 0);
    let top = list(&server, other, "").await;
    assert!(!listed(&top, &parent), "{top}");

    let mine = list(&server, owner, &format!("&parent={parent}")).await;
    assert_eq!(rows(&mine).len(), 1);
    server.cleanup().await;
}

/// The detail names the session's parent link and lists its children with
/// their own totals. Tempo is unreachable here: the lineage is Postgres's.
#[tokio::test]
#[serial]
async fn session_detail_lists_subagents_and_the_parent_link() {
    let server = common::TestServer::start_with(|config| {
        config.observability_enabled = true;
        config.tempo_url = "http://127.0.0.1:1".into();
        config.loki_url = "http://127.0.0.1:1".into();
    })
    .await;
    let (user_id, _) = setup(&server).await;
    accept(&server, user_id, &[turn("parent", 0)]).await;
    let explore: Vec<_> = (1..3)
        .map(|i| child_turn("explore", i, "parent", Some("toolu_1"), Some("Explore")))
        .collect();
    accept(&server, user_id, &explore).await;
    accept(
        &server,
        user_id,
        &[child_turn(
            "plan",
            5,
            "parent",
            Some("toolu_2"),
            Some("Plan"),
        )],
    )
    .await;
    pin_turn_cost(&server, 0.25).await;
    let parent = scoped_id(&server, user_id, "parent").await;
    let explore = scoped_id(&server, user_id, "explore").await;
    let plan = scoped_id(&server, user_id, "plan").await;
    // An indexed trace, so the detail resolves the session without searching Tempo.
    for session in [&parent, &explore] {
        sqlx::query(
            "INSERT INTO session_traces (session_id, trace_id, created_at) \
             SELECT session_id, md5(session_id), created_at FROM chat_sessions WHERE session_id = $1",
        )
        .bind(session)
        .execute(&server.db)
        .await
        .unwrap();
    }

    let detail = get_as(
        &server,
        user_id,
        &format!("/api/observability/session/{parent}"),
    )
    .await;
    let session = &detail["data"]["session"];
    assert_eq!(session["parent_session_id"], Value::Null);
    assert_eq!(
        session["subagents"],
        json!([
            {"session_id": explore, "subagent_type": "Explore", "spawn_tool_call_id": "toolu_1",
             "turns": 2, "tokens": 2 * TURN_TOKENS, "cost": 0.5,
             "start_time": "2023-11-14T22:14:20.000Z", "end_time": "2023-11-14T22:15:30.000Z"},
            {"session_id": plan, "subagent_type": "Plan", "spawn_tool_call_id": "toolu_2",
             "turns": 1, "tokens": TURN_TOKENS, "cost": 0.25,
             "start_time": "2023-11-14T22:18:20.000Z", "end_time": "2023-11-14T22:18:30.000Z"},
        ])
    );

    let detail = get_as(
        &server,
        user_id,
        &format!("/api/observability/session/{explore}"),
    )
    .await;
    let session = &detail["data"]["session"];
    assert_eq!(session["parent_session_id"], parent.as_str());
    assert_eq!(session["spawn_tool_call_id"], "toolu_1");
    assert_eq!(session["subagent_type"], "Explore");
    assert_eq!(session["subagents"], json!([]));
    server.cleanup().await;
}
