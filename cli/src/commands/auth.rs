use anyhow::{Context, Result, bail};

use crate::commands::sso::{self, SsoAvailability};
use crate::config;

/// How `auth login` signs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginMethod {
    /// SSO in the browser when the control plane has it, a password otherwise.
    Auto,
    Sso,
    Password,
}

impl LoginMethod {
    pub fn from_flags(sso: bool, password: bool) -> Self {
        match (sso, password) {
            (true, _) => Self::Sso,
            (_, true) => Self::Password,
            _ => Self::Auto,
        }
    }
}

/// Sign in to the active cluster, then offer to connect the coding agents found on this
/// machine.
pub fn login(method: LoginMethod) -> Result<()> {
    let (name, username) = authenticate(method)?;
    println!("Logged in to {} as {}", name, username);
    crate::commands::integration::auto_install_if_authenticated();
    Ok(())
}

/// Login without the automatic coding-agent setup — for `nasiko connect`, which asks what
/// to set up itself.
pub fn login_without_setup() -> Result<()> {
    let (name, username) = authenticate(LoginMethod::Auto)?;
    println!("Logged in to {} as {}", name, username);
    Ok(())
}

/// Signs in by `method` and saves the session token. Returns the cluster and username.
fn authenticate(method: LoginMethod) -> Result<(String, String)> {
    let (name, entry) = config::active_cluster()?;
    let base = entry.url.trim_end_matches('/');
    if method == LoginMethod::Password {
        return authenticate_with_password(name, base);
    }
    match sso::availability(base)? {
        SsoAvailability::Configured(provider) => {
            let (username, token) = sso::sign_in(base, &provider)?;
            config::save_login(&username, &token)?;
            Ok((name, username))
        }
        SsoAvailability::NotConfigured if method == LoginMethod::Sso => {
            bail!(
                "no SSO is configured on {base}; sign in with a password: {} --password",
                crate::commands::local_agents::login_hint()
            )
        }
        SsoAvailability::Unsupported if method == LoginMethod::Sso => {
            bail!(
                "{base} does not support SSO sign-in; sign in with a password: {} --password",
                crate::commands::local_agents::login_hint()
            )
        }
        SsoAvailability::NotConfigured | SsoAvailability::Unsupported => {
            authenticate_with_password(name, base)
        }
    }
}

/// Prompts for a username and password and saves the session token.
fn authenticate_with_password(name: String, base: &str) -> Result<(String, String)> {
    let username: String = dialoguer::Input::new()
        .with_prompt("Username")
        .interact_text()?;
    let password: String = dialoguer::Password::new()
        .with_prompt("Password")
        .interact()?;

    let http = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .http_status_as_error(false)
            .build(),
    );
    let url = format!("{base}/api/auth/login");

    let mut resp = http
        .post(&url)
        .header("Content-Type", "application/json")
        .send_json(serde_json::json!({
            "username": username,
            "password": password,
        }))
        .with_context(|| format!("cannot reach control plane at {base}"))?;

    let status = resp.status().as_u16();
    if status == 401 || status == 403 {
        bail!("invalid credentials — check your username and password");
    }
    if status >= 400 {
        let body = resp.body_mut().read_to_string().unwrap_or_default();
        bail!("login failed (HTTP {status}): {body}");
    }

    let body: serde_json::Value = resp
        .body_mut()
        .read_json()
        .context("invalid login response")?;

    let token = body
        .get("token")
        .and_then(|t| t.as_str())
        .ok_or_else(|| anyhow::anyhow!("no token in login response"))?;

    config::save_login(&username, token)?;
    Ok((name, username))
}

/// Show current auth status.
///
/// Checks the stored JWT's `exp` claim locally so an expired session is
/// reported as such instead of a misleading "Authenticated".
pub fn status() -> Result<()> {
    let (name, entry) = config::active_cluster()?;
    let Some(token) = &entry.token else {
        println!(
            "Not authenticated. Run: {}",
            crate::commands::local_agents::login_hint()
        );
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
                "Session expired for {}{} — run: {}",
                name,
                who,
                crate::commands::local_agents::login_hint()
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

/// Revoke the session server-side, then clear the stored token.
///
/// The revocation call is best-effort: an unreachable or already-invalid
/// server must not block clearing the local token, or `logout` would leave
/// you stuck "logged in" to a cluster you can no longer reach.
pub fn logout() -> Result<()> {
    let (name, _) = config::active_cluster()?;
    if let Ok(client) = crate::api::Client::from_active_cluster() {
        let _ = client.post_void("/auth/logout");
    }
    config::clear_token()?;
    println!("Logged out from: {}", name);
    Ok(())
}

/// Print the currently authenticated user's profile.
pub fn whoami() -> Result<()> {
    let (cluster_name, entry) = config::active_cluster()?;
    if entry.token.is_none() {
        bail!(
            "not logged in — run: {}",
            crate::commands::local_agents::login_hint()
        );
    }
    if config::token_expired(entry.token.as_deref().unwrap_or("")) == Some(true) {
        bail!(
            "session expired — run: {}",
            crate::commands::local_agents::login_hint()
        );
    }

    let client = crate::api::Client::from_active_cluster()?;
    let user: serde_json::Value = client.get_json("/users/me")?;

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
