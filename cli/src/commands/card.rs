use std::fs;
use std::path::Path;

use anyhow::Result;
use dialoguer::{Confirm, Input};

use crate::util;

pub fn card(directory: &str, description: Option<&str>) -> Result<()> {
    let root = Path::new(directory)
        .canonicalize()
        .unwrap_or_else(|_| Path::new(directory).to_path_buf());
    let card_path = root.join("AgentCard.json");

    println!("Agent Card Generator\n");

    // Try LLM generation via CP
    if try_generate_via_cp(&root, &card_path, description) {
        return Ok(());
    }

    // Fallback: static generation
    println!("CP not available — using static generation.\n");
    generate_static(&root, &card_path, description)
}

fn try_generate_via_cp(root: &Path, card_path: &Path, description: Option<&str>) -> bool {
    let client = match crate::api::Client::from_active_cluster() {
        Ok(c) => c,
        Err(_) => return false,
    };

    let source = collect_source(root);
    if source.is_none() && description.is_none() {
        return false;
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

    let resp: Result<serde_json::Value, _> = client.post_json("/capabilities/generate", &body);
    match resp {
        Ok(resp) => {
            if let Some(generated) = resp.get("card") {
                let existing = load_existing_card(card_path);
                let card = merge_generated_card(existing, generated, root);
                let json = serde_json::to_string_pretty(&card).unwrap_or_default();
                if fs::write(card_path, &json).is_ok() {
                    let tokens = resp
                        .get("tokens_used")
                        .and_then(|t| t.as_i64())
                        .unwrap_or(0);
                    println!("✓ Wrote AgentCard.json (LLM, {} tokens)", tokens);
                    return true;
                }
            }
            false
        }
        Err(_) => false,
    }
}

/// Overlay the LLM-generated fields (description, skills, tags, capabilities,
/// I/O modes, framework) onto the existing AgentCard.json — or a fresh
/// skeleton if there isn't one — instead of replacing the file wholesale.
/// `GeneratedCard` (server-side) only carries the fields an LLM can infer from
/// source; it has no opinion on `name`/`version`/`protocolVersion`/`url`/
/// `preferredTransport`, so those must survive from what's already on disk.
fn merge_generated_card(
    existing: Option<serde_json::Value>,
    generated: &serde_json::Value,
    root: &Path,
) -> serde_json::Value {
    let mut card = existing.unwrap_or_else(|| default_card_skeleton(root));
    let obj = card
        .as_object_mut()
        .expect("AgentCard.json must be a JSON object");

    for (generated_key, card_key) in [
        ("description", "description"),
        ("skills", "skills"),
        ("tags", "tags"),
        ("capabilities", "capabilities"),
        ("default_input_modes", "defaultInputModes"),
        ("default_output_modes", "defaultOutputModes"),
    ] {
        if let Some(value) = generated.get(generated_key) {
            obj.insert(card_key.to_string(), value.clone());
        }
    }
    if let Some(framework) = generated.get("framework").and_then(|v| v.as_str()) {
        obj.insert("agentFramework".to_string(), serde_json::json!(framework));
    }

    card
}

fn default_card_skeleton(root: &Path) -> serde_json::Value {
    let dir_name = root
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    serde_json::json!({
        "protocolVersion": "0.2.9",
        "name": dir_name,
        "description": "",
        "url": "http://localhost:8000/",
        "preferredTransport": "JSONRPC",
        "provider": {
            "organization": "Nasiko",
            "url": "https://nasiko.com"
        },
        "version": "0.1.0",
        "capabilities": {
            "streaming": false,
            "pushNotifications": false,
            "stateTransitionHistory": false
        },
        "securitySchemes": {},
        "security": [],
        "defaultInputModes": ["text/plain"],
        "defaultOutputModes": ["text/plain"],
        "skills": []
    })
}

fn collect_source(root: &Path) -> Option<String> {
    let mut source = String::new();

    // Collect Python sources
    let src_dir = root.join("src");
    if src_dir.is_dir() {
        collect_dir_sources(&src_dir, &mut source);
    }

    // Collect Go sources
    let cmd_dir = root.join("cmd");
    if cmd_dir.is_dir() {
        collect_dir_sources(&cmd_dir, &mut source);
    }

    // Main file in root
    for name in &["main.go", "main.py", "agent.py"] {
        if let Ok(content) = fs::read_to_string(root.join(name)) {
            source.push_str(&format!("// --- {name} ---\n"));
            source.push_str(&content);
            source.push('\n');
        }
    }

    if source.is_empty() {
        None
    } else {
        Some(source)
    }
}

fn collect_dir_sources(dir: &Path, out: &mut String) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            let ext = path.extension().unwrap_or_default().to_string_lossy();
            if matches!(ext.as_ref(), "py" | "go") {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if let Ok(content) = fs::read_to_string(&path) {
                    out.push_str(&format!("// --- {name} ---\n"));
                    out.push_str(&content);
                    out.push('\n');
                }
            }
        }
    }
}

fn generate_static(root: &Path, card_path: &Path, user_description: Option<&str>) -> Result<()> {
    let existing = load_existing_card(card_path);

    let dir_name = root
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let name: String = Input::new()
        .with_prompt("Agent name")
        .default(existing_str(&existing, "name").unwrap_or(dir_name))
        .interact_text()?;

    let desc_default = user_description
        .map(String::from)
        .or_else(|| existing_str(&existing, "description"))
        .unwrap_or_default();

    let description: String = Input::new()
        .with_prompt("Description")
        .default(desc_default)
        .interact_text()?;

    let version: String = Input::new()
        .with_prompt("Version")
        .default(existing_str(&existing, "version").unwrap_or_else(|| "0.1.0".into()))
        .interact_text()?;

    let url: String = Input::new()
        .with_prompt("Agent URL")
        .default(existing_str(&existing, "url").unwrap_or_else(|| "http://localhost:8000/".into()))
        .interact_text()?;

    let streaming = Confirm::new()
        .with_prompt("Supports streaming?")
        .default(existing_bool(&existing, &["capabilities", "streaming"]))
        .interact()?;

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
