use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::util;

// ─── Skill Manifest ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct SkillManifest {
    pub api_version: String,
    pub kind: String,
    pub metadata: SkillMetadata,
    pub interface: SkillInterface,
    pub runtime: SkillRuntime,
    #[serde(default)]
    pub frameworks: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct SkillMetadata {
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub author: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct SkillInterface {
    pub function: String,
    pub inputs: serde_json::Value,
    pub output: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct SkillRuntime {
    pub language: String,
    pub entrypoint: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub env_vars: Vec<String>,
}

// ─── Resolution ─────────────────────────────────────────────────────────────

use include_dir::{Dir, include_dir};

static SKILLS_DIR: Dir = include_dir!("$OUT_DIR/agents/skills");

pub fn resolve_skill(name: &str) -> Result<(SkillManifest, String)> {
    // Try registry first (pull OCI blob, extract skill.json + impl)
    if let Some(result) = resolve_from_registry(name) {
        return result;
    }

    // Fallback: embedded skills
    resolve_embedded(name)
}

fn resolve_embedded(name: &str) -> Result<(SkillManifest, String)> {
    let manifest_path = format!("{name}/skill.json");
    let manifest_file = SKILLS_DIR
        .get_file(&manifest_path)
        .with_context(|| format!("skill '{name}' not found"))?;

    let manifest: SkillManifest = serde_json::from_slice(manifest_file.contents())
        .with_context(|| format!("invalid skill.json for '{name}'"))?;

    let impl_path = format!("{name}/{}", manifest.runtime.entrypoint);
    let impl_file = SKILLS_DIR
        .get_file(&impl_path)
        .with_context(|| format!("implementation not found: {impl_path}"))?;

    let impl_code = std::str::from_utf8(impl_file.contents())
        .context("skill implementation is not valid UTF-8")?
        .to_string();

    Ok((manifest, impl_code))
}

fn resolve_from_registry(name: &str) -> Option<Result<(SkillManifest, String)>> {
    let oci = crate::api::OciClient::for_artifact_registry().ok()??;
    let repo = format!("nasiko/{name}");

    let manifest_json = match oci.pull_manifest(&repo, "latest") {
        Ok(m) => m,
        Err(_) => return None,
    };
    let manifest: serde_json::Value = match serde_json::from_str(&manifest_json) {
        Ok(m) => m,
        Err(_) => return None,
    };

    let layer_digest = manifest
        .pointer("/layers/0/digest")
        .and_then(|d| d.as_str())?;

    let blob = match oci.pull_blob(&repo, layer_digest) {
        Ok(b) => b,
        Err(_) => return None,
    };

    Some(extract_skill_from_tarball(&blob, name))
}

fn extract_skill_from_tarball(data: &[u8], name: &str) -> Result<(SkillManifest, String)> {
    use flate2::read::GzDecoder;
    use std::io::Read;

    let decoder = GzDecoder::new(data);
    let mut archive = tar::Archive::new(decoder);

    let mut skill_json: Option<Vec<u8>> = None;
    let mut impl_code: Option<String> = None;
    let mut manifest: Option<SkillManifest> = None;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().to_string();
        let path = path.trim_start_matches("./");

        if path == "skill.json" {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            let m: SkillManifest = serde_json::from_slice(&buf)
                .with_context(|| format!("invalid skill.json in registry artifact '{name}'"))?;
            skill_json = Some(buf);
            manifest = Some(m);
        }
    }

    let manifest = manifest.context(format!(
        "skill.json not found in registry artifact '{name}'"
    ))?;

    // Second pass to get the entrypoint file
    let decoder = flate2::read::GzDecoder::new(data);
    let mut archive = tar::Archive::new(decoder);
    let entrypoint = &manifest.runtime.entrypoint;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().to_string();
        let path = path.trim_start_matches("./");

        if path == entrypoint.as_str() {
            let mut buf = String::new();
            entry.read_to_string(&mut buf)?;
            impl_code = Some(buf);
            break;
        }
    }

    let _ = skill_json;
    let impl_code = impl_code.context(format!(
        "entrypoint '{}' not found in registry artifact '{name}'",
        entrypoint
    ))?;

    Ok((manifest, impl_code))
}

