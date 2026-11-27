use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, Item, Table};

use crate::commands::codex::{
    LegacyMcp, SavedItem, capture, codex_binary, config_path, decode, mcp_server_item,
    mcp_server_item_ref, mcp_servers_are_writable, read_config, remove_if_exists, restore_file,
    restore_mcp_server, snapshot, validate_config,
};
use crate::commands::coding_agent_router::{self, MCP_SERVER_NAME, McpCredential};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Record {
    config_path: PathBuf,
    created_config: bool,
    original_mcp_servers_present: bool,
    original_mcp_server: SavedItem,
    installed_mcp_server: String,
}

impl Record {
    pub(super) fn adopted(legacy: &LegacyMcp) -> Self {
        Self {
            config_path: legacy.config_path.clone(),
            created_config: false,
            original_mcp_servers_present: legacy.original_mcp_servers_present,
            original_mcp_server: legacy.original_mcp_server.clone(),
            installed_mcp_server: legacy.installed_mcp_server.clone(),
        }
    }
}

pub(super) fn check() -> Result<()> {
    codex_binary()?;
    let path = config_path();
    require_writable(&read_config(&path)?, &path)
}

fn require_writable(document: &DocumentMut, path: &Path) -> Result<()> {
    if !mcp_servers_are_writable(document) {
        bail!(
            "Codex setting 'mcp_servers' in {} is not a [mcp_servers] table, so the Nasiko MCP gateway can't be added to it",
            path.display()
        );
    }
    Ok(())
}

pub(super) fn register(credential: &McpCredential) -> Result<Record> {
    let codex = codex_binary()?;
    let path = config_path();
    let original = fs::read(&path).ok();
    let mut document = read_config(&path)?;
    let record = record_for(path.clone(), original.is_none(), &document, credential)?;
    apply(&mut document, &record)?;
    coding_agent_router::atomic_write(&path, document.to_string().as_bytes())?;
    if let Err(error) = validate_config(&codex, path.parent().unwrap_or_else(|| Path::new("."))) {
        return match restore_file(&path, original.as_deref()) {
            Ok(()) => Err(error),
            Err(restore) => Err(error.context(format!(
                "and {} could not be put back: {restore:#}",
                path.display()
            ))),
        };
    }
    Ok(record)
}

fn record_for(
    config_path: PathBuf,
    created_config: bool,
    document: &DocumentMut,
    credential: &McpCredential,
) -> Result<Record> {
    require_writable(document, &config_path)?;
    Ok(Record {
        config_path,
        created_config,
        original_mcp_servers_present: document.get("mcp_servers").is_some(),
        original_mcp_server: capture(mcp_server_item_ref(document)),
        installed_mcp_server: snapshot(&mcp_server_item(credential)),
    })
}

fn apply(document: &mut DocumentMut, record: &Record) -> Result<()> {
    if !document.contains_key("mcp_servers") {
        let mut servers = Table::new();
        servers.set_implicit(true);
        document["mcp_servers"] = Item::Table(servers);
    }
    let servers = document["mcp_servers"]
        .as_table_mut()
        .context("Codex setting 'mcp_servers' must be a table")?;
    servers[MCP_SERVER_NAME] = decode(&record.installed_mcp_server)?;
    Ok(())
}

fn restore(document: &mut DocumentMut, record: &Record) -> Result<()> {
    restore_mcp_server(
        document,
        record.original_mcp_servers_present,
        &record.original_mcp_server,
        &record.installed_mcp_server,
    )
}

pub(super) fn unregister(record: &Record) -> Result<()> {
    let mut document = read_config(&record.config_path)?;
    restore(&mut document, record)?;
    let rendered = document.to_string();
    if record.created_config && rendered.trim().is_empty() {
        remove_if_exists(&record.config_path)
    } else {
        coding_agent_router::atomic_write(&record.config_path, rendered.as_bytes())
    }
}

pub(super) fn registered_token(record: Option<&Record>) -> Result<Option<String>> {
    let path = record.map_or_else(config_path, |record| record.config_path.clone());
    Ok(token_in(&read_config(&path)?))
}

