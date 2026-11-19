//! `FlowEventBus` across replicas, with a real Redis: two buses stand for two server replicas.
//!
//! These tests require a running Redis at REDIS_URL (default redis://127.0.0.1:6379).
//! Run them with:  cargo test -p nasiko-flow --test flow_events -- --include-ignored

use std::time::Duration;

use nasiko_flow::{FlowEvent, FlowEventBus};
use nasiko_redis::SharedRedis;

fn redis() -> SharedRedis {
    let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".into());
    SharedRedis::new(redis::Client::open(url).unwrap())
}

fn approval(tool: &str) -> FlowEvent {
    FlowEvent::ToolApprovalRequired {
        agent_id: "agent".into(),
        server: "server".into(),
        tool: tool.into(),
    }
}

#[tokio::test]
#[ignore = "requires Redis"]
async fn an_event_published_on_one_replica_reaches_a_subscriber_on_another() {
    let subscriber_replica = FlowEventBus::with_redis(redis());
    let publisher_replica = FlowEventBus::with_redis(redis());
    let flow_id = uuid::Uuid::new_v4().to_string();
    let mut events = subscriber_replica.subscribe(&flow_id).await;

    publisher_replica.publish(&flow_id, approval("first")).await;
    publisher_replica
        .publish(&flow_id, approval("second"))
        .await;

    for expected in ["first", "second"] {
        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("delivered across replicas")
            .unwrap();
        assert!(
            matches!(&event, FlowEvent::ToolApprovalRequired { tool, .. } if tool == expected),
            "{event:?}"
        );
    }
}

#[tokio::test]
#[ignore = "requires Redis"]
async fn a_local_subscriber_gets_its_own_replicas_event_once() {
    let bus = FlowEventBus::with_redis(redis());
    let flow_id = uuid::Uuid::new_v4().to_string();
    let mut events = bus.subscribe(&flow_id).await;

    bus.publish(&flow_id, approval("only")).await;

    assert!(events.recv().await.is_ok());
    // Long enough for the Redis round trip that must not deliver it a second time.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(events.try_recv().is_err(), "not delivered again from redis");
}

#[tokio::test]
#[ignore = "requires Redis"]
async fn another_flows_events_are_not_delivered() {
    let subscriber_replica = FlowEventBus::with_redis(redis());
    let publisher_replica = FlowEventBus::with_redis(redis());
    let flow_id = uuid::Uuid::new_v4().to_string();
    let mut events = subscriber_replica.subscribe(&flow_id).await;

    publisher_replica
        .publish(&uuid::Uuid::new_v4().to_string(), approval("other"))
        .await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(events.try_recv().is_err());
}
