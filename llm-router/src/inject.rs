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
//! Every agent gets the env of all three SDKs: OpenAI (`OPENAI_BASE_URL` / `OPENAI_API_KEY`),
//! Anthropic (`ANTHROPIC_BASE_URL` / `ANTHROPIC_API_KEY`) and Gemini (`GOOGLE_GEMINI_BASE_URL` /
//! `GEMINI_API_KEY` / `GOOGLE_API_KEY`), all carrying the same agent JWT. Which SDK an agent's
//! code uses is not known at deploy time (`agents.inbound_format` is set by hand and defaults to
//! OpenAI), and the router tells the formats apart by request path, so every SDK is wired.
//!
//! The base URL points directly at the server (the LLM router mounts on it at `/v1/...`);
//! there is no edge proxy stripping a `/llm` prefix anymore.

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
}

/// Every SDK format the router serves; each is wired at deploy.
const ALL_FORMATS: [InboundFormat; 3] = [
    InboundFormat::OpenAi,
    InboundFormat::Anthropic,
    InboundFormat::Gemini,
];

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

    // Every SDK, whatever `agents.inbound_format` says: an Anthropic or Gemini agent left at the
    // OpenAI default otherwise got no router URL or key, and the platform's defaults put the
    // real OpenAI key in every agent's env (while `AGENT_PLATFORM_KEY_FALLBACK` is on). The
    // router accepts the same JWT on every path, so an agent that mixes SDKs reaches it too.
    for format in ALL_FORMATS {
        write_router_credential(env_vars, cfg, format, &token);
    }
    write_default_model(env_vars, cfg);
    Ok(())
}

/// The env var agent SDK templates read their model from.
const MODEL_VAR: &str = "OPENAI_MODEL";

/// Fills `OPENAI_MODEL` with the router's default model when the deployment left it unset.
///
/// Without it an agent falls back to its built-in default (`gpt-4o-mini`), which a router with
/// no OpenAI credentials (e.g. Vertex-only) cannot serve. An explicit value always wins.
fn write_default_model(env_vars: &mut HashMap<String, String>, cfg: &GatewayConfig) {
    let has_model: bool = env_vars
        .get(MODEL_VAR)
        .is_some_and(|m| !m.trim().is_empty());
    let default_model: &str = cfg.default_model.trim();
    if !has_model && !default_model.is_empty() {
        env_vars.insert(MODEL_VAR.into(), default_model.to_owned());
    }
}

/// Points the SDK for `format` at the router (base URL, shared with the coding-agent per-turn
/// path via [`inject_llm_base_url`]) and hands it `token` as its provider key.
fn write_router_credential(
    env_vars: &mut HashMap<String, String>,
    cfg: &GatewayConfig,
    format: InboundFormat,
    token: &str,
) {
    inject_llm_base_url(env_vars, &cfg.llm_gateway_base_url, format);
    for key in api_key_vars(format) {
        env_vars.insert((*key).into(), token.to_owned());
    }
}

/// The env vars each SDK reads its provider key from.
fn api_key_vars(format: InboundFormat) -> &'static [&'static str] {
    match format {
        InboundFormat::OpenAi => &["OPENAI_API_KEY"],
        InboundFormat::Anthropic => &["ANTHROPIC_API_KEY"],
        InboundFormat::Gemini => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
    }
}

