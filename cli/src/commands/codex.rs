//! Connect Codex CLI to the Nasiko Responses router without replacing Codex auth.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use toml_edit::{Array, DocumentMut, Item, Table, value};

use crate::api::Client;
use crate::commands::coding_agent_router::{self, AgentSpec, ConnectionBinding, McpMint};

const CONFIG_VERSION: u32 = 1;
const PROVIDER_ID: &str = "nasiko";
const PROVIDER_NAME: &str = "Nasiko LLM Router (managed by nasiko)";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    /// Whether `[mcp_servers]` existed before connect touched it, the pre-existing
    /// `[mcp_servers.nasiko]` item if any, and the item connect installed — `None` when no
    /// credential was minted, so disconnect has nothing to restore. Older state files omit all
    /// three; absent means not installed.
    #[serde(default)]
    original_mcp_servers_present: bool,
    #[serde(default)]
    original_mcp_server: SavedItem,
    #[serde(default)]
    installed_mcp_server: Option<String>,
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
    // Both checks below run before anything local is written, so an unusable config costs
    // only the LLM-config restore — never a half-installed `config.toml`.
    let routed = match routed_model(&prepared.resolved_config) {
        Ok(routed) => routed,
        Err(error) => return rollback_after(&prepared, error),
    };
    if let Err(error) = verify_routing(&prepared, &routed.model) {
        return rollback_after(&prepared, error);
    }
    // Minting is a network call layered onto an otherwise local-file install; a failure here
    // (or an older control plane with no gateway configured) only softens to a warning — Codex
    // routing must still connect. The credential itself is folded into the same TOML write
    // `install_prepared` already performs, so a client-side write/validate failure is already the
    // existing full-rollback path — it also needs to revoke the freshly minted credential, since
    // nothing was left registered to use it.
    let mcp_client = Client::from_cluster_entry_with_timeout(
        &prepared.entry,
        Some(coding_agent_router::CP_CALL_TIMEOUT),
    );
    let mcp_mint = coding_agent_router::mint_mcp_credential(
        &mcp_client,
        &prepared.binding.agent_id,
        &prepared.entry.url,
    );
    let mcp_credential = match &mcp_mint {
        McpMint::Minted(credential) => Some(credential),
        McpMint::Unavailable(_) => None,
    };
    let result = install_prepared(&prepared, &codex, &routed.model, mcp_credential);
    let (config_path, embed_warning) = match result {
        Ok(installed) => installed,
        Err(error) => {
            if mcp_credential.is_some() {
                coding_agent_router::revoke_mcp_credential_best_effort(
                    &mcp_client,
                    &prepared.binding.agent_id,
                );
            }
            return rollback_after(&prepared, error);
        }
    };
    // A credential was minted but couldn't be embedded (`mcp_servers` wasn't a table `apply_installed`
    // could write into) — nothing local references it, so revoke it rather than leave it live.
    if embed_warning.is_some() && mcp_credential.is_some() {
        coding_agent_router::revoke_mcp_credential_best_effort(
            &mcp_client,
            &prepared.binding.agent_id,
        );
    }
    println!(
        "Connected Codex routing to Nasiko ({}, {}/{}).",
        prepared.binding.cluster, routed.provider, routed.model
    );
    println!("Config:                    {}", config_path.display());
    println!("Provider:                  nasiko ({})", routed.model);
    match (&mcp_mint, &embed_warning) {
        (McpMint::Minted(_), None) => {
            println!("{}", coding_agent_router::mcp_connected_line());
            println!("{}", coding_agent_router::mcp_multi_machine_note());
        }
        (McpMint::Minted(_), Some(reason)) => {
            eprintln!("warning: {reason}; LLM routing is connected, MCP tools are not");
        }
        (McpMint::Unavailable(message), _) => {
            eprintln!(
                "warning: MCP gateway unavailable ({message}); LLM routing is connected, MCP tools are not"
            );
        }
    }
    println!("Session reporting is separate: nasiko agents install codex");
    Ok(())
}

