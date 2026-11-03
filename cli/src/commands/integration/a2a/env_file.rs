//! The file an A2A agent reads its variables from at startup, and the marked block `connect`
//! writes there to route its LLM calls through Nasiko.
//!
//! Deterministic: the file is decided from the agent's process alone, by the rule of the
//! framework that loads it, never by guessing among a project's env files. When no rule
//! applies, or a write would be unsafe, the answer is [`EnvTarget::Unknown`] with what the
//! user should do instead, and the variables are shown rather than written.
//!
//! | Agent | File it loads | Rule |
//! | --- | --- | --- |
//! | `langgraph dev` | `env` in the `langgraph.json` it was started with | path → that file |
//! | CrewAI | the first `.env` found walking up from where CrewAI is installed | only when installed inside the agent's folder |

use std::path::{Path, PathBuf};

use super::process::{ProcessFacts, is_within};

/// The variables `connect` can set, for the providers chosen. Both OpenAI base-URL names are set because frameworks read
/// different ones first (CrewAI `OPENAI_BASE_URL`, LangChain `OPENAI_API_BASE`); likewise
/// for Anthropic.
pub const MANAGED_KEYS: [&str; 6] = [
    "OPENAI_BASE_URL",
    "OPENAI_API_BASE",
    "OPENAI_API_KEY",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_API_URL",
    "ANTHROPIC_API_KEY",
];

/// Env files named for another environment. A Nasiko key written there would ship with that
/// environment's deployment, so `connect` never writes one.
const OTHER_ENVIRONMENTS: [&str; 5] = [
    ".env.staging",
    ".env.stage",
    ".env.prod",
    ".env.production",
    ".env.test",
];

const BLOCK_BEGIN: &str = "# >>> nasiko";
const BLOCK_END: &str = "# <<< nasiko";
/// Prefix of a user's own line for a managed variable while Nasiko's block is in place.
const SET_ASIDE: &str = "# nasiko-off: ";

/// Where an agent reads its variables from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvTarget {
    /// Nasiko can write the variables here.
    File(PathBuf),
    /// Nasiko cannot tell, or must not write; the reason says what the user should do.
    Unknown(String),
}

/// An LLM API an agent calls, each routed through Nasiko by its own variables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Provider {
    #[value(name = "openai")]
    OpenAi,
    Anthropic,
}

impl Provider {
    pub const ALL: [Provider; 2] = [Provider::OpenAi, Provider::Anthropic];

    /// How `connect` names it when asking.
    pub fn label(self) -> &'static str {
        match self {
            Provider::OpenAi => "OpenAI (and OpenAI-compatible) — OPENAI_*",
            Provider::Anthropic => "Anthropic — ANTHROPIC_*",
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Provider::OpenAi => "OPENAI_",
            Provider::Anthropic => "ANTHROPIC_",
        }
    }
}

/// The routing variables for an agent whose key is `key`, on the control plane at
/// `cluster_url`, for the `providers` it calls. The router serves OpenAI's API under `/v1`
/// and Anthropic's at the root, matching what `connect` writes for coding agents.
pub fn routing_vars(
    cluster_url: &str,
    key: &str,
    providers: &[Provider],
) -> Vec<(&'static str, String)> {
    let base = cluster_url.trim_end_matches('/');
    let mut vars = Vec::new();
    if providers.contains(&Provider::OpenAi) {
        let openai = format!("{base}/v1");
        vars.extend([
            ("OPENAI_BASE_URL", openai.clone()),
            ("OPENAI_API_BASE", openai),
            ("OPENAI_API_KEY", key.to_string()),
        ]);
    }
    if providers.contains(&Provider::Anthropic) {
        vars.extend([
            ("ANTHROPIC_BASE_URL", base.to_string()),
            ("ANTHROPIC_API_URL", base.to_string()),
            ("ANTHROPIC_API_KEY", key.to_string()),
        ]);
    }
    vars
}

/// The providers `text`, an env file, already sets variables for — the user's own lines, set
/// aside or not, and an earlier Nasiko block — in [`Provider::ALL`] order.
pub fn providers_in(text: &str) -> Vec<Provider> {
    let keys: Vec<&str> = text
        .lines()
        .filter_map(|line| key_of(line.strip_prefix(SET_ASIDE).unwrap_or(line)))
        .collect();
    Provider::ALL
        .into_iter()
        .filter(|provider| keys.iter().any(|key| key.starts_with(provider.prefix())))
        .collect()
}

