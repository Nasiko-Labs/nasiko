use serde::{Deserialize, Serialize};

use super::artifact::Artifact;

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    pub q: Option<String>,
    #[serde(rename = "type")]
    pub artifact_type: Option<String>,
    pub tags: Option<String>,
    pub framework: Option<String>,
    pub status: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    20
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub items: Vec<Artifact>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}