pub fn list_available_skills() -> Vec<String> {
    let mut skills: Vec<String> = SKILLS_DIR
        .dirs()
        .filter_map(|d| {
            let name = d.path().file_name()?.to_str()?.to_string();
            if d.get_file(format!("{name}/skill.json")).is_some() || d.contains("skill.json") {
                Some(name)
            } else {
                None
            }
        })
        .collect();

    // Merge in registry skills
    if let Some(client) = crate::api::RegistryClient::new()
        && let Ok(remote_skills) = client.list_skills(None)
    {
        for artifact in remote_skills {
            let name = artifact.name.clone();
            if !skills.contains(&name) {
                skills.push(name);
            }
        }
    }

    skills
}

// ─── Codegen ────────────────────────────────────────────────────────────────

pub mod codegen {
    use super::*;

    pub fn generate(manifest: &SkillManifest, impl_code: &str, framework: &str) -> Result<String> {
        match framework {
            "openai" | "openai-agents" | "openai-swarm" => generate_openai(manifest, impl_code),
            "claude-sdk" => generate_claude_sdk(manifest, impl_code),
            "langgraph" => generate_langgraph(manifest, impl_code),
            "crewai" => generate_crewai(manifest, impl_code),
            "a2a-go" => generate_go(manifest, impl_code),
            _ => bail!("unsupported framework for codegen: {framework}"),
        }
    }

    pub fn import_line(manifest: &SkillManifest, framework: &str) -> String {
        let module = manifest.metadata.name.replace('-', "_");
        let func = &manifest.interface.function;
        match framework {
            "claude-sdk" => format!("from {module} import TOOL_DEFINITION as {func}_TOOL, {func}"),
            _ => format!("from {module} import {func}"),
        }
    }

    pub fn tool_entry(manifest: &SkillManifest, framework: &str) -> String {
        let func = &manifest.interface.function;
        match framework {
            "claude-sdk" => format!("{func}_TOOL,"),
            _ => format!("{func},"),
        }
    }

    fn generate_openai(manifest: &SkillManifest, impl_code: &str) -> Result<String> {
        let func = &manifest.interface.function;
        let version = &manifest.metadata.version;
        let name = &manifest.metadata.name;

        let body = extract_function_body(impl_code, func);

        Ok(format!(
            r#""""{name} skill (v{version}) — OpenAI Agents SDK"""
from agents import function_tool
{imports}

@function_tool
{body}
"#,
            imports = extract_imports(impl_code, func),
            body = body,
        ))
    }

    fn generate_claude_sdk(manifest: &SkillManifest, impl_code: &str) -> Result<String> {
        let func = &manifest.interface.function;
        let name = &manifest.metadata.name;
        let version = &manifest.metadata.version;
        let desc = &manifest.metadata.description;
        let inputs_json = serde_json::to_string_pretty(&manifest.interface.inputs)?;

        let body = extract_function_body(impl_code, func);

        Ok(format!(
            r#""""{name} skill (v{version}) — Claude SDK"""
{imports}

TOOL_DEFINITION = {{
    "name": "{func}",
    "description": "{desc}",
    "input_schema": {inputs_json},
}}


{body}
"#,
            imports = extract_imports(impl_code, func),
        ))
    }

    fn generate_langgraph(manifest: &SkillManifest, impl_code: &str) -> Result<String> {
        let func = &manifest.interface.function;
        let name = &manifest.metadata.name;
        let version = &manifest.metadata.version;

        let body = extract_function_body(impl_code, func);

        Ok(format!(
            r#""""{name} skill (v{version}) — LangGraph"""
from langchain_core.tools import tool
{imports}

@tool
{body}
"#,
            imports = extract_imports(impl_code, func),
        ))
    }

