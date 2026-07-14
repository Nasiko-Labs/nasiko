use std::path::PathBuf;

use anyhow::{Context, Result};
use nasiko_utils::display::{opt_or, trunc};
use serde::{Deserialize, Serialize};
use tabled::Tabled;

use crate::commands::tui::opt_endpoint;
use crate::config;

// ─── CP session types (mirrors cp-lib/src/chat/models.rs) ───────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Tabled)]
pub struct CpSession {
    #[tabled(rename = "ID")]
    pub session_id: String,
    #[tabled(skip)]
    pub agent_url: Option<String>,
    #[tabled(rename = "AGENT", display("opt_or", "orchestrator"))]
    pub agent_name: Option<String>,
    #[tabled(rename = "UPDATED", display("trunc", 19))]
    pub updated_at: String,
    #[tabled(rename = "TITLE")]
    pub title: String,
    #[tabled(skip)]
    pub created_at: String,
    #[tabled(skip)]
    pub last_message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CpSessionPage {
    pub data: Vec<CpSession>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpMessage {
    pub role: String,
    pub content: String,
    pub timestamp: String,
}

// ─── Local session types (for direct-agent mode) ────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Tabled)]
pub struct LocalSession {
    #[tabled(rename = "ID")]
    pub id: String,
    #[tabled(skip)]
    pub context_id: String,
    #[tabled(rename = "ENDPOINT", display = "opt_endpoint")]
    pub endpoint: String,
    #[tabled(rename = "CREATED", display("trunc", 19))]
    pub created_at: String,
    #[tabled(rename = "TITLE")]
    pub title: String,
    #[tabled(skip)]
    pub messages: Vec<LocalMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalMessage {
    pub role: String,
    pub text: String,
    pub timestamp: String,
}

// ─── Unified session handle used by the TUI ─────────────────────────────────

#[derive(Debug, Clone)]
pub enum SessionBackend {
    Cp { base_url: String, token: String, session_id: String },
    Local,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub context_id: String,
    pub endpoint: String,
    pub title: String,
    pub backend: SessionBackend,
}

// ─── Local persistence ──────────────────────────────────────────────────────

fn sessions_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".nasiko")
        .join("sessions")
}

pub fn list_local_sessions() -> Result<Vec<LocalSession>> {
    let dir = sessions_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut sessions = Vec::new();
    for entry in std::fs::read_dir(&dir).context("reading sessions dir")? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json")
            && let Ok(content) = std::fs::read_to_string(&path)
                && let Ok(session) = serde_json::from_str::<LocalSession>(&content) {
                    sessions.push(session);
                }
    }
    sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(sessions)
}

pub fn load_local_session(id: &str) -> Result<LocalSession> {
    let path = sessions_dir().join(format!("{id}.json"));
    let content =
        std::fs::read_to_string(&path).with_context(|| format!("session '{id}' not found"))?;
    serde_json::from_str(&content).context("invalid session file")
}

pub fn save_local_session(session: &LocalSession) -> Result<()> {
    let dir = sessions_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", session.id));
    let content = serde_json::to_string_pretty(session)?;
    std::fs::write(path, content)?;
    Ok(())
}

pub fn create_local_session(endpoint: &str) -> LocalSession {
    let now = chrono::Utc::now().to_rfc3339();
    LocalSession {
        id: uuid::Uuid::new_v4().to_string(),
        context_id: uuid::Uuid::new_v4().to_string(),
        endpoint: endpoint.to_string(),
        created_at: now,
        title: "New chat".to_string(),
        messages: Vec::new(),
    }
}

// ─── CP API calls ───────────────────────────────────────────────────────────

fn is_cp_endpoint(endpoint: &str) -> bool {
    config::active_url()
        .ok()
        .is_some_and(|u| endpoint.starts_with(&u))
}

pub fn cp_credentials(endpoint: &str) -> Option<(String, String)> {
    if !is_cp_endpoint(endpoint) {
        return None;
    }
    let base_url = config::active_url().ok()?;
    let token = config::active_token().ok()??;
    Some((base_url, token))
}