/// Whether `mcp_servers` (if present) can hold a new sub-table: absent (a `[mcp_servers]` header
/// will be created) or already the `[mcp_servers]` header form. The inline `mcp_servers = { ... }`
/// form some tools generate is deliberately excluded even though `TableLike::insert` can write
/// into it: `installed_mcp_server` is snapshotted (`build_state`) as a freshly built header-style
/// item, but reading an entry back out of an inline table always renders it inline
/// (`nasiko = { url = ... }`), so `restore_mcp_server`'s "did this change since connect"
/// comparison would never match and the entry would linger forever, un-restorable, after every
/// disconnect. Anything else (a scalar, an array, ...) can't hold `mcp_servers.nasiko` either way.
fn mcp_servers_are_writable(document: &DocumentMut) -> bool {
    document
        .get("mcp_servers")
        .map(|item| item.as_table().is_some())
        .unwrap_or(true)
}

/// Installs the resolved LLM-routing config (and, when usable, the MCP credential) into
/// `$CODEX_HOME/config.toml`. Returns the config path and — only when a credential was minted
/// but `mcp_servers` couldn't hold it — a warning for the caller to report and revoke; this is
/// decided before `build_state` so the persisted state's `installed_mcp_server` always matches
/// what actually landed in the file.
fn install_prepared(
    prepared: &coding_agent_router::PreparedConnection,
    codex: &Path,
    model: &str,
    mcp_credential: Option<&coding_agent_router::McpCredential>,
) -> Result<(PathBuf, Option<String>)> {
    let config_path = config_path();
    let original_config = fs::read(&config_path).ok();
    let mut document = read_config(&config_path)?;
    let (usable_mcp_credential, mcp_warning) = match mcp_credential {
        Some(credential) if mcp_servers_are_writable(&document) => (Some(credential), None),
        Some(_) => (
            None,
            Some(
                "Codex setting 'mcp_servers' is not a table; skipping MCP gateway registration"
                    .to_string(),
            ),
        ),
        None => (None, None),
    };
    let state = build_state(
        prepared.binding.clone(),
        config_path.clone(),
        original_config.is_some(),
        &document,
        model,
        usable_mcp_credential,
    )?;
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
    Ok((config_path, mcp_warning))
}

/// The provider/model pair Codex is pointed at, read off the resolved Nasiko LLM config.
#[derive(Debug)]
struct RoutedModel {
    provider: String,
    model: String,
}

/// Config fields that can name the model, in the order the router itself falls back through
/// them (`llm-router/src/resolver/mod.rs:640`). `tier2_model` outranks `tier1_model` there,
/// so it does here too.
const MODEL_FIELDS: [&str; 4] = ["model", "tier2_model", "tier1_model", "tier3_model"];

/// Codex names a model in its own config, so the pair has to be readable before anything is
/// written.
///
/// The provider is **not** restricted to an allowlist: the router's Responses surface
/// forwards OpenAI natively and translates every other provider through the chat IR
/// (`llm-router/src/handlers/responses.rs:128`), including DB-registered custom providers
/// such as Bedrock Converse (`llm-router/src/providers/mod.rs:54`). Whether a given provider
/// is actually routable is settled by `verify_routing`, which asks the router.
///
/// `model` is nullable — a tier-routed config leaves it unset — and for an agent that *has*
/// an LLM config the router ignores whatever the client asked for and routes on the config
/// row itself (`resolver/mod.rs:637`; `providers/openai.rs:330` "resolved model is
/// authoritative"). So the name written into `config.toml` is a label, and [`MODEL_FIELDS`]
/// picks the same one the router will fall back to. The resolver's last two steps — a custom
/// provider's registered default, then the deployment's `DEFAULT_MODEL` — are not in this
/// row, so a config naming none of the four is reported rather than guessed at.
fn routed_model(config: &serde_json::Value) -> Result<RoutedModel> {
    let provider = config_string(config, "provider")
        .context("resolved Nasiko LLM config is missing a provider")?;
    let model = MODEL_FIELDS
        .iter()
        .find_map(|field| config_string(config, field))
        .context(
            "resolved Nasiko LLM config names no model — it relies on this deployment's \
             default, which Codex cannot name. Set one with \
             `nasiko llm-config update <config> --model <model>`",
        )?;
    Ok(RoutedModel { provider, model })
}

