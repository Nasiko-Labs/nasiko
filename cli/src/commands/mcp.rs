//! `nasiko mcp ...` — CLI surface for the MCP Gateway HTTP API
//! (`oss/server/src/mcp/*`): connectors, connections, sharing, credentials,
//! OAuth, and per-agent tool permissions.

use std::collections::HashMap;

use anyhow::{Result, bail};
use serde_json::Value;

use crate::api::Client;

// ─── Shared helpers ─────────────────────────────────────────────────────────

/// Resolve an `agent` CLI argument (bare UUID or agent name) to its UUID.
///
/// `GET /agents` returns the caller's own agents plus every public/shared
/// agent visible to them, and agent names are only unique per-owner (see
/// `oss/migrations/015_agents_unique_owner_name.sql`) — so two different
/// owners can have an agent with the same name. Unlike
/// `commands::secrets::resolve_agent_id` (which returns the first match),
/// this collects every match and refuses to guess when there's more than
/// one: silently picking one would mean a permission rule meant for your
/// own agent could instead land on someone else's (server-side
/// `can_manage_agent` would only block this for a non-superuser caller —
/// an admin running this command is exposed either way).
fn resolve_agent_id(client: &Client, name_or_id: &str) -> Result<String> {
    if uuid::Uuid::parse_str(name_or_id).is_ok() {
        return Ok(name_or_id.to_string());
    }
    let agents: Vec<Value> = client.get_json("/agents")?;
    let matches: Vec<&str> = agents
        .iter()
        .filter(|a| a.get("name").and_then(|v| v.as_str()) == Some(name_or_id))
        .filter_map(|a| a.get("id").and_then(|v| v.as_str()))
        .collect();

    match matches.as_slice() {
        [] => bail!("agent '{name_or_id}' not found — is it registered? (check `nasiko ps`)"),
        [id] => Ok(id.to_string()),
        ids => bail!(
            "agent name '{name_or_id}' is ambiguous — {} agents share this name: {}. Use the agent ID instead of the name.",
            ids.len(),
            ids.join(", "),
        ),
    }
}

/// Parse repeatable `--header "Key: Value"` flags into a map.
fn parse_headers(raw: &[String]) -> Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    for h in raw {
        let (k, v) = h
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("invalid --header '{h}' — expected 'Key: Value'"))?;
        out.insert(k.trim().to_string(), v.trim().to_string());
    }
    Ok(out)
}

fn confirm(prompt: &str, yes: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    Ok(dialoguer::Confirm::new()
        .with_prompt(prompt)
        .default(false)
        .interact()?)
}

fn print_json_or<F: FnOnce()>(json: bool, value: &Value, human: F) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        human();
    }
    Ok(())
}

fn s<'a>(v: &'a Value, field: &str) -> &'a str {
    v.get(field).and_then(|x| x.as_str()).unwrap_or("-")
}

fn b(v: &Value, field: &str) -> bool {
    v.get(field).and_then(|x| x.as_bool()).unwrap_or(false)
}

// ─── catalog / connect / connections / disconnect ──────────────────────────

pub fn catalog(json: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let resp: Value = client.get_json("/mcp/catalog")?;
    let services = resp
        .get("services")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    print_json_or(json, &resp, || {
        if services.is_empty() {
            println!("No connectable services.");
            return;
        }
        println!(
            "{:<36} {:<10} {:<24} {:<10} DESCRIPTION",
            "CONNECTOR ID", "TYPE", "NAME", "AUTH FLOW"
        );
        for svc in &services {
            println!(
                "{:<36} {:<10} {:<24} {:<10} {}",
                s(svc, "connector_id"),
                s(svc, "type"),
                s(svc, "name"),
                s(svc, "auth_flow"),
                svc.get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
            );
        }
        println!("\n{} connectable service(s).", services.len());
    })
}

#[allow(clippy::too_many_arguments)]
pub fn connect(
    connector_id: Option<&str>,
    toolkit: Option<&str>,
    url: Option<&str>,
    value: Option<&str>,
    redirect_url: Option<&str>,
    yes: bool,
) -> Result<()> {
    let modes = [connector_id.is_some(), toolkit.is_some(), url.is_some()];
    if modes.iter().filter(|m| **m).count() != 1 {
        bail!("specify exactly one of --connector-id, --toolkit, or --url");
    }

    if let Some(u) = url
        && !confirm(&format!("Register a new custom MCP connector at {u}?"), yes)?
    {
        println!("Cancelled.");
        return Ok(());
    }

    let client = Client::from_active_cluster()?;
    let body = serde_json::json!({
        "connector_id": connector_id,
        "service": toolkit,
        "url": url,
        "credentials": value.map(|v| serde_json::json!({"value": v})),
        "redirect_url": redirect_url,
    });
    let resp: Value = client.post_json("/mcp/connect", &body)?;

    let name = s(&resp, "name");
    match resp.get("status").and_then(|v| v.as_str()) {
        Some("connected") => println!(
            "Connected to '{name}' (connector {}).",
            s(&resp, "connector_id")
        ),
        Some("initiated") => println!(
            "Composio authorization started for '{name}'.\n\nOpen this URL to finish connecting:\n\n  {}\n",
            s(&resp, "oauth_url")
        ),
        Some("oauth_required") => println!(
            "Authorization required for '{name}'.\n\nOpen this URL to finish connecting:\n\n  {}\n",
            s(&resp, "authorization_url")
        ),
        other => println!("{}", other.unwrap_or("done")),
    }
    Ok(())
}

