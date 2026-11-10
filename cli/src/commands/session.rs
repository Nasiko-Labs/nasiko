//! `nasiko session new|show|ls|suspend|rm`: harness sessions on the active cluster.

use anyhow::{Result, bail};
use nasiko_utils::display::opt_dash;
use serde::{Deserialize, Serialize};
use tabled::settings::{Alignment, Style};
use tabled::{Table, Tabled};

use crate::api::{Client, urlencode};

/// Sessions per request when listing, the server's maximum.
const PAGE_SIZE: u32 = 200;

const NOT_FOUND: &str = "session not found (or not yours)";

/// `oss/server/src/sessions/routes.rs::SessionView`.
#[derive(Debug, Deserialize, Serialize)]
pub struct Session {
    pub session_id: String,
    pub harness: Option<String>,
    pub agent_id: Option<String>,
    pub runtime_state: String,
    pub sandbox_ref: Option<String>,
    pub last_turn_at: Option<String>,
    pub suspended_at: Option<String>,
    pub workspace_repo: Option<String>,
}

/// What `session ls` asks for.
pub struct ListQuery<'a> {
    pub harness: Option<&'a str>,
    pub state: Option<&'a str>,
    /// Follow every page instead of stopping after the first.
    pub all: bool,
    pub json: bool,
}

/// A paged envelope with `data` left undecoded.
#[derive(Debug, Deserialize)]
struct RawPage {
    data: Vec<serde_json::Value>,
    #[serde(default)]
    has_more: bool,
    next_cursor: Option<String>,
}

/// The envelope with `data` left undecoded, so `--json` passes unknown fields through.
#[derive(Debug, Deserialize)]
struct RawEnvelope {
    data: serde_json::Value,
}

#[derive(Serialize)]
struct NewSession<'a> {
    harness: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    repo: Option<&'a str>,
}

/// Fetch one of the caller's sessions as the server sent it; a 404 reads
/// "not found (or not yours)".
fn fetch_raw(client: &Client, id: &str) -> Result<serde_json::Value> {
    match client.get_json_or_status::<RawEnvelope>(&format!("/sessions/{id}"), &[404])? {
        Ok(env) => Ok(env.data),
        Err(_) => bail!(NOT_FOUND),
    }
}

pub fn new(harness: &str, repo: Option<&str>, json: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let raw: RawEnvelope = client.post_json("/sessions", &NewSession { harness, repo })?;
    if json {
        return print_json(&raw.data);
    }
    let s: Session = serde_json::from_value(raw.data)?;
    println!("Session:  {}", s.session_id);
    println!("State:    {}", s.runtime_state);
    println!("Chat with: nasiko chat --session {}", s.session_id);
    Ok(())
}

pub fn show(id: &str, json: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let raw = fetch_raw(&client, id)?;
    if json {
        return print_json(&raw);
    }
    let s: Session = serde_json::from_value(raw)?;
    print!("{}", render_session(&s));
    Ok(())
}

pub fn suspend(id: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    match client.post_void_or_status(&format!("/sessions/{id}/suspend"), &[404])? {
        Ok(()) => {
            println!("Suspended session {id}");
            Ok(())
        }
        Err(_) => bail!(NOT_FOUND),
    }
}

pub fn ls(query: &ListQuery) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let (items, more) = list_raw(&client, query)?;
    if query.json {
        return print_json(&serde_json::Value::Array(items));
    }
    let sessions: Vec<Session> = items
        .into_iter()
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()?;
    if sessions.is_empty() {
        println!("No sessions.");
        return Ok(());
    }
    print!("{}", render_table(&sessions));
    if more {
        println!("More sessions exist; use --all to list them.");
    }
    Ok(())
}

/// The caller's sessions as the server sent them, and whether more exist beyond the page
/// returned (always false with `all`, which follows the cursor to the end).
fn list_raw(client: &Client, query: &ListQuery) -> Result<(Vec<serde_json::Value>, bool)> {
    let mut items = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page: RawPage = client.get_json(&list_path(query, cursor.as_deref()))?;
        items.extend(page.data);
        match page.next_cursor.filter(|_| page.has_more) {
            Some(next) if query.all => cursor = Some(next),
            more => return Ok((items, more.is_some())),
        }
    }
}

