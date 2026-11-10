//! The profile a harness session runs under: which model it asks the platform's LLM router for,
//! and the settings each harness needs to actually go there.
//!
//! The harness images are generic. They carry no model, no provider and no endpoint, because
//! those are the session user's: the model comes from the user's default LLM config (else the
//! platform default), and the router and MCP gateway are reached with the sandbox's own
//! credentials ([`super::identity::session_env`]). What this module adds is the harness-specific
//! shape of those settings — a config object for OpenCode, a `config.toml` for Codex, env for
//! Claude Code — rendered per turn so a changed setting reaches a running sandbox on its next
//! turn (a handshake-mode shim restarts the harness on a changed configuration).
//!
//! Each harness talks one provider dialect to the router: OpenCode and Codex the OpenAI one,
//! Claude Code the Anthropic one. The router routes a harness agent's calls by that dialect, so
//! a model whose provider is not the dialect's is not rendered: the platform default is used
//! when it fits, and otherwise the harness is left to its own default, which the turn then
//! reports loudly rather than routing a model to the wrong provider.

use std::collections::HashMap;

use nasiko_llm_router::GatewayConfig;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::coding::proxy::CodingAgentKind;

/// The env key that carries the profile's files to the shim (a JSON array of [`ProfileFile`]).
/// The server's configure body moves it into the `files` block; an env-mode shim reads it from
/// its environment. Either way it is written to disk before the harness starts.
pub const FILES_ENV: &str = "HARNESS_FILES";

/// The OpenCode provider id the rendered config registers the router under.
const OPENCODE_PROVIDER: &str = "openai-chat";
/// The Codex provider id the rendered config registers the router under.
const CODEX_PROVIDER: &str = "nasiko";
const ROUTER_DISPLAY_NAME: &str = "Nasiko LLM router";

/// Where a session's model came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelSource {
    /// The user's default LLM config.
    UserDefault,
    /// The deployment's `DEFAULT_PROVIDER`/`DEFAULT_MODEL`.
    PlatformDefault,
}

/// The provider and model a session should ask the router for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelChoice {
    pub provider: String,
    pub model: String,
    pub source: ModelSource,
}

impl ModelChoice {
    fn platform_default(gateway: &GatewayConfig) -> Self {
        Self {
            provider: gateway.default_provider.clone(),
            model: gateway.default_model.clone(),
            source: ModelSource::PlatformDefault,
        }
    }
}

/// The user's default LLM config's provider and model, else the platform default.
///
/// The model is the config's `model`, else its tier models in the router's own fallback order.
/// A read failure logs and falls back to the platform default: the user's choice is a
/// preference, and a turn that cannot read it should still run.
pub async fn model_choice_for_user(
    db: &PgPool,
    user_id: Uuid,
    gateway: &GatewayConfig,
) -> ModelChoice {
    let platform = ModelChoice::platform_default(gateway);
    let row: Result<Option<DefaultConfigRow>, sqlx::Error> = sqlx::query_as(
        "SELECT provider, model, tier1_model, tier2_model, tier3_model \
         FROM llm_configs \
         WHERE created_by = $1 AND is_default AND deleted_at IS NULL",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await;
    match row {
        Ok(Some(row)) => choice_from_row(row, platform),
        Ok(None) => platform,
        Err(e) => {
            tracing::warn!(%user_id, error = %e, "harness profile: could not read the user's default LLM config; using the platform default");
            platform
        }
    }
}

#[derive(sqlx::FromRow)]
struct DefaultConfigRow {
    provider: String,
    model: Option<String>,
    tier1_model: Option<String>,
    tier2_model: Option<String>,
    tier3_model: Option<String>,
}

fn choice_from_row(row: DefaultConfigRow, platform: ModelChoice) -> ModelChoice {
    let model = row
        .model
        .or(row.tier2_model)
        .or(row.tier1_model)
        .or(row.tier3_model)
        .map(|m| m.trim().to_owned())
        .filter(|m| !m.is_empty());
    match model {
        Some(model) => ModelChoice {
            provider: row.provider,
            model,
            source: ModelSource::UserDefault,
        },
        // A default config with a provider but no model at all names nothing to ask for.
        None => platform,
    }
}

/// One file the shim writes before the harness starts. Mirrors the shim's `FileSpec`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileFile {
    pub path: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
}

/// What a harness needs on top of the sandbox's base env to use the session's model.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HarnessProfile {
    pub env: HashMap<String, String>,
    pub files: Vec<ProfileFile>,
}

impl HarnessProfile {
    /// Folds the profile into a sandbox env: its env, and its files as [`FILES_ENV`].
    pub fn apply_to(self, env: &mut HashMap<String, String>) {
        env.extend(self.env);
        if !self.files.is_empty() {
            env.insert(
                FILES_ENV.to_owned(),
                serde_json::to_string(&self.files).expect("profile files serialize"),
            );
        }
    }
}