pub fn connections(json: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    connections_with(&client, json)
}

fn connections_with(client: &Client, json: bool) -> Result<()> {
    let resp: Value = client.get_json("/mcp/connections")?;
    let data = resp
        .get("data")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    print_json_or(json, &resp, || {
        if data.is_empty() {
            println!("No connections.");
            return;
        }
        println!(
            "{:<36} {:<24} {:<12} CREATED AT",
            "CONNECTOR ID", "NAME", "STATUS"
        );
        for c in &data {
            println!(
                "{:<36} {:<24} {:<12} {}",
                s(c, "connector_id"),
                s(c, "name"),
                s(c, "status"),
                s(c, "created_at"),
            );
        }
        println!("\n{} connection(s).", data.len());
    })
}

pub fn disconnect(connector_id: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let resp: Value = client.delete_and_read(&format!("/mcp/connections/{connector_id}"))?;
    let mut msg = s(&resp, "message").to_string();
    if b(&resp, "composio_revoked") {
        msg.push_str(" (Composio token revoked.)");
    }
    println!("{msg}");
    Ok(())
}

// ─── toolkit (Composio auth-configs, admin) ─────────────────────────────────

pub fn toolkit_list(json: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let resp: Value = client.get_json("/mcp/auth-configs")?;
    let data = resp
        .get("data")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    print_json_or(json, &resp, || {
        if data.is_empty() {
            println!("No toolkits registered.");
            return;
        }
        println!(
            "{:<36} {:<16} {:<20} DISPLAY NAME",
            "CONNECTOR ID", "TOOLKIT", "AUTH CONFIG ID"
        );
        for t in &data {
            println!(
                "{:<36} {:<16} {:<20} {}",
                s(t, "connector_id"),
                s(t, "toolkit"),
                s(t, "auth_config_id"),
                s(t, "display_name"),
            );
        }
        println!("\n{} toolkit(s) registered.", data.len());
    })
}

#[allow(clippy::too_many_arguments)]
pub fn toolkit_register(
    toolkit: &str,
    client_id: Option<&str>,
    client_secret: Option<&str>,
    scopes: &[String],
    display_name: Option<&str>,
    logo_url: Option<&str>,
) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let use_composio_managed = client_id.is_none() && client_secret.is_none();
    let body = serde_json::json!({
        "toolkit": toolkit,
        "use_composio_managed": use_composio_managed,
        "client_id": client_id,
        "client_secret": client_secret,
        "scopes": if scopes.is_empty() { None } else { Some(scopes) },
        "display_name": display_name,
        "logo_url": logo_url,
    });
    let resp: Value = client.post_json("/mcp/auth-configs", &body)?;
    println!(
        "Registered toolkit '{}' (connector {}, auth-config {}).",
        s(&resp, "toolkit"),
        s(&resp, "connector_id"),
        s(&resp, "auth_config_id"),
    );
    Ok(())
}

pub fn toolkit_update(
    connector_id: &str,
    display_name: Option<&str>,
    logo_url: Option<&str>,
    description: Option<&str>,
) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let body = serde_json::json!({
        "display_name": display_name,
        "logo_url": logo_url,
        "description": description,
    });
    let resp: Value = client.patch_json(&format!("/mcp/auth-configs/{connector_id}"), &body)?;
    println!("Updated toolkit '{}'.", s(&resp, "toolkit"));
    Ok(())
}

pub fn toolkit_delete(connector_id: &str, yes: bool) -> Result<()> {
    if !confirm(
        &format!(
            "Delete toolkit connector {connector_id}? This affects every user's connection to it."
        ),
        yes,
    )? {
        println!("Cancelled.");
        return Ok(());
    }
    let client = Client::from_active_cluster()?;
    client.delete(&format!("/mcp/auth-configs/{connector_id}"))?;
    println!("Deleted toolkit connector {connector_id}.");
    Ok(())
}

// ─── connector (custom MCP servers) ────────────────────────────────────────

pub fn connector_list(json: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let resp: Value = client.get_json("/mcp/connectors")?;
    let data = resp
        .get("data")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    print_json_or(json, &resp, || {
        if data.is_empty() {
            println!("No connectors.");
            return;
        }
        println!(
            "{:<36} {:<20} {:<10} {:<8} {:<8} URL",
            "CONNECTOR ID", "NAME", "AUTH TYPE", "ACTIVE", "OWNER"
        );
        for c in &data {
            let owner = if b(c, "is_owner") { "you" } else { "shared" };
            let url = c.get("url").and_then(|v| v.as_str()).unwrap_or("-");
            let url = if url.len() > 40 {
                format!("{}...", &url[..url.floor_char_boundary(37)])
            } else {
                url.to_string()
            };
            println!(
                "{:<36} {:<20} {:<10} {:<8} {:<8} {}",
                s(c, "connector_id"),
                s(c, "name"),
                s(c, "auth_type"),
                b(c, "is_active"),
                owner,
                url,
            );
        }
        println!("\n{} connector(s).", data.len());
    })
}

