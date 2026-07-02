use std::io::{BufRead, Write as _};

use anyhow::{Context, Result, bail};

use crate::config;

/// Chat with an A2A agent (one-shot or interactive).
///
/// The URL determines the target:
/// - Direct agent: http://localhost:10010/
/// - CP orchestrator: http://localhost:8080/api/a2a
/// - CP agent proxy: http://localhost:8080/api/agents/{id}/a2a
pub fn chat(url: &str, message: Option<&str>, session_id: Option<&str>) -> Result<()> {
    let endpoint = url.trim_end_matches('/');

    match message {
        Some(msg) => {
            send_message(endpoint, msg, session_id)?;
            println!();
        }
        None => {
            println!("Chat with {endpoint} (type /quit to exit)\n");
            loop {
                let input: String = dialoguer::Input::new()
                    .with_prompt("you")
                    .interact_text()?;
                if input.trim().is_empty() {
                    continue;
                }
                if input.trim() == "/quit" || input.trim() == "/exit" {
                    break;
                }
                println!();
                match send_message(endpoint, &input, session_id) {
                    Ok(_) => println!("\n"),
                    Err(e) => eprintln!("  error: {e}\n"),
                }
            }
        }
    }
    Ok(())
}

/// Send an A2A message/stream request and handle the response.
fn send_message(endpoint: &str, text: &str, session_id: Option<&str>) -> Result<()> {
    let context_id = session_id
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": uuid::Uuid::new_v4().to_string(),
        "method": "SendStreamingMessage",
        "params": {
            "message": {
                "messageId": uuid::Uuid::new_v4().to_string(),
                "role": "ROLE_USER",
                "parts": [{"text": text}],
                "contextId": context_id
            }
        }
    });

    let http = ureq::Agent::new_with_config(
        ureq::config::Config::builder().timeout_global(None).build(),
    );

    // Attach auth token if endpoint matches the active cluster
    let token = config::active_token().ok().flatten().filter(|_| {
        config::active_url()
            .ok()
            .map(|u| endpoint.starts_with(&u))
            .unwrap_or(false)
    });

    // Generate W3C traceparent for flow tracking
    let trace_id = uuid::Uuid::new_v4().to_string().replace('-', "");
    let span_id = &trace_id[..16];
    let traceparent = format!("00-{trace_id}-{span_id}-01");

    let mut req = http
        .post(endpoint)
        .header("Content-Type", "application/json")
        .header("A2A-Version", "1.0")
        .header("traceparent", &traceparent);
    if let Some(ref t) = token {
        req = req.header("Authorization", &format!("Bearer {t}"));
    }

    let resp = req.send_json(&body).context("failed to reach A2A endpoint")?;

    if resp.status().as_u16() >= 400 {
        let mut resp = resp;
        let err_body = resp.body_mut().read_to_string().unwrap_or_default();
        bail!("HTTP {}: {}", resp.status().as_u16(), err_body);
    }

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    if content_type.contains("text/event-stream") {
        handle_sse_stream(resp)?;
    } else {
        let mut resp = resp;
        let resp_json: serde_json::Value =
            resp.body_mut().read_json().context("invalid JSON response")?;

        let result = resp_json.get("result").unwrap_or(&resp_json);
        if let Some(text) = nasiko_types::a2a::extract_text(result) {
            print!("{text}");
            std::io::stdout().flush().ok();
        } else if let Some(err) = resp_json.get("error") {
            bail!("A2A error: {}", err);
        } else {
            bail!("unexpected response: {}", resp_json);
        }
    }

    Ok(())
}