/// What [`render`] needs.
#[derive(Debug, Clone, Copy)]
pub struct ProfileInputs<'a> {
    pub kind: CodingAgentKind,
    pub choice: &'a ModelChoice,
    pub platform: &'a ModelChoice,
    /// The router's base URL as the sandbox reaches it (no `/v1`).
    pub llm_base_url: &'a str,
    pub hitl: bool,
}

/// Renders the profile for one harness kind.
pub fn render(inputs: ProfileInputs<'_>) -> HarnessProfile {
    match inputs.kind {
        CodingAgentKind::OpenCode => opencode(inputs),
        CodingAgentKind::Codex => codex(inputs),
        CodingAgentKind::ClaudeCode => claude_code(inputs),
    }
}

/// The provider dialect a harness speaks to the router, by the router's provider label.
fn dialect(kind: CodingAgentKind) -> &'static str {
    match kind {
        CodingAgentKind::OpenCode | CodingAgentKind::Codex => "openai",
        CodingAgentKind::ClaudeCode => "anthropic",
    }
}

/// The model to render for `inputs.kind`'s dialect: the choice when its provider matches, else
/// the platform default when that matches, else none (logged).
fn model_for(inputs: &ProfileInputs<'_>) -> Option<String> {
    let dialect = dialect(inputs.kind);
    if inputs.choice.provider == dialect {
        return Some(inputs.choice.model.clone());
    }
    if inputs.platform.provider == dialect {
        tracing::info!(
            harness = ?inputs.kind,
            provider = %inputs.choice.provider,
            model = %inputs.choice.model,
            "harness profile: the session's model is not on the harness's provider dialect; using the platform default"
        );
        return Some(inputs.platform.model.clone());
    }
    tracing::warn!(
        harness = ?inputs.kind,
        provider = %inputs.choice.provider,
        "harness profile: neither the session's model nor the platform default is on the harness's provider dialect; leaving the harness's own default"
    );
    None
}

/// The router's OpenAI-compatible endpoint, or none when no router URL is configured (the
/// base env then leaves the harness's own endpoint too, see `identity::session_env`).
fn router_v1(llm_base_url: &str) -> Option<String> {
    let base = llm_base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        tracing::warn!(
            "harness profile: LLM_GATEWAY_BASE_URL is unset; the harness is not pointed at the router"
        );
        return None;
    }
    Some(format!("{base}/v1"))
}

/// OpenCode: one inline config (`OPENCODE_CONFIG_CONTENT`) that registers the router as an
/// OpenAI-compatible provider, selects the model, and flips permissions to `ask` when HITL is on.
fn opencode(inputs: ProfileInputs<'_>) -> HarnessProfile {
    let mut config = serde_json::Map::new();
    config.insert("$schema".into(), "https://opencode.ai/config.json".into());
    config.insert("autoupdate".into(), false.into());
    if let (Some(model), Some(base_url)) = (model_for(&inputs), router_v1(inputs.llm_base_url)) {
        let qualified = format!("{OPENCODE_PROVIDER}/{model}");
        config.insert("model".into(), qualified.clone().into());
        config.insert("small_model".into(), qualified.into());
        config.insert(
            "provider".into(),
            serde_json::json!({
                OPENCODE_PROVIDER: {
                    "npm": "@ai-sdk/openai-compatible",
                    "name": ROUTER_DISPLAY_NAME,
                    "options": {
                        "baseURL": base_url,
                        "apiKey": "{env:OPENAI_API_KEY}",
                    },
                    "models": { model.as_str(): { "name": model.as_str() } },
                }
            }),
        );
    }
    if inputs.hitl {
        config.insert(
            "permission".into(),
            serde_json::json!({ "edit": "ask", "bash": "ask", "webfetch": "ask" }),
        );
    }
    HarnessProfile {
        env: HashMap::from([(
            "OPENCODE_CONFIG_CONTENT".to_owned(),
            serde_json::Value::Object(config).to_string(),
        )]),
        files: Vec::new(),
    }
}