/// Write only the SDK base-URL env var(s) for `format`, pointing the agent's LLM
/// SDK at the router mounted at `gateway_base_url`.
///
/// Unlike [`inject_llm_env`] this mints no token and writes no key: it is for
/// callers that supply their own credential separately — notably the coding-agent
/// per-turn path, which carries a session token rather than a deploy-time agent
/// JWT. The router mounts on the server (`/v1/...`, `/v1beta/...`) and each SDK
/// appends its own suffix, so the written base differs per format:
///   OpenAI    — appends `/chat/completions`, version in the base ⇒ `{base}/v1`
///   Anthropic — appends `/v1/messages` (version in the suffix)   ⇒ `{base}`
///   Gemini    — appends `/v1beta/models/{model}:…`               ⇒ `{base}`
pub fn inject_llm_base_url(
    env_vars: &mut HashMap<String, String>,
    gateway_base_url: &str,
    format: InboundFormat,
) {
    let base = gateway_base_url.trim_end_matches('/');
    match format {
        InboundFormat::OpenAi => {
            env_vars.insert("OPENAI_BASE_URL".into(), format!("{base}/v1"));
        }
        InboundFormat::Anthropic => {
            env_vars.insert("ANTHROPIC_BASE_URL".into(), base.to_string());
        }
        InboundFormat::Gemini => {
            env_vars.insert("GOOGLE_GEMINI_BASE_URL".into(), base.to_string());
        }
    }
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
        }
    }

    #[test]
    fn injects_openai_base_url_and_key() {
        let mut env = HashMap::new();
        inject_llm_env(&mut env, &ctx(), &cfg()).unwrap();
        assert_eq!(
            env.get("OPENAI_BASE_URL").map(String::as_str),
            Some("http://gateway:8080/v1")
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
    fn anthropic_base_is_bare_server_origin() {
        // The Anthropic SDK appends `/v1/messages` itself, so no `/v1` in the base.
        let mut env = HashMap::new();
        inject_llm_env(&mut env, &ctx(), &cfg()).unwrap();
        assert_eq!(
            env.get("ANTHROPIC_BASE_URL").map(String::as_str),
            Some("http://gateway:8080")
        );
        assert!(env.contains_key("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn gemini_base_is_bare_server_origin() {
        // The Google GenAI SDK appends `/v1beta/models/{model}:…` itself.
        let mut env = HashMap::new();
        inject_llm_env(&mut env, &ctx(), &cfg()).unwrap();
        assert_eq!(
            env.get("GOOGLE_GEMINI_BASE_URL").map(String::as_str),
            Some("http://gateway:8080")
        );
        assert!(env.contains_key("GEMINI_API_KEY"));
        assert!(env.contains_key("GOOGLE_API_KEY"));
    }

    #[test]
    fn base_url_only_helper_writes_url_and_no_key() {
        // The coding-agent path supplies its own session-token credential, so the
        // helper must write the base URL but never a key.
        let mut env = HashMap::new();
        inject_llm_base_url(&mut env, "http://gateway:8080", InboundFormat::OpenAi);
        assert_eq!(
            env.get("OPENAI_BASE_URL").map(String::as_str),
            Some("http://gateway:8080/v1")
        );
        assert!(!env.contains_key("OPENAI_API_KEY"));

        let mut env = HashMap::new();
        inject_llm_base_url(&mut env, "http://gateway:8080/", InboundFormat::Anthropic);
        assert_eq!(
            env.get("ANTHROPIC_BASE_URL").map(String::as_str),
            Some("http://gateway:8080")
        );
        assert!(!env.contains_key("ANTHROPIC_API_KEY"));
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
            Some("http://gateway:8080/v1")
        );
        assert_eq!(env["ANTHROPIC_BASE_URL"], "http://gateway:8080");
        assert_eq!(env["GOOGLE_GEMINI_BASE_URL"], "http://gateway:8080");
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

    const REAL_PROVIDER_KEY: &str = "sk-real-platform-key";

    /// The env an agent holds before the router wiring runs: the platform's defaults,
    /// including its real OpenAI key and base URL (`AGENT_PLATFORM_KEY_FALLBACK` on).
    fn env_with_platform_defaults() -> HashMap<String, String> {
        HashMap::from([
            ("OPENAI_API_KEY".to_owned(), REAL_PROVIDER_KEY.to_owned()),
            (
                "OPENAI_BASE_URL".to_owned(),
                "https://api.openai.com/v1".to_owned(),
            ),
            ("OPENAI_MODEL".to_owned(), "gpt-4o-mini".to_owned()),
        ])
    }

    fn injected() -> HashMap<String, String> {
        let mut env = env_with_platform_defaults();
        inject_llm_env(&mut env, &ctx(), &cfg()).unwrap();
        env
    }

    fn assert_is_agent_jwt(env: &HashMap<String, String>, key: &str) {
        let bearer = format!("Bearer {}", env[key]);
        let (agent_id, _) = verify_agent_jwt(Some(&bearer), &cfg())
            .unwrap_or_else(|e| panic!("{key} is not the agent JWT: {e:?}"));
        assert_eq!(agent_id, "11111111-1111-1111-1111-111111111111");
    }

    /// The fix for agents left at the default `inbound_format`: whichever SDK the code uses,
    /// it finds the router and the agent JWT, and the real provider key is gone.
    #[test]
    fn every_sdk_gets_the_router_and_the_agent_jwt() {
        let env = injected();
        assert_eq!(env["OPENAI_BASE_URL"], "http://gateway:8080/v1");
        assert_eq!(env["ANTHROPIC_BASE_URL"], "http://gateway:8080");
        assert_eq!(env["GOOGLE_GEMINI_BASE_URL"], "http://gateway:8080");
        for key in [
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "GEMINI_API_KEY",
            "GOOGLE_API_KEY",
        ] {
            assert_is_agent_jwt(&env, key);
        }
        assert!(env.values().all(|v| v != REAL_PROVIDER_KEY));
    }

    #[test]
    fn every_sdk_carries_the_same_jwt() {
        let env = injected();
        let token = &env["OPENAI_API_KEY"];
        for key in ["ANTHROPIC_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"] {
            assert_eq!(&env[key], token, "{key} differs from OPENAI_API_KEY");
        }
    }

    #[test]
    fn the_model_default_survives_the_wiring() {
        assert_eq!(injected()["OPENAI_MODEL"], "gpt-4o-mini");
    }

    #[test]
    fn missing_model_gets_the_router_default() {
        let c = GatewayConfig {
            default_model: "gemini-3.8-flash".into(),
            ..cfg()
        };
        for start in [
            HashMap::new(),
            HashMap::from([("OPENAI_MODEL".to_owned(), " ".to_owned())]),
        ] {
            let mut env = start;
            inject_llm_env(&mut env, &ctx(), &c).unwrap();
            assert_eq!(env["OPENAI_MODEL"], "gemini-3.8-flash");
        }
    }

    #[test]
    fn explicit_model_beats_the_router_default() {
        let c = GatewayConfig {
            default_model: "gemini-3.8-flash".into(),
            ..cfg()
        };
        let mut env = HashMap::from([(
            "OPENAI_MODEL".to_owned(),
            "deepseek-ai/deepseek-v4.1-flash-maas".to_owned(),
        )]);
        inject_llm_env(&mut env, &ctx(), &c).unwrap();
        assert_eq!(env["OPENAI_MODEL"], "deepseek-ai/deepseek-v4.1-flash-maas");
    }

    #[test]
    fn empty_router_default_writes_no_model() {
        let c = GatewayConfig {
            default_model: String::new(),
            ..cfg()
        };
        let mut env = HashMap::new();
        inject_llm_env(&mut env, &ctx(), &c).unwrap();
        assert!(!env.contains_key("OPENAI_MODEL"));
    }

    #[test]
    fn an_unconfigured_router_writes_no_half_config() {
        for unconfigured in [
            GatewayConfig {
                agent_jwt_secret: String::new(),
                ..cfg()
            },
            GatewayConfig {
                llm_gateway_base_url: "  ".into(),
                ..cfg()
            },
        ] {
            let mut env = env_with_platform_defaults();
            assert!(inject_llm_env(&mut env, &ctx(), &unconfigured).is_err());
            assert_eq!(env, env_with_platform_defaults());
        }
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