fn token_in(document: &DocumentMut) -> Option<String> {
    let url = mcp_server_item_ref(document)?.get("url")?.as_str()?;
    Some(
        url.rsplit_once("/s/")
            .map(|(_, token)| token.to_string())
            .unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credential() -> McpCredential {
        McpCredential {
            token: "ngt_secret".into(),
            gateway_url: "https://cp.example/api/mcp".into(),
            connect_url: "https://cp.example/api/mcp/s/ngt_secret".into(),
        }
    }

    fn installed(original: &str) -> (DocumentMut, Record) {
        let mut document = original.parse::<DocumentMut>().unwrap();
        let record =
            record_for("/tmp/config.toml".into(), false, &document, &credential()).unwrap();
        apply(&mut document, &record).unwrap();
        (document, record)
    }

    fn round_trip(document: &DocumentMut, record: &Record) -> String {
        let mut reparsed = document.to_string().parse::<DocumentMut>().unwrap();
        restore(&mut reparsed, record).unwrap();
        reparsed.to_string()
    }

    #[test]
    fn the_server_is_added_with_the_connect_url_and_everything_else_is_kept() {
        let (document, _) = installed("model = \"gpt-5\"\n[model_providers.x]\nname = \"x\"\n");
        assert_eq!(
            document["mcp_servers"][MCP_SERVER_NAME]["url"].as_str(),
            Some("https://cp.example/api/mcp/s/ngt_secret")
        );
        let rendered = document.to_string();
        assert!(rendered.contains("model = \"gpt-5\""));
        assert!(rendered.contains("[model_providers.x]"));
        assert_eq!(token_in(&document).as_deref(), Some("ngt_secret"));
    }

    #[test]
    fn a_new_mcp_servers_table_renders_without_an_empty_header() {
        let rendered = installed("other = 1\n").0.to_string();
        assert!(rendered.contains("[mcp_servers.nasiko]"));
        assert!(!rendered.contains("[mcp_servers]\n"));
    }

    #[test]
    fn removing_restores_the_file_exactly() {
        for original in [
            "other = 1\n",
            "[mcp_servers.other]\nurl = \"https://other.example\"\n",
            "[mcp_servers.nasiko]\nurl = \"https://user.example\"\n",
            "",
        ] {
            let (document, record) = installed(original);
            assert_eq!(round_trip(&document, &record), original, "{original:?}");
        }
    }

    #[test]
    fn a_users_own_nasiko_server_is_saved_and_put_back() {
        let (document, record) =
            installed("[mcp_servers.nasiko]\nurl = \"https://user.example\"\n");
        assert!(record.original_mcp_server.present);
        assert_eq!(
            document["mcp_servers"][MCP_SERVER_NAME]["url"].as_str(),
            Some("https://cp.example/api/mcp/s/ngt_secret")
        );
    }

    #[test]
    fn a_server_changed_since_connect_is_left_alone() {
        let (mut document, record) = installed("other = 1\n");
        document["mcp_servers"][MCP_SERVER_NAME]["url"] = toml_edit::value("https://changed");
        let rendered = document.to_string();
        let mut reparsed = rendered.parse::<DocumentMut>().unwrap();
        restore(&mut reparsed, &record).unwrap();
        assert_eq!(reparsed.to_string(), rendered);
    }

    #[test]
    fn an_inline_or_scalar_mcp_servers_value_is_refused() {
        for original in [
            "mcp_servers = { other = { url = \"https://other.example\" } }\n",
            "mcp_servers = \"not a table\"\n",
        ] {
            let document = original.parse::<DocumentMut>().unwrap();
            let error = record_for("/tmp/c.toml".into(), false, &document, &credential())
                .unwrap_err()
                .to_string();
            assert!(error.contains("is not a [mcp_servers] table"), "{error}");
        }
    }

    #[test]
    fn the_token_is_read_back_only_from_the_nasiko_server() {
        let document = "[mcp_servers.other]\nurl = \"https://x/api/mcp/s/ngt_other\"\n"
            .parse::<DocumentMut>()
            .unwrap();
        assert_eq!(token_in(&document), None);
        let document = "[mcp_servers.nasiko]\nurl = \"https://user.example\"\n"
            .parse::<DocumentMut>()
            .unwrap();
        assert_eq!(token_in(&document).as_deref(), Some(""));
    }

    fn write(dir: &Path, content: &str) -> PathBuf {
        let path = dir.join("config.toml");
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn unregister_deletes_only_a_file_it_created_and_left_empty() {
        let dir = tempfile::tempdir().unwrap();
        let (document, mut record) = installed("");
        let path = write(dir.path(), &document.to_string());
        record.config_path = path.clone();
        record.created_config = true;
        unregister(&record).unwrap();
        assert!(!path.exists());

        let (document, mut record) = installed("");
        let path = write(dir.path(), &document.to_string());
        record.config_path = path.clone();
        record.created_config = false;
        unregister(&record).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
    }

    #[test]
    fn unregister_keeps_a_created_file_that_routing_still_uses() {
        let dir = tempfile::tempdir().unwrap();
        let (mut document, mut record) = installed("");
        document["model"] = toml_edit::value("gpt-5");
        let path = write(dir.path(), &document.to_string());
        record.config_path = path.clone();
        record.created_config = true;
        unregister(&record).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "model = \"gpt-5\"\n");
    }

    #[test]
    fn an_adopted_legacy_server_never_deletes_the_file() {
        let legacy = LegacyMcp {
            binding: crate::commands::coding_agent_router::ConnectionBinding {
                version: coding_agent_router::STATE_VERSION,
                integration_id: Some("codex".into()),
                cluster: "local".into(),
                cluster_url: "https://cp.example".into(),
                principal_id: None,
                agent_id: "agent".into(),
                agent_name: "codex".into(),
                executable: None,
            },
            config_path: "/tmp/config.toml".into(),
            original_mcp_servers_present: true,
            original_mcp_server: SavedItem::default(),
            installed_mcp_server: "saved = 1\n".into(),
        };
        let record = Record::adopted(&legacy);
        assert!(!record.created_config);
        assert!(record.original_mcp_servers_present);
        assert_eq!(record.installed_mcp_server, "saved = 1\n");
    }

    #[test]
    fn the_generated_server_is_accepted_by_codex_when_available() {
        let Ok(codex) = which::which("codex") else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let (document, _) = installed("");
        fs::write(dir.path().join("config.toml"), document.to_string()).unwrap();
        validate_config(&codex, dir.path()).unwrap();
    }
}
