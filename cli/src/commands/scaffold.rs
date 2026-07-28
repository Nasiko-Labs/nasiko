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
    key: String,
    label: String,
    model_providers: Vec<String>,
    streaming: bool,
    language: String,
}

/// Frameworks are derived from the `framework` field of published agent
/// artifacts — every artifact is reusable, there is no separate "template"
/// flavor. Per-framework attributes come from artifact `metadata`:
/// `modelProviders` (string array, omit/empty when the framework hardcodes
/// its provider), `streaming` (bool), and `language` ("python"/"rust"/... —
/// used for skill-injection compatibility and to finish AgentCard.json).
fn fetch_frameworks() -> Result<Vec<Framework>> {
    let client = crate::api::RegistryClient::new().context(
        "no registry connected — run `nasiko registry connect <url>` \
        (nasiko new derives frameworks from the registry's agent artifacts)",
    )?;
    let artifacts = client
        .search(None, Some("agent"), None)
        .context("failed to fetch agent artifacts from the registry")?;

    let mut frameworks: Vec<Framework> = Vec::new();
    for a in artifacts {
        let Some(fw) = a.framework.clone().filter(|f| !f.is_empty()) else {
            continue;
        };
        if frameworks.iter().any(|f| f.key == fw) {
            continue;
        }
        let model_providers = a
            .metadata
            .get("modelProviders")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let streaming = a
            .metadata
            .get("streaming")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let language = a
            .metadata
            .get("language")
            .and_then(|v| v.as_str())
            .unwrap_or("python")
            .to_string();
        frameworks.push(Framework {
            label: fw.clone(),
            key: fw,
            model_providers,
            streaming,
            language,
        });
    }
    frameworks.sort_by(|a, b| a.key.cmp(&b.key));

    if frameworks.is_empty() {
        anyhow::bail!(
            "no agent artifacts with a framework found in the connected registry \
            (publish agents with --framework, or set agentFramework in AgentCard.json)"
        );
    }
    Ok(frameworks)
}

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
    let frameworks = fetch_frameworks()?;
    let fw_labels: Vec<String> = frameworks
        .iter()
        .enumerate()
        .map(|(i, f)| format!("{}. {}", i + 1, f.label))
        .collect();

    let fw_idx = Select::new()
        .with_prompt("Framework")
        .items(&fw_labels)
        .default(0)
        .interact()?;
    let framework = &frameworks[fw_idx];

    // 2. Starting point: blank template or existing artifact from registry
    let mut use_artifact: Option<crate::api::Artifact> = None;
    let registry_artifacts = crate::api::RegistryClient::new()
        .and_then(|c| {
            c.search(None, Some("agent"), Some(framework.key.as_str()))
                .ok()
        })
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

    if dest.exists()
        && fs::read_dir(&dest)?.next().is_some()
        && !Confirm::new()
            .with_prompt(format!(
                "{} already exists and is non-empty. Overwrite?",
                dest.display()
            ))
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
        extract_template(&framework.key, &dest)?;

        // Description
        let default_desc = format!("A Nasiko agent called {agent_name}");
        let description: String = Input::new()
            .with_prompt("Description")
            .default(default_desc)
            .interact_text()?;

        // Model provider (only for frameworks that support multiple)
        let _model_provider: Option<&str> = if framework.model_providers.is_empty() {
            None
        } else {
            let idx = Select::new()
                .with_prompt("Model provider")
                .items(&framework.model_providers)
                .default(0)
                .interact()?;
            Some(framework.model_providers[idx].as_str())
        };

        // Skills
        let project_lang = framework.language.as_str();
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
        write_agent_card(
            &dest,
            &agent_name,
            framework,
            &description,
            &selected_skills,
            &capabilities,
            &version,
        )?;

        // Inject skills
        for skill_name in &selected_skills {
            let (manifest, impl_code) = crate::skill::resolve_skill(skill_name)?;
            crate::skill::inject_skill(&dest, &manifest, &impl_code, &framework.key)?;
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
            Err(e) => {
                // Say why the registry path lost — a silent fallback makes
                // "template never comes from the registry" undiagnosable.
                println!("  registry: {url} ({e:#}) — using built-in template");
            }
        }
    } else {
        println!("  registry: none (using built-in templates)");
    }

    // Fallback: embedded templates
    let dir = AGENTS_DIR.get_dir(template).with_context(|| {
        let mut available: Vec<String> = Vec::new();
        // Try registry for the canonical list
        if let Some(client) = crate::api::RegistryClient::new()
            && let Ok(templates) = client.list_templates()
        {
            available = templates.iter().map(|a| a.name.clone()).collect();
        }
        // Fallback to the embedded template directory names
        if available.is_empty() {
            available = AGENTS_DIR
                .dirs()
                .filter_map(|d| {
                    d.path()
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(String::from)
                })
                .collect();
        }
        format!(
            "template '{template}' not found.\nAvailable: {}",
            available.join(", ")
        )
    })?;

    util::extract_embedded_dir(dir, dest)
}

/// Pull a specific artifact from the registry by owner/name (e.g. "nasiko/image-generator-agent").
fn pull_artifact(repo: &str, dest: &Path) -> Result<()> {
    let url = crate::config::artifact_registry_url()
        .context("no artifact registry configured (set NASIKO_REGISTRY_URL)")?;
    println!("  registry: {url}");
    println!("  pulling {repo}...");

    let data = crate::oci::pull_artifact_tarball(repo)?;
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

    fs::write(
        dest.join("AgentCard.json"),
        serde_json::to_string_pretty(&card)?,
    )?;
    Ok(())
}
