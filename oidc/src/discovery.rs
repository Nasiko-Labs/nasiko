use serde::Deserialize;

use crate::error::OidcError;

/// Only the fields we actually use — the real document has many more
/// (`response_types_supported`, `scopes_supported`, etc.); unknown fields are
/// silently ignored by serde since this struct doesn't `deny_unknown_fields`.
#[derive(Debug, Clone, Deserialize)]
pub struct DiscoveryDocument {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
}

pub(crate) async fn fetch_discovery(
    http: &reqwest::Client,
    issuer_url: &str,
) -> Result<DiscoveryDocument, OidcError> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        issuer_url.trim_end_matches('/')
    );
    let resp = http
        .get(&url)
        .send()
        .await
        .map_err(|e| OidcError::Discovery(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(OidcError::Discovery(format!(
            "{} returned {}",
            url,
            resp.status()
        )));
    }
    resp.json::<DiscoveryDocument>()
        .await
        .map_err(|e| OidcError::Discovery(e.to_string()))
}