fn list_path(query: &ListQuery, cursor: Option<&str>) -> String {
    let mut path = format!("/sessions?limit={PAGE_SIZE}");
    for (key, value) in [
        ("harness", query.harness),
        ("state", query.state),
        ("cursor", cursor),
    ] {
        if let Some(value) = value {
            path.push_str(&format!("&{key}={}", urlencode(value)));
        }
    }
    path
}

pub fn rm(id: &str, yes: bool) -> Result<()> {
    if !yes {
        let confirmed = dialoguer::Confirm::new()
            .with_prompt(format!("Delete session '{id}' and its sandbox?"))
            .default(false)
            .interact()?;
        if !confirmed {
            println!("Cancelled.");
            return Ok(());
        }
    }
    remove(&Client::from_active_cluster()?, id)?;
    println!("Deleted session {id}");
    Ok(())
}

fn remove(client: &Client, id: &str) -> Result<()> {
    match client.delete_or_status(&format!("/sessions/{id}"), &[404, 409])? {
        Ok(()) => Ok(()),
        Err(409) => bail!("a turn is running; `nasiko session suspend {id}` first"),
        Err(_) => bail!(NOT_FOUND),
    }
}

#[derive(Tabled)]
struct SessionTableRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "HARNESS")]
    harness: String,
    #[tabled(rename = "STATE")]
    state: String,
    #[tabled(rename = "LAST TURN")]
    last_turn: String,
    #[tabled(rename = "REPO")]
    repo: String,
}

fn render_table(sessions: &[Session]) -> String {
    let rows: Vec<SessionTableRow> = sessions
        .iter()
        .map(|s| SessionTableRow {
            id: s.session_id.clone(),
            harness: opt_dash(&s.harness),
            state: s.runtime_state.clone(),
            last_turn: opt_dash(&s.last_turn_at),
            repo: opt_dash(&s.workspace_repo),
        })
        .collect();
    let mut table = Table::new(rows);
    table.with(Style::blank()).with(Alignment::left());
    format!("{table}\n")
}

/// `nasiko chat --session <id> [message]`: resolve the session to its agent, then run the
/// ordinary chat loop against that agent's proxy URL with the session id as the A2A
/// `contextId`. With `--session` the one positional is the message, never a target.
pub fn chat(session: &str, first: Option<String>, second: Option<String>) -> Result<()> {
    let message = message_from_positionals(first, second)?;
    let client = Client::from_active_cluster()?;
    let s: Session = serde_json::from_value(fetch_raw(&client, session)?)?;
    let Some(agent_id) = s.agent_id else {
        bail!(
            "session {session} has no harness agent yet; try again shortly or open a new one \
             with `nasiko session new`"
        );
    };
    let endpoint = format!(
        "{}/api/agents/{agent_id}",
        client.base_url().trim_end_matches('/')
    );
    // No target label: the resume hint is then `nasiko chat --session <id>`, the one command
    // that resolves the session to its agent (a harness agent is named after the harness id,
    // see `seed.rs::upsert_harness_agent`, so a label would also work but teach a second form).
    super::chat::chat(
        &endpoint,
        message.as_deref(),
        Some(session),
        false,
        "",
        None,
        "--session",
    )
}

/// With `--session` the agent is fixed, so the only positional allowed is the message; clap
/// fills the first positional slot (`url`) first, hence one value may arrive in either slot.
fn message_from_positionals(
    first: Option<String>,
    second: Option<String>,
) -> Result<Option<String>> {
    if first.is_some() && second.is_some() {
        bail!("--session picks the agent; pass only a message, not a target and a message");
    }
    Ok(first.or(second))
}

fn print_json(s: &serde_json::Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(s)?);
    Ok(())
}