/// Codex: `~/.codex/config.toml` naming the router as a Responses-API provider authenticated
/// from `OPENAI_API_KEY`. The HITL approval flags travel on the command line and win over it.
///
/// `CODEX_HOME` is deliberately not set: Codex defaults it to `$HOME/.codex`, and HOME is
/// per-session under shared runtimes, so the default is right wherever HOME points. The shim
/// expands a leading `~` in a file path against the session's HOME in both modes, whereas a `~`
/// in an env value is expanded only in handshake mode and would reach an env-mode harness
/// literally.
fn codex(inputs: ProfileInputs<'_>) -> HarnessProfile {
    let mut content = String::new();
    let base_url = router_v1(inputs.llm_base_url);
    if base_url.is_some() {
        content.push_str(&format!(
            "model_provider = {}\n",
            toml_string(CODEX_PROVIDER)
        ));
    }
    if let Some(model) = model_for(&inputs) {
        content.push_str(&format!("model = {}\n", toml_string(&model)));
    }
    if let Some(base_url) = base_url {
        content.push_str(&format!(
            "\n[model_providers.{CODEX_PROVIDER}]\nname = {}\nbase_url = {}\nenv_key = \"OPENAI_API_KEY\"\nwire_api = \"responses\"\n",
            toml_string(ROUTER_DISPLAY_NAME),
            toml_string(&base_url),
        ));
    }
    HarnessProfile {
        env: HashMap::new(),
        files: vec![ProfileFile {
            path: "~/.codex/config.toml".to_owned(),
            content,
            mode: None,
        }],
    }
}

/// Claude Code: env only. It reads `ANTHROPIC_BASE_URL`/`ANTHROPIC_API_KEY` from the base env;
/// this names the model and keeps it from calling anything but the router.
fn claude_code(inputs: ProfileInputs<'_>) -> HarnessProfile {
    let mut env = HashMap::from([(
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".to_owned(),
        "1".to_owned(),
    )]);
    if let Some(model) = model_for(&inputs) {
        env.insert("ANTHROPIC_MODEL".to_owned(), model);
    }
    HarnessProfile {
        env,
        files: Vec::new(),
    }
}

