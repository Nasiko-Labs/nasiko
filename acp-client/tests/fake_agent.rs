//! Hermetic end-to-end tests against an in-process fake ACP agent (NAS-836
//! review #15). No real binary, no network: the "agent" is the far end of an
//! in-process [`Channel`] pair, driven by hand as raw JSON-RPC so each test can
//! script exactly the handshake and turn behaviour it needs.
//!
//! Covers the client-loop guarantees the review called out:
//! - a full turn streams updates and finishes (ACP-1),
//! - a second `prompt` during a live turn is rejected (one active turn, #9),
//! - `cancel` reaches the agent *mid-turn* (#1),
//! - a version mismatch degrades to `Incompatible`, not a hang (ACP-2/15).
//!
//! The handshake-timeout guarantee has its own test in `handshake_timeout.rs`.

use std::time::Duration;

use futures::StreamExt;
use nasiko_acp_client::sdk::schema::ProtocolVersion;
use nasiko_acp_client::sdk::schema::v1::{
    ContentBlock, ContentChunk, InitializeResponse, NewSessionResponse, PromptResponse, RequestId,
    SessionId, SessionNotification, SessionUpdate, StopReason, TextContent,
};
use nasiko_acp_client::sdk::{Channel, RawJsonRpcMessage, TransportFrame};
use nasiko_acp_client::{AcpConnectConfig, AcpConnection, AcpError, NewSessionParams, TurnEvent};
use tokio::sync::oneshot;

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

/// One message read from the client, reduced to what the fake agent branches on.
enum Incoming {
    Request { id: RequestId, method: String },
    Notification { method: String },
    Closed,
}

/// Read the next single message the client sent.
async fn recv(peer: &mut Channel) -> Incoming {
    match peer.rx.next().await {
        Some(TransportFrame::Single(RawJsonRpcMessage::Request(req))) => Incoming::Request {
            id: req.id.clone(),
            method: req.method.to_string(),
        },
        Some(TransportFrame::Single(RawJsonRpcMessage::Notification(notif))) => {
            Incoming::Notification {
                method: notif.method.to_string(),
            }
        }
        Some(TransportFrame::Single(_)) => panic!("agent received an unexpected response message"),
        Some(TransportFrame::Batch(_)) => panic!("agent received an unexpected batch"),
        Some(_) => panic!("agent received a malformed frame"),
        None => Incoming::Closed,
    }
}

/// Reply to a request with a successful result.
fn respond(peer: &Channel, id: RequestId, result: serde_json::Value) {
    peer.tx
        .unbounded_send(TransportFrame::Single(RawJsonRpcMessage::response(
            id,
            Ok(result),
        )))
        .expect("client should accept the response");
}

/// Push a `session/update` notification to the client mid-turn.
fn notify_update(peer: &Channel, session_id: &SessionId, text: &str) {
    let raw = RawJsonRpcMessage::notification(
        "session/update".to_owned(),
        serde_json::to_value(SessionNotification::new(
            session_id.clone(),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new(text),
            ))),
        ))
        .expect("session notification should serialize"),
    )
    .expect("session notification should form valid JSON-RPC");
    peer.tx
        .unbounded_send(TransportFrame::Single(raw))
        .expect("client should accept the update");
}

/// Expect the next message to be a request for `method`; return its id.
async fn expect_request(peer: &mut Channel, method: &str) -> RequestId {
    match recv(peer).await {
        Incoming::Request { id, method: got } => {
            assert_eq!(got, method, "unexpected request method");
            id
        }
        Incoming::Notification { method: got } => {
            panic!("expected {method}, got notification {got}")
        }
        Incoming::Closed => panic!("client closed before sending {method}"),
    }
}

/// Answer `initialize` (v1) then `session/new`, returning the session id handed back.
async fn handshake(peer: &mut Channel) -> SessionId {
    let init_id = expect_request(peer, "initialize").await;
    respond(
        peer,
        init_id,
        serde_json::to_value(InitializeResponse::new(ProtocolVersion::V1)).unwrap(),
    );

    let new_id = expect_request(peer, "session/new").await;
    let session_id = SessionId::new("fake-session");
    respond(
        peer,
        new_id,
        serde_json::to_value(NewSessionResponse::new(session_id.clone())).unwrap(),
    );
    session_id
}

#[tokio::test]
async fn full_turn_streams_updates_and_finishes() {
    let (transport, mut peer) = Channel::duplex();

    let agent = tokio::spawn(async move {
        let session_id = handshake(&mut peer).await;
        let prompt_id = expect_request(&mut peer, "session/prompt").await;
        notify_update(&peer, &session_id, "working on it");
        respond(
            &peer,
            prompt_id,
            serde_json::to_value(PromptResponse::new(StopReason::EndTurn)).unwrap(),
        );
        // Drain until the client drops the connection.
        while peer.rx.next().await.is_some() {}
    });

    let conn = AcpConnection::connect(transport, AcpConnectConfig::default())
        .await
        .expect("connect");
    let sid = conn
        .new_session(NewSessionParams::new("/workspace"))
        .await
        .expect("new_session");
    let mut turn = conn.prompt(sid, "do the thing").expect("prompt");

    let mut saw_update = false;
    let stop = loop {
        match turn.recv().await {
            Some(TurnEvent::Update(_)) => saw_update = true,
            Some(TurnEvent::Finished(reason)) => break reason,
            Some(TurnEvent::Failed(e)) => panic!("turn failed: {e:?}"),
            None => panic!("stream ended without a terminal event"),
        }
    };
    assert!(saw_update, "expected at least one streamed update");
    assert!(matches!(stop, StopReason::EndTurn));

    drop(conn);
    let _ = tokio::time::timeout(TEST_TIMEOUT, agent).await;
}