    fn generate_crewai(manifest: &SkillManifest, impl_code: &str) -> Result<String> {
        let func = &manifest.interface.function;
        let name = &manifest.metadata.name;
        let version = &manifest.metadata.version;
        let display_name = util::title_case(&name.replace('-', " "));

        let body = extract_function_body(impl_code, func);

        Ok(format!(
            r#""""{name} skill (v{version}) — CrewAI"""
from crewai.tools import tool
{imports}

@tool("{display_name}")
{body}
"#,
            imports = extract_imports(impl_code, func),
        ))
    }

    fn generate_go(manifest: &SkillManifest, impl_code: &str) -> Result<String> {
        Ok(format!(
            "// {name} skill (v{version}) — A2A Go\n// TODO: Go codegen not yet implemented\n{impl_code}",
            name = manifest.metadata.name,
            version = manifest.metadata.version,
        ))
    }

    fn extract_imports(impl_code: &str, _func_name: &str) -> String {
        impl_code
            .lines()
            .filter(|line| {
                let trimmed = line.trim();
                trimmed.starts_with("import ") || trimmed.starts_with("from ")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn extract_function_body(impl_code: &str, func_name: &str) -> String {
        let mut lines: Vec<&str> = Vec::new();
        let mut inside = false;
        let target = format!("def {func_name}(");

        for line in impl_code.lines() {
            if !inside {
                if line.contains(&target) {
                    inside = true;
                    lines.push(line);
                }
            } else {
                if !line.is_empty() && !line.starts_with(' ') && !line.starts_with('\t') {
                    break;
                }
                lines.push(line);
            }
        }

        lines.join("\n")
    }
}

// ─── Injection ──────────────────────────────────────────────────────────────

pub fn inject_skill(
    project_dir: &Path,
    manifest: &SkillManifest,
    impl_code: &str,
    framework: &str,
) -> Result<()> {
    let module_name = manifest.metadata.name.replace('-', "_");

    // 1. Generate framework-specific skill file
    let generated = codegen::generate(manifest, impl_code, framework)?;
    let skill_file = if manifest.runtime.language == "go" {
        project_dir.join(format!("internal/skills/{module_name}.go"))
    } else {
        project_dir.join(format!("src/{module_name}.py"))
    };
    if let Some(parent) = skill_file.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&skill_file, &generated)?;

    // 2. Inject at markers in agent source
    let agent_source = find_agent_source(project_dir, framework);
    if let Some(agent_path) = agent_source {
        let content = fs::read_to_string(&agent_path)?;

        let import_line = codegen::import_line(manifest, framework);
        let tool_entry = codegen::tool_entry(manifest, framework);

        let content = content.replace(
            "# [nasiko:imports]",
            &format!("{import_line}\n# [nasiko:imports]"),
        );
        let content = content.replace(
            "# [nasiko:tools]",
            &format!("{tool_entry}\n        # [nasiko:tools]"),
        );
        // Go markers use //
        let content = content.replace(
            "// [nasiko:imports]",
            &format!("{import_line}\n\t// [nasiko:imports]"),
        );

        fs::write(&agent_path, content)?;
    }

    // 3. Update dependencies
    if manifest.runtime.language == "python" {
        update_pyproject(project_dir, &manifest.runtime.dependencies)?;
    }

    // 4. Update AgentCard.json
    update_agent_card(project_dir, manifest)?;

    Ok(())
}

pub fn remove_skill(project_dir: &Path, skill_name: &str, framework: &str) -> Result<()> {
    let module_name = skill_name.replace('-', "_");

    // Remove skill file
    let skill_file = if framework == "a2a-go" {
        project_dir.join(format!("internal/skills/{module_name}.go"))
    } else {
        project_dir.join(format!("src/{module_name}.py"))
    };
    if skill_file.exists() {
        fs::remove_file(&skill_file)?;
    }

    // Remove injected lines from agent source
    let agent_source = find_agent_source(project_dir, framework);
    if let Some(agent_path) = agent_source {
        let content = fs::read_to_string(&agent_path)?;
        let content: String = content
            .lines()
            .filter(|line| !line.contains(&module_name))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&agent_path, content)?;
    }

    Ok(())
}

fn find_agent_source(project_dir: &Path, framework: &str) -> Option<std::path::PathBuf> {
    let candidates = match framework {
        "a2a-go" => vec!["cmd/main.go", "main.go"],
        _ => vec!["src/agent.py", "agent.py"],
    };
    for c in candidates {
        let p = project_dir.join(c);
        if p.exists() {
            return Some(p);
        }
    }
    // Try first .go or .py in cmd/
    if framework == "a2a-go"
        && let Ok(entries) = fs::read_dir(project_dir.join("cmd"))
    {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                let main = entry.path().join("main.go");
                if main.exists() {
                    return Some(main);
                }
            }
        }
    }
    None
}

