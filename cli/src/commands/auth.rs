use anyhow::{Context, Result, bail};

use crate::config;

/// Login to the active cluster with username/password.
pub fn login() -> Result<()> {
    let (name, entry) = config::active_cluster()?;

    let username: String = dialoguer::Input::new()
        .with_prompt("Username")
        .interact_text()?;
    let password: String = dialoguer::Password::new()
        .with_prompt("Password")
        .interact()?;

    let http = ureq::Agent::new_with_defaults();
    let url = format!("{}/api/auth/login", entry.url);

    let resp = http
        .post(&url)
        .header("Content-Type", "application/json")
        .send_json(serde_json::json!({
            "username": username,
            "password": password,
        }))
        .context("failed to reach control plane")?;

    if resp.status().as_u16() != 200 {
        let mut resp = resp;
        let body = resp.body_mut().read_to_string().unwrap_or_default();
        bail!("login failed (HTTP {}): {}", resp.status().as_u16(), body);
    }

    let mut resp = resp;
    let body: serde_json::Value = resp.body_mut().read_json().context("invalid login response")?;

    let token = body
        .get("token")
        .and_then(|t| t.as_str())
        .ok_or_else(|| anyhow::anyhow!("no token in login response"))?;

    config::save_login(&username, token)?;
    println!("Logged in to {} as {}", name, username);
    Ok(())
}

/// Show current auth status.
pub fn status() -> Result<()> {
    let (name, entry) = config::active_cluster()?;
    match (&entry.token, &entry.username) {
        (Some(_), Some(user)) => println!("Authenticated to {} as {}", name, user),
        (Some(_), None) => println!("Authenticated to: {}", name),
        _ => println!("Not authenticated. Run: nasiko auth login"),
    }
    Ok(())
}

/// Clear stored token.
pub fn logout() -> Result<()> {
    let (name, _) = config::active_cluster()?;
    config::clear_token()?;
    println!("Logged out from: {}", name);
    Ok(())
}

/// Print the currently authenticated user's profile.
pub fn whoami() -> Result<()> {
    let (cluster_name, entry) = config::active_cluster()?;
    let token = entry
        .token
        .ok_or_else(|| anyhow::anyhow!("not logged in — run: nasiko auth login"))?;

    let http = ureq::Agent::new_with_defaults();
    let url = format!("{}/api/users/me", entry.url);

    let resp = http
        .get(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .call()
        .context("failed to reach control plane")?;

    if resp.status().as_u16() != 200 {
        let mut resp = resp;
        let body = resp.body_mut().read_to_string().unwrap_or_default();
        bail!("HTTP {}: {}", resp.status().as_u16(), body);
    }

    let mut resp = resp;
    let user: serde_json::Value = resp.body_mut().read_json().context("invalid response")?;

    println!("Cluster:   {}", cluster_name);
    if let Some(v) = user.get("username").and_then(|v| v.as_str()) {
        println!("Username:  {}", v);
    }
    if let Some(v) = user.get("email").and_then(|v| v.as_str()) {
        println!("Email:     {}", v);
    }
    if let Some(v) = user.get("role").and_then(|v| v.as_str()) {
        println!("Role:      {}", v);
    }
    if let Some(v) = user.get("is_superuser").and_then(|v| v.as_bool()) {
        println!("Superuser: {}", v);
    }
    if let Some(v) = user.get("is_active").and_then(|v| v.as_bool()) {
        println!("Active:    {}", v);
    }
    if let Some(v) = user.get("created_at").and_then(|v| v.as_str()) {
        println!("Since:     {}", v);
    }
    Ok(())
}
