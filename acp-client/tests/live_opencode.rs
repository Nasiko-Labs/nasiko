//! Live integration test for the ACP client — the NAS-836 acceptance criterion:
//! a full turn against `opencode acp`.
//!
//! Ignored by default because it needs OpenCode installed and authenticated
//! (it uses OpenCode's own LLM credentials). Run explicitly with:
//!
//! ```bash
//! cargo test -p nasiko-acp-client --test live_opencode -- --ignored --nocapture
//! ```
//!
//! Override the agent command with `NASIKO_ACP_CMD` (default `"opencode acp"`).

use nasiko_acp_client::{
    AcpConnectConfig, NewSessionParams, PermissionPolicy, StopReason, TurnEvent, connect_local,
};

#[tokio::test]
#[ignore = "requires an installed, authenticated `opencode` (ACP agent)"]
async fn full_turn_against_opencode() {
    let command = std::env::var("NASIKO_ACP_CMD").unwrap_or_else(|_| "opencode acp".to_string());

    let config = AcpConnectConfig {
        permission_policy: PermissionPolicy::ApproveAll,
        ..Default::default()
    };

    let conn = connect_local(&command, config)
        .await
        .expect("connect + initialize + negotiate");

    // The capability set we read at initialize (feeds later cold-path work).
    let caps = conn.capabilities();
    eprintln!(
        "negotiated: protocol={:?} load_session={} resume={} mcp_http={} mcp_sse={}",
        caps.protocol_version, caps.load_session, caps.resume, caps.mcp_http, caps.mcp_sse
    );

    let cwd = std::env::current_dir().expect("cwd");
    let session = conn
        .new_session(NewSessionParams::new(cwd))
        .await
        .expect("session/new");

    let mut turn = conn
        .prompt(session, "What is 2 + 2? Reply with just the number.")
        .expect("send prompt");

    let mut updates = 0usize;
    let mut stop = None;
    while let Some(event) = turn.recv().await {
        match event {
            TurnEvent::Update(update) => {
                updates += 1;
                eprintln!("update #{updates}: {update:?}");
            }
            TurnEvent::Finished(reason) => {
                stop = Some(reason);
                break;
            }
            TurnEvent::Failed(err) => panic!("turn failed: {err}"),
        }
    }

    assert_eq!(
        stop,
        Some(StopReason::EndTurn),
        "turn should end normally (got {stop:?} after {updates} updates)"
    );
}
