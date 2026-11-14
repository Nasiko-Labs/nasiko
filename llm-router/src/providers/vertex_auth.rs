//! Application Default Credentials (ADC) token source for Vertex AI.
//!
//! Resolution order:
//! 1. `VERTEX_CREDENTIALS_JSON` — the complete ADC JSON as the env value (not a path),
//! 2. the GCE/GKE metadata server.
//!
//! The JSON is either an `authorized_user` (refresh token) or a `service_account`
//! (RS256-signed JWT exchanged for an access token). Tokens are cached and refreshed
//! a minute before expiry. Parsing and request building are pure and unit-tested; only
//! [`TokenSource::token`] does IO.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::ProviderError;

const SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const DEFAULT_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
const METADATA_URL: &str =
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";
const REFRESH_MARGIN: Duration = Duration::from_secs(60);
/// Env var holding the full ADC JSON document.
pub const CREDENTIALS_ENV: &str = "VERTEX_CREDENTIALS_JSON";

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
        let raw: Option<String> = read_adc_text()?;
        let origin: Origin = origin_from(raw.as_deref())?;
        Ok(Self {
            http,
            origin,
            cached: Mutex::new(None),
        })
    }

    /// Build from inline ADC JSON text (same parsing as `VERTEX_CREDENTIALS_JSON`).
    pub fn from_credentials_json(http: reqwest::Client, text: &str) -> Result<Self, ProviderError> {
        Ok(Self {
            http,
            origin: origin_from(Some(text))?,
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
            Origin::Metadata => self
                .http
                .get(METADATA_URL)
                .header("Metadata-Flavor", "Google"),
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

/// ADC text in documented order: inline `VERTEX_CREDENTIALS_JSON`, the file named by
/// `GOOGLE_APPLICATION_CREDENTIALS` (must exist), then gcloud's well-known file.
/// `None` means use the metadata server.
fn read_adc_text() -> Result<Option<String>, ProviderError> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    if let Some(inline) = env(CREDENTIALS_ENV) {
        return Ok(Some(inline));
    }
    let home: Option<String> = env("HOME");
    if let Some(path) = env("GOOGLE_APPLICATION_CREDENTIALS") {
        let path: String = expand_home(&path, home.as_deref());
        return std::fs::read_to_string(&path).map(Some).map_err(|e| {
            ProviderError::Parse(format!("GOOGLE_APPLICATION_CREDENTIALS={path}: {e}"))
        });
    }
    let well_known: Option<String> =
        well_known_adc_path(env("CLOUDSDK_CONFIG").as_deref(), home.as_deref());
    Ok(well_known.and_then(|p| std::fs::read_to_string(p).ok()))
}

/// Pure: gcloud ADC path (`$CLOUDSDK_CONFIG` or `~/.config/gcloud`).
fn well_known_adc_path(cloudsdk_config: Option<&str>, home: Option<&str>) -> Option<String> {
    let dir: String = match cloudsdk_config {
        Some(d) => d.trim_end_matches('/').to_string(),
        None => format!("{}/.config/gcloud", home?.trim_end_matches('/')),
    };
    Some(format!("{dir}/application_default_credentials.json"))
}

/// Pure: expand a leading `~/` (the env example documents that form).
fn expand_home(path: &str, home: Option<&str>) -> String {
    match (path.strip_prefix("~/"), home) {
        (Some(rest), Some(h)) => format!("{}/{rest}", h.trim_end_matches('/')),
        _ => path.to_string(),
    }
}

#[cfg(test)]
mod adc_path_tests {
    use super::*;

    #[test]
    fn well_known_path_prefers_cloudsdk_config() {
        assert_eq!(
            well_known_adc_path(Some("/c/"), Some("/h")).as_deref(),
            Some("/c/application_default_credentials.json")
        );
        assert_eq!(
            well_known_adc_path(None, Some("/h")).as_deref(),
            Some("/h/.config/gcloud/application_default_credentials.json")
        );
        assert_eq!(well_known_adc_path(None, None), None);
    }

    #[test]
    fn expand_home_only_expands_tilde_prefix() {
        assert_eq!(expand_home("~/a.json", Some("/h")), "/h/a.json");
        assert_eq!(expand_home("/abs/a.json", Some("/h")), "/abs/a.json");
    }
}

/// Pure: ADC JSON text when present and non-blank, else the metadata server.
fn origin_from(raw: Option<&str>) -> Result<Origin, ProviderError> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(text) => Ok(Origin::File(parse_credentials(text)?)),
        None => Ok(Origin::Metadata),
    }
}

pub fn parse_credentials(text: &str) -> Result<Credentials, ProviderError> {
    serde_json::from_str(text)
        .map_err(|e| ProviderError::Parse(format!("{CREDENTIALS_ENV} is not valid ADC JSON: {e}")))
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
        assert!(matches!(
            c,
            Credentials::ServiceAccount {
                token_uri: None,
                ..
            }
        ));
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
    fn inline_json_selects_credentials() {
        let raw = r#" {"type":"authorized_user","client_id":"i","client_secret":"s","refresh_token":"r"} "#;
        assert!(matches!(origin_from(Some(raw)), Ok(Origin::File(_))));
    }

    #[test]
    fn missing_or_blank_falls_back_to_metadata() {
        assert!(matches!(origin_from(None), Ok(Origin::Metadata)));
        assert!(matches!(origin_from(Some("  ")), Ok(Origin::Metadata)));
    }

    #[test]
    fn path_instead_of_json_is_error() {
        assert!(origin_from(Some("/etc/adc.json")).is_err());
    }

    #[test]
    fn percent_encodes_reserved() {
        assert_eq!(percent_encode("a b/c=d"), "a%20b%2Fc%3Dd");
    }
}
