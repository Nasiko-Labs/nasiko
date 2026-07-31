use std::io::{BufRead, Write as _};

use anyhow::{Context, Result, bail};

use crate::commands::tui::session::{self as cp};
use crate::config;
use nasiko_utils::term as status;

/// Live status line shown while the CLI waits on the backend. Re-settable:
/// each `set` clears the previous line first, so event output printed between
/// states never collides with an animation frame. Also tracks whether a
/// sub-agent's inline reply line is open, so it can be closed with a newline
/// before any other output prints.
struct Spinner {
    handle: Option<status::StatusHandle>,
    sub_open: bool,
    /// Whether the current agent call already streamed its reply inline —
    /// the result line then shrinks to a checkmark instead of repeating it.
    sub_streamed: bool,
    /// When the current agent call started, for the completion timing.
    call_started: Option<std::time::Instant>,
    /// Set whenever streamed answer text was printed to stdout without a
    /// trailing newline. The next status/progress line (stderr) checks this
    /// first so it never glues onto the end of the streamed text.
    stdout_dirty: bool,
}

impl Spinner {
    fn new() -> Self {
        Spinner {
            handle: None,
            sub_open: false,
            sub_streamed: false,
            call_started: None,
            stdout_dirty: false,
        }
    }

    /// Replace the status line with a new message (clears the old one first).
    fn set(&mut self, msg: impl Into<String>) {
        self.handle = None;
        self.handle = Some(status::start_status(msg));
    }

    /// Clear the status line (e.g. while streamed text is being printed).
    fn pause(&mut self) {
        self.handle = None;
    }

    /// End an in-progress sub-agent reply line, if one is open.
    fn close_sub(&mut self) {
        if self.sub_open {
            eprintln!("\x1b[0m");
            self.sub_open = false;
        }
    }

    /// Emit a newline to stdout if streamed answer text is still mid-line.
    /// Call this right before printing a new stderr status/progress line —
    /// NOT from `pause()`/`close_sub()`, which also run between consecutive
    /// chunks of the *same* streamed answer and must never break those apart.
    fn break_stdout(&mut self) {
        if self.stdout_dirty {
            println!();
            self.stdout_dirty = false;
        }
    }
}

/// Prior turns of a resumed CP session. Sessions themselves are owned by the
/// server: the agent proxy / orchestrator mint a `ses_*` id on the first
/// message and echo it back as the A2A `contextId` — the CLI never creates
/// one, it only reuses an id (from `--session-id` or a previous turn's echo).
fn fetch_cp_history(endpoint: &str, session_id: &str) -> Vec<cp::CpMessage> {
    let Some((base_url, token)) = cp::cp_credentials(endpoint) else {
        return Vec::new();
    };
    cp::fetch_cp_messages(&base_url, &token, session_id).unwrap_or_default()
}

/// Print a session's prior turns before resuming it, in the same visual
/// language as a live turn (`❯ you` / plain agent text), so continuing a
/// session reads as a pickup rather than starting cold.
fn print_history(history: &[cp::CpMessage]) {
    if history.is_empty() {
        return;
    }
    for msg in history {
        match msg.role.as_str() {
            "user" => println!("\x1b[1;36m❯ you\x1b[0m {}", msg.content),
            _ => println!("{}\n", msg.content),
        }
    }
    println!("\x1b[2m· resumed above — continuing below ·\x1b[0m\n");
}