/// The MCP gateway variables for an agent whose gateway credential is `token`, on the control
/// plane at `cluster_url`: the names and forms a Nasiko-deployed agent gets
/// (`nasiko_mcp_gateway::injector`), so code written for one runs unchanged on the other.
pub fn mcp_vars(cluster_url: &str, token: &str) -> Vec<(&'static str, String)> {
    let gateway = format!("{}/api/mcp", cluster_url.trim_end_matches('/'));
    vec![
        ("MCP_GATEWAY_URL", gateway.clone()),
        ("MCP_GATEWAY_TOKEN", token.to_string()),
        ("MCP_GATEWAY_CONNECT_URL", format!("{gateway}/s/{token}")),
    ]
}

/// The agent framework the process `facts` describes runs, when it is one `connect` knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentFramework {
    LangGraph,
    CrewAi,
}

/// Which framework the agent run as `facts` uses, by the same signs [`locate`] reads.
pub fn framework_of(facts: &ProcessFacts) -> Option<AgentFramework> {
    let folder = facts.cwd.as_deref()?;
    if langgraph_config(&facts.args, folder).is_some() {
        return Some(AgentFramework::LangGraph);
    }
    crewai_package(&facts.site_packages).map(|_| AgentFramework::CrewAi)
}

/// The file the agent run as `facts` reads its variables from.
pub fn locate(facts: &ProcessFacts) -> EnvTarget {
    let Some(folder) = facts.cwd.as_deref() else {
        return unknown("its working folder could not be read");
    };
    if let Some(config) = langgraph_config(&facts.args, folder) {
        return langgraph_env_file(&config, folder);
    }
    if let Some(crewai) = crewai_package(&facts.site_packages) {
        return crewai_env_file(&crewai, folder);
    }
    unknown("Nasiko can't tell which file it reads its variables from")
}

fn unknown(reason: &str) -> EnvTarget {
    EnvTarget::Unknown(reason.to_string())
}

/// The `langgraph.json` a `langgraph dev` process was started with: its `--config`, else
/// `langgraph.json` in its working folder. `None` when the process isn't `langgraph dev`.
fn langgraph_config(args: &[String], folder: &Path) -> Option<PathBuf> {
    let at = args.iter().position(|arg| {
        Path::new(arg)
            .file_name()
            .is_some_and(|name| name == "langgraph")
    })?;
    if args.get(at + 1).map(String::as_str) != Some("dev") {
        return None;
    }
    let mut config = "langgraph.json".to_string();
    let mut rest = args[at + 2..].iter();
    while let Some(arg) = rest.next() {
        if let Some(value) = arg.strip_prefix("--config=") {
            config = value.to_string();
        } else if arg == "--config" {
            config = rest.next()?.clone();
        }
    }
    Some(folder.join(config))
}

fn langgraph_env_file(config: &Path, folder: &Path) -> EnvTarget {
    let Ok(text) = std::fs::read_to_string(config) else {
        return EnvTarget::Unknown(format!("{} could not be read", config.display()));
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return EnvTarget::Unknown(format!("{} is not valid JSON", config.display()));
    };
    match json.get("env") {
        Some(serde_json::Value::String(path)) => {
            let file = config.parent().unwrap_or(folder).join(path);
            if is_within(&file, folder) {
                EnvTarget::File(file)
            } else {
                EnvTarget::Unknown(format!(
                    "its env file {} is outside {}",
                    file.display(),
                    folder.display()
                ))
            }
        }
        Some(serde_json::Value::Object(_)) => EnvTarget::Unknown(format!(
            "{} sets its variables inline under \"env\"; set them there",
            config.display()
        )),
        _ => EnvTarget::Unknown(format!(
            "{} names no env file; add \"env\": \".env\" to it and run this again",
            config.display()
        )),
    }
}

/// CrewAI's package folder, when one of the process's `site-packages` holds it.
fn crewai_package(site_packages: &[PathBuf]) -> Option<PathBuf> {
    site_packages
        .iter()
        .map(|dir| dir.join("crewai"))
        .find(|package| package.is_dir())
}

/// CrewAI calls `load_dotenv()` from inside its own package, so python-dotenv searches
/// upward from there and uses the first `.env` it meets. When CrewAI is installed inside the
/// agent's folder (the `crewai create` layout), that search passes through the folder: the
/// file is the first `.env` on the way, else a new one in the folder, which the search then
/// reaches before any `.env` above it.
fn crewai_env_file(package: &Path, folder: &Path) -> EnvTarget {
    if !is_within(package, folder) {
        return EnvTarget::Unknown(format!(
            "CrewAI is installed outside {} ({}), so it doesn't read the project's .env",
            folder.display(),
            package.display()
        ));
    }
    let found = package
        .ancestors()
        .take_while(|dir| is_within(dir, folder))
        .map(|dir| dir.join(".env"))
        .find(|file| file.is_file());
    EnvTarget::File(found.unwrap_or_else(|| folder.join(".env")))
}

