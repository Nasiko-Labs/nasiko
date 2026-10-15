//! Hermetic test for the handshake budget (NAS-836 review): an agent that accepts
//! the transport but never answers `initialize` must not wedge `connect()` — it
//! has to fail with [`AcpError::Timeout`] within the configured window, so the
//! caller is free to fall back to the hook path.
//!
//! No real agent, no network: the "agent" is one end of two in-memory pipes that
//! we deliberately never write to.

use std::time::Duration;

use nasiko_acp_client::sdk::ByteStreams;
use nasiko_acp_client::{AcpConnectConfig, AcpConnection, AcpError};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

#[tokio::test]
async fn connect_times_out_when_the_agent_never_answers_initialize() {
    // Two simplex pipes model the stdio bridge: the client writes its request on
    // one and reads replies on the other. We hold both agent-side halves open but
    // never read or write them, so the client's `initialize` request is delivered
    // yet never answered and the reply stream never hits EOF — a pure hang.
    let (agent_in, client_out) = tokio::io::duplex(4096);
    let (client_in, agent_out) = tokio::io::duplex(4096);
    let transport = ByteStreams::new(client_out.compat_write(), client_in.compat());

    let config = AcpConnectConfig {
        handshake_timeout: Duration::from_millis(200),
        request_timeout: Duration::from_millis(200),
        ..Default::default()
    };

    let started = std::time::Instant::now();
    let result = AcpConnection::connect(transport, config).await;
    let elapsed = started.elapsed();

    // Matched by arm (not `expect_err`) so we never need `AcpConnection: Debug`.
    match result {
        Err(AcpError::Timeout(_)) => {}
        Err(other) => panic!("expected Timeout, got {other:?}"),
        Ok(_) => panic!("expected the handshake to time out, got a live connection"),
    }
    // It should give up near the budget, not hang indefinitely.
    assert!(
        elapsed < Duration::from_secs(5),
        "connect took {elapsed:?}, far past the 200ms budget"
    );

    // Keep the agent halves alive until after the assertion, so the timeout — not
    // an early EOF — is what ends the handshake.
    drop((agent_in, agent_out));
}