/// Chat with an A2A agent (one-shot or interactive).
///
/// The URL is used as-is. The caller is responsible for providing the full endpoint:
/// - CP orchestrator: http://localhost:8080/api/orchestrator/a2a
/// - CP agent proxy:  http://localhost:8080/api/agents/{id}{transport_path}
///   (as printed by `nasiko ps` — the path comes from the agent's card)
/// - Direct agent:    http://localhost:10010/
///
/// `target_label` is what the user typed (agent name/id, or "" for the
/// orchestrator) — used verbatim in the resume hint so it can be copy-pasted.
pub fn chat(
    url: &str,
    message: Option<&str>,
    session_id: Option<&str>,
    target_label: &str,
) -> Result<()> {
    use std::io::IsTerminal;

    let endpoint = url.trim_end_matches('/').to_string();
    let is_cp = cp::cp_credentials(&endpoint).is_some();
    // Direct-agent chats (not routed through a CP orchestrator/proxy, which
    // already carries its own correct full path) don't all answer at the
    // bare host:port — e.g. the Go `a2a-go` SDK mounts its handler at
    // `/a2a`, not `/`. Read the agent's own card to find the right path
    // instead of assuming root.
    let endpoint = if is_cp {
        endpoint
    } else {
        let (_, rpc_path) = fetch_agent_card(&endpoint);
        format!("{endpoint}{rpc_path}")
    };
    // The session id, once known: passed in via --session-id, or adopted from
    // the server's echo after the first turn (see `fetch_cp_history`'s doc).
    let mut session: Option<String> = session_id.map(str::to_string);
    let history = match session.as_deref() {
        Some(sid) if is_cp => fetch_cp_history(&endpoint, sid),
        _ => Vec::new(),
    };

    // At a terminal, a message argument is just the first turn of a
    // conversation — answer it and keep the session open. Piped/scripted
    // invocations (non-TTY) stay strictly one-shot.
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();

    if let Some(msg) = message {
        print_history(&history);
        if let Some(sid) = send_message(&endpoint, msg, session.as_deref())? {
            session = Some(sid);
        }
        println!();
        if !interactive {
            print_resume_hint(is_cp, session.as_deref(), target_label);
            return Ok(());
        }
        println!();
    } else {
        let session_note = session
            .as_ref()
            .map(|sid| format!(" \x1b[2m· session {sid}\x1b[0m"))
            .unwrap_or_default();
        println!("\x1b[1mnasiko chat\x1b[0m \x1b[2m·\x1b[0m {endpoint}{session_note}");
        println!("\x1b[2mtype /quit to exit\x1b[0m\n");
        print_history(&history);
    }

    // Ctrl-C / Ctrl-D breaks the loop and leaves gracefully instead of erroring out.
    while let Ok(input) = dialoguer::Input::<String>::new()
        .with_prompt("\x1b[1;36m❯ you\x1b[0m")
        .allow_empty(true)
        .interact_text()
    {
        if input.trim().is_empty() {
            continue;
        }
        if input.trim() == "/quit" || input.trim() == "/exit" {
            break;
        }
        println!();
        match send_message(&endpoint, &input, session.as_deref()) {
            Ok(sid) => {
                if let Some(sid) = sid {
                    session = Some(sid);
                }
                println!("\n");
            }
            Err(e) => eprintln!("  \x1b[31merror:\x1b[0m {e}\n"),
        }
    }
    print_resume_hint(is_cp, session.as_deref(), target_label);
    Ok(())
}

/// Tell the user which session this chat belongs to and how to pick it back
/// up. Goes to stderr so piped/scripted stdout stays clean. Only meaningful
/// for CP endpoints — the server is what stores and resumes sessions.
fn print_resume_hint(is_cp: bool, session: Option<&str>, target_label: &str) {
    let Some(sid) = session.filter(|_| is_cp) else {
        return;
    };
    let target = if target_label.is_empty() {
        String::new()
    } else {
        format!("{target_label} ")
    };
    eprintln!(
        "\x1b[2msession: {sid} — continue with: nasiko chat {target}--session-id {sid}\x1b[0m"
    );
}

