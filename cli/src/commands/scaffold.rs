use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use dialoguer::{Confirm, Input, Select};
use include_dir::{Dir, include_dir};

use crate::util;

static AGENTS_DIR: Dir = include_dir!("$OUT_DIR/agents");

// ─── Framework definitions ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Framework {
    key: &'static str,
    label: &'static str,
    model_providers: Option<&'static [&'static str]>,
    streaming: bool,
}

// TODO: add JS/TS and Rust framework templates + skill injection for those languages
const FRAMEWORKS: &[Framework] = &[
    // Python
    Framework { key: "claude-sdk", label: "Anthropic Claude API (claude-sonnet-4-6) [Python]", model_providers: None, streaming: true },
    Framework { key: "openai", label: "OpenAI Agents SDK [Python]", model_providers: None, streaming: false },
    Framework { key: "gemini", label: "Google Gemini SDK [Python]", model_providers: None, streaming: false },
    Framework { key: "google-adk", label: "Google Agent Development Kit (ADK) [Python]", model_providers: None, streaming: false },
    Framework { key: "langchain", label: "LangChain tool-calling agent [Python]", model_providers: Some(&["openai", "anthropic", "gemini"]), streaming: false },
    Framework { key: "langgraph", label: "LangGraph state-machine agent [Python]", model_providers: Some(&["openai", "anthropic", "gemini"]), streaming: true },
    Framework { key: "crewai", label: "CrewAI role-based crew [Python]", model_providers: Some(&["openai", "anthropic"]), streaming: false },
    Framework { key: "autogen", label: "Microsoft AutoGen conversational agents [Python]", model_providers: Some(&["openai", "anthropic"]), streaming: false },
    // Go
    Framework { key: "a2a-go", label: "A2A Go SDK [Go]", model_providers: None, streaming: false },
];

// ─── Public entry points ────────────────────────────────────────────────────

/// Non-interactive: scaffold from a named template or registry artifact.
///
/// If `template` contains a `/` (e.g. `nasiko/image-generator-agent`), pulls
/// that exact artifact from the registry. Otherwise treats it as a framework
/// template name (e.g. `crewai`).
pub fn new_agent(template: &str, name: &str) -> Result<()> {
    let dest = Path::new(name);
    if dest.exists() {
        anyhow::bail!("directory '{name}' already exists");
    }

    if template.contains('/') {
        pull_artifact(template, dest)?;
    } else {
        extract_template(template, dest)?;
    }

    println!("Scaffolded: {template} → {name}/");
    println!("\nNext steps:");
    println!("  cd {name}");
    println!("  nasiko run .");
    Ok(())
}

