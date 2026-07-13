use serde::Deserialize;

use crate::error::OidcError;

#[derive(Debug, Clone, Deserialize)]
pub struct Jwk {
    pub kty: String,
    #[serde(default)]
    pub kid: Option<String>,
    #[serde(default)]
    pub n: Option<String>,
    #[serde(default)]
    pub e: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Jwks {
    pub keys: Vec<Jwk>,
}

impl Jwks {
    /// Find an RSA key by `kid`. Returns `None` for non-RSA keys too — this
    /// client only supports RS256, so a matching `kid` on e.g. an EC key is
    /// still "no usable key".
    pub fn find_rsa(&self, kid: &str) -> Option<&Jwk> {
        self.keys
            .iter()
            .find(|k| k.kty == "RSA" && k.kid.as_deref() == Some(kid))
    }
}

pub(crate) async fn fetch_jwks(http: &reqwest::Client, jwks_uri: &str) -> Result<Jwks, OidcError> {
    let resp = http
        .get(jwks_uri)
        .send()
        .await
        .map_err(|e| OidcError::Jwks(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(OidcError::Jwks(format!("{} returned {}", jwks_uri, resp.status())));
    }
    resp.json::<Jwks>().await.map_err(|e| OidcError::Jwks(e.to_string()))
}
