//! Mirrors a seed agent's image from an external OCI registry into the
//! embedded registry (`/v2/*`), so the agent deploys — and every later pull
//! (restart, scale-up, redeploy) is served — from inside the cluster.
//!
//! The server, not a node, talks to the external registry: it resolves the
//! manifest (picking one platform out of a multi-platform index), copies the
//! config and layer blobs it does not already hold, and writes the manifest
//! under `nasiko/<name>:<tag>`. Seeded agents therefore never pull from the
//! external registry, and the credential is only ever sent to the registry it
//! belongs to (including its token service — see [`realm_is_trusted`]).

use std::collections::HashMap;
use std::time::Duration;

use base64::Engine;
use bytes::{BufMut, Bytes, BytesMut};
use futures::StreamExt;
use nasiko_oci::OciState;
use reqwest::{StatusCode, header};

/// Upper bound for one registry request (a blob download included) — agent
/// images are expected well under this; a stalled registry fails the seed
/// instead of hanging it.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Largest manifest accepted from an external registry. Real manifests are a
/// few KiB; the cap stops a hostile registry from exhausting server memory.
const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
/// Largest single blob mirrored. Each blob is buffered in memory before it is
/// written, so this stays well below the server pod's memory limit (and below
/// the embedded registry's own upload cap).
const MAX_MIRROR_BLOB_BYTES: usize = 1024 * 1024 * 1024;

const DOCKER_HUB_API_HOST: &str = "registry-1.docker.io";
const DOCKER_HUB_AUTH_HOST: &str = "auth.docker.io";
const DOCKER_HUB_ALIASES: [&str; 3] = ["docker.io", "index.docker.io", DOCKER_HUB_API_HOST];

const OCI_INDEX: &str = "application/vnd.oci.image.index.v1+json";
const DOCKER_MANIFEST_LIST: &str = "application/vnd.docker.distribution.manifest.list.v2+json";
const MANIFEST_ACCEPT: &str = "application/vnd.oci.image.index.v1+json, \
    application/vnd.docker.distribution.manifest.list.v2+json, \
    application/vnd.oci.image.manifest.v1+json, \
    application/vnd.docker.distribution.manifest.v2+json";

