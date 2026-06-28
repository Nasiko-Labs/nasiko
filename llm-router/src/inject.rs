//! Deploy-time agent wiring (Phase 2, P2.0).
//!
//! When the platform deploys a user-uploaded agent container, it must point the agent's
//! LLM SDK at this router and give it an identity token. [`inject_llm_env`] mints a
//! 1-year agent-identity JWT (the OSS minting decision — see `PHASE_2_PLAN.md`) and
//! writes the SDK-appropriate base-URL + key env vars into the deployment's `env_vars`,
//! just before `runtime.deploy(spec)`.
//!
//! This mirrors the observability `InstrumentationInjector` pattern
//! (`oss/observability/src/injector.rs`): a pure function that mutates an
//! `env_vars: HashMap<String, String>` from a small context.
//!
//! P2.0 ships the **OpenAI** mapping only (`OPENAI_BASE_URL` / `OPENAI_API_KEY`).
//! Per-SDK mappings (Anthropic / Gemini) are added in P2.5 once the inbound parsers
//! and the agent `inbound_format` attribute exist.

use std::collections::HashMap;

use crate::auth::{DEFAULT_TTL_SECONDS, mint_agent_token, parse_algorithm};
use crate::config::GatewayConfig;
use crate::error::GatewayError;
use crate::inbound::InboundFormat;

/// Inputs for [`inject_llm_env`], built from the agent being deployed.
pub struct LlmInjectCtx {
    /// `agents.id` (UUID string) — becomes the JWT `agent_id` claim.
    pub agent_id: String,
    /// Owner UUID string — becomes the JWT `owner_id` claim (may be empty).
    pub owner_id: String,
    /// Which SDK the agent speaks; determines the env-var names written.
    pub inbound_format: InboundFormat,
}

/// Mint an agent JWT and inject the base-URL + key env vars into `env_vars`.
///
/// On success the agent can reach the router with no code changes. Fails closed: if the
/// gateway isn't wired (`agent_jwt_secret` or `llm_gateway_base_url` empty) it returns an
/// error and **leaves `env_vars` untouched** — the caller (P2.1 deploy path) decides
/// whether that's a hard failure or a deploy-time warning.
///
/// Deploy is authoritative: existing values for the written keys are overwritten.
pub fn inject_llm_env(
    env_vars: &mut HashMap<String, String>,
    ctx: &LlmInjectCtx,
    cfg: &GatewayConfig,
) -> Result<(), GatewayError> {
    // Fail closed — never inject a base URL the agent can't authenticate against.
    if cfg.agent_jwt_secret.is_empty() {
        return Err(GatewayError::JwtSecretNotConfigured);
    }
    if cfg.llm_gateway_base_url.trim().is_empty() {
        return Err(GatewayError::Internal(
            "LLM_GATEWAY_BASE_URL not configured; cannot inject agent LLM wiring".into(),
        ));
    }

    let token = mint_agent_token(
        &ctx.agent_id,
        &ctx.owner_id,
        &cfg.agent_jwt_secret,
        DEFAULT_TTL_SECONDS,
        parse_algorithm(&cfg.agent_jwt_algorithm),
    )
    .map_err(|e| GatewayError::Internal(format!("failed to mint agent token: {e}")))?;

    // Gateway origin + the Pingora `/llm` strip route. Each SDK appends its own suffix:
    // the OpenAI SDK adds `/chat/completions` to `…/llm/v1`; the Anthropic SDK adds
    // `/v1/messages` to `…/llm`. So the injected base differs per SDK.
    let base = cfg.llm_gateway_base_url.trim_end_matches('/');

    match ctx.inbound_format {
        InboundFormat::OpenAi => {
            env_vars.insert("OPENAI_BASE_URL".into(), format!("{base}/llm/v1"));
            env_vars.insert("OPENAI_API_KEY".into(), token);
        }
        InboundFormat::Anthropic => {
            env_vars.insert("ANTHROPIC_BASE_URL".into(), format!("{base}/llm"));
            env_vars.insert("ANTHROPIC_API_KEY".into(), token);
        }
        InboundFormat::Gemini => {
            // The Google GenAI SDK appends `/v1beta/models/{model}:…` to its base URL.
            env_vars.insert("GOOGLE_GEMINI_BASE_URL".into(), format!("{base}/llm"));
            env_vars.insert("GEMINI_API_KEY".into(), token.clone());
            env_vars.insert("GOOGLE_API_KEY".into(), token);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::verify_agent_jwt;

    fn cfg() -> GatewayConfig {
        GatewayConfig {
            agent_jwt_secret: "deploy-test-secret".into(),
            llm_gateway_base_url: "http://gateway:8080".into(),
            ..Default::default()
        }
    }

    fn ctx() -> LlmInjectCtx {
        LlmInjectCtx {
            agent_id: "11111111-1111-1111-1111-111111111111".into(),
            owner_id: "22222222-2222-2222-2222-222222222222".into(),
            inbound_format: InboundFormat::OpenAi,
        }
    }

    #[test]
    fn injects_openai_base_url_and_key() {
        let mut env = HashMap::new();
        inject_llm_env(&mut env, &ctx(), &cfg()).unwrap();
        assert_eq!(
            env.get("OPENAI_BASE_URL").map(String::as_str),
            Some("http://gateway:8080/llm/v1")
        );
        assert!(env.contains_key("OPENAI_API_KEY"));
    }

    #[test]
    fn injected_key_verifies_back_to_agent_and_owner() {
        let mut env = HashMap::new();
        let c = cfg();
        inject_llm_env(&mut env, &ctx(), &c).unwrap();
        let bearer = format!("Bearer {}", env["OPENAI_API_KEY"]);
        let (agent_id, owner_id) = verify_agent_jwt(Some(&bearer), &c).unwrap();
        assert_eq!(agent_id, "11111111-1111-1111-1111-111111111111");
        assert_eq!(owner_id, "22222222-2222-2222-2222-222222222222");
    }

    #[test]
    fn trims_trailing_slash_on_base_url() {
        let mut env = HashMap::new();
        let c = GatewayConfig {
            llm_gateway_base_url: "http://gateway:8080/".into(),
            ..cfg()
        };
        inject_llm_env(&mut env, &ctx(), &c).unwrap();
        assert_eq!(
            env.get("OPENAI_BASE_URL").map(String::as_str),
            Some("http://gateway:8080/llm/v1")
        );
    }

    #[test]
    fn empty_secret_fails_closed_and_leaves_env_untouched() {
        let mut env = HashMap::new();
        let c = GatewayConfig {
            agent_jwt_secret: String::new(),
            ..cfg()
        };
        let err = inject_llm_env(&mut env, &ctx(), &c).unwrap_err();
        assert!(matches!(err, GatewayError::JwtSecretNotConfigured));
        assert!(env.is_empty());
    }

    #[test]
    fn empty_base_url_fails_closed_and_leaves_env_untouched() {
        let mut env = HashMap::new();
        let c = GatewayConfig {
            llm_gateway_base_url: String::new(),
            ..cfg()
        };
        let err = inject_llm_env(&mut env, &ctx(), &c).unwrap_err();
        assert!(matches!(err, GatewayError::Internal(_)));
        assert!(env.is_empty());
    }
}