/// Why `file` must not be written, if anything: it is named for another environment, or
/// another env file in `folder` sets a variable `connect` sets (and would decide which value
/// wins).
pub fn refusal(file: &Path, folder: &Path) -> Option<String> {
    let name = file.file_name()?.to_string_lossy();
    if OTHER_ENVIRONMENTS.contains(&name.as_ref()) {
        return Some(format!(
            "{} is for another environment; Nasiko writes only the local one",
            file.display()
        ));
    }
    let conflicts = conflicting_lines(file, folder);
    (!conflicts.is_empty()).then(|| {
        format!(
            "other env files also set these variables: {}",
            conflicts.join(", ")
        )
    })
}

/// `file:line` of every line in `folder`'s other `.env*` files that sets a managed variable.
fn conflicting_lines(file: &Path, folder: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut others: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path != file && path.is_file() && is_env_file(path))
        .collect();
    others.sort();
    others
        .iter()
        .flat_map(|path| {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            managed_line_numbers(&text)
                .into_iter()
                .map(|line| format!("{}:{line}", path.display()))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// `.env` and `.env.<anything>`, except example and template files nothing loads.
fn is_env_file(path: &Path) -> bool {
    let Some(name) = path.file_name().map(|name| name.to_string_lossy()) else {
        return false;
    };
    let is_env = name == ".env" || name.starts_with(".env.");
    let is_sample = [".example", ".sample", ".template", ".dist"]
        .iter()
        .any(|suffix| name.ends_with(suffix));
    is_env && !is_sample
}

/// 1-based numbers of the lines that set a managed variable.
fn managed_line_numbers(text: &str) -> Vec<usize> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| key_of(line).is_some_and(|key| MANAGED_KEYS.contains(&key)))
        .map(|(index, _)| index + 1)
        .collect()
}

/// The variable a dotenv line sets (`KEY=…` or `export KEY=…`), if it sets one.
fn key_of(line: &str) -> Option<&str> {
    let line = line.trim_start();
    let line = line.strip_prefix("export ").unwrap_or(line);
    let (key, _) = line.split_once('=')?;
    let key = key.trim();
    let valid = !key.is_empty()
        && key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_');
    valid.then_some(key)
}

/// `text` with Nasiko's block for `agent` at the end, replacing any earlier one. The user's
/// own lines for the variables it sets are set aside as comments, so the block decides and
/// undo can restore them; their lines for a provider not routed stay in force.
pub fn with_block(text: &str, header: &str, vars: &[(&str, String)]) -> String {
    let sets = |key: &str| vars.iter().any(|(set, _)| *set == key);
    let mut lines: Vec<String> = without_block(text)
        .lines()
        .map(|line| match key_of(line) {
            Some(key) if sets(key) => format!("{SET_ASIDE}{line}"),
            _ => line.to_string(),
        })
        .collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    if !lines.is_empty() {
        lines.push(String::new());
    }
    lines.push(format!("{BLOCK_BEGIN}: {header}"));
    lines.extend(vars.iter().map(|(key, value)| format!("{key}={value}")));
    lines.push(BLOCK_END.to_string());
    lines.join("\n") + "\n"
}

/// `text` without Nasiko's block, with the user's own lines restored.
pub fn without_block(text: &str) -> String {
    let mut out = Vec::new();
    let mut in_block = false;
    for line in text.lines() {
        if line.starts_with(BLOCK_BEGIN) {
            in_block = true;
        } else if in_block {
            in_block = line.trim_end() != BLOCK_END;
        } else {
            out.push(line.strip_prefix(SET_ASIDE).unwrap_or(line));
        }
    }
    while out.last().is_some_and(|line| line.trim().is_empty()) {
        out.pop();
    }
    if out.is_empty() {
        String::new()
    } else {
        out.join("\n") + "\n"
    }
}

/// Whether `text` holds Nasiko's block.
pub fn has_block(text: &str) -> bool {
    text.lines().any(|line| line.starts_with(BLOCK_BEGIN))
}

/// Write `text` to `file`, creating it readable only by its owner when it is new.
pub fn write(file: &Path, text: &str) -> std::io::Result<()> {
    if !file.exists() {
        create_private(file)?;
    }
    std::fs::write(file, text)
}

