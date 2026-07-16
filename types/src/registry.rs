use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub owner: String,
    pub name: String,
    pub version: String,
    pub artifact_type: String,
    pub status: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
    #[serde(default)]
    pub oci_digest: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<i64>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub framework: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    /// Semantic relevance score (cosine similarity, 0..1) on discovery results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishRequest {
    pub owner: String,
    pub name: String,
    pub version: String,
    pub artifact_type: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub framework: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishResponse {
    pub artifact: Artifact,
    pub upload_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub data: Vec<Artifact>,
    #[serde(default)]
    pub total: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactResponse {
    pub data: Artifact,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionsResponse {
    pub data: Vec<Artifact>,
}