fn render_session(s: &Session) -> String {
    format!(
        "Session:      {}\nHarness:      {}\nState:        {}\nSandbox:      {}\n\
         Last turn:    {}\nSuspended at: {}\nRepo:         {}\n",
        s.session_id,
        opt_dash(&s.harness),
        s.runtime_state,
        opt_dash(&s.sandbox_ref),
        opt_dash(&s.last_turn_at),
        opt_dash(&s.suspended_at),
        opt_dash(&s.workspace_repo),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_server_envelope() {
        let body = r#"{"data":{"session_id":"s1","harness":"opencode",
            "agent_id":"7b0f3c1e-0000-4000-8000-000000000001","runtime_state":"running",
            "sandbox_ref":"docker:abc","last_turn_at":"2026-10-28T10:00:00Z",
            "suspended_at":null,"workspace_repo":"https://x/y.git"}}"#;
        let env: RawEnvelope = serde_json::from_str(body).unwrap();
        let s: Session = serde_json::from_value(env.data).unwrap();
        let out = render_session(&s);
        assert!(out.contains("running"));
        assert!(out.contains("docker:abc"));
        assert!(out.contains("Suspended at: -"));
    }

    fn session_json(id: &str, harness: &str) -> String {
        format!(
            r#"{{"session_id":"{id}","harness":"{harness}","agent_id":null,
            "runtime_state":"parked","sandbox_ref":null,"last_turn_at":null,
            "suspended_at":null,"workspace_repo":null}}"#
        )
    }

    fn query(all: bool) -> ListQuery<'static> {
        ListQuery {
            harness: Some("opencode"),
            state: None,
            all,
            json: false,
        }
    }

    #[test]
    fn ls_sends_the_filters_and_follows_the_cursor_only_with_all() {
        let mut srv = mockito::Server::new();
        let first = srv
            .mock("GET", "/api/sessions?limit=200&harness=opencode")
            .with_body(format!(
                r#"{{"data":[{}],"has_more":true,"next_cursor":"c1","prev_cursor":null}}"#,
                session_json("s1", "opencode")
            ))
            .expect(2)
            .create();
        let second = srv
            .mock("GET", "/api/sessions?limit=200&harness=opencode&cursor=c1")
            .with_body(format!(
                r#"{{"data":[{}],"has_more":false,"next_cursor":null,"prev_cursor":null}}"#,
                session_json("s2", "opencode")
            ))
            .expect(1)
            .create();
        let client = Client::for_test(&srv.url(), None);

        let (one_page, more) = list_raw(&client, &query(false)).unwrap();
        assert_eq!((one_page.len(), more), (1, true));
        let (every, more) = list_raw(&client, &query(true)).unwrap();
        assert_eq!((every.len(), more), (2, false));
        first.assert();
        second.assert();
    }

    #[test]
    fn the_table_lists_id_harness_state_last_turn_and_repo() {
        let s: Session = serde_json::from_str(&session_json("s1", "opencode")).unwrap();
        let out = render_table(&[s]);
        for header in ["ID", "HARNESS", "STATE", "LAST TURN", "REPO"] {
            assert!(out.contains(header), "{header}");
        }
        assert!(out.contains("s1") && out.contains("opencode") && out.contains("parked"));
    }

    #[test]
    fn rm_succeeds_on_204_and_explains_a_busy_or_missing_session() {
        let mut srv = mockito::Server::new();
        let gone = srv
            .mock("DELETE", "/api/sessions/ok")
            .with_status(204)
            .create();
        srv.mock("DELETE", "/api/sessions/busy")
            .with_status(409)
            .create();
        srv.mock("DELETE", "/api/sessions/nope")
            .with_status(404)
            .create();
        let client = Client::for_test(&srv.url(), None);

        remove(&client, "ok").unwrap();
        gone.assert();
        let busy = remove(&client, "busy").unwrap_err().to_string();
        assert_eq!(
            busy,
            "a turn is running; `nasiko session suspend busy` first"
        );
        assert_eq!(remove(&client, "nope").unwrap_err().to_string(), NOT_FOUND);
    }

    #[test]
    fn one_positional_is_the_message_in_either_slot() {
        let hi = Some("hi".to_string());
        assert_eq!(message_from_positionals(hi.clone(), None).unwrap(), hi);
        assert_eq!(message_from_positionals(None, hi.clone()).unwrap(), hi);
        assert_eq!(message_from_positionals(None, None).unwrap(), None);
    }

    #[test]
    fn target_plus_message_is_refused() {
        let a = Some("a".to_string());
        assert!(message_from_positionals(a.clone(), a).is_err());
    }

    #[test]
    fn omits_repo_when_absent() {
        let body = serde_json::to_string(&NewSession {
            harness: "opencode",
            repo: None,
        })
        .unwrap();
        assert_eq!(body, r#"{"harness":"opencode"}"#);
    }
}