fn update_pyproject(project_dir: &Path, deps: &[String]) -> Result<()> {
    let pyproject_path = project_dir.join("pyproject.toml");
    if !pyproject_path.exists() || deps.is_empty() {
        return Ok(());
    }

    let content = fs::read_to_string(&pyproject_path)?;

    // Simple approach: find dependencies array and append if not present
    let mut new_deps = Vec::new();
    for dep in deps {
        let pkg_name = dep
            .split(">=")
            .next()
            .unwrap_or(dep)
            .split("==")
            .next()
            .unwrap_or(dep);
        if !content.contains(pkg_name) {
            new_deps.push(dep.clone());
        }
    }

    if new_deps.is_empty() {
        return Ok(());
    }

    // Find the closing ] of dependencies array and insert before it
    if let Some(deps_start) = content.find("dependencies = [")
        && let Some(deps_end) = content[deps_start..].find(']')
    {
        let insert_pos = deps_start + deps_end;
        let additions: String = new_deps.iter().map(|d| format!("    \"{d}\",\n")).collect();
        let mut result = content.clone();
        result.insert_str(insert_pos, &additions);
        fs::write(&pyproject_path, result)?;
    }

    Ok(())
}

fn update_agent_card(project_dir: &Path, manifest: &SkillManifest) -> Result<()> {
    let card_path = project_dir.join("AgentCard.json");
    if !card_path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&card_path)?;
    let mut card: serde_json::Value = serde_json::from_str(&content)?;

    let skills = card.get_mut("skills").and_then(|s| s.as_array_mut());

    if let Some(skills) = skills {
        // Don't add duplicates
        let already = skills
            .iter()
            .any(|s| s.get("id").and_then(|i| i.as_str()) == Some(&manifest.metadata.name));
        if !already {
            skills.push(serde_json::json!({
                "id": manifest.metadata.name,
                "name": util::title_case(&manifest.metadata.name.replace('-', " ")),
                "description": manifest.metadata.description,
                "tags": manifest.metadata.tags,
                "examples": [],
                "inputModes": ["text/plain"],
                "outputModes": ["text/plain"]
            }));
        }
    }

    fs::write(&card_path, serde_json::to_string_pretty(&card)?)?;
    Ok(())
}

// ─── Detect framework from project ─────────────────────────────────────────

pub fn detect_framework(project_dir: &Path) -> Option<String> {
    let card_path = project_dir.join("AgentCard.json");
    if card_path.exists()
        && let Ok(content) = fs::read_to_string(&card_path)
        && let Ok(card) = serde_json::from_str::<serde_json::Value>(&content)
        && let Some(fw) = card.get("agentFramework").and_then(|f| f.as_str())
        && !fw.is_empty()
    {
        return Some(fw.to_string());
    }

    if project_dir.join("go.mod").exists() {
        return Some("a2a-go".into());
    }

    let agent_py = project_dir.join("src/agent.py");
    if agent_py.exists()
        && let Ok(source) = fs::read_to_string(&agent_py)
    {
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
        if lower.contains("anthropic") {
            return Some("claude-sdk".into());
        }
        if lower.contains("from agents import") {
            return Some("openai".into());
        }
    }

    None
}
