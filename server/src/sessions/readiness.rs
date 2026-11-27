//! Whether this server can run harness sessions at all, besides the `HARNESS_SESSIONS_ENABLED`
//! switch: a sandbox's credentials are signed with `AGENT_JWT_SECRET`, so without it every turn
//! fails, and a server whose sandbox provider is not available (a backend that is not built in or
//! not reachable) cannot start one. Until both hold, the catalog lists no harness and opening a
//! session is `503 harness_not_ready`, so no harness is offered that cannot work.

use crate::state::AppState;

/// Why sessions cannot run on this server, if they cannot: `(level_is_error, message)`, the
/// message naming the setting for the operator.
pub fn unavailable(state: &AppState) -> Option<(bool, String)> {
    let session_key = nasiko_runtime::SandboxKey::Session {
        session_id: uuid::Uuid::nil(),
    };
    blocked(
        !state.config.agent_jwt_secret.is_empty(),
        state.sandbox.is_available_for(&session_key),
        &state.sandbox.kind().to_string(),
    )
}

/// Logs once at boot why sessions, though switched on, cannot run: an error for a missing
/// secret, a warning for an unavailable provider. Nothing while sessions are switched off.
pub fn log_at_boot(state: &AppState) {
    if !state.config.harness_sessions_enabled {
        return;
    }
    match unavailable(state) {
        Some((true, reason)) => tracing::error!("{reason}"),
        Some((false, reason)) => tracing::warn!("{reason}"),
        None => {}
    }
}

fn blocked(
    jwt_secret_set: bool,
    provider_available: bool,
    provider_kind: &str,
) -> Option<(bool, String)> {
    if !jwt_secret_set {
        return Some((
            true,
            "harness sessions disabled: AGENT_JWT_SECRET unset".into(),
        ));
    }
    if !provider_available {
        return Some((
            false,
            format!("harness sessions disabled: sandbox provider {provider_kind} unavailable"),
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::blocked;

    #[test]
    fn sessions_need_a_secret_and_a_live_provider() {
        assert_eq!(blocked(true, true, "docker"), None);
        let (error, msg) = blocked(false, true, "docker").unwrap();
        assert!(error && msg.contains("AGENT_JWT_SECRET unset"), "{msg}");
        let (error, msg) = blocked(true, false, "pod").unwrap();
        assert!(!error, "an unavailable provider is a warning");
        assert_eq!(
            msg,
            "harness sessions disabled: sandbox provider pod unavailable"
        );
        // The secret is reported first when both are missing.
        assert!(blocked(false, false, "pod").unwrap().0);
    }
}