#[cfg(unix)]
fn create_private(file: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(file)
        .map(drop)
}

#[cfg(not(unix))]
fn create_private(file: &Path) -> std::io::Result<()> {
    std::fs::File::create(file).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> Vec<(&'static str, String)> {
        routing_vars("https://cp.example/", "jwt", &Provider::ALL)
    }

    fn facts(cwd: &Path, args: &[&str], site_packages: &[PathBuf]) -> ProcessFacts {
        ProcessFacts {
            cwd: Some(cwd.to_path_buf()),
            args: args.iter().map(|arg| arg.to_string()).collect(),
            site_packages: site_packages.to_vec(),
        }
    }

    #[test]
    fn routing_vars_point_both_sdks_at_the_router() {
        assert_eq!(
            vars(),
            vec![
                ("OPENAI_BASE_URL", "https://cp.example/v1".to_string()),
                ("OPENAI_API_BASE", "https://cp.example/v1".to_string()),
                ("OPENAI_API_KEY", "jwt".to_string()),
                ("ANTHROPIC_BASE_URL", "https://cp.example".to_string()),
                ("ANTHROPIC_API_URL", "https://cp.example".to_string()),
                ("ANTHROPIC_API_KEY", "jwt".to_string()),
            ]
        );
    }

    #[test]
    fn routing_vars_cover_only_the_chosen_providers() {
        let anthropic = routing_vars("https://cp.example", "jwt", &[Provider::Anthropic]);
        assert!(
            anthropic
                .iter()
                .all(|(key, _)| key.starts_with("ANTHROPIC_"))
        );
        assert_eq!(anthropic.len(), 3);
        assert!(routing_vars("https://cp.example", "jwt", &[]).is_empty());
    }

    #[test]
    fn the_providers_a_file_uses_are_read_from_its_lines_set_aside_or_not() {
        assert_eq!(providers_in("OTHER=1\n"), vec![]);
        assert_eq!(
            providers_in("export ANTHROPIC_API_KEY=a\n# OPENAI_API_KEY=commented\n"),
            vec![Provider::Anthropic]
        );
        let connected = with_block("OPENAI_API_KEY=mine\n", "h", &vars());
        assert_eq!(providers_in(&connected), Provider::ALL.to_vec());
    }

    #[test]
    fn a_provider_not_routed_keeps_the_users_own_lines_in_force() {
        let original = "OPENAI_API_KEY=mine\nANTHROPIC_API_KEY=theirs\n";
        let openai = routing_vars("https://cp.example", "jwt", &[Provider::OpenAi]);
        let connected = with_block(original, "h", &openai);
        assert!(connected.contains("# nasiko-off: OPENAI_API_KEY=mine\n"));
        assert!(
            connected.starts_with("# nasiko-off: OPENAI_API_KEY=mine\nANTHROPIC_API_KEY=theirs\n")
        );
        assert_eq!(without_block(&connected), original);
    }

    #[test]
    fn mcp_vars_are_the_ones_a_deployed_agent_gets() {
        assert_eq!(
            mcp_vars("https://cp.example/", "ngt_abc"),
            vec![
                ("MCP_GATEWAY_URL", "https://cp.example/api/mcp".to_string()),
                ("MCP_GATEWAY_TOKEN", "ngt_abc".to_string()),
                (
                    "MCP_GATEWAY_CONNECT_URL",
                    "https://cp.example/api/mcp/s/ngt_abc".to_string()
                ),
            ]
        );
    }

    #[test]
    fn the_framework_is_told_by_the_same_signs_as_the_env_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("langgraph.json"), "{}").unwrap();
        let site_packages = dir.path().join(".venv/lib/python3.12/site-packages");
        std::fs::create_dir_all(site_packages.join("crewai")).unwrap();

        assert_eq!(
            framework_of(&facts(dir.path(), &["/v/bin/langgraph", "dev"], &[])),
            Some(AgentFramework::LangGraph)
        );
        assert_eq!(
            framework_of(&facts(dir.path(), &["python", "main.py"], &[site_packages])),
            Some(AgentFramework::CrewAi)
        );
        assert_eq!(
            framework_of(&facts(dir.path(), &["python", "main.py"], &[])),
            None
        );
    }

    #[test]
    fn the_block_sets_aside_the_users_lines_and_undo_restores_them() {
        let original =
            "DEBUG=1\nOPENAI_API_KEY=sk-real\nexport OPENAI_BASE_URL=https://api.openai.com/v1\n";

        let connected = with_block(original, "LLM routing for crew", &vars());

        assert_eq!(
            connected,
            "DEBUG=1\n\
             # nasiko-off: OPENAI_API_KEY=sk-real\n\
             # nasiko-off: export OPENAI_BASE_URL=https://api.openai.com/v1\n\
             \n\
             # >>> nasiko: LLM routing for crew\n\
             OPENAI_BASE_URL=https://cp.example/v1\n\
             OPENAI_API_BASE=https://cp.example/v1\n\
             OPENAI_API_KEY=jwt\n\
             ANTHROPIC_BASE_URL=https://cp.example\n\
             ANTHROPIC_API_URL=https://cp.example\n\
             ANTHROPIC_API_KEY=jwt\n\
             # <<< nasiko\n"
        );
        assert!(has_block(&connected));
        assert_eq!(without_block(&connected), original);
    }

    #[test]
    fn connecting_again_replaces_the_block_instead_of_adding_one() {
        let once = with_block("", "first", &vars());
        let twice = with_block(&once, "second", &vars());

        assert_eq!(twice.matches(BLOCK_BEGIN).count(), 1);
        assert!(twice.contains("second") && !twice.contains("first"));
        assert_eq!(without_block(&twice), "");
    }

    #[test]
    fn langgraph_reads_the_env_file_its_config_names() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("langgraph.json"),
            r#"{"env": ".env.local"}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("prod.json"), r#"{"env": "./.env.prod"}"#).unwrap();
        let langgraph = ["/v/bin/python", "/v/bin/langgraph", "dev", "--port", "2024"];

        assert_eq!(
            locate(&facts(dir.path(), &langgraph, &[])),
            EnvTarget::File(dir.path().join(".env.local"))
        );
        let with_config = [&langgraph[..], &["--config", "prod.json"]].concat();
        assert_eq!(
            locate(&facts(dir.path(), &with_config, &[])),
            EnvTarget::File(dir.path().join("./.env.prod"))
        );
    }

    #[test]
    fn langgraph_without_an_env_file_is_not_written() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("langgraph.json"), r#"{"graphs": {}}"#).unwrap();

        let target = locate(&facts(dir.path(), &["/v/bin/langgraph", "dev"], &[]));

        assert!(
            matches!(target, EnvTarget::Unknown(reason) if reason.contains("\"env\": \".env\""))
        );
    }

    #[test]
    fn crewai_inside_the_folder_reads_the_first_env_above_its_package() {
        let dir = tempfile::tempdir().unwrap();
        let site_packages = dir.path().join(".venv/lib/python3.12/site-packages");
        std::fs::create_dir_all(site_packages.join("crewai")).unwrap();
        let python = [".venv/bin/python", "main.py"];

        assert_eq!(
            locate(&facts(
                dir.path(),
                &python,
                std::slice::from_ref(&site_packages)
            )),
            EnvTarget::File(dir.path().join(".env"))
        );
        std::fs::write(dir.path().join(".venv/.env"), "").unwrap();
        assert_eq!(
            locate(&facts(dir.path(), &python, &[site_packages])),
            EnvTarget::File(dir.path().join(".venv/.env"))
        );
    }

    #[test]
    fn crewai_installed_elsewhere_is_not_written() {
        let project = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let site_packages = elsewhere.path().join("site-packages");
        std::fs::create_dir_all(site_packages.join("crewai")).unwrap();

        let target = locate(&facts(
            project.path(),
            &["python", "main.py"],
            &[site_packages],
        ));

        assert!(matches!(target, EnvTarget::Unknown(reason) if reason.contains("outside")));
    }

    #[test]
    fn an_unknown_framework_is_not_written() {
        let dir = tempfile::tempdir().unwrap();

        let target = locate(&facts(dir.path(), &["node", "server.js"], &[]));

        assert!(matches!(target, EnvTarget::Unknown(_)));
    }

    #[test]
    fn other_environments_and_conflicting_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let env = dir.path().join(".env");
        std::fs::write(dir.path().join(".env.example"), "OPENAI_API_KEY=\n").unwrap();

        assert_eq!(refusal(&env, dir.path()), None);
        assert!(
            refusal(&dir.path().join(".env.production"), dir.path())
                .is_some_and(|reason| reason.contains("another environment"))
        );
        std::fs::write(
            dir.path().join(".env.local"),
            "X=1\nOPENAI_API_BASE=http://x\n",
        )
        .unwrap();
        assert!(refusal(&env, dir.path()).is_some_and(|reason| reason.contains(".env.local:2")));
    }

    #[test]
    fn a_new_file_is_readable_only_by_its_owner() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(".env");

        write(&file, "A=1\n").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "A=1\n");
    }
}
