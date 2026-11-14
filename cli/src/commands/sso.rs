//! Browser sign-in to a control plane through its SSO, as the device grant (RFC 8628): the CLI
//! asks for a code, the person approves it in a browser signed in through the control plane's
//! identity provider, and the CLI polls until a session token comes back.
//!
//! The control plane says whether it has an identity provider — `GET /api/auth/oidc/status`
//! answers without a session — so there is nothing to ask before acting on it. A control plane
//! without these endpoints answers 404, and the caller signs in with a password instead.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use nasiko_utils::term as status;
use serde::Deserialize;
use ureq::Agent;

/// What `GET /api/auth/oidc/status` says about the control plane at `base`.
pub enum SsoAvailability {
    /// An identity provider is configured; the label is its recognisable name.
    Configured(String),
    /// The control plane supports SSO, but none is configured.
    NotConfigured,
    /// The control plane has no SSO endpoints (an open-source one, or older than this CLI).
    Unsupported,
}

pub fn availability(base: &str) -> Result<SsoAvailability> {
    let mut resp = agent()
        .get(&format!("{base}/api/auth/oidc/status"))
        .call()
        .with_context(|| format!("cannot reach the control plane at {base}"))?;
    let http_status = resp.status().as_u16();
    if http_status == 404 {
        return Ok(SsoAvailability::Unsupported);
    }
    if http_status >= 400 {
        bail!("could not check the control plane's SSO (HTTP {http_status})");
    }
    let body: serde_json::Value = resp.body_mut().read_json().unwrap_or_default();
    if !body
        .get("configured")
        .and_then(|c| c.as_bool())
        .unwrap_or(false)
    {
        return Ok(SsoAvailability::NotConfigured);
    }
    Ok(SsoAvailability::Configured(provider_label(
        body.get("provider").and_then(|p| p.as_str()),
    )))
}

/// Sign in through `provider` in the browser and return `(username, token)`: the username read
/// back from the new session, since the token belongs to whoever approved in the browser.
pub fn sign_in(base: &str, provider: &str) -> Result<(String, String)> {
    let agent = agent();
    let token = run_device_grant(&agent, base, provider)?;
    let username = signed_in_username(&agent, base, &token)?;
    Ok((username, token))
}

#[derive(Debug, Deserialize)]
struct CodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: String,
    expires_in: u64,
    interval: u64,
}

/// Request a code, send the person to a browser, poll until they decide. Returns the
/// control-plane session token.
fn run_device_grant(agent: &Agent, base: &str, provider: &str) -> Result<String> {
    let (code_status, body) = post_json(
        agent,
        &format!("{base}/api/auth/device/code"),
        serde_json::json!({}),
    )?;
    if code_status == 404 {
        bail!(
            "this control plane does not support browser sign-in. Use: {} --password",
            crate::commands::local_agents::login_hint()
        );
    }
    if code_status >= 400 {
        bail!("could not start sign-in (HTTP {code_status}): {body}");
    }
    let code: CodeResponse = serde_json::from_value(body)
        .context("the control plane returned an unexpected device-code response")?;

    println!();
    println!("  Signing in with {provider}.");
    println!();
    println!("  Open:  {}", code.verification_uri);
    println!("  Code:  {}", code.user_code);
    println!();
    crate::util::open_browser(&code.verification_uri_complete);

    poll(agent, base, &code)
}

fn poll(agent: &Agent, base: &str, code: &CodeResponse) -> Result<String> {
    let deadline = Instant::now() + Duration::from_secs(code.expires_in);
    // The server's interval is a floor: `slow_down` raises it for the rest of the grant, per
    // RFC 8628 §3.5, and it never comes back down.
    let mut interval = Duration::from_secs(code.interval.max(1));
    let url = format!("{base}/api/auth/device/token");

    let spinner = status::start_status("Waiting for you to approve in the browser...");

    while Instant::now() < deadline {
        std::thread::sleep(interval);
        let (http_status, body) = post_json(
            agent,
            &url,
            serde_json::json!({ "device_code": code.device_code }),
        )?;

        if http_status < 400 {
            drop(spinner);
            return body
                .get("access_token")
                .and_then(|t| t.as_str())
                .map(str::to_string)
                .context("sign-in was approved but no session came back");
        }

        match body.get("error").and_then(|e| e.as_str()).unwrap_or("") {
            "authorization_pending" => {}
            "slow_down" => interval += Duration::from_secs(5),
            "access_denied" => {
                drop(spinner);
                bail!("sign-in was denied in the browser");
            }
            "expired_token" => {
                drop(spinner);
                bail!("the code expired before it was approved — run the command again");
            }
            "" => {
                drop(spinner);
                let message = body
                    .get("message")
                    .and_then(|m| m.as_str())
                    .map(str::to_owned)
                    .unwrap_or_else(|| body.to_string());
                bail!("sign-in failed (HTTP {http_status}): {message}");
            }
            other => {
                drop(spinner);
                bail!("sign-in failed ({other})");
            }
        }
    }

    drop(spinner);
    bail!("timed out waiting for approval — run the command again")
}

fn signed_in_username(agent: &Agent, base: &str, token: &str) -> Result<String> {
    let mut resp = agent
        .get(&format!("{base}/api/users/me"))
        .header("Authorization", &format!("Bearer {token}"))
        .call()
        .context("signed in, but could not read the session back")?;
    if resp.status().as_u16() >= 400 {
        bail!(
            "signed in, but the control plane rejected the new session (HTTP {})",
            resp.status().as_u16()
        );
    }
    let user: serde_json::Value = resp.body_mut().read_json()?;
    user.get("username")
        .or_else(|| user.get("email"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .context("the control plane did not say who this session belongs to")
}

/// The name a person would recognise, for the line telling them where their browser is going.
fn provider_label(slug: Option<&str>) -> String {
    match slug {
        Some("okta") => "Okta".to_string(),
        Some("entra") => "Microsoft Entra".to_string(),
        Some("google") => "Google".to_string(),
        Some("keycloak") => "Keycloak".to_string(),
        Some(other) if !other.is_empty() => other.to_string(),
        _ => "your identity provider".to_string(),
    }
}

fn post_json(
    agent: &Agent,
    url: &str,
    body: serde_json::Value,
) -> Result<(u16, serde_json::Value)> {
    let mut resp = agent
        .post(url)
        .header("Content-Type", "application/json")
        .send_json(body)
        .with_context(|| format!("cannot reach the control plane at {url}"))?;
    let http_status = resp.status().as_u16();
    let parsed = resp
        .body_mut()
        .read_json()
        .unwrap_or(serde_json::Value::Null);
    Ok((http_status, parsed))
}

fn agent() -> Agent {
    Agent::new_with_config(
        ureq::config::Config::builder()
            .http_status_as_error(false)
            .build(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_slugs_become_recognisable_names() {
        assert_eq!(provider_label(Some("okta")), "Okta");
        assert_eq!(provider_label(Some("entra")), "Microsoft Entra");
        assert_eq!(provider_label(Some("google")), "Google");
        assert_eq!(provider_label(Some("custom-idp")), "custom-idp");
    }

    /// A missing or empty provider must still produce a readable sentence.
    #[test]
    fn an_unknown_provider_still_reads_naturally() {
        assert_eq!(provider_label(None), "your identity provider");
        assert_eq!(provider_label(Some("")), "your identity provider");
    }
}
