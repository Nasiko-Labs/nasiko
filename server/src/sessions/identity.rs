//! The credentials a harness sandbox carries: a signed, session-bound token and the env that
//! points the harness's model and tool clients at the platform.
//!
//! The router and MCP gateway authorize the sandbox's calls off the token's signed claims, so
//! the token is the sandbox's identity: whose session it is, which agent, which flow.

use std::collections::HashMap;

use nasiko_config::Config;
use nasiko_llm_router::{InboundFormat, inject_llm_base_url};
use nasiko_session_token::{MintedToken, SessionTokenError, SessionTokenInput};
use uuid::Uuid;

/// Session-token lifetime: one hour, which also bounds how long a HITL approval can be
/// awaited.
pub const SESSION_TOKEN_TTL_SECS: u64 = 3600;

/// Lifetime of the token a sandbox carries in its environment: 30 days, and never shorter than
/// a turn's token (asserted below). The environment is fixed when the sandbox is created and the
/// sandbox is resumed across many turns, so the credential must outlive any single one; it is
/// revoked when the sandbox is cleared or the session deleted, not when it expires.
pub const SANDBOX_TOKEN_TTL_SECS: u64 = 30 * 24 * 3600;
const _: () = assert!(SANDBOX_TOKEN_TTL_SECS >= SESSION_TOKEN_TTL_SECS);

/// Mint the session-bound token (claims `{agent_id, user_id, session_id, flow_id, jti}`,
/// audience `nasiko-coding-session`). Fails closed when `AGENT_JWT_SECRET` is unset.
pub fn mint_session_token(
    config: &Config,
    agent_id: Uuid,
    user_id: Uuid,
    session_id: &str,
    flow_id: &str,
) -> Result<MintedToken, SessionTokenError> {
    mint_with_ttl(
        config,
        agent_id,
        user_id,
        session_id,
        flow_id,
        SESSION_TOKEN_TTL_SECS,
    )
}

/// Mint the token a sandbox carries in its environment ([`SANDBOX_TOKEN_TTL_SECS`]). The router
/// and gateway read the user, agent and session from its claims. Its `flow_id` is only the flow
/// of the turn that created the sandbox; which flow a given call belongs to is resolved
/// server-side per turn, not read from this claim.
pub fn mint_sandbox_token(
    config: &Config,
    agent_id: Uuid,
    user_id: Uuid,
    session_id: &str,
    flow_id: &str,
) -> Result<MintedToken, SessionTokenError> {
    mint_with_ttl(
        config,
        agent_id,
        user_id,
        session_id,
        flow_id,
        SANDBOX_TOKEN_TTL_SECS,
    )
}

fn mint_with_ttl(
    config: &Config,
    agent_id: Uuid,
    user_id: Uuid,
    session_id: &str,
    flow_id: &str,
    ttl_secs: u64,
) -> Result<MintedToken, SessionTokenError> {
    let input = SessionTokenInput {
        agent_id: agent_id.to_string(),
        user_id: user_id.to_string(),
        session_id: session_id.to_owned(),
        flow_id: flow_id.to_owned(),
    };
    nasiko_session_token::mint_session_token(
        &input,
        &config.agent_jwt_secret,
        ttl_secs,
        nasiko_session_token::parse_algorithm(&config.agent_jwt_algorithm),
    )
}

/// The bearer the sandbox's shim demands on `/acp`: `hex(HMAC-SHA256(secret, sandbox name))`.
/// Derived, not stored, so it is the same at create (written into the env) and on every later
/// attach (sent as the header), across resumes and control-plane restarts, and differs per
/// sandbox.
///
/// Rotating `AGENT_JWT_SECRET` invalidates every existing sandbox's env token and this bearer;
/// the turn path notices the token no longer verifies and releases the sandbox so it is rebuilt.
pub fn shim_token(
    secret: &str,
    key: &nasiko_runtime::SandboxKey,
) -> Result<String, hmac::digest::InvalidLength> {
    use hmac::{Hmac, Mac};
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes())?;
    mac.update(key.name().as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// The env a sandbox needs to reach the platform as the session's user.
///
/// The model keys carry the session token so whichever provider SDK the image uses
/// authenticates against the router; the base URLs point those SDKs at it. Empty URLs are
/// skipped, leaving the image's own defaults.
pub fn session_env(
    token: &str,
    llm_gateway_base_url: &str,
    mcp_gateway_url: &str,
) -> HashMap<String, String> {
    let mut env = HashMap::new();
    for key in [
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "NASIKO_SESSION_TOKEN",
    ] {
        env.insert(key.to_owned(), token.to_owned());
    }
    if !llm_gateway_base_url.trim().is_empty() {
        inject_llm_base_url(&mut env, llm_gateway_base_url, InboundFormat::OpenAi);
        inject_llm_base_url(&mut env, llm_gateway_base_url, InboundFormat::Anthropic);
    }
    if !mcp_gateway_url.trim().is_empty() {
        env.insert("MCP_GATEWAY_URL".to_owned(), mcp_gateway_url.to_owned());
        // For MCP clients that accept only a URL: the same endpoint with the credential in the
        // path, composed exactly as `McpInjector` does for container agents. The gateway
        // authorizes it identically to the bearer form. This URL is a secret.
        env.insert(
            "MCP_GATEWAY_CONNECT_URL".to_owned(),
            nasiko_mcp_gateway::injector::connect_url(mcp_gateway_url, token),
        );
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_carries_token_under_every_credential_key() {
        let env = session_env("tok", "http://gw:8080/", "http://srv/api/mcp");
        for key in [
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "NASIKO_SESSION_TOKEN",
        ] {
            assert_eq!(env[key], "tok");
        }
        assert_eq!(env["OPENAI_BASE_URL"], "http://gw:8080/v1");
        assert_eq!(env["ANTHROPIC_BASE_URL"], "http://gw:8080");
        assert_eq!(env["MCP_GATEWAY_URL"], "http://srv/api/mcp");
        assert_eq!(env["MCP_GATEWAY_CONNECT_URL"], "http://srv/api/mcp/s/tok");
    }

    #[test]
    fn shim_token_is_stable_per_sandbox_and_secret() {
        let key = |n: u128| nasiko_runtime::SandboxKey::Session {
            session_id: Uuid::from_u128(n),
        };
        let token = |secret, n| shim_token(secret, &key(n)).unwrap();
        let a = token("secret", 1);
        assert_eq!(a, token("secret", 1));
        assert_ne!(a, token("secret", 2));
        assert_ne!(a, token("other", 1));
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn empty_urls_are_left_unset() {
        let env = session_env("tok", " ", "");
        assert_eq!(env.len(), 3);
    }
}