#[derive(Debug, thiserror::Error)]
pub enum MirrorError {
    #[error("invalid image reference `{0}`")]
    InvalidReference(String),
    #[error("registry {host} unreachable: {source}")]
    Unreachable {
        host: String,
        source: reqwest::Error,
    },
    #[error("registry {host} returned {status} for {what}")]
    Status {
        host: String,
        status: StatusCode,
        what: String,
    },
    #[error("registry {host} authentication failed: {reason}")]
    Auth { host: String, reason: String },
    #[error("malformed manifest: {0}")]
    Manifest(String),
    #[error("no `{platform}` image in the index")]
    PlatformNotFound { platform: String },
    #[error("manifest {reference} does not match its digest (got {actual})")]
    DigestMismatch { reference: String, actual: String },
    #[error("{what} exceeds the {limit}-byte seed mirror limit")]
    TooLarge { what: String, limit: usize },
    #[error("embedded registry: {0}")]
    Registry(#[from] nasiko_oci::error::OciError),
}

/// A credential for exactly one external registry.
#[derive(Clone)]
pub struct RegistryCredential {
    /// Bare host; `None` = Docker Hub.
    pub host: Option<String>,
    pub username: String,
    pub password: String,
}

/// Everything a mirror run needs besides the image itself.
pub struct MirrorSettings {
    pub credential: Option<RegistryCredential>,
    /// `os/arch[/variant]` picked out of a multi-platform index.
    pub platform: String,
    /// `AGENT_IMAGE_REGISTRY` — the host internal refs are qualified with.
    pub internal_registry: String,
}

/// A parsed external image reference, addressed the way the registry API needs.
#[derive(Debug, PartialEq, Eq)]
pub struct ImageRef {
    /// Host the `/v2/` API lives on (Docker Hub normalised to its API host).
    pub host: String,
    pub repository: String,
    /// Tag or `sha256:` digest.
    pub reference: String,
}

/// Result of a mirror run.
#[derive(Debug, PartialEq, Eq)]
pub struct MirrorOutcome {
    /// Internal reference to deploy (`<registry>/nasiko/<name>:<tag>`).
    pub internal_ref: String,
    /// The internal tag now points at different content than before this run
    /// (first mirror, or the external tag moved). The internal ref string is
    /// the same either way, so callers must redeploy on this signal.
    pub repointed: bool,
}

/// Copies `image` into the embedded registry as `nasiko/<name>` and returns
/// the internal reference to deploy. Idempotent: an already-mirrored manifest
/// is detected by digest and nothing is copied again.
pub async fn mirror_image(
    oci: &OciState,
    settings: &MirrorSettings,
    image: &str,
    name: &str,
) -> Result<MirrorOutcome, MirrorError> {
    let source = parse_image_ref(image)?;
    let credential = settings
        .credential
        .as_ref()
        .filter(|c| credential_applies(c.host.as_deref(), &source.host))
        .map(|c| (c.username.clone(), c.password.clone()));
    let mut client = RegistryClient::new(&source, credential)?;

    let (media_type, body) = client.resolve_manifest(&settings.platform).await?;
    let repository = format!("nasiko/{}", crate::agents::image_name_slug(name));
    let tag = internal_tag(&source.reference);
    let digest = nasiko_oci::ops::sha256_of(&body);

    let repointed = !already_mirrored(oci, &repository, &tag, &digest).await;
    if repointed {
        for blob in referenced_blobs(&body)? {
            copy_blob(oci, &mut client, &repository, &blob).await?;
        }
        nasiko_oci::ops::put_manifest(oci, &repository, &tag, &media_type, &body).await?;
        tracing::info!(%image, %repository, %tag, %digest, "seed image mirrored");
    } else {
        tracing::info!(%image, %repository, %tag, "seed image already mirrored");
    }
    Ok(MirrorOutcome {
        internal_ref: crate::agents::build_legacy_image_tag(
            &settings.internal_registry,
            name,
            &tag,
        ),
        repointed,
    })
}

/// Parses `[host[:port]/]path[:tag|@digest]` the way `docker pull` does: the
/// first segment is a host only when it has a `.` or `:` or is `localhost`.
pub fn parse_image_ref(image: &str) -> Result<ImageRef, MirrorError> {
    let (name, reference) = split_reference(image.trim());
    let (host, path) = match name.split_once('/') {
        Some((first, rest)) if first.contains(['.', ':']) || first == "localhost" => {
            (first.to_owned(), rest.to_owned())
        }
        _ => ("docker.io".to_owned(), name.to_owned()),
    };
    if path.is_empty() || path.contains("//") || reference.is_empty() {
        return Err(MirrorError::InvalidReference(image.to_owned()));
    }
    let is_docker_hub = DOCKER_HUB_ALIASES.contains(&host.as_str());
    let repository = if is_docker_hub && !path.contains('/') {
        format!("library/{path}")
    } else {
        path
    };
    let host = if is_docker_hub {
        DOCKER_HUB_API_HOST.to_owned()
    } else {
        host
    };
    Ok(ImageRef {
        host,
        repository,
        reference,
    })
}

/// Splits off `@digest` or a `:tag` that follows the last `/` (so a ported
/// host is never mistaken for a tag); no tag means `latest`.
fn split_reference(image: &str) -> (&str, String) {
    if let Some((name, digest)) = image.split_once('@') {
        return (name, digest.to_owned());
    }
    let last_slash = image.rfind('/').map_or(0, |i| i + 1);
    match image[last_slash..].rfind(':') {
        Some(rel) => {
            let at = last_slash + rel;
            (&image[..at], image[at + 1..].to_owned())
        }
        None => (image, "latest".to_owned()),
    }
}

/// Whether a credential configured for `credential_host` may be sent to
/// `image_host` (an already-normalised API host). Never true across hosts.
pub fn credential_applies(credential_host: Option<&str>, image_host: &str) -> bool {
    let normalise = |h: &str| {
        let h = h.trim();
        if h.is_empty() || DOCKER_HUB_ALIASES.contains(&h) {
            DOCKER_HUB_API_HOST.to_owned()
        } else {
            h.to_owned()
        }
    };
    normalise(credential_host.unwrap_or("")) == normalise(image_host)
}

/// A tag for the internal copy: the source tag, or `sha256-<12 hex>` for a
/// digest-pinned source (a tag may not contain `:`).
pub fn internal_tag(reference: &str) -> String {
    match reference.strip_prefix("sha256:") {
        Some(hex) => format!("sha256-{}", &hex[..hex.len().min(12)]),
        None => reference.to_owned(),
    }
}

/// Rejects a manifest fetched by digest whose bytes hash to anything else, so
/// `@sha256:` pins and index-selected platform manifests are actually what was
/// asked for. Tag references have nothing to verify against.
pub fn verify_manifest_digest(reference: &str, body: &[u8]) -> Result<(), MirrorError> {
    if !reference.starts_with("sha256:") {
        return Ok(());
    }
    let actual = nasiko_oci::ops::sha256_of(body);
    if actual == reference {
        Ok(())
    } else {
        Err(MirrorError::DigestMismatch {
            reference: reference.to_owned(),
            actual,
        })
    }
}

/// Whether a Bearer token `realm` (chosen by the registry's challenge) may
/// receive the registry credential. Only an HTTPS realm on the registry's own
/// host qualifies — plus Docker Hub, whose registry and token service live on
/// different hosts — so a malicious registry cannot redirect the credential to
/// a host it controls. Plain HTTP is accepted only for a loopback registry,
/// matching [`scheme_for`].
pub fn realm_is_trusted(realm: &str, registry_host: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(realm) else {
        return false;
    };
    let secure =
        url.scheme() == "https" || (url.scheme() == "http" && scheme_for(registry_host) == "http");
    let Some(realm_host) = url.host_str() else {
        return false;
    };
    // Exact host *and* effective port: another port on the same host can be a
    // different service, while `harbor.corp:443` and `https://harbor.corp/` are
    // the same endpoint.
    let same_host =
        reqwest::Url::parse(&format!("{}://{registry_host}/", scheme_for(registry_host)))
            .is_ok_and(|registry| {
                registry.host_str() == Some(realm_host)
                    && registry.port_or_known_default() == url.port_or_known_default()
            });
    let docker_hub = registry_host == DOCKER_HUB_API_HOST && realm_host == DOCKER_HUB_AUTH_HOST;
    secure && (same_host || docker_hub)
}

/// The config + layer digests a single-platform manifest references.
pub fn referenced_blobs(manifest: &[u8]) -> Result<Vec<String>, MirrorError> {
    let doc = parse_json(manifest)?;
    let digest_of = |v: &serde_json::Value| v["digest"].as_str().map(str::to_owned);
    let config = digest_of(&doc["config"])
        .ok_or_else(|| MirrorError::Manifest("no config digest".into()))?;
    let layers = doc["layers"]
        .as_array()
        .ok_or_else(|| MirrorError::Manifest("no layers".into()))?
        .iter()
        .map(|l| digest_of(l).ok_or_else(|| MirrorError::Manifest("layer without digest".into())))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(std::iter::once(config).chain(layers).collect())
}

/// The digest of the `os/arch[/variant]` entry in a multi-platform index.
pub fn select_platform(index: &[u8], platform: &str) -> Result<String, MirrorError> {
    let doc = parse_json(index)?;
    let mut wanted = platform.split('/');
    let (os, arch, variant) = (wanted.next(), wanted.next(), wanted.next());
    let matches = |m: &&serde_json::Value| {
        let p = &m["platform"];
        p["os"].as_str() == os
            && p["architecture"].as_str() == arch
            && variant.is_none_or(|v| p["variant"].as_str() == Some(v))
    };
    doc["manifests"]
        .as_array()
        .ok_or_else(|| MirrorError::Manifest("index without manifests".into()))?
        .iter()
        .find(matches)
        .and_then(|m| m["digest"].as_str().map(str::to_owned))
        .ok_or_else(|| MirrorError::PlatformNotFound {
            platform: platform.to_owned(),
        })
}

fn parse_json(body: &[u8]) -> Result<serde_json::Value, MirrorError> {
    serde_json::from_slice(body).map_err(|e| MirrorError::Manifest(e.to_string()))
}

async fn already_mirrored(oci: &OciState, repository: &str, tag: &str, digest: &str) -> bool {
    matches!(
        nasiko_oci::ops::get_manifest(oci, repository, tag).await,
        Ok(existing) if existing.digest == digest
    )
}

async fn copy_blob(
    oci: &OciState,
    client: &mut RegistryClient,
    repository: &str,
    digest: &str,
) -> Result<(), MirrorError> {
    if nasiko_oci::ops::blob_linked(oci, repository, digest).await? {
        return Ok(());
    }
    let data = client.fetch_blob(digest).await?;
    // Verifies `data` hashes to `digest` before storing it.
    nasiko_oci::ops::upload_blob_monolithic(oci, repository, data, digest).await?;
    Ok(())
}

/// Minimal OCI distribution client: anonymous first, then whatever the
/// registry's `WWW-Authenticate` challenge asks for (Bearer token or Basic).
struct RegistryClient {
    http: reqwest::Client,
    base: String,
    host: String,
    repository: String,
    reference: String,
    credential: Option<(String, String)>,
    authorization: Option<String>,
}

impl RegistryClient {
    fn new(source: &ImageRef, credential: Option<(String, String)>) -> Result<Self, MirrorError> {
        // reqwest drops `Authorization` on a cross-host redirect, so following
        // a blob redirect to a CDN/object store never leaks the credential.
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| MirrorError::Unreachable {
                host: source.host.clone(),
                source: e,
            })?;
        Ok(Self {
            http,
            base: format!("{}://{}", scheme_for(&source.host), source.host),
            host: source.host.clone(),
            repository: source.repository.clone(),
            reference: source.reference.clone(),
            credential,
            authorization: None,
        })
    }