/// Interactive mode: walk user through framework/artifact/skill selection.
pub fn new_agent_interactive(name: Option<&str>) -> Result<()> {
    let agent_name = match name {
        Some(n) => n.to_string(),
        None => Input::new()
            .with_prompt("Agent name")
            .default("my-agent".into())
            .interact_text()?,
    };
    let agent_name = agent_name.trim().to_lowercase().replace(' ', "-");

    // 1. Framework
    let fw_labels: Vec<String> = FRAMEWORKS
        .iter()
        .enumerate()
        .map(|(i, f)| format!("{}. {}", i + 1, f.label))
        .collect();

    let fw_idx = Select::new()
        .with_prompt("Framework")
        .items(&fw_labels)
        .default(0)
        .interact()?;
    let framework = &FRAMEWORKS[fw_idx];

    // 2. Starting point: blank template or existing artifact from registry
    let mut use_artifact: Option<crate::api::Artifact> = None;
    let registry_artifacts = crate::api::RegistryClient::new()
        .and_then(|c| c.search(None, Some("agent"), Some(framework.key)).ok())
        .unwrap_or_default();

    if !registry_artifacts.is_empty() {
        let mut items: Vec<String> = vec!["Blank template (starter project)".to_string()];
        for a in &registry_artifacts {
            let desc = a.description.as_deref().unwrap_or("");
            items.push(format!("{}/{} — {}", a.owner, a.name, desc));
        }

        let choice = Select::new()
            .with_prompt("Start from")
            .items(&items)
            .default(0)
            .interact()?;

        if choice > 0 {
            use_artifact = Some(registry_artifacts[choice - 1].clone());
        }
    }

    // 3. Output directory
    let default_dir = format!("./{agent_name}");
    let out_dir: String = Input::new()
        .with_prompt("Output directory")
        .default(default_dir)
        .interact_text()?;
    let dest = PathBuf::from(&out_dir);

    if dest.exists() && fs::read_dir(&dest)?.next().is_some()
        && !Confirm::new()
            .with_prompt(format!("{} already exists and is non-empty. Overwrite?", dest.display()))
            .default(false)
            .interact()?
        {
            anyhow::bail!("aborted");
        }

    if let Some(ref artifact) = use_artifact {
        // Pull the specific artifact from registry
        let repo = format!("{}/{}", artifact.owner, artifact.name);
        println!("  registry: pulling {repo}...");
        pull_artifact(&repo, &dest)?;
    } else {
        // Blank template flow
        extract_template(framework.key, &dest)?;

        // Description
        let default_desc = format!("A Nasiko agent called {agent_name}");
        let description: String = Input::new()
            .with_prompt("Description")
            .default(default_desc)
            .interact_text()?;

        // Model provider (only for frameworks that support multiple)
        let _model_provider: Option<&str> = match framework.model_providers {
            Some(providers) => {
                let items: Vec<String> = providers.iter().map(|p| p.to_string()).collect();
                let idx = Select::new()
                    .with_prompt("Model provider")
                    .items(&items)
                    .default(0)
                    .interact()?;
                Some(providers[idx])
            }
            None => None,
        };

        // Skills
        let project_lang = match framework.key {
            "a2a-go" => "go",
            _ => "python",
        };
        let available_skills: Vec<String> = crate::skill::list_available_skills()
            .into_iter()
            .filter(|name| {
                crate::skill::resolve_skill(name)
                    .map(|(m, _)| m.runtime.language == project_lang)
                    .unwrap_or(false)
            })
            .collect();
        let selected_skills = pick_skills(&available_skills)?;

        // Capabilities
        let capabilities_input: String = Input::new()
            .with_prompt("Capabilities (comma-separated)")
            .default("answer questions, process text".into())
            .interact_text()?;
        let capabilities: Vec<String> = capabilities_input
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        // Version
        let version: String = Input::new()
            .with_prompt("Version")
            .default("0.1.0".into())
            .interact_text()?;

        // Generate AgentCard.json
        write_agent_card(&dest, &agent_name, framework, &description, &selected_skills, &capabilities, &version)?;

        // Inject skills
        for skill_name in &selected_skills {
            let (manifest, impl_code) = crate::skill::resolve_skill(skill_name)?;
            crate::skill::inject_skill(&dest, &manifest, &impl_code, framework.key)?;
        }
    }

    // ─── Print summary ──────────────────────────────────────────────────────
    println!("\n✓ {}/", dest.display());
    let rel_path = dest.display();
    println!("\nNext steps:");
    println!("  cd {rel_path}");
    println!("  nasiko run .");

    Ok(())
}

// ─── Template extraction ────────────────────────────────────────────────────

fn extract_template(template: &str, dest: &Path) -> Result<()> {
    // Try artifact registry first
    if let Some(url) = crate::config::artifact_registry_url() {
        match crate::oci::pull_template(template) {
            Ok(data) => {
                println!("  registry: {url}");
                util::extract_tar_gz(&data, dest)?;
                return Ok(());
            }
            Err(_) => {
                println!("  registry: {url} (template not found, using built-in)");
            }
        }
    } else {
        println!("  registry: none (using built-in templates)");
    }

    // Fallback: embedded templates
    let dir = AGENTS_DIR
        .get_dir(template)
        .with_context(|| {
            let mut available: Vec<String> = Vec::new();
            // Try registry for the canonical list
            if let Some(client) = crate::api::RegistryClient::new()
                && let Ok(templates) = client.list_templates() {
                    available = templates.iter().map(|a| a.name.clone()).collect();
                }
            // Fallback to embedded template names
            if available.is_empty() {
                available = FRAMEWORKS.iter().map(|f| f.key.to_string()).collect();
            }
            format!("template '{template}' not found.\nAvailable: {}", available.join(", "))
        })?;

    util::extract_embedded_dir(dir, dest)
}