/// Send an A2A streaming request and handle the response. Returns the session
/// id this turn belongs to: the one passed in, or — on a first turn — the id
/// the server minted and echoed back as the response's `contextId`.
fn send_message(endpoint: &str, text: &str, session_id: Option<&str>) -> Result<Option<String>> {
    // Agents in this repo disagree on the streaming method name depending on
    // which `a2a-sdk` version they're pinned to: newer ones accept the
    // gRPC-style `SendStreamingMessage` (confirmed against a real deployed
    // `oss/agents/translator` build), older ones only the spec-standard
    // `message/stream`. Try the spec name first — `send_with_method` below
    // retries with the other on a JSON-RPC "method not found".
    let build_body = |method: &str, role: &str, session_id: Option<&str>| {
        let mut body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": uuid::Uuid::new_v4().to_string(),
            "method": method,
            "params": {
                "message": {
                    "messageId": uuid::Uuid::new_v4().to_string(),
                    "role": role,
                    "parts": [{"text": text}]
                }
            }
        });
        if let Some(sid) = session_id {
            body["params"]["message"]["contextId"] = serde_json::Value::String(sid.to_string());
            body["params"]["metadata"] = serde_json::json!({ "session_id": sid });
        }
        body
    };
    // A known session rides as the A2A contextId; a first turn sends none and
    // the server (agent proxy / orchestrator) mints and echoes one. The
    // metadata field additionally names the session explicitly so the server
    // loads prior turns as conversation history (it also falls back to
    // contextId, but metadata is the documented contract the web UI uses).
    let http = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(None)
            .http_status_as_error(false)
            .build(),
    );

    // Attach auth token if endpoint matches the active cluster
    let token = config::active_token().ok().flatten().filter(|_| {
        config::active_url()
            .ok()
            .map(|u| endpoint.starts_with(&u))
            .unwrap_or(false)
    });

    // Catch a locally-detectable expired token before making the request —
    // the server would only answer with an opaque 401.
    if let Some(ref t) = token
        && config::token_expired(t) == Some(true)
    {
        bail!("session expired — run: nasiko auth login");
    }

    // Generate W3C traceparent for flow tracking
    let trace_id = uuid::Uuid::new_v4().to_string().replace('-', "");
    let span_id = &trace_id[..16];
    let traceparent = format!("00-{trace_id}-{span_id}-01");

    let send_once = |body: &serde_json::Value| -> Result<ureq::http::Response<ureq::Body>> {
        let mut req = http
            .post(endpoint)
            .header("Content-Type", "application/json")
            .header("A2A-Version", "1.0")
            .header("traceparent", &traceparent);
        if let Some(ref t) = token {
            req = req.header("Authorization", &format!("Bearer {t}"));
        }
        req.send_json(body).context("failed to reach A2A endpoint")
    };

    let check_http_error = |resp: &mut ureq::http::Response<ureq::Body>| -> Result<()> {
        if resp.status().as_u16() < 400 {
            return Ok(());
        }
        let err_body = resp.body_mut().read_to_string().unwrap_or_default();
        let msg = serde_json::from_str::<serde_json::Value>(&err_body)
            .ok()
            .and_then(|v| {
                v.pointer("/error/message")
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or(err_body);
        let msg = if msg.trim().is_empty() {
            "(empty response body)".to_string()
        } else {
            msg
        };
        let hint = if resp.status().as_u16() == 401 {
            "\nhint: your session may have expired — run: nasiko auth login"
        } else {
            ""
        };
        bail!(
            "HTTP {} from {}: {}{}",
            resp.status().as_u16(),
            endpoint,
            msg,
            hint
        );
    };

    let mut spin = Spinner::new();
    spin.set("connecting");
    let mut method = "message/stream";
    let mut role = "ROLE_USER";
    let mut resp = send_once(&build_body(method, role, session_id))?;
    check_http_error(&mut resp)?;

    let mut content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // A JSON-RPC-level rejection (method not found, a role-enum casing
    // mismatch, or streaming outright unsupported) always arrives as a plain
    // JSON body — the server never upgrades to a stream for a request it
    // rejects outright — so it's safe to read it here and retry with the
    // other convention before falling into the streaming/non-streaming
    // response handling below. These SDK disagreements are independent and
    // can compound, so this loops rather than checking once.
    let mut resp_json: Option<serde_json::Value> = None;
    for _ in 0..4 {
        if content_type.contains("text/event-stream") {
            break;
        }
        let parsed: serde_json::Value = resp
            .body_mut()
            .read_json()
            .context("invalid JSON response")?;
        let error = parsed.get("error");
        let is_streaming_method = method == "message/stream" || method == "SendStreamingMessage";
        let retry_method = method == "message/stream"
            && error.and_then(|e| e.get("code")).and_then(|c| c.as_i64()) == Some(-32601);
        let retry_role = role == "ROLE_USER" && error.is_some_and(is_role_casing_error);
        let retry_streaming =
            is_streaming_method && error.is_some_and(is_streaming_unsupported_error);
        if !retry_method && !retry_role && !retry_streaming {
            resp_json = Some(parsed);
            break;
        }
        if retry_streaming {
            method = if method == "SendStreamingMessage" {
                "SendMessage"
            } else {
                "message/send"
            };
        } else if retry_method {
            method = "SendStreamingMessage";
        }
        if retry_role {
            role = "user";
        }
        resp = send_once(&build_body(method, role, session_id))?;
        check_http_error(&mut resp)?;
        content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
    }

    let observed_session = if content_type.contains("text/event-stream") {
        spin.set("thinking");
        let (_agent_text, observed) = handle_sse_stream(resp, &mut spin)?;
        observed
    } else {
        spin.set("thinking");
        let resp_json = match resp_json {
            Some(v) => v,
            None => resp
                .body_mut()
                .read_json()
                .context("invalid JSON response")?,
        };
        spin.pause();

        let result = resp_json.get("result").unwrap_or(&resp_json);
        if let Some(t) = nasiko_types::a2a::extract_text(result) {
            print!("{t}");
            std::io::stdout().flush().ok();
        } else if let Some(err) = resp_json.get("error") {
            bail!("A2A error: {}", err);
        } else {
            bail!("unexpected response: {}", resp_json);
        }
        event_context_id(result)
    };

    Ok(session_id.map(str::to_string).or(observed_session))
}

/// The session id a response event carries, across the A2A response shapes
/// (task-wrapped, status/artifact update events, bare message replies).
fn event_context_id(result: &serde_json::Value) -> Option<String> {
    [
        "/contextId",
        "/task/contextId",
        "/statusUpdate/contextId",
        "/artifactUpdate/contextId",
        "/message/contextId",
    ]
    .iter()
    .find_map(|p| result.pointer(p).and_then(|v| v.as_str()))
    .filter(|s| !s.is_empty())
    .map(str::to_string)
}

/// Parse SSE stream, render events to the terminal, and return the full agent text.
/// The spinner animates whenever the stream is quiet; every print pauses it
/// first so output never collides with an animation frame.
fn handle_sse_stream(
    resp: ureq::http::Response<ureq::Body>,
    spin: &mut Spinner,
) -> Result<(String, Option<String>)> {
    let (_parts, body) = resp.into_parts();
    let buf = std::io::BufReader::new(body.into_reader());
    let mut collected = String::new();
    let mut observed_session: Option<String> = None;

    for line in buf.lines() {
        let line = line.context("reading SSE stream")?;
        let data = match line.strip_prefix("data:") {
            Some(d) => d.trim(),
            None => continue,
        };
        if data.is_empty() {
            continue;
        }

        let event: serde_json::Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // v1.0 JSONRPC SSE: {"jsonrpc":"2.0","result":{"statusUpdate":...}} or {"result":{"artifactUpdate":...}}
        // Also supports top-level {"statusUpdate":...} for REST/proto format
        let result = event.get("result").unwrap_or(&event);

        if observed_session.is_none() {
            observed_session = event_context_id(result);
        }

        let mut is_terminal = false;

        if let Some(task) = result.get("task") {
            // A "task" event with terminal state means we're done (non-streaming response).
            // Otherwise it's the initial task submission — keep reading.
            let state = task
                .pointer("/status/state")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            if matches!(
                state,
                "TASK_STATE_COMPLETED" | "TASK_STATE_FAILED" | "TASK_STATE_CANCELED"
            ) {
                spin.pause();
                spin.close_sub();
                if let Some(t) = handle_task_result(task) {
                    spin.stdout_dirty = true;
                    collected.push_str(&t);
                }
                is_terminal = true;
            }
        } else if let Some(status_update) = result.get("statusUpdate") {
            handle_status_update(status_update, spin);
            is_terminal = is_terminal_state(status_update);
        } else if result.get("message").is_some() {
            // Bare message reply (e.g. a2a-go SDK agents): terminal, the
            // message text is the full answer.
            spin.pause();
            spin.close_sub();
            if let Some(t) = nasiko_types::a2a::extract_text(result) {
                print!("{t}");
                std::io::stdout().flush().ok();
                spin.stdout_dirty = true;
                collected.push_str(&t);
            }
            is_terminal = true;
        } else if let Some(artifact_update) = result.get("artifactUpdate") {
            // Answer text is flowing — the text itself is the progress indicator.
            spin.pause();
            spin.close_sub();
            if let Some(t) = handle_artifact_update(artifact_update) {
                spin.stdout_dirty = true;
                collected.push_str(&t);
            }
        } else if let Some(kind) = result.get("kind").and_then(|k| k.as_str()) {
            match kind {
                "artifact-update" => {
                    spin.pause();
                    spin.close_sub();
                    if let Some(t) = handle_artifact_update(result) {
                        spin.stdout_dirty = true;
                        collected.push_str(&t);
                    }
                }
                "status-update" => {
                    spin.pause();
                    handle_status_update_jsonrpc(result);
                    is_terminal = is_terminal_state(result);
                }
                _ => {}
            }
        }

        if is_terminal {
            break;
        }
    }

    spin.pause();
    spin.close_sub();
    Ok((collected, observed_session))
}

fn handle_task_result(task: &serde_json::Value) -> Option<String> {
    let text = nasiko_types::a2a::extract_text(task)?;
    print!("{text}");
    std::io::stdout().flush().ok();
    Some(text)
}

fn handle_status_update(event: &serde_json::Value, spin: &mut Spinner) {
    let state = event
        .pointer("/status/state")
        .and_then(|s| s.as_str())
        .unwrap_or("");

    match state {
        "TASK_STATE_WORKING" => {
            if let Some(parts) = event
                .pointer("/status/message/parts")
                .and_then(|p| p.as_array())
            {
                for part in parts {
                    if let Some(data) = part.get("data") {
                        render_status_data(data, spin);
                    } else if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                        spin.pause();
                        spin.close_sub();
                        spin.break_stdout();
                        eprintln!("  \x1b[2m{text}\x1b[0m");
                        spin.set("working");
                    }
                }
            }
        }
        "TASK_STATE_FAILED" => {
            spin.pause();
            spin.close_sub();
            spin.break_stdout();
            if let Some(parts) = event
                .pointer("/status/message/parts")
                .and_then(|p| p.as_array())
            {
                for part in parts {
                    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                        eprintln!("  \x1b[31merror: {text}\x1b[0m");
                    }
                }
            }
        }
        _ => {}
    }
}

