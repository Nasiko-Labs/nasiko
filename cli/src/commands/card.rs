use std::fs;
use std::io::IsTerminal;
use std::path::Path;

use anyhow::Result;
use dialoguer::{Confirm, Input};

use crate::util;

pub fn card(directory: &str, description: Option<&str>) -> Result<()> {
    // `nasiko card <path>` is a natural mistake (the positional arg is the
    // description; the directory is --dir). If the "description" is a path to
    // an existing directory and --dir was left at its default, the user meant
    // the directory.
    let (directory, description) = match description {
        Some(d) if directory == "." && Path::new(d).is_dir() => (d, None),
        other => (directory, other),
    };
    let root = Path::new(directory)
        .canonicalize()
        .unwrap_or_else(|_| Path::new(directory).to_path_buf());
    let card_path = root.join("AgentCard.json");

    println!("Agent Card Generator\n");

    // Try LLM generation via CP; fall back to static generation, but tell the
    // user WHY the CP path was skipped — "CP not available" hiding an auth or
    // server error makes the real problem undiagnosable.
    match try_generate_via_cp(&root, &card_path, description) {
        Ok(()) => Ok(()),
        Err(reason) => {
            println!("LLM generation via CP skipped: {reason:#}");
            println!("Falling back to static generation.\n");
            generate_static(&root, &card_path, description)
        }
    }
}

fn try_generate_via_cp(root: &Path, card_path: &Path, description: Option<&str>) -> Result<()> {
    let client = crate::api::Client::from_active_cluster()
        .map_err(|e| anyhow::anyhow!("no active cluster ({e}) — run `nasiko connect`"))?;

    // The server's /capabilities/generate requires source_code unconditionally
    // (there's nothing to generate a card *from* without code) — bailing here
    // whenever there's no source, not just when there's also no description,
    // avoids a request the CLI already knows is doomed to fail.
    let source = collect_source(root);
    if source.is_none() {
        anyhow::bail!(
            "no source files found in '{}' — LLM generation requires source code",
            root.display()
        );
    }

    let agent_name = root
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let mut body = serde_json::json!({
        "agent_name": agent_name,
    });
    if let Some(src) = &source {
        body["source_code"] = serde_json::Value::String(src.clone());
    }
    if let Some(desc) = description {
        body["description"] = serde_json::Value::String(desc.to_string());
    }

    let resp: serde_json::Value = client
        .post_json("/capabilities/generate", &body)
        .map_err(|e| anyhow::anyhow!("CP request failed: {e}"))?;
    let generated = resp
        .get("card")
        .ok_or_else(|| anyhow::anyhow!("CP response has no 'card' field"))?;
    let card = complete_card(generated, &agent_name, &load_existing_card(card_path));
    let json = serde_json::to_string_pretty(&card)?;
    fs::write(card_path, &json)
        .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", card_path.display()))?;
    let tokens = resp
        .get("tokens_used")
        .and_then(|t| t.as_i64())
        .unwrap_or(0);
    println!("✓ Wrote AgentCard.json (LLM, {} tokens)", tokens);
    Ok(())
}

/// The CP's capability generator returns only what it can infer from source
/// (description, skills, capabilities, modes, framework...). A publishable
/// A2A card also needs identity fields — merge in `name`, `version`,
/// `protocolVersion`, `url`, etc., preferring values from a pre-existing
/// card so regeneration never loses them.
fn complete_card(
    generated: &serde_json::Value,
    agent_name: &str,
    existing: &Option<serde_json::Value>,
) -> serde_json::Value {
    let mut card = generated.clone();
    let Some(map) = card.as_object_mut() else {
        return card;
    };

    let keep = |field: &str, default: serde_json::Value| {
        existing
            .as_ref()
            .and_then(|e| e.get(field).cloned())
            .unwrap_or(default)
    };

    let mut identity = serde_json::Map::new();
    identity.insert(
        "protocolVersion".into(),
        keep("protocolVersion", "0.2.9".into()),
    );
    identity.insert("name".into(), keep("name", agent_name.into()));
    identity.insert("version".into(), keep("version", "0.1.0".into()));
    identity.insert("url".into(), keep("url", "http://localhost:8000/".into()));
    identity.insert(
        "preferredTransport".into(),
        keep("preferredTransport", "JSONRPC".into()),
    );
    // The generator emits lowercase `framework` (fastapi, axum...); the card
    // field is `agentFramework`. Prefer the existing card's value.
    let framework = map
        .get("framework")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let agent_framework = keep("agentFramework", framework);
    if !agent_framework.is_null() {
        identity.insert("agentFramework".into(), agent_framework);
    }

    for (key, value) in identity {
        map.entry(key).or_insert(value);
    }
    card
}

