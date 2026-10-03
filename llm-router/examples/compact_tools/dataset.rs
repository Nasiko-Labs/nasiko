use anyhow::{Context, Result, bail};
use nasiko_tool_compact::{ToolCall, ToolDef};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

#[derive(Deserialize)]
pub(super) struct Dataset {
    pub tools: Vec<ToolDef>,
    #[serde(default)]
    pub cases: Vec<Case>,
    #[serde(default)]
    pub decoder_cases: Vec<DecoderCase>,
}

#[derive(Deserialize)]
pub(super) struct Case {
    pub id: String,
    pub tools: Vec<String>,
    pub messages: Vec<Value>,
    pub expected: Vec<ToolCall>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Deserialize)]
pub(super) struct DecoderCase {
    pub id: String,
    pub tools: Vec<String>,
    pub chunks: Vec<String>,
}

impl Dataset {
    pub fn lookup(&self) -> Result<BTreeMap<&str, &ToolDef>> {
        let mut lookup = BTreeMap::new();
        for tool in &self.tools {
            if lookup.insert(tool.function.name.as_str(), tool).is_some() {
                bail!("duplicate dataset tool name");
            }
        }
        Ok(lookup)
    }
}

pub(super) fn resolve(lookup: &BTreeMap<&str, &ToolDef>, names: &[String]) -> Result<Vec<ToolDef>> {
    names
        .iter()
        .map(|name| {
            lookup
                .get(name.as_str())
                .map(|tool| (*tool).clone())
                .with_context(|| format!("case references missing dataset tool {name}"))
        })
        .collect()
}
