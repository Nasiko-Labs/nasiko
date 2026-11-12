//! Application Default Credentials (ADC) token source for Vertex AI.
//!
//! Resolution order, as Google's client libraries do it:
//! 1. `GOOGLE_APPLICATION_CREDENTIALS` file path,
//! 2. `~/.config/gcloud/application_default_credentials.json`,
//! 3. the GCE/GKE metadata server.
//!
//! The file is either an `authorized_user` (refresh token) or a `service_account`
//! (RS256-signed JWT exchanged for an access token). Tokens are cached and refreshed
//! a minute before expiry. Parsing and request building are pure and unit-tested; only
//! [`TokenSource::token`] does IO.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::ProviderError;

const SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const DEFAULT_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
const METADATA_URL: &str =
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";
const REFRESH_MARGIN: Duration = Duration::from_secs(60);

/// What an ADC JSON file contains.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum Credentials {
    #[serde(rename = "authorized_user")]
    AuthorizedUser {
        client_id: String,
        client_secret: String,
        refresh_token: String,
    },
    #[serde(rename = "service_account")]
    ServiceAccount {
        client_email: String,
        private_key: String,
        token_uri: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: Option<u64>,
}

struct Cached {
    token: String,
    expires_at: Instant,
}

/// Where tokens come from.
enum Origin {
    File(Credentials),
    Metadata,
}

pub struct TokenSource {
    http: reqwest::Client,
    origin: Origin,
    cached: Mutex<Option<Cached>>,
}

impl TokenSource {
    /// Locate ADC and build a token source. No network call happens here.
    pub fn discover(http: reqwest::Client) -> Result<Self, ProviderError> {
        let origin: Origin = match credentials_path() {
            Some(path) if path.exists() => {
                let text: String = std::fs::read_to_string(&path).map_err(|e| {
                    ProviderError::Transport(format!("read ADC file {}: {e}", path.display()))
                })?;
                Origin::File(parse_credentials(&text)?)
            }
            _ => Origin::Metadata,
        };
        Ok(Self {
            http,
            origin,
            cached: Mutex::new(None),
        })
    }

    /// A valid access token, refreshed when within a minute of expiry.
    pub async fn token(&self) -> Result<String, ProviderError> {
        if let Some(token) = self.fresh_cached() {
            return Ok(token);
        }
        let response: TokenResponse = self.fetch().await?;
        let ttl = Duration::from_secs(response.expires_in.unwrap_or(3600));
        let mut guard = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(Cached {
            token: response.access_token.clone(),
            expires_at: Instant::now() + ttl,
        });
        Ok(response.access_token)
    }

    fn fresh_cached(&self) -> Option<String> {
        let guard = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .as_ref()
            .filter(|c| c.expires_at > Instant::now() + REFRESH_MARGIN)
            .map(|c| c.token.clone())
    }

    async fn fetch(&self) -> Result<TokenResponse, ProviderError> {
        let request: reqwest::RequestBuilder = match &self.origin {
            Origin::Metadata => self.http.get(METADATA_URL).header("Metadata-Flavor", "Google"),
            Origin::File(creds) => {
                let (uri, body): (String, String) = token_request(creds, now_secs())?;
                self.http
                    .post(uri)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(body)
            }
        };
        let resp: reqwest::Response = request
            .send()
            .await
            .map_err(|e| ProviderError::Transport(format!("ADC token request: {e}")))?;
        let status = resp.status();
        let text: String = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(ProviderError::Status {
                status: status.as_u16(),
                message: format!("ADC token request failed: {text}"),
                retryable: status.is_server_error(),
            });
        }
        serde_json::from_str(&text)
            .map_err(|e| ProviderError::Parse(format!("ADC token response: {e}")))
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn credentials_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("GOOGLE_APPLICATION_CREDENTIALS") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    let home: String = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".config/gcloud/application_default_credentials.json"))
}

pub fn parse_credentials(text: &str) -> Result<Credentials, ProviderError> {
    serde_json::from_str(text)
        .map_err(|e| ProviderError::Parse(format!("unsupported ADC credentials file: {e}")))
}

/// Token endpoint URL and form body for `creds`. Pure: `now` is injected.
pub fn token_request(creds: &Credentials, now: u64) -> Result<(String, String), ProviderError> {
    match creds {
        Credentials::AuthorizedUser {
            client_id,
            client_secret,
            refresh_token,
        } => Ok((
            DEFAULT_TOKEN_URI.to_string(),
            form(&[
                ("grant_type", "refresh_token"),
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("refresh_token", refresh_token),
            ]),
        )),
        Credentials::ServiceAccount {
            client_email,
            private_key,
            token_uri,
        } => {
            let uri: String = token_uri
                .clone()
                .unwrap_or_else(|| DEFAULT_TOKEN_URI.to_string());
            let claims = serde_json::json!({
                "iss": client_email,
                "scope": SCOPE,
                "aud": uri,
                "iat": now,
                "exp": now + 3600,
            });
            let key = jsonwebtoken::EncodingKey::from_rsa_pem(private_key.as_bytes())
                .map_err(|e| ProviderError::Parse(format!("service account key: {e}")))?;
            let assertion: String = jsonwebtoken::encode(
                &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
                &claims,
                &key,
            )
            .map_err(|e| ProviderError::Parse(format!("sign service account JWT: {e}")))?;
            let body: String = form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &assertion),
            ]);
            Ok((uri, body))
        }
    }
}

fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect::<Vec<String>>()
        .join("&")
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_authorized_user() {
        let c = parse_credentials(
            r#"{"type":"authorized_user","client_id":"i","client_secret":"s","refresh_token":"r/1"}"#,
        )
        .unwrap();
        let (uri, body) = token_request(&c, 0).unwrap();
        assert_eq!(uri, DEFAULT_TOKEN_URI);
        assert_eq!(
            body,
            "grant_type=refresh_token&client_id=i&client_secret=s&refresh_token=r%2F1"
        );
    }

    #[test]
    fn parses_service_account_without_token_uri() {
        let c = parse_credentials(
            r#"{"type":"service_account","client_email":"a@b","private_key":"k"}"#,
        )
        .unwrap();
        assert!(matches!(c, Credentials::ServiceAccount { token_uri: None, .. }));
    }

    #[test]
    fn rejects_unknown_type() {
        assert!(parse_credentials(r#"{"type":"external_account"}"#).is_err());
    }

    #[test]
    fn bad_private_key_is_parse_error() {
        let c = Credentials::ServiceAccount {
            client_email: "a@b".into(),
            private_key: "not a pem".into(),
            token_uri: None,
        };
        assert!(matches!(token_request(&c, 0), Err(ProviderError::Parse(_))));
    }

    #[test]
    fn percent_encodes_reserved() {
        assert_eq!(percent_encode("a b/c=d"), "a%20b%2Fc%3Dd");
    }
}