fn render_status_data(data: &serde_json::Value, spin: &mut Spinner) {
    let event_type = data.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match event_type {
        "thinking" => {
            if let Some(content) = data.get("content").and_then(|c| c.as_str()) {
                if content.trim().is_empty() {
                    spin.set("thinking");
                } else {
                    spin.pause();
                    spin.close_sub();
                    spin.break_stdout();
                    eprintln!("\x1b[2m{content}\x1b[0m");
                    spin.set("thinking");
                }
            }
        }
        "tool_call" => {
            let agent = data.get("agent").and_then(|a| a.as_str()).unwrap_or("?");
            let message = data.get("message").and_then(|m| m.as_str()).unwrap_or("");
            spin.pause();
            spin.close_sub();
            spin.break_stdout();
            // The call header is the visual anchor — bold, colored, flush
            // left. Everything the agent does below it is dim and indented.
            eprintln!("\x1b[1;36m❯ {agent}\x1b[0m \x1b[2m· {message}\x1b[0m");
            spin.sub_streamed = false;
            spin.call_started = Some(std::time::Instant::now());
            spin.set(format!("{agent} working"));
        }
        "tool_result" => {
            let agent = data.get("agent").and_then(|a| a.as_str()).unwrap_or("?");
            let success = data
                .get("success")
                .and_then(|s| s.as_bool())
                .unwrap_or(false);
            let result = data.get("result").and_then(|r| r.as_str()).unwrap_or("");
            let icon = if success { "✓" } else { "✗" };
            let color = if success { "32" } else { "31" };
            spin.pause();
            spin.close_sub();
            spin.break_stdout();
            let elapsed = spin
                .call_started
                .take()
                .map(|t| format!(" \x1b[2m({:.1}s)\x1b[0m", t.elapsed().as_secs_f64()))
                .unwrap_or_default();
            if success && spin.sub_streamed {
                // Reply already streamed above — don't repeat it.
                eprintln!("\x1b[{color}m{icon} {agent}\x1b[0m{elapsed}");
            } else {
                // Results arrive JSON-encoded ("\"…\"" with escaped
                // quotes/newlines); decode and collapse to one line so the
                // preview reads as prose.
                let decoded =
                    serde_json::from_str::<String>(result).unwrap_or_else(|_| result.to_string());
                let one_line = decoded.split_whitespace().collect::<Vec<_>>().join(" ");
                let display = if one_line.len() > 200 {
                    let n = one_line.floor_char_boundary(200);
                    format!("{}…", &one_line[..n])
                } else {
                    one_line
                };
                eprintln!("\x1b[{color}m{icon} {agent}\x1b[0m{elapsed} \x1b[2m{display}\x1b[0m");
            }
            spin.sub_streamed = false;
            spin.set("thinking");
        }
        "sub_status" => {
            // A sub-agent's own progress (its internal tool calls), relayed
            // through the orchestrator's stream. The header above already
            // names the agent — no need to repeat it per line. Nesting is
            // shown with a dim "›" marker instead of leading spaces, so the
            // line stays flush-left and scans easily; the tool name itself
            // is highlighted (like the agent name on the header line) so
            // it's the first thing the eye catches.
            let agent = data.get("agent").and_then(|a| a.as_str()).unwrap_or("?");
            let message = data.get("message").and_then(|m| m.as_str()).unwrap_or("");
            spin.pause();
            spin.close_sub();
            spin.break_stdout();
            match message.split_once(": ") {
                Some((tool, rest)) => {
                    eprintln!("\x1b[2m›\x1b[0m \x1b[1;36m{tool}\x1b[0m\x1b[2m: {rest}\x1b[0m")
                }
                None => eprintln!("\x1b[2m› {message}\x1b[0m"),
            }
            spin.set(format!("{agent} working"));
        }
        "sub_content" => {
            // A sub-agent's reply streaming in — shown dim inline while it
            // generates; the result line closes with just a checkmark.
            let content = data.get("content").and_then(|c| c.as_str()).unwrap_or("");
            if content.is_empty() {
                return;
            }
            spin.pause();
            if !spin.sub_open {
                spin.break_stdout();
                eprint!("  \x1b[2m");
                spin.sub_open = true;
                spin.sub_streamed = true;
            }
            // Keep continuation lines aligned under the same indent.
            eprint!("\x1b[2m{}\x1b[0m", content.replace('\n', "\n  \x1b[2m"));
            let _ = std::io::Write::flush(&mut std::io::stderr());
        }
        _ => {}
    }
}

