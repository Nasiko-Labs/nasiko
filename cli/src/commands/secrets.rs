use anyhow::{Result, bail};

use crate::api::Client;

pub fn set(key: &str, value: &str, agent: Option<&str>) -> Result<()> {
    let client = Client::from_active_cluster()?;
    match agent {
        Some(name) => {
            let agent_id = resolve_agent_id(&client, name)?;
            client.post_json_void(
                &format!("/agents/{agent_id}/secrets"),
                &serde_json::json!({"name": key, "value": value}),
            )?;
            println!("Set {key} on agent '{name}'");
        }
        None => {
            let _: serde_json::Value = client.post_json(
                "/secrets",
                &serde_json::json!({"name": key, "value": value}),
            )?;
            println!("Set {key} (vault — applies to all agents)");
        }
    }
    Ok(())
}

pub fn get(key: &str, agent: Option<&str>) -> Result<()> {
    let client = Client::from_active_cluster()?;
    match agent {
        Some(name) => {
            // No server route decrypts and returns an agent secret's value (only
            // list/set/delete exist) — agent secrets are write-only by design.
            bail!(
                "agent-scoped secrets are write-only — use `secrets ls --agent {name}` to see names, or `secrets set --agent {name}` to overwrite"
            );
        }
        None => {
            let raw: serde_json::Value = client.get_json(&format!("/secrets/{key}"))?;
            let resp: serde_json::Value = crate::api::unwrap_data(raw)?;
            if let Some(v) = resp.get("value").and_then(|v| v.as_str()) {
                println!("{v}");
            }
        }
    }
    Ok(())
}

pub fn ls(agent: Option<&str>) -> Result<()> {
    let client = Client::from_active_cluster()?;
    match agent {
        Some(name) => {
            let agent_id = resolve_agent_id(&client, name)?;
            let secrets: Vec<serde_json::Value> =
                client.get_json(&format!("/agents/{agent_id}/secrets"))?;
            if secrets.is_empty() {
                println!("No secrets on agent '{name}'.");
                return Ok(());
            }
            println!("Secrets for '{name}':");
            for s in &secrets {
                let key = s.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                println!("  {key}");
            }
        }
        None => {
            let raw: serde_json::Value = client.get_json("/secrets")?;
            let secrets: Vec<serde_json::Value> = crate::api::unwrap_data(raw)?;
            if secrets.is_empty() {
                println!("Vault is empty.");
                return Ok(());
            }
            println!("Vault secrets (injected into all agents):");
            for s in &secrets {
                let key = s.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                let updated = s.get("updated_at").and_then(|v| v.as_str()).unwrap_or("-");
                println!("  {key}  {updated}");
            }
        }
    }
    Ok(())
}

pub fn rm(key: &str, agent: Option<&str>) -> Result<()> {
    let client = Client::from_active_cluster()?;
    match agent {
        Some(name) => {
            let agent_id = resolve_agent_id(&client, name)?;
            client.delete(&format!("/agents/{agent_id}/secrets/{key}"))?;
            println!("Deleted {key} from agent '{name}'");
        }
        None => {
            client.delete(&format!("/secrets/{key}"))?;
            println!("Deleted {key} from vault");
        }
    }
    Ok(())
}

fn resolve_agent_id(client: &Client, name: &str) -> Result<String> {
    let agents: Vec<serde_json::Value> = client.get_json("/agents")?;
    for a in &agents {
        let agent_name = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if agent_name == name
            && let Some(id) = a.get("id").and_then(|v| v.as_str())
        {
            return Ok(id.to_string());
        }
    }
    anyhow::bail!("agent '{name}' not found — is it registered? (check `nasiko ps`)")
}