#[tokio::test]
async fn overlapping_prompt_is_rejected() {
    let (transport, mut peer) = Channel::duplex();
    let (release_tx, release_rx) = oneshot::channel::<()>();

    let agent = tokio::spawn(async move {
        handshake(&mut peer).await;
        let prompt_id = expect_request(&mut peer, "session/prompt").await;
        // Hold the turn open until the test has attempted the second prompt.
        let _ = release_rx.await;
        respond(
            &peer,
            prompt_id,
            serde_json::to_value(PromptResponse::new(StopReason::EndTurn)).unwrap(),
        );
        while peer.rx.next().await.is_some() {}
    });

    let conn = AcpConnection::connect(transport, AcpConnectConfig::default())
        .await
        .expect("connect");
    let sid = conn
        .new_session(NewSessionParams::new("/workspace"))
        .await
        .expect("new_session");
    let mut turn = conn.prompt(sid.clone(), "first").expect("first prompt");

    // A second prompt while the first turn is live must be rejected up front.
    match conn.prompt(sid, "second") {
        Err(AcpError::TurnInProgress) => {}
        Err(other) => panic!("expected TurnInProgress, got {other:?}"),
        Ok(_) => panic!("expected TurnInProgress, got a live second turn"),
    }

    // Release the held turn; it should still complete cleanly.
    let _ = release_tx.send(());
    let stop = loop {
        match turn.recv().await {
            Some(TurnEvent::Finished(reason)) => break reason,
            Some(TurnEvent::Failed(e)) => panic!("turn failed: {e:?}"),
            Some(_) => {}
            None => panic!("stream ended without a terminal event"),
        }
    };
    assert!(matches!(stop, StopReason::EndTurn));

    drop(conn);
    let _ = tokio::time::timeout(TEST_TIMEOUT, agent).await;
}

#[tokio::test]
async fn cancel_reaches_the_agent_mid_turn() {
    let (transport, mut peer) = Channel::duplex();
    // Fires once the agent has the prompt in hand, so the test cancels a turn that
    // is genuinely underway (not before `run_turn` has started).
    let (started_tx, started_rx) = oneshot::channel::<()>();
    // Fires once the agent has observed the `session/cancel` the client sent.
    let (cancelled_tx, cancelled_rx) = oneshot::channel::<()>();

    let agent = tokio::spawn(async move {
        handshake(&mut peer).await;
        let prompt_id = expect_request(&mut peer, "session/prompt").await;
        let _ = started_tx.send(());

        // Answer the prompt only after the client's cancel arrives.
        loop {
            match recv(&mut peer).await {
                Incoming::Notification { method } if method == "session/cancel" => {
                    let _ = cancelled_tx.send(());
                    break;
                }
                Incoming::Notification { .. } | Incoming::Request { .. } => {}
                Incoming::Closed => panic!("client closed before cancelling"),
            }
        }
        respond(
            &peer,
            prompt_id,
            serde_json::to_value(PromptResponse::new(StopReason::EndTurn)).unwrap(),
        );
        while peer.rx.next().await.is_some() {}
    });

    let conn = AcpConnection::connect(transport, AcpConnectConfig::default())
        .await
        .expect("connect");
    let sid = conn
        .new_session(NewSessionParams::new("/workspace"))
        .await
        .expect("new_session");
    let mut turn = conn.prompt(sid.clone(), "long task").expect("prompt");

    // Wait until the turn is running on the agent, then cancel it.
    tokio::time::timeout(TEST_TIMEOUT, started_rx)
        .await
        .expect("agent never received the prompt")
        .expect("start signal dropped");
    conn.cancel(sid).expect("cancel");

    let stop = loop {
        match turn.recv().await {
            Some(TurnEvent::Finished(reason)) => break reason,
            Some(TurnEvent::Failed(e)) => panic!("turn failed: {e:?}"),
            Some(_) => {}
            None => panic!("stream ended without a terminal event"),
        }
    };
    assert!(matches!(stop, StopReason::EndTurn));

    // The cancel must have reached the agent while the turn was still open.
    tokio::time::timeout(TEST_TIMEOUT, cancelled_rx)
        .await
        .expect("agent never observed the cancel")
        .expect("cancel signal dropped");

    drop(conn);
    let _ = tokio::time::timeout(TEST_TIMEOUT, agent).await;
}

#[tokio::test]
async fn version_mismatch_degrades_to_incompatible() {
    let (transport, mut peer) = Channel::duplex();

    let agent = tokio::spawn(async move {
        let init_id = expect_request(&mut peer, "initialize").await;
        // Answer with a protocol version this client does not speak.
        respond(
            &peer,
            init_id,
            serde_json::to_value(InitializeResponse::new(ProtocolVersion::V0)).unwrap(),
        );
        while peer.rx.next().await.is_some() {}
    });

    match AcpConnection::connect(transport, AcpConnectConfig::default()).await {
        Err(AcpError::Incompatible(_)) => {}
        Err(other) => panic!("expected Incompatible, got {other:?}"),
        Ok(_) => panic!("expected incompatible negotiation, got a live connection"),
    }

    let _ = tokio::time::timeout(TEST_TIMEOUT, agent).await;
}