/// Handle JSONRPC-wrapped status-update (kind: "status-update" inside result)
fn handle_status_update_jsonrpc(result: &serde_json::Value) {
    let state = result
        .pointer("/status/state")
        .and_then(|s| s.as_str())
        .unwrap_or("");

    if state == "failed"
        && let Some(msg) = result.pointer("/status/message/parts")
        && let Some(parts) = msg.as_array()
    {
        for part in parts {
            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                eprintln!("  \x1b[31merror: {text}\x1b[0m");
            }
        }
    }
}

fn is_terminal_state(event: &serde_json::Value) -> bool {
    let state = event
        .pointer("/status/state")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    matches!(
        state,
        "TASK_STATE_COMPLETED"
            | "TASK_STATE_FAILED"
            | "TASK_STATE_CANCELED"
            | "completed"
            | "failed"
            | "canceled"
    )
}

fn handle_artifact_update(event: &serde_json::Value) -> Option<String> {
    // Support both: {"artifact": {"parts": [...]}} and {"parts": [...]} directly
    let parts = event
        .pointer("/artifact/parts")
        .or_else(|| event.get("parts"))
        .and_then(|p| p.as_array())?;

    let mut buf = String::new();
    for part in parts {
        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
            print!("{text}");
            std::io::stdout().flush().ok();
            buf.push_str(text);
        }
    }
    if buf.is_empty() { None } else { Some(buf) }
}