    /// The single-platform manifest for `platform` and its media type.
    async fn resolve_manifest(&mut self, platform: &str) -> Result<(String, Bytes), MirrorError> {
        let reference = self.reference.clone();
        let (media_type, body) = self.fetch_manifest(&reference).await?;
        if media_type != OCI_INDEX && media_type != DOCKER_MANIFEST_LIST {
            return Ok((media_type, body));
        }
        let digest = select_platform(&body, platform)?;
        self.fetch_manifest(&digest).await
    }

    async fn fetch_manifest(&mut self, reference: &str) -> Result<(String, Bytes), MirrorError> {
        let path = format!("/v2/{}/manifests/{reference}", self.repository);
        let response = self.get(&path, Some(MANIFEST_ACCEPT)).await?;
        let media_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.split(';').next().unwrap_or(v).trim().to_owned())
            .unwrap_or_default();
        let body = self
            .read_bounded(
                response,
                MAX_MANIFEST_BYTES,
                &format!("manifest {reference}"),
            )
            .await?;
        verify_manifest_digest(reference, &body)?;
        Ok((media_type, body))
    }

    async fn fetch_blob(&mut self, digest: &str) -> Result<Bytes, MirrorError> {
        let path = format!("/v2/{}/blobs/{digest}", self.repository);
        let response = self.get(&path, None).await?;
        self.read_bounded(response, MAX_MIRROR_BLOB_BYTES, &format!("blob {digest}"))
            .await
    }

    /// Reads a response body, failing as soon as it grows past `limit` —
    /// never trusting `Content-Length` to stay honest.
    async fn read_bounded(
        &self,
        response: reqwest::Response,
        limit: usize,
        what: &str,
    ) -> Result<Bytes, MirrorError> {
        let too_large = || MirrorError::TooLarge {
            what: what.to_owned(),
            limit,
        };
        if response
            .content_length()
            .is_some_and(|len| len > limit as u64)
        {
            return Err(too_large());
        }
        let mut buf = BytesMut::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| self.unreachable(e))?;
            if buf.len() + chunk.len() > limit {
                return Err(too_large());
            }
            buf.put(chunk);
        }
        Ok(buf.freeze())
    }

    /// GET with the current authorization; one challenge-driven retry on 401.
    async fn get(
        &mut self,
        path: &str,
        accept: Option<&str>,
    ) -> Result<reqwest::Response, MirrorError> {
        let mut response = self.send(path, accept).await?;
        if response.status() == StatusCode::UNAUTHORIZED && self.authorization.is_none() {
            let challenge = response
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            self.authorization = Some(self.answer_challenge(&challenge).await?);
            response = self.send(path, accept).await?;
        }
        if !response.status().is_success() {
            return Err(MirrorError::Status {
                host: self.host.clone(),
                status: response.status(),
                what: path.to_owned(),
            });
        }
        Ok(response)
    }

    async fn send(
        &self,
        path: &str,
        accept: Option<&str>,
    ) -> Result<reqwest::Response, MirrorError> {
        let mut request = self.http.get(format!("{}{path}", self.base));
        if let Some(accept) = accept {
            request = request.header(header::ACCEPT, accept);
        }
        if let Some(auth) = &self.authorization {
            request = request.header(header::AUTHORIZATION, auth);
        }
        request.send().await.map_err(|e| self.unreachable(e))
    }

    /// Turns a `WWW-Authenticate` challenge into an `Authorization` value.
    async fn answer_challenge(&self, challenge: &str) -> Result<String, MirrorError> {
        let (scheme, params) = parse_challenge(challenge)
            .ok_or_else(|| self.auth_error("no usable WWW-Authenticate challenge"))?;
        if scheme.eq_ignore_ascii_case("basic") {
            let (user, pass) = self
                .credential
                .as_ref()
                .ok_or_else(|| self.auth_error("registry requires credentials"))?;
            return Ok(format!("Basic {}", basic_token(user, pass)));
        }
        let realm = params
            .get("realm")
            .ok_or_else(|| self.auth_error("Bearer challenge without realm"))?;
        self.fetch_bearer_token(realm, params.get("service")).await
    }

    async fn fetch_bearer_token(
        &self,
        realm: &str,
        service: Option<&String>,
    ) -> Result<String, MirrorError> {
        let mut query = vec![("scope", format!("repository:{}:pull", self.repository))];
        if let Some(service) = service {
            query.push(("service", service.clone()));
        }
        let mut request = self.http.get(realm).query(&query);
        if let Some((user, pass)) = &self.credential {
            if !realm_is_trusted(realm, &self.host) {
                let reason =
                    format!("refusing to send credentials to untrusted token realm `{realm}`");
                return Err(self.auth_error(&reason));
            }
            request = request.basic_auth(user, Some(pass));
        }
        let response = request.send().await.map_err(|e| self.unreachable(e))?;
        if !response.status().is_success() {
            let reason = format!("token endpoint returned {}", response.status());
            return Err(self.auth_error(&reason));
        }
        let body: serde_json::Value = response.json().await.map_err(|e| self.unreachable(e))?;
        body["token"]
            .as_str()
            .or_else(|| body["access_token"].as_str())
            .map(|t| format!("Bearer {t}"))
            .ok_or_else(|| self.auth_error("token endpoint returned no token"))
    }

    fn unreachable(&self, source: reqwest::Error) -> MirrorError {
        MirrorError::Unreachable {
            host: self.host.clone(),
            source,
        }
    }

    fn auth_error(&self, reason: &str) -> MirrorError {
        MirrorError::Auth {
            host: self.host.clone(),
            reason: reason.to_owned(),
        }
    }
}