pub fn connector_probe(url: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let resp: Value =
        client.post_json("/mcp/connectors/probe", &serde_json::json!({ "url": url }))?;
    println!(
        "{}\n  Detected auth type: {}\n  {}",
        s(&resp, "url"),
        s(&resp, "auth_type"),
        s(&resp, "hint"),
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn connector_register(
    name: &str,
    url: &str,
    transport: &str,
    auth_type: &str,
    url_param_name: Option<&str>,
    credential_header_name: Option<&str>,
    headers: &[String],
    basic_username: Option<&str>,
    basic_password: Option<&str>,
    description: Option<&str>,
    display_name: Option<&str>,
    logo_url: Option<&str>,
) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let headers = parse_headers(headers)?;
    let body = serde_json::json!({
        "name": name,
        "url": url,
        "transport": transport,
        "auth_type": auth_type,
        "url_param_name": url_param_name,
        "credential_header_name": credential_header_name,
        "headers": if headers.is_empty() { None } else { Some(headers) },
        "basic_username": basic_username,
        "basic_password": basic_password,
        "description": description,
        "display_name": display_name,
        "logo_url": logo_url,
    });
    let resp: Value = client.post_json("/mcp/connectors", &body)?;
    let connector_id = s(&resp, "connector_id").to_string();
    println!(
        "Registered connector '{}' ({connector_id}). Auth type: {}.",
        s(&resp, "name"),
        s(&resp, "auth_type")
    );
    if auth_type != "none" {
        println!(
            "Run 'nasiko mcp credential set {connector_id}' if this server requires a credential."
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn connector_update(
    connector_id: &str,
    name: Option<&str>,
    url: Option<&str>,
    transport: Option<&str>,
    auth_type: Option<&str>,
    url_param_name: Option<&str>,
    credential_header_name: Option<&str>,
    headers: &[String],
    description: Option<&str>,
    display_name: Option<&str>,
    logo_url: Option<&str>,
    active: Option<bool>,
) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let headers = parse_headers(headers)?;
    let body = serde_json::json!({
        "name": name,
        "url": url,
        "transport": transport,
        "auth_type": auth_type,
        "url_param_name": url_param_name,
        "credential_header_name": credential_header_name,
        "headers": if headers.is_empty() { None } else { Some(headers) },
        "description": description,
        "display_name": display_name,
        "logo_url": logo_url,
        "is_active": active,
    });
    let resp: Value = client.patch_json(&format!("/mcp/connectors/{connector_id}"), &body)?;
    println!("Updated connector '{}' ({connector_id}).", s(&resp, "name"));
    Ok(())
}

pub fn connector_delete(connector_id: &str, yes: bool) -> Result<()> {
    if !confirm(&format!("Delete connector {connector_id}?"), yes)? {
        println!("Cancelled.");
        return Ok(());
    }
    let client = Client::from_active_cluster()?;
    client.delete(&format!("/mcp/connectors/{connector_id}"))?;
    println!("Deleted connector {connector_id}.");
    Ok(())
}

// ─── connector share ────────────────────────────────────────────────────────

fn share_target_body(user: Option<&str>, public: bool) -> Result<Value> {
    if public && user.is_some() {
        bail!("specify --user <name> or --public, not both");
    }
    if public {
        Ok(serde_json::json!({ "public": true }))
    } else if let Some(u) = user {
        if u.trim().is_empty() {
            bail!("--user cannot be empty");
        }
        Ok(serde_json::json!({ "username": u }))
    } else {
        bail!("specify --user <name> or --public")
    }
}

fn share_target_label(user: Option<&str>, public: bool) -> String {
    if public {
        "everyone".to_string()
    } else {
        user.unwrap_or("?").to_string()
    }
}

pub fn share_list(connector_id: &str, json: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let resp: Value = client.get_json(&format!("/mcp/connectors/{connector_id}/share"))?;
    let data = resp
        .get("data")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    print_json_or(json, &resp, || {
        if data.is_empty() {
            println!("No grants.");
            return;
        }
        // `grantee_id` is the grantee's raw user ID, not their username — the
        // API stores/returns it that way (grants are created by username, but
        // persisted by ID), and there's no universally-available endpoint this
        // CLI can call to resolve it back: the one that could (`GET
        // /users/{id}`) is EE-only and superuser-gated, so relying on it would
        // 404 on OSS and 403 for the common case of a non-admin listing shares
        // on their own connector. Label the column honestly instead of
        // guessing wrong.
        println!(
            "{:<36} {:<8} {:<36} CREATED AT",
            "GRANT ID", "TYPE", "GRANTEE (user id)"
        );
        for g in &data {
            let grantee = s(g, "grantee_id");
            let grantee = if grantee == "*" { "everyone" } else { grantee };
            println!(
                "{:<36} {:<8} {:<36} {}",
                s(g, "grant_id"),
                s(g, "grant_type"),
                grantee,
                s(g, "created_at")
            );
        }
        println!("\n{} grant(s).", data.len());
    })
}

pub fn share_add(connector_id: &str, user: Option<&str>, public: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    share_add_with(&client, connector_id, user, public)
}

fn share_add_with(
    client: &Client,
    connector_id: &str,
    user: Option<&str>,
    public: bool,
) -> Result<()> {
    let body = share_target_body(user, public)?;
    client.post_json_void(&format!("/mcp/connectors/{connector_id}/share"), &body)?;
    println!(
        "Shared connector {connector_id} with {}.",
        share_target_label(user, public)
    );
    Ok(())
}

pub fn share_remove(connector_id: &str, user: Option<&str>, public: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    share_remove_with(&client, connector_id, user, public)
}

fn share_remove_with(
    client: &Client,
    connector_id: &str,
    user: Option<&str>,
    public: bool,
) -> Result<()> {
    let body = share_target_body(user, public)?;
    client.delete_json_void(&format!("/mcp/connectors/{connector_id}/share"), &body)?;
    println!(
        "Revoked connector {connector_id}'s share with {}.",
        share_target_label(user, public)
    );
    Ok(())
}

// ─── credential ─────────────────────────────────────────────────────────────

pub fn credential_set(connector_id: &str, value: Option<&str>) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let value = match value {
        Some(v) => v.to_string(),
        None => dialoguer::Password::new()
            .with_prompt("Credential value")
            .interact()?,
    };
    credential_set_with(&client, connector_id, &value)
}

fn credential_set_with(client: &Client, connector_id: &str, value: &str) -> Result<()> {
    let resp: Value = client.post_json(
        &format!("/mcp/connectors/{connector_id}/credential"),
        &serde_json::json!({ "value": value }),
    )?;
    println!(
        "Credential stored for '{}' ({connector_id}). Connected.",
        s(&resp, "name")
    );
    Ok(())
}

pub fn credential_status(connector_id: &str, json: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    credential_status_with(&client, connector_id, json)
}

fn credential_status_with(client: &Client, connector_id: &str, json: bool) -> Result<()> {
    let resp: Value =
        client.get_json(&format!("/mcp/connectors/{connector_id}/credential/status"))?;
    print_json_or(json, &resp, || {
        println!(
            "{} ({connector_id})\n  Connected:  {}\n  Auth type:  {}",
            s(&resp, "name"),
            b(&resp, "connected"),
            s(&resp, "auth_type"),
        );
    })
}

pub fn credential_delete(connector_id: &str, yes: bool) -> Result<()> {
    if !confirm(
        &format!("Remove the stored credential for connector {connector_id}?"),
        yes,
    )? {
        println!("Cancelled.");
        return Ok(());
    }
    let client = Client::from_active_cluster()?;
    client.delete(&format!("/mcp/connectors/{connector_id}/credential"))?;
    println!("Credential removed for connector {connector_id}.");
    Ok(())
}

// ─── oauth (generic-server OAuth 2.1) ───────────────────────────────────────

pub fn oauth_authorize(
    connector_id: &str,
    client_id: Option<&str>,
    redirect_url: Option<&str>,
) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let body = serde_json::json!({ "client_id": client_id, "redirect_url": redirect_url });
    let resp: Value = client.post_json(
        &format!("/mcp/connectors/{connector_id}/oauth/authorize"),
        &body,
    )?;
    println!(
        "Open this URL to authorize '{}':\n\n  {}\n\nThen re-run `nasiko mcp oauth status {connector_id}` to confirm.",
        s(&resp, "name"),
        s(&resp, "authorization_url"),
    );
    Ok(())
}

pub fn oauth_status(connector_id: &str, json: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let resp: Value = client.get_json(&format!("/mcp/connectors/{connector_id}/oauth/status"))?;
    print_json_or(json, &resp, || {
        println!(
            "{} ({connector_id})\n  Authorized: {}\n  Expires:    {}\n  Scope:      {}",
            s(&resp, "name"),
            b(&resp, "authorized"),
            s(&resp, "expires_at"),
            s(&resp, "scope"),
        );
    })
}

pub fn oauth_revoke(connector_id: &str, yes: bool) -> Result<()> {
    if !confirm(
        &format!("Revoke the OAuth token for connector {connector_id}?"),
        yes,
    )? {
        println!("Cancelled.");
        return Ok(());
    }
    let client = Client::from_active_cluster()?;
    client.delete(&format!("/mcp/connectors/{connector_id}/oauth/token"))?;
    println!("OAuth token revoked for connector {connector_id}.");
    Ok(())
}

// ─── agent-tools (per-agent connector access + tool permissions) ───────────

pub fn agent_tools_connectors(agent: &str, json: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let agent_id = resolve_agent_id(&client, agent)?;
    let resp: Value = client.get_json(&format!("/mcp/agents/{agent_id}/connectors"))?;
    let data = resp
        .get("data")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    print_json_or(json, &resp, || {
        if data.is_empty() {
            println!("No connectors visible to this agent.");
            return;
        }
        println!(
            "{:<36} {:<20} {:<10} {:<8} CONNECTED",
            "CONNECTOR ID", "NAME", "TYPE", "ENABLED"
        );
        for c in &data {
            println!(
                "{:<36} {:<20} {:<10} {:<8} {}",
                s(c, "connector_id"),
                s(c, "name"),
                s(c, "provider_type"),
                b(c, "enabled"),
                b(c, "connected"),
            );
        }
        println!("\n{} connector(s) visible to this agent.", data.len());
    })
}

fn set_connector_enabled(agent: &str, connector_id: &str, enabled: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let agent_id = resolve_agent_id(&client, agent)?;
    let _: Value = client.put_json(
        &format!("/mcp/agents/{agent_id}/connectors/{connector_id}"),
        &serde_json::json!({ "enabled": enabled }),
    )?;
    println!(
        "Connector {connector_id} {} for agent '{agent}'.",
        if enabled { "enabled" } else { "disabled" }
    );
    Ok(())
}

pub fn agent_tools_enable(agent: &str, connector_id: &str) -> Result<()> {
    set_connector_enabled(agent, connector_id, true)
}

pub fn agent_tools_disable(agent: &str, connector_id: &str) -> Result<()> {
    set_connector_enabled(agent, connector_id, false)
}

pub fn agent_tools_tools(agent: &str, connector_id: &str, json: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let agent_id = resolve_agent_id(&client, agent)?;
    let resp: Value = client.get_json(&format!(
        "/mcp/agents/{agent_id}/connectors/{connector_id}/tools"
    ))?;
    let data = resp
        .get("data")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    print_json_or(json, &resp, || {
        if data.is_empty() {
            println!("No tools synced for this connector.");
            return;
        }
        println!("{:<32} {:<8} DESCRIPTION", "TOOL", "STANCE");
        for t in &data {
            println!(
                "{:<32} {:<8} {}",
                s(t, "name"),
                s(t, "stance"),
                t.get("description").and_then(|v| v.as_str()).unwrap_or("")
            );
        }
        println!("\n{} tool(s).", data.len());
    })
}

pub fn agent_tools_rules(agent: &str, json: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let agent_id = resolve_agent_id(&client, agent)?;
    let resp: Value = client.get_json(&format!("/mcp/agents/{agent_id}/tools"))?;
    let data = resp
        .get("data")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    print_json_or(json, &resp, || {
        if data.is_empty() {
            println!("No tool rules set (agent is on full default-allow).");
            return;
        }
        println!("{:<36} {:<28} STANCE", "CONNECTOR ID", "PATTERN");
        for r in &data {
            println!(
                "{:<36} {:<28} {}",
                s(r, "connector_id"),
                s(r, "tool_pattern"),
                s(r, "stance")
            );
        }
        println!("\n{} rule(s).", data.len());
    })
}

/// Merge one tool-pattern/stance rule into an agent's existing rule set for
/// one connector: replace the pattern's stance if it's already present,
/// otherwise append it. Rules belonging to other connectors are dropped —
/// `PUT /mcp/agents/{agent_id}/tools` replaces the *entire* rule set for
/// every connector named in the request (it is not a per-pattern upsert),
/// so the caller must only ever submit the one connector it's touching.
fn merge_tool_rule(
    existing: &[Value],
    connector_id: &str,
    pattern: &str,
    stance: &str,
) -> Vec<Value> {
    let mut rules: Vec<Value> = existing
        .iter()
        .filter(|r| s(r, "connector_id") == connector_id)
        .cloned()
        .collect();

    match rules.iter_mut().find(|r| s(r, "tool_pattern") == pattern) {
        Some(r) => r["stance"] = serde_json::json!(stance),
        None => rules.push(serde_json::json!({ "connector_id": connector_id, "tool_pattern": pattern, "stance": stance })),
    }
    rules
}

/// Set (or update) one tool-pattern rule for a connector on an agent.
///
/// `PUT /mcp/agents/{agent_id}/tools` replaces the *entire* rule set for every
/// connector named in the request — it is not a per-pattern upsert. Sending
/// only the new rule would silently drop every other pattern already set on
/// this connector, so this is a read-modify-write: fetch the connector's
/// current rules, merge in the new pattern (replacing it if already present),
/// and PUT the full merged set back.
pub fn agent_tools_set_rule(
    agent: &str,
    connector_id: &str,
    pattern: &str,
    stance: &str,
) -> Result<()> {
    if !["allow", "ask", "block"].contains(&stance) {
        bail!("stance must be one of allow, ask, block");
    }
    let client = Client::from_active_cluster()?;
    agent_tools_set_rule_with(&client, agent, connector_id, pattern, stance)
}

fn agent_tools_set_rule_with(
    client: &Client,
    agent: &str,
    connector_id: &str,
    pattern: &str,
    stance: &str,
) -> Result<()> {
    let agent_id = resolve_agent_id(client, agent)?;

    let existing: Value = client.get_json(&format!("/mcp/agents/{agent_id}/tools"))?;
    let existing_rules = existing
        .get("data")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let rules = merge_tool_rule(&existing_rules, connector_id, pattern, stance);

    let count = rules.len();
    let _: Value = client.put_json(
        &format!("/mcp/agents/{agent_id}/tools"),
        &serde_json::json!({ "rules": rules }),
    )?;
    println!(
        "Set '{pattern}' → {stance} for connector {connector_id} on agent '{agent}' ({count} rule(s) now active for this connector)."
    );
    Ok(())
}

pub fn agent_tools_reset(agent: &str, yes: bool) -> Result<()> {
    if !confirm(
        &format!(
            "Reset agent '{agent}' to full default-allow? This clears every connector/tool override."
        ),
        yes,
    )? {
        println!("Cancelled.");
        return Ok(());
    }
    let client = Client::from_active_cluster()?;
    let agent_id = resolve_agent_id(&client, agent)?;
    let resp: Value = client.delete_and_read(&format!("/mcp/agents/{agent_id}/permissions"))?;
    println!(
        "Reset agent '{agent}' to full default-allow ({} rule row(s) removed).",
        resp.get("rows_deleted")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
    );
    Ok(())
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Matcher;

    // ── parse_headers ───────────────────────────────────────────────────────

    #[test]
    fn parse_headers_basic() {
        let h = parse_headers(&["X-Foo: bar".to_string()]).unwrap();
        assert_eq!(h.get("X-Foo").map(String::as_str), Some("bar"));
    }

    #[test]
    fn parse_headers_multiple_entries() {
        let h = parse_headers(&["X-Foo: bar".to_string(), "X-Baz: qux".to_string()]).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h.get("X-Baz").map(String::as_str), Some("qux"));
    }

    #[test]
    fn parse_headers_trims_whitespace_around_key_and_value() {
        let h = parse_headers(&["  X-Foo  :  bar  ".to_string()]).unwrap();
        assert_eq!(h.get("X-Foo").map(String::as_str), Some("bar"));
    }

    #[test]
    fn parse_headers_missing_colon_is_an_error() {
        let err = parse_headers(&["NoColonHere".to_string()]).unwrap_err();
        assert!(err.to_string().contains("invalid --header"), "got: {err}");
    }

    #[test]
    fn parse_headers_empty_value_is_allowed() {
        let h = parse_headers(&["X-Foo:".to_string()]).unwrap();
        assert_eq!(h.get("X-Foo").map(String::as_str), Some(""));
    }

    #[test]
    fn parse_headers_duplicate_key_last_one_wins() {
        let h = parse_headers(&["X-Foo: 1".to_string(), "X-Foo: 2".to_string()]).unwrap();
        assert_eq!(h.get("X-Foo").map(String::as_str), Some("2"));
    }

    #[test]
    fn parse_headers_only_splits_on_first_colon() {
        // A bearer-token value containing its own colon (e.g. "type:secret")
        // must survive whole, not get truncated at the first ':'.
        let h = parse_headers(&["Authorization: Bearer abc:def".to_string()]).unwrap();
        assert_eq!(
            h.get("Authorization").map(String::as_str),
            Some("Bearer abc:def")
        );
    }

    #[test]
    fn parse_headers_empty_list_returns_empty_map() {
        let h = parse_headers(&[]).unwrap();
        assert!(h.is_empty());
    }

    // ── share_target_body / share_target_label ──────────────────────────────

    #[test]
    fn share_target_body_rejects_both_user_and_public() {
        let err = share_target_body(Some("alice"), true).unwrap_err();
        assert!(err.to_string().contains("not both"), "got: {err}");
    }

    #[test]
    fn share_target_body_rejects_neither_user_nor_public() {
        let err = share_target_body(None, false).unwrap_err();
        assert!(err.to_string().contains("--user"), "got: {err}");
    }

    #[test]
    fn share_target_body_public_true() {
        let body = share_target_body(None, true).unwrap();
        assert_eq!(body, serde_json::json!({ "public": true }));
    }

    #[test]
    fn share_target_body_named_user() {
        let body = share_target_body(Some("alice"), false).unwrap();
        assert_eq!(body, serde_json::json!({ "username": "alice" }));
    }

    #[test]
    fn share_target_body_rejects_empty_username() {
        assert!(share_target_body(Some(""), false).is_err());
    }

    #[test]
    fn share_target_body_rejects_whitespace_only_username() {
        assert!(share_target_body(Some("   "), false).is_err());
    }

    #[test]
    fn share_target_label_public_is_everyone() {
        assert_eq!(share_target_label(None, true), "everyone");
    }

    #[test]
    fn share_target_label_named_user() {
        assert_eq!(share_target_label(Some("alice"), false), "alice");
    }

    #[test]
    fn share_target_label_falls_back_when_neither_given() {
        assert_eq!(share_target_label(None, false), "?");
    }

    // ── s / b JSON accessors ─────────────────────────────────────────────────

    #[test]
    fn s_returns_dash_when_field_missing() {
        let v = serde_json::json!({});
        assert_eq!(s(&v, "name"), "-");
    }

    #[test]
    fn s_returns_dash_when_field_is_wrong_type() {
        let v = serde_json::json!({ "name": 42 });
        assert_eq!(s(&v, "name"), "-");
    }

    #[test]
    fn s_returns_dash_when_field_is_null() {
        let v = serde_json::json!({ "name": null });
        assert_eq!(s(&v, "name"), "-");
    }

    #[test]
    fn s_returns_present_string_value() {
        let v = serde_json::json!({ "name": "tutorial-server" });
        assert_eq!(s(&v, "name"), "tutorial-server");
    }

    #[test]
    fn b_returns_false_when_field_missing() {
        let v = serde_json::json!({});
        assert!(!b(&v, "enabled"));
    }

    #[test]
    fn b_returns_false_when_field_is_wrong_type() {
        let v = serde_json::json!({ "enabled": "true" });
        assert!(!b(&v, "enabled"));
    }

    #[test]
    fn b_returns_present_true_value() {
        let v = serde_json::json!({ "enabled": true });
        assert!(b(&v, "enabled"));
    }

    // ── stance validation (agent_tools_set_rule bails before any network call) ─

    #[test]
    fn set_rule_rejects_invalid_stance_without_touching_the_network() {
        // No cluster is configured in the test environment, so if this reached
        // `Client::from_active_cluster()` it would fail with a config error,
        // not the stance-validation message below — asserting on the exact
        // message proves the bail happens first.
        let err = agent_tools_set_rule("agent", "connector", "some_tool", "maybe").unwrap_err();
        assert!(
            err.to_string()
                .contains("stance must be one of allow, ask, block"),
            "got: {err}"
        );
    }

    #[test]
    fn set_rule_stance_is_case_sensitive() {
        let err = agent_tools_set_rule("agent", "connector", "some_tool", "ALLOW").unwrap_err();
        assert!(
            err.to_string()
                .contains("stance must be one of allow, ask, block"),
            "got: {err}"
        );
    }

    // ── merge_tool_rule (the read-modify-write correctness core) ─────────────

    #[test]
    fn merge_tool_rule_appends_new_pattern_to_empty_set() {
        let rules = merge_tool_rule(&[], "conn-a", "delete_all_data", "block");
        assert_eq!(
            rules,
            vec![
                serde_json::json!({ "connector_id": "conn-a", "tool_pattern": "delete_all_data", "stance": "block" })
            ]
        );
    }

    #[test]
    fn merge_tool_rule_updates_existing_pattern_in_place_without_duplicating() {
        let existing = vec![
            serde_json::json!({ "connector_id": "conn-a", "tool_pattern": "echo", "stance": "allow" }),
        ];
        let rules = merge_tool_rule(&existing, "conn-a", "echo", "block");
        assert_eq!(rules.len(), 1);
        assert_eq!(s(&rules[0], "stance"), "block");
    }

    #[test]
    fn merge_tool_rule_drops_rules_belonging_to_other_connectors() {
        let existing = vec![
            serde_json::json!({ "connector_id": "conn-a", "tool_pattern": "echo", "stance": "allow" }),
            serde_json::json!({ "connector_id": "conn-b", "tool_pattern": "other_tool", "stance": "ask" }),
        ];
        let rules = merge_tool_rule(&existing, "conn-a", "echo", "block");
        // Only conn-a's rules are ever submitted in one PUT — conn-b's rule
        // isn't touched by this call, so it must not appear in this result.
        assert_eq!(rules.len(), 1);
        assert_eq!(s(&rules[0], "connector_id"), "conn-a");
    }

    #[test]
    fn merge_tool_rule_preserves_other_patterns_on_the_same_connector() {
        let existing = vec![
            serde_json::json!({ "connector_id": "conn-a", "tool_pattern": "delete_all_data", "stance": "block" }),
            serde_json::json!({ "connector_id": "conn-a", "tool_pattern": "echo", "stance": "allow" }),
        ];
        let rules = merge_tool_rule(&existing, "conn-a", "send_notification", "ask");
        assert_eq!(rules.len(), 3);
        assert!(
            rules
                .iter()
                .any(|r| s(r, "tool_pattern") == "delete_all_data" && s(r, "stance") == "block")
        );
        assert!(
            rules
                .iter()
                .any(|r| s(r, "tool_pattern") == "echo" && s(r, "stance") == "allow")
        );
        assert!(
            rules
                .iter()
                .any(|r| s(r, "tool_pattern") == "send_notification" && s(r, "stance") == "ask")
        );
    }

    #[test]
    fn merge_tool_rule_pattern_matching_is_case_sensitive() {
        let existing = vec![
            serde_json::json!({ "connector_id": "conn-a", "tool_pattern": "Echo", "stance": "allow" }),
        ];
        let rules = merge_tool_rule(&existing, "conn-a", "echo", "block");
        // "Echo" and "echo" are different patterns — both must be present,
        // not merged into one.
        assert_eq!(rules.len(), 2);
    }

    #[test]
    fn merge_tool_rule_treats_wildcard_patterns_as_opaque_strings() {
        let rules = merge_tool_rule(&[], "conn-a", "SEND_*", "ask");
        assert_eq!(s(&rules[0], "tool_pattern"), "SEND_*");
    }

    // ── resolve_agent_id ──────────────────────────────────────────────────────

    #[test]
    fn resolve_agent_id_uuid_input_short_circuits_without_a_network_call() {
        // Port 0 on loopback refuses immediately — if this function made an
        // HTTP request for a bare UUID input, this call would return an Err
        // (connection refused) instead of Ok.
        let client = Client::for_test("http://127.0.0.1:0", None);
        let id = "550e8400-e29b-41d4-a716-446655440000";
        assert_eq!(resolve_agent_id(&client, id).unwrap(), id);
    }

    #[test]
    fn resolve_agent_id_by_name_single_match() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/api/agents")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"[{"id":"11111111-1111-1111-1111-111111111111","name":"research-agent-1"}]"#,
            )
            .create();
        let client = Client::for_test(&server.url(), None);
        assert_eq!(
            resolve_agent_id(&client, "research-agent-1").unwrap(),
            "11111111-1111-1111-1111-111111111111"
        );
    }

    #[test]
    fn resolve_agent_id_by_name_not_found() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/api/agents")
            .with_status(200)
            .with_body("[]")
            .create();
        let client = Client::for_test(&server.url(), None);
        let err = resolve_agent_id(&client, "nonexistent-agent").unwrap_err();
        assert!(err.to_string().contains("not found"), "got: {err}");
    }

    #[test]
    fn resolve_agent_id_by_name_ambiguous_across_owners_bails_instead_of_guessing() {
        // Two different owners can each have an agent named "shared-name"
        // (the DB's uniqueness constraint is per-owner, not global) — GET
        // /agents can legitimately return both if they're public/granted to
        // the caller. Silently picking one would risk mutating a stranger's
        // agent's tool permissions.
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/api/agents")
            .with_status(200)
            .with_body(
                r#"[
                    {"id":"11111111-1111-1111-1111-111111111111","name":"shared-name"},
                    {"id":"22222222-2222-2222-2222-222222222222","name":"shared-name"}
                ]"#,
            )
            .create();
        let client = Client::for_test(&server.url(), None);
        let err = resolve_agent_id(&client, "shared-name").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("ambiguous"), "got: {msg}");
        assert!(
            msg.contains("11111111-1111-1111-1111-111111111111"),
            "got: {msg}"
        );
        assert!(
            msg.contains("22222222-2222-2222-2222-222222222222"),
            "got: {msg}"
        );
    }

    // ── full HTTP round trips ─────────────────────────────────────────────────

    #[test]
    fn set_rule_sends_only_the_target_connectors_merged_rules() {
        let mut server = mockito::Server::new();
        let agent_id = "550e8400-e29b-41d4-a716-446655440000";

        let _get = server
            .mock("GET", format!("/api/mcp/agents/{agent_id}/tools").as_str())
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"data": [
                    {"connector_id": "conn-a", "tool_pattern": "delete_all_data", "stance": "block"},
                    {"connector_id": "conn-a", "tool_pattern": "echo", "stance": "allow"},
                    {"connector_id": "conn-b", "tool_pattern": "unrelated_tool", "stance": "ask"}
                ]}"#,
            )
            .create();

        // The PUT must contain conn-a's two rules (echo's stance flipped to
        // "block", delete_all_data untouched) and must NOT contain conn-b's
        // rule — that's the whole point of the read-modify-write.
        let expected_body = serde_json::json!({
            "rules": [
                {"connector_id": "conn-a", "tool_pattern": "delete_all_data", "stance": "block"},
                {"connector_id": "conn-a", "tool_pattern": "echo", "stance": "block"}
            ]
        });
        let _put = server
            .mock("PUT", format!("/api/mcp/agents/{agent_id}/tools").as_str())
            .match_body(Matcher::Json(expected_body))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("{}")
            .create();

        let client = Client::for_test(&server.url(), None);
        agent_tools_set_rule_with(&client, agent_id, "conn-a", "echo", "block").unwrap();
        // If the PUT body didn't match, mockito would 501 and `put_json`
        // would surface that as an Err — `.unwrap()` above already proves it.
    }

    #[test]
    fn set_rule_by_agent_name_resolves_before_reading_tools() {
        let mut server = mockito::Server::new();
        let agent_id = "550e8400-e29b-41d4-a716-446655440000";

        let _agents = server
            .mock("GET", "/api/agents")
            .with_status(200)
            .with_body(format!(
                r#"[{{"id":"{agent_id}","name":"research-agent-1"}}]"#
            ))
            .create();
        let _get = server
            .mock("GET", format!("/api/mcp/agents/{agent_id}/tools").as_str())
            .with_status(200)
            .with_body(r#"{"data": []}"#)
            .create();
        let _put = server
            .mock("PUT", format!("/api/mcp/agents/{agent_id}/tools").as_str())
            .match_body(Matcher::Json(serde_json::json!({
                "rules": [{"connector_id": "conn-a", "tool_pattern": "echo", "stance": "allow"}]
            })))
            .with_status(200)
            .with_body("{}")
            .create();

        let client = Client::for_test(&server.url(), None);
        agent_tools_set_rule_with(&client, "research-agent-1", "conn-a", "echo", "allow").unwrap();
    }

    #[test]
    fn share_add_public_sends_public_true_body() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("POST", "/api/mcp/connectors/conn-a/share")
            .match_body(Matcher::Json(serde_json::json!({ "public": true })))
            .with_status(200)
            .with_body("{}")
            .create();
        let client = Client::for_test(&server.url(), None);
        share_add_with(&client, "conn-a", None, true).unwrap();
    }

    #[test]
    fn share_add_named_user_sends_username_body() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("POST", "/api/mcp/connectors/conn-a/share")
            .match_body(Matcher::Json(serde_json::json!({ "username": "alice" })))
            .with_status(200)
            .with_body("{}")
            .create();
        let client = Client::for_test(&server.url(), None);
        share_add_with(&client, "conn-a", Some("alice"), false).unwrap();
    }

    #[test]
    fn share_remove_sends_delete_with_a_json_body() {
        // DELETE with a body is off-spec — this exercises `force_send_body()`
        // actually round-tripping through a real HTTP request, not just
        // compiling.
        let mut server = mockito::Server::new();
        let _m = server
            .mock("DELETE", "/api/mcp/connectors/conn-a/share")
            .match_body(Matcher::Json(serde_json::json!({ "username": "alice" })))
            .with_status(200)
            .with_body("{}")
            .create();
        let client = Client::for_test(&server.url(), None);
        share_remove_with(&client, "conn-a", Some("alice"), false).unwrap();
    }

    #[test]
    fn credential_set_with_inline_value_skips_the_interactive_prompt() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("POST", "/api/mcp/connectors/conn-a/credential")
            .match_body(Matcher::Json(serde_json::json!({ "value": "secret-123" })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"name": "my-server"}"#)
            .create();
        let client = Client::for_test(&server.url(), None);
        credential_set_with(&client, "conn-a", "secret-123").unwrap();
    }

    #[test]
    fn get_json_error_response_bubbles_up_with_status_and_body() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/api/mcp/connections")
            .with_status(401)
            .with_body(r#"{"error":"session expired"}"#)
            .create();
        let client = Client::for_test(&server.url(), None);
        let err = connections_with(&client, false).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("401"), "got: {msg}");
        assert!(msg.contains("session expired"), "got: {msg}");
        assert!(msg.contains("nasiko auth login"), "got: {msg}");
    }

    #[test]
    fn credential_status_reports_disconnected_state() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/api/mcp/connectors/conn-a/credential/status")
            .with_status(200)
            .with_body(r#"{"name": "my-server", "connected": false, "auth_type": "bearer"}"#)
            .create();
        let client = Client::for_test(&server.url(), None);
        credential_status_with(&client, "conn-a", false).unwrap();
    }
}