/// A trimmed, non-blank string field of the resolved LLM config.
fn config_string(config: &serde_json::Value, field: &str) -> Option<String> {
    config
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Fails `connect` when the router itself refuses this LLM config, so an unroutable provider
/// surfaces here rather than as a 400 on the first Codex turn. Anything the probe could not
/// establish is a warning: routing installs either way.
fn verify_routing(prepared: &coding_agent_router::PreparedConnection, model: &str) -> Result<()> {
    match coding_agent_router::preflight_routing(&prepared.entry, &prepared.binding.agent_id, model)
    {
        coding_agent_router::RoutingPreflight::Reachable => Ok(()),
        coding_agent_router::RoutingPreflight::Rejected(reason) => bail!(
            "Nasiko cannot route Codex with this LLM config: {reason}. Choose another with `nasiko connect codex --config <name>`"
        ),
        coding_agent_router::RoutingPreflight::Inconclusive(reason) => {
            eprintln!("warning: could not verify LLM routing before connecting ({reason})");
            Ok(())
        }
    }
}

/// Restores the agent's previous LLM config after a failed connect, keeping both failures
/// when the restore itself fails.
fn rollback_after(
    prepared: &coding_agent_router::PreparedConnection,
    error: anyhow::Error,
) -> Result<()> {
    match coding_agent_router::rollback_config(prepared) {
        Ok(()) => Err(error),
        Err(rollback) => Err(error.context(format!(
            "Codex connect failed and the prior Nasiko LLM config could not be restored: {rollback:#}"
        ))),
    }
}

pub fn disconnect(force: bool) -> Result<()> {
    let Some(state) = load_state()? else {
        println!("Codex routing: not connected");
        return Ok(());
    };
    coding_agent_router::disconnect_preflight("Codex", &["codex"], force)?;
    let mut document = read_config(&state.config_path)?;
    // Remove the MCP server registration and revoke its credential before touching the
    // unrelated LLM-routing settings below, so a warning from either step never masks whether
    // the routing restore itself succeeded.
    restore_mcp_server(&mut document, &state)?;
    if state.installed_mcp_server.is_some() {
        coding_agent_router::revoke_bound_mcp_credential_best_effort(&state.binding);
    }
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
    println!(
        "MCP gateway:             {}",
        if state.installed_mcp_server.is_some() {
            "configured"
        } else {
            "not configured"
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
    model: &str,
    mcp_credential: Option<&coding_agent_router::McpCredential>,
) -> Result<ConnectionState> {
    // `prepare()` always fills in `ConnectionBinding.executable` (the CLI's own binary path,
    // installed as the command-backed `auth.command` entry Codex invokes to obtain a bearer
    // token); reading it here instead of taking a separate parameter avoids passing the same
    // value into this function twice.
    let executable = binding
        .executable
        .as_deref()
        .context("Codex connection is missing the nasiko executable path")?;
    let installed_provider = provider_item(executable, &binding.cluster_url);
    let installed_mcp_server =
        mcp_credential.map(|credential| snapshot(&mcp_server_item(credential)));
    Ok(ConnectionState {
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
        original_mcp_servers_present: document.get("mcp_servers").is_some(),
        original_mcp_server: capture(mcp_server_item_ref(document)),
        installed_mcp_server,
    })
}

/// Nasiko installs the credential-in-URL form here (`connect_url`, `/api/mcp/s/{token}`) rather
/// than an `Authorization` header — a deliberate choice, not a Codex limitation this CLI has
/// verified either way (Codex isn't installed on every machine that builds this crate); the
/// header form is untested against a real Codex MCP client.
fn mcp_server_item(credential: &coding_agent_router::McpCredential) -> Item {
    let mut server = Table::new();
    server["url"] = value(credential.connect_url.as_str());
    Item::Table(server)
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

    if let Some(installed_mcp_server) = &state.installed_mcp_server {
        if !document.contains_key("mcp_servers") {
            let mut servers_table = Table::new();
            // This parent exists only to hold `mcp_servers.nasiko`; without `set_implicit`,
            // toml_edit renders an empty `[mcp_servers]` header of its own above the nested one.
            servers_table.set_implicit(true);
            document["mcp_servers"] = Item::Table(servers_table);
        }
        // `mcp_servers_are_writable` only lets `installed_mcp_server` be `Some` when this key is
        // absent (just created above) or already the header form, so a plain `as_table_mut`
        // suffices — see that function for why the inline form is excluded entirely.
        let servers = document["mcp_servers"]
            .as_table_mut()
            .context("Codex setting 'mcp_servers' must be a table")?;
        servers[coding_agent_router::MCP_SERVER_NAME] = decode(installed_mcp_server)?;
    }
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

/// No-op when this connection never installed an MCP server (`installed_mcp_server` is `None`).
fn restore_mcp_server(document: &mut DocumentMut, state: &ConnectionState) -> Result<()> {
    let Some(installed) = &state.installed_mcp_server else {
        return Ok(());
    };
    if mcp_server_snapshot(document).as_deref() != Some(installed.as_str()) {
        eprintln!(
            "warning: Codex setting 'mcp_servers.{}' changed since connect; leaving it unchanged",
            coding_agent_router::MCP_SERVER_NAME
        );
        return Ok(());
    }
    let Some(servers) = document.get_mut("mcp_servers").and_then(Item::as_table_mut) else {
        return Ok(());
    };
    if state.original_mcp_server.present {
        servers[coding_agent_router::MCP_SERVER_NAME] =
            decode(&state.original_mcp_server.snapshot)?;
    } else {
        servers.remove(coding_agent_router::MCP_SERVER_NAME);
    }
    if servers.is_empty() && !state.original_mcp_servers_present {
        document.remove("mcp_servers");
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

fn mcp_server_item_ref(document: &DocumentMut) -> Option<&Item> {
    document
        .get("mcp_servers")
        .and_then(Item::as_table)
        .and_then(|servers| servers.get(coding_agent_router::MCP_SERVER_NAME))
}

fn mcp_server_snapshot(document: &DocumentMut) -> Option<String> {
    mcp_server_item_ref(document).map(snapshot)
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
        installed_with_mcp(original, None)
    }

    fn mcp_credential() -> coding_agent_router::McpCredential {
        coding_agent_router::McpCredential {
            token: "ngt_secret".into(),
            gateway_url: "https://cp.example/api/mcp".into(),
            connect_url: "https://cp.example/api/mcp/s/ngt_secret".into(),
        }
    }

    fn installed_with_mcp(
        original: &str,
        mcp_credential: Option<&coding_agent_router::McpCredential>,
    ) -> (DocumentMut, ConnectionState) {
        let document = original.parse::<DocumentMut>().unwrap();
        let state = build_state(
            binding(),
            "/tmp/config.toml".into(),
            true,
            &document,
            "gpt-5.4",
            mcp_credential,
        )
        .unwrap();
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
    fn mcp_server_is_installed_with_the_connect_url_when_a_credential_is_minted() {
        let credential = mcp_credential();
        let (document, state) = installed_with_mcp("other = 1\n", Some(&credential));
        assert_eq!(
            document["mcp_servers"][coding_agent_router::MCP_SERVER_NAME]["url"].as_str(),
            Some("https://cp.example/api/mcp/s/ngt_secret")
        );
        assert!(state.installed_mcp_server.is_some());
        assert!(document.to_string().contains("other = 1"));
    }

    #[test]
    fn mcp_servers_are_writable_accepts_absent_and_header_forms_but_rejects_inline_tables() {
        assert!(mcp_servers_are_writable(
            &"other = 1\n".parse::<DocumentMut>().unwrap()
        ));
        assert!(mcp_servers_are_writable(
            &"[mcp_servers.other]\nurl = \"https://other.example\"\n"
                .parse::<DocumentMut>()
                .unwrap()
        ));
        // Rejected, not merely tolerated: writing into this shape would leave a `nasiko` entry
        // `restore_mcp_server` can never recognize as installed on a later read-back (it always
        // renders inline-read entries as `nasiko = { ... }`, never matching the header-style
        // snapshot `build_state` captured at install time) — see the round-trip test below.
        assert!(!mcp_servers_are_writable(
            &"mcp_servers = { other = { url = \"https://other.example\" } }\n"
                .parse::<DocumentMut>()
                .unwrap()
        ));
        assert!(!mcp_servers_are_writable(
            &"mcp_servers = \"not a table\"\n"
                .parse::<DocumentMut>()
                .unwrap()
        ));
    }

    #[test]
    fn a_pre_existing_inline_mcp_servers_table_is_left_untouched_through_connect_and_disconnect() {
        // `mcp_servers_are_writable` rejects the inline form, so `install_prepared` would pass no
        // credential into `build_state` here (simulated directly with `None`, since driving the
        // full `install_prepared` needs a real `codex` binary for `validate_config`). This must
        // hold even when the inline table already has its own `nasiko` entry: that entry belongs
        // to the user (or another tool) and must survive byte-for-byte, not get silently
        // overwritten by `apply_installed` or "changed since connect"-skipped by `disconnect`.
        let original = "mcp_servers = { nasiko = { url = \"https://user.example\" } }\n";
        assert!(!mcp_servers_are_writable(
            &original.parse::<DocumentMut>().unwrap()
        ));
        let (document, state) = installed_with_mcp(original, None);
        assert!(state.installed_mcp_server.is_none());
        // A real round trip (serialize -> parse -> restore), matching what `install_local`
        // writes to disk and `disconnect` reads back.
        let rendered = document.to_string();
        let mut reparsed = rendered.parse::<DocumentMut>().unwrap();
        restore_if_unchanged(
            &mut reparsed,
            "model_provider",
            &state.installed_model_provider,
            &state.original_model_provider,
        )
        .unwrap();
        restore_if_unchanged(
            &mut reparsed,
            "model",
            &state.installed_model,
            &state.original_model,
        )
        .unwrap();
        restore_provider(&mut reparsed, &state).unwrap();
        restore_mcp_server(&mut reparsed, &state).unwrap();
        assert_eq!(reparsed.to_string(), original);
    }

    #[test]
    fn a_freshly_created_mcp_servers_table_renders_without_an_empty_header() {
        let credential = mcp_credential();
        let (document, _) = installed_with_mcp("other = 1\n", Some(&credential));
        let rendered = document.to_string();
        assert!(rendered.contains("[mcp_servers.nasiko]"));
        assert!(!rendered.contains("[mcp_servers]\n"));
    }

    #[test]
    fn mcp_server_survives_a_real_toml_round_trip_through_install_and_disconnect() {
        // A real round trip (serialize -> parse -> restore) rather than restoring the same
        // in-memory `DocumentMut` `install` produced, matching what `install_local` actually
        // writes to disk and `disconnect` reads back.
        let credential = mcp_credential();
        let (document, state) = installed_with_mcp("other = 1\n", Some(&credential));
        let rendered = document.to_string();
        let mut reparsed = rendered.parse::<DocumentMut>().unwrap();
        restore_if_unchanged(
            &mut reparsed,
            "model_provider",
            &state.installed_model_provider,
            &state.original_model_provider,
        )
        .unwrap();
        restore_if_unchanged(
            &mut reparsed,
            "model",
            &state.installed_model,
            &state.original_model,
        )
        .unwrap();
        restore_provider(&mut reparsed, &state).unwrap();
        restore_mcp_server(&mut reparsed, &state).unwrap();
        assert_eq!(reparsed.to_string(), "other = 1\n");
    }

    #[test]
    fn no_credential_means_no_mcp_servers_table_is_touched() {
        let (document, state) = installed("other = 1\n");
        assert!(!document.contains_key("mcp_servers"));
        assert!(state.installed_mcp_server.is_none());
    }

    #[test]
    fn restore_mcp_server_is_a_no_op_when_this_connection_never_installed_one() {
        let (mut document, state) = installed("other = 1\n");
        let before = document.to_string();
        restore_mcp_server(&mut document, &state).unwrap();
        assert_eq!(document.to_string(), before);
    }

    #[test]
    fn disconnect_removes_the_installed_mcp_server_and_leaves_an_unrelated_one_intact() {
        let credential = mcp_credential();
        let original = "[mcp_servers.other]\nurl = \"https://other.example\"\n";
        let (mut document, state) = installed_with_mcp(original, Some(&credential));
        restore_mcp_server(&mut document, &state).unwrap();
        assert!(
            document["mcp_servers"]
                .as_table()
                .unwrap()
                .get(coding_agent_router::MCP_SERVER_NAME)
                .is_none()
        );
        assert_eq!(
            document["mcp_servers"]["other"]["url"].as_str(),
            Some("https://other.example")
        );
    }

    #[test]
    fn disconnect_restores_a_pre_existing_mcp_server_byte_for_byte() {
        let credential = mcp_credential();
        let original = "[mcp_servers.nasiko]\nurl = \"https://user.example\"\n";
        let (mut document, state) = installed_with_mcp(original, Some(&credential));
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
        restore_mcp_server(&mut document, &state).unwrap();
        assert_eq!(document.to_string(), original);
    }

    #[test]
    fn disconnect_drops_the_mcp_servers_table_when_it_became_empty_and_did_not_exist_before() {
        let credential = mcp_credential();
        let (mut document, state) = installed_with_mcp("other = 1\n", Some(&credential));
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
        restore_mcp_server(&mut document, &state).unwrap();
        assert_eq!(document.to_string(), "other = 1\n");
    }

    #[test]
    fn state_lacking_the_mcp_fields_still_deserializes() {
        let state: ConnectionState = serde_json::from_value(serde_json::json!({
            "version": 1,
            "cluster": "local",
            "cluster_url": "https://cp.example/",
            "agent_id": "agent-id",
            "agent_name": "codex",
            "config_version": 1,
            "config_path": "/tmp/config.toml",
            "original_config_present": true,
            "original_model_providers_present": true,
            "original_model_provider": {"present": false},
            "original_model": {"present": false},
            "original_provider": {"present": false},
            "installed_model_provider": "nasiko",
            "installed_model": "gpt-5.4",
            "installed_provider": "nasiko"
        }))
        .unwrap();
        assert!(!state.original_mcp_servers_present);
        assert!(!state.original_mcp_server.present);
        assert!(state.installed_mcp_server.is_none());
    }

    #[test]
    fn installed_schema_is_accepted_by_codex_0148_when_available() {
        let Ok(codex) = which::which("codex") else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let credential = mcp_credential();
        let (document, _) = installed_with_mcp("", Some(&credential));
        fs::write(dir.path().join("config.toml"), document.to_string()).unwrap();
        validate_config(&codex, dir.path()).unwrap();
    }

    #[test]
    fn routed_model_accepts_every_provider_the_router_can_reach() {
        // Built-in, translated built-in, and DB-registered custom providers alike: the
        // Responses surface translates anything that isn't OpenAI, so the CLI no longer
        // keeps an allowlist of its own.
        for (provider, model) in [
            ("openai", "gpt-5.4"),
            ("anthropic", "claude-opus-4"),
            ("gemini", "gemini-2.5-pro"),
            ("openrouter", "z-ai/glm-4.6"),
            ("bedrock-provider", "anthropic.claude-sonnet-4-v1:0"),
            ("azure-prod", "gpt-4o"),
        ] {
            let routed =
                routed_model(&serde_json::json!({"provider":provider,"model":model})).unwrap();
            assert_eq!(routed.provider, provider);
            assert_eq!(routed.model, model);
        }
    }

    #[test]
    fn a_tier_routed_config_names_the_model_the_router_falls_back_to() {
        // `model` is nullable: a tier-routed config leaves it unset, and the router falls
        // back through tier2 → tier1 → tier3 (`resolver/mod.rs:640`). Codex still has to
        // write *a* model, so it writes that same one rather than refusing to connect.
        let tiered = serde_json::json!({
            "provider": "bedrock-provider",
            "model": serde_json::Value::Null,
            "tier1_model": "anthropic.claude-haiku-4-v1:0",
            "tier2_model": "anthropic.claude-sonnet-4-v1:0",
            "tier3_model": "anthropic.claude-opus-4-v1:0",
        });
        assert_eq!(
            routed_model(&tiered).unwrap().model,
            "anthropic.claude-sonnet-4-v1:0"
        );

        // An explicit `model` still outranks every tier.
        let explicit = serde_json::json!({
            "provider": "bedrock-provider",
            "model": "anthropic.claude-opus-4-v1:0",
            "tier2_model": "anthropic.claude-sonnet-4-v1:0",
        });
        assert_eq!(
            routed_model(&explicit).unwrap().model,
            "anthropic.claude-opus-4-v1:0"
        );

        // Lower tiers are reached only when the ones above them are absent.
        let only_tier3 = serde_json::json!({
            "provider": "bedrock-provider",
            "tier3_model": "anthropic.claude-opus-4-v1:0",
        });
        assert_eq!(
            routed_model(&only_tier3).unwrap().model,
            "anthropic.claude-opus-4-v1:0"
        );
    }

    #[test]
    fn routed_model_rejects_a_config_that_names_no_provider_or_no_model_at_all() {
        for (config, expected) in [
            (
                serde_json::json!({"model": "gpt-5.4"}),
                "missing a provider",
            ),
            (
                serde_json::json!({"provider": "  ", "model": "gpt-5.4"}),
                "missing a provider",
            ),
            (serde_json::json!({"provider": "openai"}), "names no model"),
            (
                serde_json::json!({"provider": "openai", "model": "  ", "tier2_model": ""}),
                "names no model",
            ),
        ] {
            let error = routed_model(&config).unwrap_err();
            assert!(error.to_string().contains(expected), "{config}: {error}");
        }
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