pub fn list_cp_sessions(
    base_url: &str,
    token: &str,
    cursor: Option<&str>,
    limit: Option<u32>,
) -> Result<CpSessionPage> {
    let http = ureq::Agent::new_with_config(
        ureq::config::Config::builder().timeout_global(None).build(),
    );
    let mut url = format!("{base_url}/api/chat/sessions");
    let mut params: Vec<String> = Vec::new();
    if let Some(c) = cursor { params.push(format!("cursor={}", crate::api::urlencode(c))); }
    if let Some(l) = limit { params.push(format!("limit={l}")); }
    if !params.is_empty() { url.push('?'); url.push_str(&params.join("&")); }
    let mut resp = http
        .get(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .call()
        .context("failed to list CP sessions")?;
    resp.body_mut().read_json().context("invalid sessions JSON")
}

pub fn create_cp_session(
    base_url: &str,
    token: &str,
    agent_url: &str,
    title: &str,
) -> Result<CpSession> {
    create_cp_session_with_id(base_url, token, agent_url, title, None)
}

/// Create a CP session, optionally with a client-chosen `session_id`.
/// When the ID already belongs to the caller the server returns the existing
/// session, so this doubles as an ensure-exists call.
pub fn create_cp_session_with_id(
    base_url: &str,
    token: &str,
    agent_url: &str,
    title: &str,
    session_id: Option<&str>,
) -> Result<CpSession> {
    let http = ureq::Agent::new_with_config(
        ureq::config::Config::builder().timeout_global(None).build(),
    );
    let url = format!("{base_url}/api/chat/sessions");
    let mut body = serde_json::json!({
        "agent_url": agent_url,
        "title": title,
    });
    if let Some(sid) = session_id {
        body["session_id"] = serde_json::Value::String(sid.to_string());
    }
    let mut resp = http
        .post(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .send_json(&body)
        .context("failed to create CP session")?;
    let session: CpSession = resp.body_mut().read_json().context("invalid session JSON")?;
    Ok(session)
}

pub fn fetch_cp_messages(
    base_url: &str,
    token: &str,
    session_id: &str,
) -> Result<Vec<CpMessage>> {
    #[derive(Deserialize)]
    struct Page { data: Vec<CpMessage> }

    let http = ureq::Agent::new_with_config(
        ureq::config::Config::builder().timeout_global(None).build(),
    );
    let url = format!("{base_url}/api/chat/sessions/{session_id}/messages");
    let mut resp = http
        .get(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .call()
        .context("failed to fetch messages")?;
    let page: Page = resp.body_mut().read_json().context("invalid messages JSON")?;
    Ok(page.data)
}

pub fn post_cp_message(
    base_url: &str,
    token: &str,
    session_id: &str,
    role: &str,
    content: &str,
    trace_id: Option<&str>,
) -> Result<()> {
    let http = ureq::Agent::new_with_config(
        ureq::config::Config::builder().timeout_global(None).build(),
    );
    let url = format!("{base_url}/api/chat/sessions/{session_id}/messages");
    let mut body = serde_json::json!({ "role": role, "content": content });
    if let Some(tid) = trace_id {
        body["trace_id"] = serde_json::Value::String(tid.to_string());
    }
    http.post(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .send_json(&body)
        .context("failed to post message")?;
    Ok(())
}

pub fn delete_cp_session(base_url: &str, token: &str, session_id: &str) -> Result<()> {
    let http = ureq::Agent::new_with_config(
        ureq::config::Config::builder().timeout_global(None).build(),
    );
    let url = format!("{base_url}/api/chat/sessions/{session_id}");
    let resp = http
        .delete(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .call()
        .context("failed to delete session")?;
    let status = resp.status().as_u16();
    if status >= 400 {
        anyhow::bail!("server returned HTTP {status} for delete session");
    }
    Ok(())
}

// ─── Unified helpers ────────────────────────────────────────────────────────

pub fn start_session(endpoint: &str) -> Result<Session> {
    if let Some((base_url, token)) = cp_credentials(endpoint) {
        let cp_session = create_cp_session(&base_url, &token, endpoint, "New chat")?;
        Ok(Session {
            id: cp_session.session_id.clone(),
            context_id: cp_session.session_id.clone(),
            endpoint: endpoint.to_string(),
            title: cp_session.title,
            backend: SessionBackend::Cp {
                base_url,
                token,
                session_id: cp_session.session_id,
            },
        })
    } else {
        let local = create_local_session(endpoint);
        save_local_session(&local)?;
        Ok(Session {
            id: local.id.clone(),
            context_id: local.context_id.clone(),
            endpoint: endpoint.to_string(),
            title: local.title,
            backend: SessionBackend::Local,
        })
    }
}

pub fn resume_session(session_id: &str, endpoint: &str) -> Result<(Session, Vec<(String, String)>)> {
    if let Some((base_url, token)) = cp_credentials(endpoint) {
        let messages = fetch_cp_messages(&base_url, &token, session_id)?;
        let history: Vec<(String, String)> = messages
            .iter()
            .map(|m| (m.role.clone(), m.content.clone()))
            .collect();
        let session = Session {
            id: session_id.to_string(),
            context_id: session_id.to_string(),
            endpoint: endpoint.to_string(),
            title: "Resumed".to_string(),
            backend: SessionBackend::Cp {
                base_url,
                token,
                session_id: session_id.to_string(),
            },
        };
        Ok((session, history))
    } else {
        let local = load_local_session(session_id)?;
        let history: Vec<(String, String)> = local
            .messages
            .iter()
            .map(|m| (m.role.clone(), m.text.clone()))
            .collect();
        let session = Session {
            id: local.id.clone(),
            context_id: local.context_id.clone(),
            endpoint: local.endpoint.clone(),
            title: local.title,
            backend: SessionBackend::Local,
        };
        Ok((session, history))
    }
}