/// Parses `Bearer realm="...",service="...",scope="..."` (or `Basic realm=...`)
/// into the scheme and its lowercase-keyed parameters.
pub fn parse_challenge(header: &str) -> Option<(String, HashMap<String, String>)> {
    let header = header.trim();
    let (scheme, rest) = header.split_once(' ').unwrap_or((header, ""));
    if scheme.is_empty() {
        return None;
    }
    let params = rest
        .split(',')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| {
            (
                k.trim().to_ascii_lowercase(),
                v.trim().trim_matches('"').to_owned(),
            )
        })
        .collect();
    Some((scheme.to_owned(), params))
}

fn basic_token(user: &str, pass: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"))
}

/// Plain HTTP only for a loopback registry (as `docker pull` does by
/// default); everything else is HTTPS.
fn scheme_for(host: &str) -> &'static str {
    let bare = host.rsplit_once(':').map_or(host, |(h, _)| h);
    if bare == "localhost" || bare == "127.0.0.1" {
        "http"
    } else {
        "https"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(image: &str) -> ImageRef {
        parse_image_ref(image).expect("valid reference")
    }

    #[test]
    fn docker_hub_short_name_gets_library_namespace_and_api_host() {
        assert_eq!(
            parsed("nginx"),
            ImageRef {
                host: "registry-1.docker.io".into(),
                repository: "library/nginx".into(),
                reference: "latest".into(),
            }
        );
    }

    #[test]
    fn docker_hub_user_repo_keeps_its_namespace() {
        let r = parsed("example/hr-agent:v2");
        assert_eq!(r.host, "registry-1.docker.io");
        assert_eq!(r.repository, "example/hr-agent");
        assert_eq!(r.reference, "v2");
    }

    #[test]
    fn explicit_docker_io_host_is_normalised() {
        assert_eq!(
            parsed("docker.io/library/redis:7").host,
            "registry-1.docker.io"
        );
    }

    #[test]
    fn external_host_with_nested_path() {
        let r = parsed("us-central1-docker.pkg.dev/proj/agents/hr:v1");
        assert_eq!(r.host, "us-central1-docker.pkg.dev");
        assert_eq!(r.repository, "proj/agents/hr");
        assert_eq!(r.reference, "v1");
    }

    #[test]
    fn ported_host_is_not_mistaken_for_a_tag() {
        let r = parsed("harbor.corp:5000/team/agent");
        assert_eq!(r.host, "harbor.corp:5000");
        assert_eq!(r.repository, "team/agent");
        assert_eq!(r.reference, "latest");
    }

    #[test]
    fn digest_reference_is_kept_whole() {
        let r = parsed("ghcr.io/org/hr@sha256:abcdef0123456789");
        assert_eq!(r.repository, "org/hr");
        assert_eq!(r.reference, "sha256:abcdef0123456789");
    }

    #[test]
    fn empty_or_malformed_reference_is_rejected() {
        for bad in ["", "ghcr.io/", "ghcr.io/org//x", "ghcr.io/org/x:"] {
            assert!(parse_image_ref(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn credential_is_only_sent_to_its_own_host() {
        assert!(credential_applies(Some("ghcr.io"), "ghcr.io"));
        assert!(!credential_applies(Some("ghcr.io"), "registry-1.docker.io"));
        assert!(!credential_applies(Some("ghcr.io"), "evil.example.com"));
        assert!(!credential_applies(None, "ghcr.io"));
    }

    #[test]
    fn unset_or_docker_hub_alias_credential_host_means_docker_hub() {
        for host in [None, Some(""), Some("docker.io"), Some("index.docker.io")] {
            assert!(credential_applies(host, "registry-1.docker.io"), "{host:?}");
        }
    }

    #[test]
    fn digest_pinned_source_gets_a_valid_internal_tag() {
        assert_eq!(internal_tag("v1.2.3"), "v1.2.3");
        assert_eq!(
            internal_tag("sha256:0123456789abcdef0123"),
            "sha256-0123456789ab"
        );
    }

    #[test]
    fn bearer_challenge_parameters_are_parsed() {
        let (scheme, params) = parse_challenge(
            r#"Bearer realm="https://auth.docker.io/token",service="registry.docker.io",scope="repository:library/nginx:pull""#,
        )
        .unwrap();
        assert_eq!(scheme, "Bearer");
        assert_eq!(params["realm"], "https://auth.docker.io/token");
        assert_eq!(params["service"], "registry.docker.io");
    }

    #[test]
    fn basic_challenge_is_recognised() {
        let (scheme, params) = parse_challenge(r#"Basic realm="Harbor""#).unwrap();
        assert_eq!(scheme, "Basic");
        assert_eq!(params["realm"], "Harbor");
        assert!(parse_challenge("").is_none());
    }

    fn index() -> Vec<u8> {
        serde_json::json!({
            "manifests": [
                {"digest": "sha256:arm", "platform": {"os": "linux", "architecture": "arm64", "variant": "v8"}},
                {"digest": "sha256:amd", "platform": {"os": "linux", "architecture": "amd64"}},
            ]
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn platform_is_selected_from_an_index() {
        assert_eq!(
            select_platform(&index(), "linux/amd64").unwrap(),
            "sha256:amd"
        );
        assert_eq!(
            select_platform(&index(), "linux/arm64").unwrap(),
            "sha256:arm"
        );
        assert_eq!(
            select_platform(&index(), "linux/arm64/v8").unwrap(),
            "sha256:arm"
        );
    }

    #[test]
    fn missing_platform_is_an_error_not_a_wrong_pick() {
        assert!(matches!(
            select_platform(&index(), "windows/amd64"),
            Err(MirrorError::PlatformNotFound { .. })
        ));
    }

    #[test]
    fn manifest_blobs_are_config_then_layers() {
        let manifest = serde_json::json!({
            "config": {"digest": "sha256:cfg"},
            "layers": [{"digest": "sha256:l1"}, {"digest": "sha256:l2"}],
        })
        .to_string();
        assert_eq!(
            referenced_blobs(manifest.as_bytes()).unwrap(),
            vec!["sha256:cfg", "sha256:l1", "sha256:l2"]
        );
    }

    #[test]
    fn manifest_without_config_or_layers_is_rejected() {
        assert!(referenced_blobs(br#"{"layers": []}"#).is_err());
        assert!(referenced_blobs(br#"{"config": {"digest": "sha256:c"}}"#).is_err());
        assert!(referenced_blobs(b"not json").is_err());
    }

    #[test]
    fn credentials_go_only_to_a_token_realm_on_the_registry_host() {
        assert!(realm_is_trusted(
            "https://us-central1-docker.pkg.dev/v2/token",
            "us-central1-docker.pkg.dev"
        ));
        assert!(realm_is_trusted("https://ghcr.io/token", "ghcr.io"));
        assert!(realm_is_trusted(
            "https://harbor.corp:5000/service/token",
            "harbor.corp:5000"
        ));
    }

    #[test]
    fn docker_hub_token_service_is_the_one_cross_host_exception() {
        assert!(realm_is_trusted(
            "https://auth.docker.io/token",
            "registry-1.docker.io"
        ));
        assert!(!realm_is_trusted("https://auth.docker.io/token", "ghcr.io"));
    }

    #[test]
    fn foreign_or_insecure_token_realms_are_refused() {
        for realm in [
            "https://attacker.example/token",
            "https://us-central1-docker.pkg.dev.attacker.example/token",
            "http://us-central1-docker.pkg.dev/v2/token",
            "not a url",
        ] {
            assert!(
                !realm_is_trusted(realm, "us-central1-docker.pkg.dev"),
                "{realm}"
            );
        }
    }

    #[test]
    fn loopback_registries_may_use_a_plain_http_realm() {
        assert!(realm_is_trusted(
            "http://127.0.0.1:5000/token",
            "127.0.0.1:5000"
        ));
        assert!(!realm_is_trusted(
            "http://127.0.0.1:6000/token",
            "harbor.corp"
        ));
    }

    #[test]
    fn same_host_on_another_port_is_not_trusted() {
        assert!(!realm_is_trusted(
            "http://127.0.0.1:6000/token",
            "127.0.0.1:5000"
        ));
        assert!(!realm_is_trusted(
            "https://harbor.corp:6000/token",
            "harbor.corp:5000"
        ));
        assert!(!realm_is_trusted("https://ghcr.io:8443/token", "ghcr.io"));
        assert!(realm_is_trusted("https://ghcr.io:443/token", "ghcr.io"));
    }

    fn response(body: Vec<u8>) -> reqwest::Response {
        reqwest::Response::from(axum::http::Response::new(body))
    }

    #[tokio::test]
    async fn bodies_past_the_limit_are_refused() {
        let client = RegistryClient::new(&parsed("ghcr.io/org/app:v1"), None).unwrap();

        let within = client
            .read_bounded(response(vec![0; 16]), 16, "manifest v1")
            .await;
        let over = client
            .read_bounded(response(vec![0; 17]), 16, "manifest v1")
            .await;

        assert_eq!(within.unwrap().len(), 16);
        assert!(matches!(over, Err(MirrorError::TooLarge { limit: 16, .. })));
    }

    #[test]
    fn explicit_default_port_on_the_registry_host_still_matches() {
        assert!(realm_is_trusted(
            "https://harbor.corp/token",
            "harbor.corp:443"
        ));
        assert!(realm_is_trusted(
            "https://harbor.corp:443/token",
            "harbor.corp:443"
        ));
        assert!(!realm_is_trusted(
            "https://harbor.corp:8443/token",
            "harbor.corp:443"
        ));
    }

    #[test]
    fn digest_referenced_manifest_must_hash_to_that_digest() {
        let body = br#"{"schemaVersion":2}"#;
        let digest = nasiko_oci::ops::sha256_of(body);
        assert!(verify_manifest_digest(&digest, body).is_ok());
        assert!(matches!(
            verify_manifest_digest(&digest, br#"{"schemaVersion":3}"#),
            Err(MirrorError::DigestMismatch { .. })
        ));
    }

    #[test]
    fn tag_referenced_manifest_has_nothing_to_verify() {
        assert!(verify_manifest_digest("v1.2.3", b"anything").is_ok());
    }

    #[test]
    fn only_loopback_registries_use_plain_http() {
        assert_eq!(scheme_for("localhost:5000"), "http");
        assert_eq!(scheme_for("127.0.0.1:8080"), "http");
        assert_eq!(scheme_for("ghcr.io"), "https");
        assert_eq!(scheme_for("harbor.corp:5000"), "https");
    }
}
