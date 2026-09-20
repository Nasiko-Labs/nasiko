//! Connect Codex CLI to the Nasiko Responses router without replacing Codex auth.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use toml_edit::{Array, DocumentMut, Item, Table, value};

use crate::commands::coding_agent_router::{self, AgentSpec, ConnectionBinding};

const CONFIG_VERSION: u32 = 1;
const PROVIDER_ID: &str = "nasiko";
const PROVIDER_NAME: &str = "Nasiko LLM Router (managed by nasiko)";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SavedItem {
    present: bool,
    #[serde(default)]
    snapshot: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConnectionState {
    #[serde(flatten)]
    binding: ConnectionBinding,
    config_version: u32,
    config_path: PathBuf,
    original_config_present: bool,
    original_model_providers_present: bool,
    original_model_provider: SavedItem,
    original_model: SavedItem,
    original_provider: SavedItem,
    installed_model_provider: String,
    installed_model: String,
    installed_provider: String,
}

pub fn connect(agent: Option<&str>, llm_config: Option<&str>) -> Result<()> {
    let codex =
        which::which("codex").context("Codex is not installed or 'codex' is not on PATH")?;
    if state_path().exists() {
        bail!("Codex routing is already connected; run `nasiko disconnect codex` first");
    }
    let executable = std::env::current_exe().context("cannot locate the nasiko executable")?;
    let prepared = coding_agent_router::prepare(
        AgentSpec {
            id: "codex",
            display_name: "Codex CLI",
            default_name: "codex",
        },
        agent,
        llm_config,
        executable.clone(),
    )?;
    let result = install_prepared(&prepared, &codex, &executable);
    let (config_path, model) = match result {
        Ok(installed) => installed,
        Err(error) => {
            return match coding_agent_router::rollback_config(&prepared) {
                Ok(()) => Err(error),
                Err(rollback) => Err(error.context(format!(
                    "Codex install failed and the prior Nasiko LLM config could not be restored: {rollback:#}"
                ))),
            };
        }
    };
    let provider = prepared.resolved_config["provider"]
        .as_str()
        .expect("provider was validated during installation");
    println!(
        "Connected Codex routing to Nasiko ({}, {provider}/{model}).",
        prepared.binding.cluster
    );
    println!("Config:                    {}", config_path.display());
    println!("Provider:                  nasiko ({model})");
    println!("Session reporting is separate: nasiko agents install codex");
    Ok(())
}

fn install_prepared(
    prepared: &coding_agent_router::PreparedConnection,
    codex: &Path,
    executable: &Path,
) -> Result<(PathBuf, String)> {
    let model = responses_model(&prepared.resolved_config)?.to_string();
    let config_path = config_path();
    let original_config = fs::read(&config_path).ok();
    let mut document = read_config(&config_path)?;
    let state = build_state(
        prepared.binding.clone(),
        config_path.clone(),
        original_config.is_some(),
        &document,
        executable,
        &prepared.binding.cluster_url,
        &model,
    );
    apply_installed(&mut document, &state)?;
    install_local(
        &state_path(),
        &config_path,
        &state,
        document.to_string().as_bytes(),
    )?;
    if let Err(error) = validate_config(
        codex,
        config_path.parent().unwrap_or_else(|| Path::new(".")),
    ) {
        return match recover_failed_validation(
            &config_path,
            original_config.as_deref(),
            &state_path(),
        ) {
            Ok(()) => Err(error),
            Err(recovery) => Err(error.context(format!(
                "Codex install recovery failed; routing state was preserved when needed: {recovery:#}"
            ))),
        };
    }
    Ok((config_path, model))
}

fn responses_model(config: &serde_json::Value) -> Result<&str> {
    let provider = config
        .get("provider")
        .and_then(serde_json::Value::as_str)
        .context("resolved Nasiko LLM config is missing a provider")?;
    if !matches!(provider, "openai" | "anthropic" | "gemini") {
        bail!("Codex Responses routing does not support provider '{provider}'");
    }
    config
        .get("model")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .context("resolved Nasiko LLM config is missing a model")
}

pub fn disconnect(force: bool) -> Result<()> {
    let Some(state) = load_state()? else {
        println!("Codex routing: not connected");
        return Ok(());
    };
    coding_agent_router::disconnect_preflight("Codex", &["codex"], force)?;
    let mut document = read_config(&state.config_path)?;
    restore_if_unchanged(
        &mut document,
        "model_provider",
        &state.installed_model_provider,
        &state.original_model_provider,
    )?;
    restore_if_unchanged(
        &mut document,
        "model",
        &state.installed_model,
        &state.original_model,
    )?;
    restore_provider(&mut document, &state)?;

    let rendered = document.to_string();
    if !state.original_config_present && rendered.trim().is_empty() {
        if state.config_path.exists() {
            fs::remove_file(&state.config_path)
                .with_context(|| format!("failed to remove {}", state.config_path.display()))?;
        }
    } else {
        coding_agent_router::atomic_write(&state.config_path, rendered.as_bytes())?;
    }
    fs::remove_file(state_path()).context("failed to remove Codex routing state")?;
    println!("Disconnected Codex routing from Nasiko.");
    println!("Session reporting, hooks, auth, history, and the registered agent were kept.");
    println!("Restart Codex so the restored provider settings take effect.");
    Ok(())
}

pub fn status() -> Result<()> {
    let reporting = crate::commands::integration::reporting_status_for("codex")?;
    let Some(state) = load_state()? else {
        println!("Codex routing:           not connected");
        println!("Codex session reporting: {reporting}");
        println!("Connect routing with: nasiko connect codex");
        return Ok(());
    };
    let document = read_config(&state.config_path)?;
    let active = current_snapshot(&document, "model_provider").as_deref()
        == Some(state.installed_model_provider.as_str())
        && current_snapshot(&document, "model").as_deref() == Some(state.installed_model.as_str())
        && provider_snapshot(&document).as_deref() == Some(state.installed_provider.as_str());
    println!("Codex routing:           connected");
    println!(
        "Cluster:                 {} ({})",
        state.binding.cluster, state.binding.cluster_url
    );
    println!("Agent:                   {}", state.binding.agent_name);
    println!(
        "Nasiko auth:             {}",
        coding_agent_router::auth_status(&state.binding)?
    );
    println!("Config:                  {}", state.config_path.display());
    println!(
        "Router settings:         {}",
        if active {
            "active"
        } else {
            "changed since connect"
        }
    );
    println!("Codex session reporting: {reporting}");
    Ok(())
}

/// Command-backed Codex provider auth. Stdout must contain only the bearer token.
pub fn credential() -> Result<()> {
    let state = load_state()?.ok_or_else(|| {
        anyhow::anyhow!("Codex routing is not connected; run: nasiko connect codex")
    })?;
    println!("{}", coding_agent_router::credential(&state.binding)?.token);
    Ok(())
}

pub fn config_path() -> PathBuf {
    config_path_from(
        std::env::var_os("CODEX_HOME"),
        dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")),
    )
}

fn config_path_from(codex_home: Option<OsString>, home: PathBuf) -> PathBuf {
    codex_home
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"))
        .join("config.toml")
}

fn state_path() -> PathBuf {
    coding_agent_router::state_path("codex-router")
}

fn load_state() -> Result<Option<ConnectionState>> {
    let state: Option<ConnectionState> = coding_agent_router::read_json(&state_path())?;
    if let Some(state) = &state
        && (state.binding.version != coding_agent_router::STATE_VERSION
            || state.config_version != CONFIG_VERSION)
    {
        bail!("unsupported Codex routing state version");
    }
    Ok(state)
}

fn read_config(path: &Path) -> Result<DocumentMut> {
    if !path.exists() {
        return Ok(DocumentMut::new());
    }
    if path.is_symlink() {
        bail!(
            "refusing to replace symlinked Codex config {}",
            path.display()
        );
    }
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    if content.trim().is_empty() {
        return Ok(DocumentMut::new());
    }
    content
        .parse::<DocumentMut>()
        .with_context(|| format!("{} is not valid TOML", path.display()))
}

fn build_state(
    binding: ConnectionBinding,
    config_path: PathBuf,
    original_config_present: bool,
    document: &DocumentMut,
    executable: &Path,
    cluster_url: &str,
    model: &str,
) -> ConnectionState {
    let installed_provider = provider_item(executable, cluster_url);
    ConnectionState {
        binding,
        config_version: CONFIG_VERSION,
        config_path,
        original_config_present,
        original_model_providers_present: document.get("model_providers").is_some(),
        original_model_provider: capture(document.get("model_provider")),
        original_model: capture(document.get("model")),
        original_provider: capture(provider_item_ref(document)),
        installed_model_provider: snapshot(&value(PROVIDER_ID)),
        installed_model: snapshot(&value(model)),
        installed_provider: snapshot(&installed_provider),
    }
}

fn provider_item(executable: &Path, cluster_url: &str) -> Item {
    let mut provider = Table::new();
    provider["name"] = value(PROVIDER_NAME);
    provider["base_url"] = value(format!("{}/v1", cluster_url.trim_end_matches('/')));
    provider["wire_api"] = value("responses");
    provider["supports_websockets"] = value(false);
    provider["supports_standalone_web_search"] = value(false);
    let mut auth = Table::new();
    auth["command"] = value(executable.to_string_lossy().to_string());
    let mut args = Array::new();
    args.push("__coding-agent-token");
    args.push("codex");
    auth["args"] = value(args);
    auth["timeout_ms"] = value(10_000);
    auth["refresh_interval_ms"] = value(300_000);
    provider["auth"] = Item::Table(auth);
    Item::Table(provider)
}

fn apply_installed(document: &mut DocumentMut, state: &ConnectionState) -> Result<()> {
    document["model_provider"] = decode(&state.installed_model_provider)?;
    document["model"] = decode(&state.installed_model)?;
    if !document.contains_key("model_providers") {
        document["model_providers"] = Item::Table(Table::new());
    }
    let providers = document["model_providers"]
        .as_table_mut()
        .context("Codex setting 'model_providers' must be a table")?;
    providers[PROVIDER_ID] = decode(&state.installed_provider)?;
    Ok(())
}

fn install_local(
    state_path: &Path,
    config_path: &Path,
    state: &ConnectionState,
    config: &[u8],
) -> Result<()> {
    let old_state = fs::read(state_path).ok();
    let old_config = fs::read(config_path).ok();
    coding_agent_router::atomic_write_json(state_path, state)?;
    if let Err(error) = coding_agent_router::atomic_write(config_path, config) {
        let state_restore = restore_file(state_path, old_state.as_deref());
        let config_restore = restore_file(config_path, old_config.as_deref());
        let mut message = "failed to write Codex config".to_string();
        if let Err(restore) = state_restore {
            message.push_str(&format!("; state restoration failed: {restore:#}"));
        }
        if let Err(restore) = config_restore {
            message.push_str(&format!("; config restoration failed: {restore:#}"));
        }
        return Err(error.context(message));
    }
    Ok(())
}

fn validate_config(codex: &Path, codex_home: &Path) -> Result<()> {
    let output = Command::new(codex)
        .arg("--strict-config")
        .arg("--version")
        .env("CODEX_HOME", codex_home)
        .output()
        .context("failed to validate Codex configuration")?;
    if !output.status.success() {
        bail!(
            "Codex rejected the generated configuration: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn restore_if_unchanged(
    document: &mut DocumentMut,
    key: &str,
    installed: &str,
    original: &SavedItem,
) -> Result<()> {
    if current_snapshot(document, key).as_deref() != Some(installed) {
        eprintln!("warning: Codex setting '{key}' changed since connect; leaving it unchanged");
        return Ok(());
    }
    restore_top_level(document, key, original)
}

fn restore_provider(document: &mut DocumentMut, state: &ConnectionState) -> Result<()> {
    if provider_snapshot(document).as_deref() != Some(state.installed_provider.as_str()) {
        eprintln!(
            "warning: Codex setting 'model_providers.nasiko' changed since connect; leaving it unchanged"
        );
        return Ok(());
    }
    let Some(providers) = document
        .get_mut("model_providers")
        .and_then(Item::as_table_mut)
    else {
        return Ok(());
    };
    if state.original_provider.present {
        providers[PROVIDER_ID] = decode(&state.original_provider.snapshot)?;
    } else {
        providers.remove(PROVIDER_ID);
    }
    if providers.is_empty() && !state.original_model_providers_present {
        document.remove("model_providers");
    }
    Ok(())
}

fn restore_top_level(document: &mut DocumentMut, key: &str, original: &SavedItem) -> Result<()> {
    if original.present {
        document[key] = decode(&original.snapshot)?;
    } else {
        document.remove(key);
    }
    Ok(())
}

fn capture(item: Option<&Item>) -> SavedItem {
    SavedItem {
        present: item.is_some(),
        snapshot: item.map(snapshot).unwrap_or_default(),
    }
}

fn snapshot(item: &Item) -> String {
    let mut document = DocumentMut::new();
    document["saved"] = item.clone();
    document.to_string()
}

fn decode(snapshot: &str) -> Result<Item> {
    let mut document = snapshot
        .parse::<DocumentMut>()
        .context("invalid saved Codex TOML item")?;
    document
        .remove("saved")
        .context("saved Codex TOML item is missing")
}

fn current_snapshot(document: &DocumentMut, key: &str) -> Option<String> {
    document.get(key).map(snapshot)
}

fn provider_item_ref(document: &DocumentMut) -> Option<&Item> {
    document
        .get("model_providers")
        .and_then(Item::as_table)
        .and_then(|providers| providers.get(PROVIDER_ID))
}

fn provider_snapshot(document: &DocumentMut) -> Option<String> {
    provider_item_ref(document).map(snapshot)
}

fn restore_file(path: &Path, content: Option<&[u8]>) -> Result<()> {
    match content {
        Some(content) => coding_agent_router::atomic_write(path, content),
        None => remove_if_exists(path),
    }
}

fn recover_failed_validation(
    config_path: &Path,
    original_config: Option<&[u8]>,
    state_path: &Path,
) -> Result<()> {
    restore_file(config_path, original_config).context("config restoration failed")?;
    remove_if_exists(state_path).context("state cleanup failed")
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to remove {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> ConnectionBinding {
        ConnectionBinding {
            version: coding_agent_router::STATE_VERSION,
            integration_id: Some("codex".into()),
            cluster: "local".into(),
            cluster_url: "https://cp.example/".into(),
            principal_id: Some("owner".into()),
            agent_id: "agent".into(),
            agent_name: "codex-owner".into(),
            executable: Some("/Applications/Nasiko CLI/nasiko".into()),
        }
    }

    fn installed(original: &str) -> (DocumentMut, ConnectionState) {
        let document = original.parse::<DocumentMut>().unwrap();
        let state = build_state(
            binding(),
            "/tmp/config.toml".into(),
            true,
            &document,
            Path::new("/Applications/Nasiko CLI/nasiko"),
            "https://cp.example/",
            "gpt-5.4",
        );
        let mut installed = document;
        apply_installed(&mut installed, &state).unwrap();
        (installed, state)
    }

    #[test]
    fn config_path_honors_codex_home_then_home() {
        assert_eq!(
            config_path_from(Some("/custom".into()), "/home/me".into()),
            PathBuf::from("/custom/config.toml")
        );
        assert_eq!(
            config_path_from(None, "/home/me".into()),
            PathBuf::from("/home/me/.codex/config.toml")
        );
    }

    #[test]
    fn generated_provider_matches_codex_command_auth_schema_and_preserves_comments() {
        let (document, _) = installed("# keep me\nmodel = \"old\" # inline\nother = 7\n");
        let rendered = document.to_string();
        assert!(rendered.contains("# keep me"));
        assert!(rendered.contains("other = 7"));
        assert_eq!(document["model_provider"].as_str(), Some("nasiko"));
        assert_eq!(document["model"].as_str(), Some("gpt-5.4"));
        let provider = provider_item_ref(&document).unwrap().as_table().unwrap();
        assert_eq!(provider["base_url"].as_str(), Some("https://cp.example/v1"));
        assert_eq!(provider["wire_api"].as_str(), Some("responses"));
        assert_eq!(provider["supports_websockets"].as_bool(), Some(false));
        let auth = provider["auth"].as_table().unwrap();
        assert_eq!(auth["timeout_ms"].as_integer(), Some(10_000));
        assert_eq!(auth["refresh_interval_ms"].as_integer(), Some(300_000));
        assert_eq!(
            auth["args"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>(),
            vec!["__coding-agent-token", "codex"]
        );
    }

    #[test]
    fn disconnect_restores_absent_and_present_values_and_prior_provider() {
        let original = r#"model_provider = "old"
model = "old-model"
[model_providers.nasiko]
name = "User provider"
base_url = "https://user.example"
"#;
        let (mut document, state) = installed(original);
        restore_if_unchanged(
            &mut document,
            "model_provider",
            &state.installed_model_provider,
            &state.original_model_provider,
        )
        .unwrap();
        restore_if_unchanged(
            &mut document,
            "model",
            &state.installed_model,
            &state.original_model,
        )
        .unwrap();
        restore_provider(&mut document, &state).unwrap();
        assert_eq!(document.to_string(), original);

        let (mut document, state) = installed("other = 1\n");
        restore_if_unchanged(
            &mut document,
            "model_provider",
            &state.installed_model_provider,
            &state.original_model_provider,
        )
        .unwrap();
        restore_if_unchanged(
            &mut document,
            "model",
            &state.installed_model,
            &state.original_model,
        )
        .unwrap();
        restore_provider(&mut document, &state).unwrap();
        assert_eq!(document.to_string(), "other = 1\n");
    }

    #[test]
    fn disconnect_preserves_user_changes_and_reporting_files_are_unrelated() {
        let (mut document, state) = installed("other = 1\n");
        document["model"] = value("user-change");
        document["model_providers"][PROVIDER_ID]["base_url"] = value("https://user-change/v1");
        restore_if_unchanged(
            &mut document,
            "model",
            &state.installed_model,
            &state.original_model,
        )
        .unwrap();
        restore_provider(&mut document, &state).unwrap();
        assert_eq!(document["model"].as_str(), Some("user-change"));
        assert_eq!(
            document["model_providers"][PROVIDER_ID]["base_url"].as_str(),
            Some("https://user-change/v1")
        );
        assert!(!document.to_string().contains("hooks"));
    }

    #[test]
    fn installed_schema_is_accepted_by_codex_0148_when_available() {
        let Ok(codex) = which::which("codex") else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let (document, _) = installed("");
        fs::write(dir.path().join("config.toml"), document.to_string()).unwrap();
        validate_config(&codex, dir.path()).unwrap();
    }

    #[test]
    fn codex_provider_validation_accepts_responses_providers() {
        for (provider, model) in [
            ("openai", "gpt-5.4"),
            ("anthropic", "claude-opus-4"),
            ("gemini", "gemini-2.5-pro"),
        ] {
            assert_eq!(
                responses_model(&serde_json::json!({"provider":provider,"model":model})).unwrap(),
                model
            );
        }
        let error =
            responses_model(&serde_json::json!({"provider":"other","model":"m"})).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("does not support provider 'other'")
        );
    }

    #[cfg(unix)]
    #[test]
    fn restore_file_reports_refused_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let link = dir.path().join("config.toml");
        fs::write(&target, "original").unwrap();
        symlink(&target, &link).unwrap();
        let error = restore_file(&link, Some(b"restored")).unwrap_err();
        assert!(error.to_string().contains("symlinked"));
        assert_eq!(fs::read_to_string(target).unwrap(), "original");
    }

    #[cfg(unix)]
    #[test]
    fn failed_validation_recovery_preserves_state_when_config_restore_fails() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let config = dir.path().join("config.toml");
        let state = dir.path().join("state.json");
        fs::write(&target, "installed").unwrap();
        symlink(&target, &config).unwrap();
        fs::write(&state, "recovery state").unwrap();

        let error = recover_failed_validation(&config, Some(b"original"), &state).unwrap_err();
        assert!(error.to_string().contains("config restoration failed"));
        assert_eq!(fs::read_to_string(&state).unwrap(), "recovery state");
        assert_eq!(fs::read_to_string(&target).unwrap(), "installed");
    }
}