/// Pull a specific artifact from the registry by owner/name (e.g. "nasiko/image-generator-agent").
fn pull_artifact(repo: &str, dest: &Path) -> Result<()> {
    let url = crate::config::artifact_registry_url()
        .context("no artifact registry configured (set NASIKO_REGISTRY_URL)")?;
    println!("  registry: {url}");

    let oci = crate::api::OciClient::for_artifact_registry()?
        .context("failed to connect to artifact registry")?;

    // Resolve the latest tag (artifacts use semver tags, not "latest")
    let tags_json = oci.list_tags(repo)
        .with_context(|| format!("artifact '{repo}' not found in registry"))?;
    let tags: serde_json::Value = serde_json::from_str(&tags_json)?;
    let tag = tags.get("tags")
        .and_then(|t| t.as_array())
        .and_then(|arr| arr.last())
        .and_then(|t| t.as_str())
        .context("no tags found for artifact")?;

    println!("  pulling {repo}:{tag}...");

    let manifest_json = oci.pull_manifest(repo, tag)
        .with_context(|| format!("failed to pull manifest for '{repo}:{tag}'"))?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_json)?;

    let layers = manifest
        .get("layers")
        .and_then(|l| l.as_array())
        .context("invalid manifest: no layers")?;

    let layer_digest = layers
        .first()
        .and_then(|l| l.get("digest"))
        .and_then(|d| d.as_str())
        .context("invalid manifest: no layer digest")?;

    let data = oci.pull_blob(repo, layer_digest)?;
    util::extract_tar_gz(&data, dest)
}

// ─── Skills ─────────────────────────────────────────────────────────────────

fn pick_skills(available: &[String]) -> Result<Vec<String>> {
    if available.is_empty() {
        return Ok(vec![]);
    }

    println!("\nAvailable skills (0 to skip):");
    for (i, s) in available.iter().enumerate() {
        println!("  {}. {}", i + 1, s);
    }

    let input: String = Input::new()
        .with_prompt("Skills (comma-separated numbers, 0 to skip)")
        .default("0".into())
        .interact_text()?;

    if input.trim() == "0" {
        return Ok(vec![]);
    }

    Ok(input
        .split(',')
        .filter_map(|s| s.trim().parse::<usize>().ok())
        .filter(|&i| i >= 1 && i <= available.len())
        .map(|i| available[i - 1].clone())
        .collect())
}

// ─── AgentCard generation ───────────────────────────────────────────────────

fn write_agent_card(
    dest: &Path,
    name: &str,
    framework: &Framework,
    description: &str,
    skills: &[String],
    capabilities: &[String],
    version: &str,
) -> Result<()> {
    let mut skill_entries: Vec<serde_json::Value> = capabilities
        .iter()
        .take(10)
        .map(|cap| {
            serde_json::json!({
                "id": cap.to_lowercase().replace(' ', "-"),
                "name": util::title_case(cap),
                "description": cap,
                "tags": [],
                "examples": [],
                "inputModes": ["text/plain"],
                "outputModes": ["text/plain"]
            })
        })
        .collect();

    for skill_name in skills {
        skill_entries.push(serde_json::json!({
            "id": skill_name,
            "name": util::title_case(&skill_name.replace('-', " ")),
            "description": format!("{} tool", skill_name.replace('-', " ")),
            "tags": [],
            "examples": [],
            "inputModes": ["text/plain"],
            "outputModes": ["text/plain"]
        }));
    }

    let card = serde_json::json!({
        "protocolVersion": "0.2.9",
        "name": name,
        "description": description,
        "url": "http://localhost:8000/",
        "agentFramework": framework.key,
        "preferredTransport": "JSONRPC",
        "provider": {
            "organization": "Nasiko",
            "url": "https://nasiko.com"
        },
        "version": version,
        "capabilities": {
            "streaming": framework.streaming,
            "pushNotifications": false,
            "stateTransitionHistory": false
        },
        "securitySchemes": {},
        "security": [],
        "defaultInputModes": ["text/plain"],
        "defaultOutputModes": ["text/plain"],
        "skills": skill_entries
    });

    fs::write(dest.join("AgentCard.json"), serde_json::to_string_pretty(&card)?)?;
    Ok(())
}