/// Detects the `a2a-sdk` (Python) role-enum casing rejection: some pins only
/// accept the spec-standard lowercase `"user"`/`"agent"` and reject the
/// gRPC-style `"ROLE_USER"` this CLI sends by default, as a pydantic
/// validation error on `params.message.role` — distinct from a JSON-RPC
/// "method not found" (-32601), so it needs its own detection rather than
/// piggybacking on that check.
fn is_role_casing_error(error: &serde_json::Value) -> bool {
    error.get("code").and_then(|c| c.as_i64()) == Some(-32602)
        && error
            .get("data")
            .and_then(|d| d.as_array())
            .is_some_and(|items| {
                items.iter().any(|item| {
                    item.get("loc")
                        .and_then(|l| l.as_array())
                        .is_some_and(|loc| loc.iter().any(|v| v.as_str() == Some("role")))
                })
            })
}

/// Detects an agent rejecting the streaming call outright (some SDKs report
/// `capabilities.streaming: false` on their card and reject `message/stream`/
/// `SendStreamingMessage` with a generic internal error rather than just
/// answering as if `message/send` had been called) — the message text is the
/// only signal here, there's no dedicated JSON-RPC code for this.
fn is_streaming_unsupported_error(error: &serde_json::Value) -> bool {
    error
        .get("message")
        .and_then(|m| m.as_str())
        .map(|m| m.to_ascii_lowercase())
        .is_some_and(|m| {
            m.contains("stream") && (m.contains("not support") || m.contains("unsupported"))
        })
}