/// A TOML basic string: quoted, with the characters TOML requires escaped.
fn toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\u{:04X}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(provider: &str, model: &str, source: ModelSource) -> ModelChoice {
        ModelChoice {
            provider: provider.into(),
            model: model.into(),
            source,
        }
    }

    fn platform() -> ModelChoice {
        choice("openai", "gpt-4o-mini", ModelSource::PlatformDefault)
    }

    fn inputs<'a>(
        kind: CodingAgentKind,
        choice: &'a ModelChoice,
        platform: &'a ModelChoice,
        hitl: bool,
    ) -> ProfileInputs<'a> {
        ProfileInputs {
            kind,
            choice,
            platform,
            llm_base_url: "https://cp.example/api/llm/",
            hitl,
        }
    }

    fn opencode_config(profile: &HarnessProfile) -> serde_json::Value {
        serde_json::from_str(&profile.env["OPENCODE_CONFIG_CONTENT"]).unwrap()
    }

    #[test]
    fn opencode_registers_the_router_and_selects_the_users_model() {
        let user = choice("openai", "gpt-4o", ModelSource::UserDefault);
        let profile = render(inputs(CodingAgentKind::OpenCode, &user, &platform(), false));
        assert!(profile.files.is_empty());
        let config = opencode_config(&profile);
        assert_eq!(config["model"], "openai-chat/gpt-4o");
        assert_eq!(config["small_model"], "openai-chat/gpt-4o");
        assert_eq!(config["autoupdate"], false);
        let provider = &config["provider"]["openai-chat"];
        assert_eq!(provider["npm"], "@ai-sdk/openai-compatible");
        assert_eq!(
            provider["options"]["baseURL"],
            "https://cp.example/api/llm/v1"
        );
        assert_eq!(provider["options"]["apiKey"], "{env:OPENAI_API_KEY}");
        assert!(provider["models"]["gpt-4o"].is_object());
        assert!(config.get("permission").is_none());
    }

    #[test]
    fn opencode_hitl_adds_the_ask_permissions_beside_the_model() {
        let user = choice("openai", "gpt-4o", ModelSource::UserDefault);
        let profile = render(inputs(CodingAgentKind::OpenCode, &user, &platform(), true));
        let config = opencode_config(&profile);
        assert_eq!(config["permission"]["edit"], "ask");
        assert_eq!(config["permission"]["bash"], "ask");
        assert_eq!(config["permission"]["webfetch"], "ask");
        assert_eq!(config["model"], "openai-chat/gpt-4o");
    }

    #[test]
    fn a_model_off_the_harness_dialect_falls_back_to_the_platform_default() {
        let user = choice("anthropic", "claude-sonnet-4-6", ModelSource::UserDefault);
        let profile = render(inputs(CodingAgentKind::OpenCode, &user, &platform(), false));
        assert_eq!(
            opencode_config(&profile)["model"],
            "openai-chat/gpt-4o-mini"
        );

        // And when the platform default is off it too, nothing is rendered.
        let gemini = choice("gemini", "gemini-2.5-pro", ModelSource::PlatformDefault);
        let profile = render(inputs(CodingAgentKind::OpenCode, &user, &gemini, false));
        let config = opencode_config(&profile);
        assert!(config.get("model").is_none());
        assert!(config.get("provider").is_none());
        assert_eq!(config["autoupdate"], false);
    }

    #[test]
    fn codex_writes_a_config_toml_under_its_home() {
        let user = choice("openai", "gpt-5.4", ModelSource::UserDefault);
        let profile = render(inputs(CodingAgentKind::Codex, &user, &platform(), true));
        assert!(!profile.env.contains_key("CODEX_HOME"));
        assert_eq!(profile.files.len(), 1);
        let file = &profile.files[0];
        assert_eq!(file.path, "~/.codex/config.toml");
        assert_eq!(file.mode, None);
        assert_eq!(
            file.content,
            "model_provider = \"nasiko\"\nmodel = \"gpt-5.4\"\n\n[model_providers.nasiko]\nname = \"Nasiko LLM router\"\nbase_url = \"https://cp.example/api/llm/v1\"\nenv_key = \"OPENAI_API_KEY\"\nwire_api = \"responses\"\n"
        );
    }

    #[test]
    fn without_a_router_url_no_provider_is_registered() {
        let user = choice("openai", "gpt-4o", ModelSource::UserDefault);
        let platform = platform();
        let mut inputs = inputs(CodingAgentKind::OpenCode, &user, &platform, true);
        inputs.llm_base_url = " ";
        let config = opencode_config(&render(inputs));
        assert!(config.get("model").is_none());
        assert!(config.get("provider").is_none());
        assert_eq!(config["permission"]["bash"], "ask");

        inputs.kind = CodingAgentKind::Codex;
        let profile = render(inputs);
        assert_eq!(profile.files[0].content, "model = \"gpt-4o\"\n");
    }

    #[test]
    fn claude_code_is_env_only_and_takes_an_anthropic_model() {
        let user = choice("anthropic", "claude-sonnet-4-6", ModelSource::UserDefault);
        let profile = render(inputs(
            CodingAgentKind::ClaudeCode,
            &user,
            &platform(),
            false,
        ));
        assert!(profile.files.is_empty());
        assert_eq!(profile.env["ANTHROPIC_MODEL"], "claude-sonnet-4-6");
        assert_eq!(profile.env["CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"], "1");

        let openai_user = choice("openai", "gpt-4o", ModelSource::UserDefault);
        let profile = render(inputs(
            CodingAgentKind::ClaudeCode,
            &openai_user,
            &platform(),
            false,
        ));
        assert!(!profile.env.contains_key("ANTHROPIC_MODEL"));
    }

    #[test]
    fn apply_to_carries_the_files_as_json_env() {
        let user = choice("openai", "gpt-5.4", ModelSource::UserDefault);
        let profile = render(inputs(CodingAgentKind::Codex, &user, &platform(), false));
        let mut env = HashMap::from([("A".to_owned(), "b".to_owned())]);
        profile.clone().apply_to(&mut env);
        assert_eq!(env["A"], "b");
        assert!(!env.contains_key("CODEX_HOME"));
        let files: Vec<ProfileFile> = serde_json::from_str(&env[FILES_ENV]).unwrap();
        assert_eq!(files, profile.files);
        assert!(!env[FILES_ENV].contains("\"mode\""));

        let mut env = HashMap::new();
        HarnessProfile::default().apply_to(&mut env);
        assert!(env.is_empty());
    }

    #[test]
    fn the_default_config_row_names_the_model_in_router_order() {
        let row = |model: Option<&str>, t1: Option<&str>, t2: Option<&str>, t3: Option<&str>| {
            DefaultConfigRow {
                provider: "anthropic".into(),
                model: model.map(Into::into),
                tier1_model: t1.map(Into::into),
                tier2_model: t2.map(Into::into),
                tier3_model: t3.map(Into::into),
            }
        };
        let pick = |r| choice_from_row(r, platform());
        assert_eq!(
            pick(row(Some("m"), Some("1"), Some("2"), Some("3"))).model,
            "m"
        );
        assert_eq!(pick(row(None, Some("1"), Some("2"), Some("3"))).model, "2");
        assert_eq!(pick(row(None, Some("1"), None, Some("3"))).model, "1");
        assert_eq!(pick(row(None, None, None, Some("3"))).model, "3");
        let picked = pick(row(Some(" m "), None, None, None));
        assert_eq!(
            (picked.provider.as_str(), picked.source),
            ("anthropic", ModelSource::UserDefault)
        );
        assert_eq!(pick(row(Some("  "), None, None, None)), platform());
    }

    #[test]
    fn toml_strings_are_escaped() {
        assert_eq!(toml_string("plain"), "\"plain\"");
        assert_eq!(toml_string("a\"b\\c\n"), "\"a\\\"b\\\\c\\n\"");
        assert_eq!(toml_string("\u{1}"), "\"\\u0001\"");
    }
}
