use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::Type, PartialEq)]
#[sqlx(type_name = "varchar")]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum ArtifactType {
    Skill,
    Agent,
    Tool,
}

impl std::fmt::Display for ArtifactType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArtifactType::Skill => write!(f, "skill"),
            ArtifactType::Agent => write!(f, "agent"),
            ArtifactType::Tool => write!(f, "tool"),
        }
    }
}

impl std::str::FromStr for ArtifactType {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "skill" => Ok(ArtifactType::Skill),
            "agent" => Ok(ArtifactType::Agent),
            "tool" => Ok(ArtifactType::Tool),
            other => Err(format!("unknown artifact type: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum ArtifactStatus {
    Preview,
    Stable,
    Verified,
    Yanked,
}

impl std::fmt::Display for ArtifactStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArtifactStatus::Preview => write!(f, "preview"),
            ArtifactStatus::Stable => write!(f, "stable"),
            ArtifactStatus::Verified => write!(f, "verified"),
            ArtifactStatus::Yanked => write!(f, "yanked"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Artifact {
    pub id: Uuid,
    pub owner: String,
    pub name: String,
    pub version: String,
    pub artifact_type: String,
    pub status: String,
    pub description: Option<String>,
    pub metadata: serde_json::Value,
    pub oci_digest: Option<String>,
    pub size_bytes: Option<i64>,
    pub tags: Vec<String>,
    pub framework: Option<String>,
    pub license: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Semantic relevance score (cosine similarity, 0..1) when returned from discovery.
    /// Absent for browse/list results that aren't ranked by a query embedding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sqlx(default)]
    pub score: Option<f32>,
}

#[derive(Debug, Deserialize)]
pub struct PublishRequest {
    pub owner: String,
    pub name: String,
    pub version: String,
    pub artifact_type: String,
    pub description: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
    #[serde(default)]
    pub tags: Vec<String>,
    pub framework: Option<String>,
    pub license: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PublishResponse {
    pub artifact: Artifact,
    pub upload_url: String,
}