/// Parse SSE stream and render events to the terminal.
fn handle_sse_stream(resp: ureq::http::Response<ureq::Body>) -> Result<()> {
    let (_parts, body) = resp.into_parts();
    let buf = std::io::BufReader::new(body.into_reader());

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

        let mut is_terminal = false;

        if let Some(task) = result.get("task") {
            // A "task" event with terminal state means we're done (non-streaming response).
            // Otherwise it's the initial task submission — keep reading.
            let state = task.pointer("/status/state").and_then(|s| s.as_str()).unwrap_or("");
            if matches!(state, "TASK_STATE_COMPLETED" | "TASK_STATE_FAILED" | "TASK_STATE_CANCELED") {
                handle_task_result(task);
                is_terminal = true;
            }
        } else if let Some(status_update) = result.get("statusUpdate") {
            handle_status_update(status_update);
            is_terminal = is_terminal_state(status_update);
        } else if let Some(artifact_update) = result.get("artifactUpdate") {
            handle_artifact_update(artifact_update);
        } else if let Some(kind) = result.get("kind").and_then(|k| k.as_str()) {
            match kind {
                "artifact-update" => handle_artifact_update(result),
                "status-update" => {
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

    Ok(())
}

fn handle_task_result(task: &serde_json::Value) {
    if let Some(text) = nasiko_types::a2a::extract_text(task) {
        print!("{text}");
        std::io::stdout().flush().ok();
    }
}

fn handle_status_update(event: &serde_json::Value) {
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
                        render_status_data(data);
                    } else if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                        eprintln!("  \x1b[2m{text}\x1b[0m");
                    }
                }
            }
        }
        "TASK_STATE_FAILED" => {
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

fn render_status_data(data: &serde_json::Value) {
    let event_type = data.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match event_type {
        "thinking" => {
            if let Some(content) = data.get("content").and_then(|c| c.as_str()) {
                eprintln!("  \x1b[2m{content}\x1b[0m");
            }
        }
        "tool_call" => {
            let agent = data.get("agent").and_then(|a| a.as_str()).unwrap_or("?");
            let message = data.get("message").and_then(|m| m.as_str()).unwrap_or("");
            eprintln!("  \x1b[36m-> {agent}\x1b[0m: {message}");
        }
        "tool_result" => {
            let agent = data.get("agent").and_then(|a| a.as_str()).unwrap_or("?");
            let success = data.get("success").and_then(|s| s.as_bool()).unwrap_or(false);
            let result = data.get("result").and_then(|r| r.as_str()).unwrap_or("");
            let icon = if success { "ok" } else { "err" };
            let color = if success { "32" } else { "31" };
            let display = if result.len() > 200 {
                let n = result.floor_char_boundary(200);
                format!("{}...", &result[..n])
            } else {
                result.to_string()
            };
            eprintln!("  \x1b[{color}m[{icon}] {agent}\x1b[0m: {display}");
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
            && let Some(parts) = msg.as_array() {
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
    matches!(state, "TASK_STATE_COMPLETED" | "TASK_STATE_FAILED" | "TASK_STATE_CANCELED"
        | "completed" | "failed" | "canceled")
}

fn handle_artifact_update(event: &serde_json::Value) {
    // Support both: {"artifact": {"parts": [...]}} and {"parts": [...]} directly
    let parts = event
        .pointer("/artifact/parts")
        .or_else(|| event.get("parts"))
        .and_then(|p| p.as_array());

    if let Some(parts) = parts {
        for part in parts {
            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                print!("{text}");
                std::io::stdout().flush().ok();
            }
        }
    }
}

/// Chat directly with a locally running agent via A2A JSON-RPC (used by `nasiko agents chat`).
pub fn agent_chat(url: &str, message: Option<&str>, session_id: Option<&str>) -> Result<()> {
    let base = url.trim_end_matches('/');

    let agent_name = ureq::get(&format!("{}/.well-known/agent.json", base))
        .call().ok()
        .and_then(|mut r| r.body_mut().read_json::<serde_json::Value>().ok())
        .and_then(|card| card.get("name").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .unwrap_or_else(|| "Agent".to_string());

    println!("Chatting with '{}' at {}", agent_name, base);
    if let Some(sid) = session_id { println!("Session: {}", sid); }
    println!("Type 'exit' to quit.\n");

    let send_msg = |msg: &str, ctx_id: Option<String>| -> Result<Option<String>> {
        let msg_id = format!("{:x}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
        let mut payload = serde_json::json!({
            "jsonrpc": "2.0", "method": "SendMessage", "id": &msg_id,
            "params": { "message": {
                "role": "ROLE_USER", "parts": [{ "text": msg }],
                "messageId": &msg_id,
            }}
        });
        if let Some(ref cid) = ctx_id {
            payload["params"]["message"]["contextId"] = serde_json::Value::String(cid.clone());
        }
        let mut resp = ureq::post(&format!("{}/", base))
            .header("Content-Type", "application/json")
            .header("A2A-Version", "1.0")
            .send_json(&payload)
            .map_err(|e| anyhow::anyhow!("failed to reach agent: {}", e))?;
        let raw: serde_json::Value = resp.body_mut().read_json()?;
        let result = raw.get("result").cloned().unwrap_or_default();
        let new_ctx = result.get("contextId").and_then(|v| v.as_str()).map(|s| s.to_string());
        let text = result.get("artifacts").and_then(|a| a.as_array()).and_then(|a| a.first())
            .and_then(|a| a.get("parts")).and_then(|p| p.as_array())
            .and_then(|p| p.iter().find(|x| x.get("kind").and_then(|k| k.as_str()) == Some("text")))
            .and_then(|p| p.get("text")).and_then(|t| t.as_str())
            .or_else(|| result.get("status").and_then(|s| s.get("message"))
                .and_then(|m| m.get("parts")).and_then(|p| p.as_array())
                .and_then(|p| p.first()).and_then(|p| p.get("text")).and_then(|t| t.as_str()))
            .unwrap_or("(no response)");
        println!("Agent: {}\n", text);
        Ok(new_ctx.or(ctx_id))
    };

    let initial_ctx = session_id.map(|s| s.to_string());
    if let Some(msg) = message {
        send_msg(msg, initial_ctx)?;
        return Ok(());
    }

    let mut ctx_id: Option<String> = initial_ctx;
    loop {
        let input: String = dialoguer::Input::new().with_prompt("You").allow_empty(true).interact_text()?;
        let input = input.trim();
        if input.is_empty() { continue; }
        if input == "exit" || input == "quit" { println!("Goodbye."); break; }
        ctx_id = send_msg(input, ctx_id)?;
    }
    Ok(())
}