/// Cap on total collected source sent to the CP (keeps the request and the
/// LLM prompt bounded for large repos).
const MAX_TOTAL_SOURCE_BYTES: usize = 256_000;

/// Collect agent source for capability generation — any language. File
/// selection rules are shared with the server's archive reader
/// (nasiko_utils::source_files), so local and uploaded generation agree.
fn collect_source(root: &Path) -> Option<String> {
    let mut source = String::new();
    collect_dir_sources(root, root, &mut source);
    if source.is_empty() {
        None
    } else {
        Some(source)
    }
}

fn collect_dir_sources(root: &Path, dir: &Path, out: &mut String) {
    use nasiko_utils::source_files::{MAX_SOURCE_FILE_BYTES, SKIP_DIRS, is_agent_source_file};

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_TOTAL_SOURCE_BYTES {
            return;
        }
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if path.is_dir() {
            if !SKIP_DIRS.contains(&name.as_ref()) && !name.starts_with('.') {
                collect_dir_sources(root, &path, out);
            }
            continue;
        }
        if !is_agent_source_file(&name) {
            continue;
        }
        let too_large = entry
            .metadata()
            .map(|m| m.len() > MAX_SOURCE_FILE_BYTES)
            .unwrap_or(true);
        if too_large {
            continue;
        }
        if let Ok(content) = fs::read_to_string(&path) {
            let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy();
            out.push_str(&format!("// --- {rel} ---\n"));
            out.push_str(&content);
            out.push('\n');
        }
    }
}

/// Ask for a string when stdin is a terminal; otherwise take the default
/// silently (echoed for the record). `nasiko card` must work in scripts and
/// CI, where dialoguer's prompts error out with "not a terminal".
fn prompt_str(interactive: bool, prompt: &str, default: String) -> Result<String> {
    if !interactive {
        println!("  {prompt}: {default}");
        return Ok(default);
    }
    Ok(Input::new()
        .with_prompt(prompt)
        .default(default)
        .interact_text()?)
}

fn prompt_bool(interactive: bool, prompt: &str, default: bool) -> Result<bool> {
    if !interactive {
        println!("  {prompt}: {default}");
        return Ok(default);
    }
    Ok(Confirm::new()
        .with_prompt(prompt)
        .default(default)
        .interact()?)
}

