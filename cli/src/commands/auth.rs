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
    let body: serde_json::Value = resp
        .body_mut()
        .read_json()
        .context("invalid login response")?;

    let token = body
        .get("token")
        .and_then(|t| t.as_str())
        .ok_or_else(|| anyhow::anyhow!("no token in login response"))?;

    config::save_login(&username, token)?;
    println!("Logged in to {} as {}", name, username);
    Ok(())
}

/// Show current auth status.
///
/// Checks the stored JWT's `exp` claim locally so an expired session is
/// reported as such instead of a misleading "Authenticated".
pub fn status() -> Result<()> {
    let (name, entry) = config::active_cluster()?;
    let Some(token) = &entry.token else {
        println!("Not authenticated. Run: nasiko auth login");
        return Ok(());
    };

    let who = entry
        .username
        .as_deref()
        .map(|u| format!(" as {u}"))
        .unwrap_or_default();

    match config::token_expired(token) {
        Some(true) => {
            println!(
                "Session expired for {}{} — run: nasiko auth login",
                name, who
            );
        }
        Some(false) => {
            let remaining = config::token_expiry(token)
                .map(|exp| exp - chrono::Utc::now().timestamp())
                .filter(|s| *s > 0)
                .map(|s| format!(" (expires in {})", format_duration(s)))
                .unwrap_or_default();
            println!("Authenticated to {}{}{}", name, who, remaining);
        }
        None => println!("Authenticated to {}{}", name, who),
    }
    Ok(())
}

fn format_duration(secs: i64) -> String {
    if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{}s", secs)
    }
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
    if config::token_expired(&token) == Some(true) {
        bail!("session expired — run: nasiko auth login");
    }

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
