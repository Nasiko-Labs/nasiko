use anyhow::Result;

use crate::api::Client;

/// Set a secret.
pub fn set(key: &str, value: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let _: serde_json::Value =
        client.post_json("/secrets", &serde_json::json!({"key": key, "value": value}))?;
    println!("Set: {key}");
    Ok(())
}

/// Get a secret value.
pub fn get(key: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let resp: serde_json::Value = client.get_json(&format!("/secrets/{key}"))?;
    if let Some(v) = resp.get("value").and_then(|v| v.as_str()) {
        println!("{v}");
    }
    Ok(())
}

/// List all secrets.
pub fn ls() -> Result<()> {
    let client = Client::from_active_cluster()?;
    let secrets: Vec<serde_json::Value> = client.get_json("/secrets")?;
    if secrets.is_empty() {
        println!("No secrets.");
        return Ok(());
    }
    println!("KEY UPDATED");
    for s in &secrets {
        let key = s.get("key").and_then(|v| v.as_str()).unwrap_or("?");
        let updated = s.get("updated_at").and_then(|v| v.as_str()).unwrap_or("-");
        println!("{} {}", key, updated);
    }
    Ok(())
}

/// Delete a secret.
pub fn rm(key: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    client.delete(&format!("/secrets/{key}"))?;
    println!("Deleted: {key}");
    Ok(())
}