fn generate_static(root: &Path, card_path: &Path, user_description: Option<&str>) -> Result<()> {
    let existing = load_existing_card(card_path);
    let interactive = std::io::stdin().is_terminal();
    if !interactive {
        println!("(non-interactive — using defaults; edit AgentCard.json to adjust)");
    }

    let dir_name = root
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let name = prompt_str(
        interactive,
        "Agent name",
        existing_str(&existing, "name").unwrap_or(dir_name),
    )?;

    let desc_default = user_description
        .map(String::from)
        .or_else(|| existing_str(&existing, "description"))
        .unwrap_or_default();

    let description = prompt_str(interactive, "Description", desc_default)?;

    let version = prompt_str(
        interactive,
        "Version",
        existing_str(&existing, "version").unwrap_or_else(|| "0.1.0".into()),
    )?;

    let url = prompt_str(
        interactive,
        "Agent URL",
        existing_str(&existing, "url").unwrap_or_else(|| "http://localhost:8000/".into()),
    )?;

    let streaming = prompt_bool(
        interactive,
        "Supports streaming?",
        existing_bool(&existing, &["capabilities", "streaming"]),
    )?;

    let framework = detect_framework(root)
        .or_else(|| existing_str(&existing, "agentFramework"))
        .unwrap_or_default();
    if !framework.is_empty() {
        println!("  Detected framework: {framework}");
    }

    let skills = detect_skills(root);
    if skills.is_empty() {
        println!("  No skills auto-detected — add them manually to AgentCard.json");
    } else {
        println!("  Detected {} skill(s)", skills.len());
    }

    let card = serde_json::json!({
        "protocolVersion": "0.2.9",
        "name": name,
        "description": description,
        "url": if url.ends_with('/') { url.clone() } else { format!("{url}/") },
        "agentFramework": framework,
        "preferredTransport": "JSONRPC",
        "provider": {
            "organization": "Nasiko",
            "url": "https://nasiko.com"
        },
        "version": version,
        "capabilities": {
            "streaming": streaming,
            "pushNotifications": false,
            "stateTransitionHistory": false
        },
        "securitySchemes": {},
        "security": [],
        "defaultInputModes": ["text/plain"],
        "defaultOutputModes": ["text/plain"],
        "skills": skills
    });

    fs::write(card_path, serde_json::to_string_pretty(&card)?)?;
    println!("\n✓ Wrote AgentCard.json");
    println!("  Connect to a cluster for LLM-powered generation.");

    Ok(())
}

fn detect_framework(root: &Path) -> Option<String> {
    let agent_py = root.join("src/agent.py");
    let source = if agent_py.exists() {
        fs::read_to_string(&agent_py).ok()?
    } else if root.join("go.mod").exists() {
        return Some("a2a-go".into());
    } else if root.join("Cargo.toml").exists() {
        return Some("a2a-rs".into());
    } else {
        return None;
    };

    let lower = source.to_lowercase();
    if lower.contains("crewai") {
        return Some("crewai".into());
    }
    if lower.contains("langgraph") {
        return Some("langgraph".into());
    }
    if lower.contains("langchain") {
        return Some("langchain".into());
    }
    if lower.contains("autogen") {
        return Some("autogen".into());
    }
    if lower.contains("anthropic") {
        return Some("claude-sdk".into());
    }
    if lower.contains("google.adk") || lower.contains("google-adk") {
        return Some("google-adk".into());
    }
    if lower.contains("google.generativeai") || lower.contains("genai") {
        return Some("gemini".into());
    }
    if lower.contains("openai") {
        return Some("openai".into());
    }
    None
}

fn detect_skills(root: &Path) -> Vec<serde_json::Value> {
    let agent_py = root.join("src/agent.py");
    let source = match fs::read_to_string(&agent_py) {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    let mut skills = Vec::new();

    // Look for @tool or @function_tool decorated functions
    let mut prev_is_decorator = false;
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("@tool") || trimmed.starts_with("@function_tool") {
            prev_is_decorator = true;
            continue;
        }
        if prev_is_decorator && trimmed.starts_with("def ") {
            if let Some(name) = trimmed
                .strip_prefix("def ")
                .and_then(|s| s.split('(').next())
            {
                let name = name.trim();
                skills.push(serde_json::json!({
                    "id": name,
                    "name": util::title_case(&name.replace('_', " ")),
                    "description": "",
                    "tags": [],
                    "examples": [],
                    "inputModes": ["text/plain"],
                    "outputModes": ["text/plain"]
                }));
            }
            prev_is_decorator = false;
        } else {
            prev_is_decorator = false;
        }
    }

    skills
}

fn load_existing_card(path: &Path) -> Option<serde_json::Value> {
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn existing_str(card: &Option<serde_json::Value>, field: &str) -> Option<String> {
    card.as_ref()?.get(field)?.as_str().map(String::from)
}

fn existing_bool(card: &Option<serde_json::Value>, path: &[&str]) -> bool {
    let mut val = card.as_ref().cloned();
    for &key in path {
        val = val.and_then(|v| v.get(key).cloned());
    }
    val.and_then(|v| v.as_bool()).unwrap_or(false)
}