/// Extracts the path (and beyond) from an absolute URL, e.g.
/// `http://localhost:8000/a2a` -> `/a2a`. Falls back to `/` when the input
/// isn't an absolute URL with a path — agent cards vary on whether they set
/// this at all, and some report an unreachable host (`0.0.0.0`), so callers
/// combine this path with their own known-good host rather than trusting the
/// card's host outright.
fn path_from_url(url: &str) -> String {
    let after_scheme = url.find("://").map(|i| &url[i + 3..]).unwrap_or(url);
    match after_scheme.find('/') {
        Some(i) if !after_scheme[i..].is_empty() => after_scheme[i..].to_string(),
        _ => "/".to_string(),
    }
}

/// Fetches an agent's card, trying the current spec path before the legacy
/// one (footgun: `A2A_PROTOCOL.md` documents both as valid, and different
/// SDKs in this repo only serve one or the other). Returns the display name
/// and the JSON-RPC path to actually call, read from the card's own `url`
/// (or `supportedInterfaces[0].url`) — this is what varies by SDK (`/` vs
/// `/a2a`) instead of assuming every agent answers at the root path.
fn fetch_agent_card(base: &str) -> (String, String) {
    let card = ureq::get(&format!("{base}/.well-known/agent-card.json"))
        .call()
        .ok()
        .or_else(|| {
            ureq::get(&format!("{base}/.well-known/agent.json"))
                .call()
                .ok()
        })
        .and_then(|mut r| r.body_mut().read_json::<serde_json::Value>().ok());

    let name = card
        .as_ref()
        .and_then(|c| c.get("name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "Agent".to_string());

    let rpc_path = card
        .as_ref()
        .and_then(|c| {
            c.get("url").and_then(|u| u.as_str()).or_else(|| {
                c.get("supportedInterfaces")
                    .and_then(|i| i.as_array())
                    .and_then(|i| i.first())
                    .and_then(|i| i.get("url"))
                    .and_then(|u| u.as_str())
            })
        })
        .map(path_from_url)
        .unwrap_or_else(|| "/".to_string());

    (name, rpc_path)
}

/// Chat directly with a locally running agent via A2A JSON-RPC (used by `nasiko agents chat`).
pub fn agent_chat(url: &str, message: Option<&str>, session_id: Option<&str>) -> Result<()> {
    let base = url.trim_end_matches('/');
    let (agent_name, rpc_path) = fetch_agent_card(base);
    let rpc_url = format!("{base}{rpc_path}");

    println!("Chatting with '{}' at {}", agent_name, base);
    if let Some(sid) = session_id {
        println!("Session: {}", sid);
    }
    println!("Type 'exit' to quit.\n");

    let send_msg = |msg: &str, ctx_id: Option<String>| -> Result<Option<String>> {
        let msg_id = format!(
            "{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let build_payload = |method: &str, role: &str| {
            let mut payload = serde_json::json!({
                "jsonrpc": "2.0", "method": method, "id": &msg_id,
                "params": { "message": {
                    "role": role, "parts": [{ "text": msg }],
                    "messageId": &msg_id,
                }}
            });
            if let Some(ref cid) = ctx_id {
                payload["params"]["message"]["contextId"] = serde_json::Value::String(cid.clone());
            }
            payload
        };
        let send_once = |method: &str, role: &str| -> Result<serde_json::Value> {
            ureq::post(&rpc_url)
                .header("Content-Type", "application/json")
                .header("A2A-Version", "1.0")
                .send_json(build_payload(method, role))
                .map_err(|e| anyhow::anyhow!("failed to reach agent: {}", e))?
                .body_mut()
                .read_json()
                .map_err(Into::into)
        };

        let spin = status::start_status(format!("{agent_name} is thinking"));
        // SDKs in this repo disagree on both the RPC method name (spec-current
        // `message/send` vs the Go `a2a-go` SDK's `SendMessage`) and the role
        // enum casing (gRPC-style `ROLE_USER` vs the spec-standard lowercase
        // `user` some `a2a-sdk` pins require). Try the spec conventions first
        // and retry with the alternate on the matching JSON-RPC error —
        // cheaper than guessing from the card, and self-correcting if either
        // SDK's convention changes. The two mismatches are independent, so
        // this loops rather than checking once.
        let mut method = "message/send";
        let mut role = "ROLE_USER";
        let mut raw = send_once(method, role)?;
        for _ in 0..2 {
            let error = raw.get("error");
            let retry_method = method == "message/send"
                && error.and_then(|e| e.get("code")).and_then(|c| c.as_i64()) == Some(-32601);
            let retry_role = role == "ROLE_USER" && error.is_some_and(is_role_casing_error);
            if !retry_method && !retry_role {
                break;
            }
            if retry_method {
                method = "SendMessage";
            }
            if retry_role {
                role = "user";
            }
            raw = send_once(method, role)?;
        }
        drop(spin);
        if let Some(err) = raw.get("error") {
            bail!("A2A error: {}", err);
        }
        let result = raw.get("result").cloned().unwrap_or_default();
        // Shared with `nasiko chat`'s `send_message`: handles both the flat
        // 0.3.x shape and the 1.0 task-wrapped shape (`result.task.artifacts[]`)
        // — a hand-rolled chain here previously only checked the flat shape
        // directly under `result` and silently printed "(no response)" for
        // any agent (e.g. the Rust `a2a-server-lf` sample) that wraps its
        // reply in a task.
        let new_ctx = event_context_id(&result);
        let text =
            nasiko_types::a2a::extract_text(&result).unwrap_or_else(|| "(no response)".to_string());
        println!("Agent: {}\n", text);
        Ok(new_ctx.or(ctx_id))
    };

    let initial_ctx = session_id.map(|s| s.to_string());
    if let Some(msg) = message {
        send_msg(msg, initial_ctx)?;
        return Ok(());
    }

    let mut ctx_id: Option<String> = initial_ctx;
    // Ctrl-C / Ctrl-D breaks the loop and leaves gracefully instead of erroring out.
    while let Ok(input) = dialoguer::Input::<String>::new()
        .with_prompt("\x1b[1;36m❯ you\x1b[0m")
        .allow_empty(true)
        .interact_text()
    {
        let input = input.trim();
        if input.is_empty() {
            continue;
        }
        if input == "exit" || input == "quit" {
            println!("Goodbye.");
            break;
        }
        ctx_id = send_msg(input, ctx_id)?;
    }
    Ok(())
}
