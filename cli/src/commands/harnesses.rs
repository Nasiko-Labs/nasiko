//! `nasiko harnesses`: the coding-agent harnesses a session can run on.

use anyhow::Result;
use serde::Deserialize;
use tabled::settings::{Alignment, Style};
use tabled::{Table, Tabled};

use crate::api::Client;

/// One entry of `GET /api/harnesses` (`oss/server/src/sessions/routes.rs::HarnessView`).
#[derive(Debug, Deserialize)]
pub struct Harness {
    pub id: String,
    pub name: String,
    pub image: String,
    pub protocol_version: i32,
    pub enabled: bool,
}

/// A paged envelope: `{"data": [...], "has_more": ..}`.
#[derive(Debug, Deserialize)]
struct Page {
    data: Vec<Harness>,
}

pub fn list() -> Result<()> {
    let client = Client::from_active_cluster()?;
    let page: Page = client.get_json("/harnesses")?;
    if page.data.is_empty() {
        println!("No harnesses available.");
        return Ok(());
    }
    print!("{}", render_table(&page.data));
    Ok(())
}

/// One table line.
#[derive(Tabled)]
struct HarnessRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "NAME")]
    name: String,
    #[tabled(rename = "IMAGE")]
    image: String,
    /// ACP protocol major version.
    #[tabled(rename = "ACP")]
    acp: i32,
    #[tabled(rename = "ENABLED")]
    enabled: &'static str,
}

fn render_table(harnesses: &[Harness]) -> String {
    let rows: Vec<HarnessRow> = harnesses
        .iter()
        .map(|h| HarnessRow {
            id: h.id.clone(),
            name: h.name.clone(),
            image: h.image.clone(),
            acp: h.protocol_version,
            enabled: if h.enabled { "yes" } else { "no" },
        })
        .collect();
    format!(
        "{}\n",
        Table::new(rows)
            .with(Style::blank())
            .with(Alignment::left())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_server_page_and_renders_a_row() {
        let body = r#"{"data":[{"id":"opencode","name":"OpenCode","image":"img:1",
            "protocol_version":1,"enabled":true}],
            "has_more":false,"next_cursor":null,"prev_cursor":null}"#;
        let page: Page = serde_json::from_str(body).unwrap();
        let table = render_table(&page.data);
        assert!(table.contains("ACP"));
        assert!(table.contains("opencode"));
        assert!(table.contains("yes"));
    }
}
