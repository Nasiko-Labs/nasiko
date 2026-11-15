//! Integration tests for mirroring seed images into the embedded registry.
//!
//! A local fake registry demands a Bearer token (issued only for the right
//! Basic credential) and serves a multi-platform index, a single-platform
//! manifest and its blobs. `seed_mirror::mirror_image` must copy exactly the
//! selected platform into a real `OciState` (Postgres + S3), skip the copy on a
//! second run, and never send the credential to a registry it isn't for.
//!
//! Requires infra (Postgres :5432, S3):
//!   cargo test -p nasiko-server --test seed_mirror -- --test-threads=1

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use base64::Engine;
use nasiko_server::seed_mirror::{MirrorError, MirrorSettings, RegistryCredential, mirror_image};
use serial_test::serial;
use sha2::{Digest, Sha256};

const USER: &str = "puller";
const PASS: &str = "s3cret";
const TOKEN: &str = "fake-bearer-token";
const OCI_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";
const OCI_INDEX: &str = "application/vnd.oci.image.index.v1+json";

fn digest(data: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(data)))
}

/// One image: a config blob, one layer, its manifest, and an index listing
/// it as `linux/amd64` next to an `arm64` entry that must never be fetched.
struct FakeImage {
    config: Vec<u8>,
    layer: Vec<u8>,
    manifest: Vec<u8>,
    index: Vec<u8>,
}

impl FakeImage {
    fn new() -> Self {
        let config = br#"{"architecture":"amd64","os":"linux"}"#.to_vec();
        let layer = b"pretend this is a gzipped tar layer".to_vec();
        let manifest = serde_json::json!({
            "schemaVersion": 2,
            "mediaType": OCI_MANIFEST,
            "config": {"mediaType": "application/vnd.oci.image.config.v1+json",
                       "digest": digest(&config), "size": config.len()},
            "layers": [{"mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
                        "digest": digest(&layer), "size": layer.len()}],
        })
        .to_string()
        .into_bytes();
        let index = serde_json::json!({
            "schemaVersion": 2,
            "mediaType": OCI_INDEX,
            "manifests": [
                {"mediaType": OCI_MANIFEST, "digest": "sha256:arm64-never-fetched", "size": 1,
                 "platform": {"os": "linux", "architecture": "arm64"}},
                {"mediaType": OCI_MANIFEST, "digest": digest(&manifest), "size": manifest.len(),
                 "platform": {"os": "linux", "architecture": "amd64"}},
            ],
        })
        .to_string()
        .into_bytes();
        Self {
            config,
            layer,
            manifest,
            index,
        }
    }
}

struct Registry {
    image: FakeImage,
    base: String,
    /// Token endpoint the challenge points at — normally this registry's own
    /// `/token`, overridden to test a hostile realm.
    realm: String,
    blob_fetches: AtomicUsize,
    token_requests: AtomicUsize,
    credentialed_token_requests: AtomicUsize,
}

type Shared = Arc<Registry>;

fn authorized(headers: &HeaderMap) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        == Some(format!("Bearer {TOKEN}").as_str())
}

fn challenge(reg: &Registry) -> Response {
    let value = format!(r#"Bearer realm="{}",service="fake-registry""#, reg.realm);
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, value)],
    )
        .into_response()
}

async fn token(State(reg): State<Shared>, headers: HeaderMap) -> Response {
    reg.token_requests.fetch_add(1, Ordering::SeqCst);
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if presented.is_some() {
        reg.credentialed_token_requests
            .fetch_add(1, Ordering::SeqCst);
    }
    let expected = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{USER}:{PASS}"))
    );
    if presented != Some(expected.as_str()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    axum::Json(serde_json::json!({"token": TOKEN})).into_response()
}

async fn manifest(
    State(reg): State<Shared>,
    Path(reference): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&headers) {
        return challenge(&reg);
    }
    let image = &reg.image;
    let (media_type, body) = if reference == "v1" {
        (OCI_INDEX, image.index.clone())
    } else if reference == digest(&image.manifest) {
        (OCI_MANIFEST, image.manifest.clone())
    } else {
        return StatusCode::NOT_FOUND.into_response();
    };
    ([(header::CONTENT_TYPE, media_type)], body).into_response()
}

async fn blob(
    State(reg): State<Shared>,
    Path(wanted): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&headers) {
        return challenge(&reg);
    }
    reg.blob_fetches.fetch_add(1, Ordering::SeqCst);
    let image = &reg.image;
    match [&image.config, &image.layer]
        .into_iter()
        .find(|b| digest(b) == wanted)
    {
        Some(bytes) => bytes.clone().into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Starts the fake registry on a loopback port (plain HTTP, like `docker`
/// allows for localhost) and returns its shared state.
async fn start_registry() -> Shared {
    start_registry_with_realm(None).await
}

/// Same, but the challenge's token realm can point somewhere else.
async fn start_registry_with_realm(realm: Option<String>) -> Shared {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let reg = Arc::new(Registry {
        image: FakeImage::new(),
        realm: realm.unwrap_or_else(|| format!("{base}/token")),
        base,
        blob_fetches: AtomicUsize::new(0),
        token_requests: AtomicUsize::new(0),
        credentialed_token_requests: AtomicUsize::new(0),
    });
    let app = Router::new()
        .route("/token", get(token))
        .route("/v2/demo/hr-agent/manifests/{reference}", get(manifest))
        .route("/v2/demo/hr-agent/blobs/{digest}", get(blob))
        .with_state(reg.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    reg
}

async fn oci_state(server: &common::TestServer) -> nasiko_oci::OciState {
    let storage = nasiko_oci::storage::S3Storage::from_env("nasiko-test-artifacts".into()).await;
    nasiko_oci::OciState::new(server.db.clone(), Arc::new(storage), String::new())
}

fn settings(credential_host: &str, password: &str) -> MirrorSettings {
    MirrorSettings {
        credential: Some(RegistryCredential {
            host: Some(credential_host.to_owned()),
            username: USER.to_owned(),
            password: password.to_owned(),
        }),
        platform: "linux/amd64".to_owned(),
        internal_registry: String::new(),
    }
}

fn host_of(reg: &Registry) -> String {
    reg.base.trim_start_matches("http://").to_owned()
}

#[tokio::test]
#[serial]
async fn mirrors_the_selected_platform_then_skips_on_the_second_run() {
    let server = common::TestServer::start().await;
    let reg = start_registry().await;
    let oci = oci_state(&server).await;
    let host = host_of(&reg);
    let image = format!("{host}/demo/hr-agent:v1");

    let first = mirror_image(&oci, &settings(&host, PASS), &image, "hr-agent")
        .await
        .expect("first mirror");

    assert_eq!(first.internal_ref, "nasiko/hr-agent:v1");
    assert!(first.repointed, "first mirror points the internal tag");
    let stored = nasiko_oci::ops::get_manifest(&oci, "nasiko/hr-agent", "v1")
        .await
        .expect("manifest stored under nasiko/hr-agent:v1");
    assert_eq!(
        stored.digest,
        digest(&reg.image.manifest),
        "the amd64 manifest, not the index"
    );
    for blob in [&reg.image.config, &reg.image.layer] {
        let linked = nasiko_oci::ops::blob_linked(&oci, "nasiko/hr-agent", &digest(blob))
            .await
            .unwrap();
        assert!(linked, "blob {} must be stored", digest(blob));
    }
    assert_eq!(reg.blob_fetches.load(Ordering::SeqCst), 2);

    let second = mirror_image(&oci, &settings(&host, PASS), &image, "hr-agent")
        .await
        .expect("second mirror");
    assert!(!second.repointed, "unchanged external tag: no redeploy");
    assert_eq!(
        reg.blob_fetches.load(Ordering::SeqCst),
        2,
        "nothing re-copied"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn stores_under_the_same_slug_the_deploy_reference_uses() {
    let server = common::TestServer::start().await;
    let reg = start_registry().await;
    let oci = oci_state(&server).await;
    let host = host_of(&reg);
    let image = format!("{host}/demo/hr-agent:v1");

    let outcome = mirror_image(&oci, &settings(&host, PASS), &image, "HR Agent")
        .await
        .expect("mirror");

    assert_eq!(outcome.internal_ref, "nasiko/hr-agent:v1");
    nasiko_oci::ops::get_manifest(&oci, "nasiko/hr-agent", "v1")
        .await
        .expect("stored under the slug the internal ref points at");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn wrong_credential_fails_with_an_auth_error_and_stores_nothing() {
    let server = common::TestServer::start().await;
    let reg = start_registry().await;
    let oci = oci_state(&server).await;
    let host = host_of(&reg);
    let image = format!("{host}/demo/hr-agent:v1");

    let err = mirror_image(&oci, &settings(&host, "wrong"), &image, "hr-agent")
        .await
        .unwrap_err();

    assert!(matches!(err, MirrorError::Auth { .. }), "got {err}");
    let stored = nasiko_oci::ops::get_manifest(&oci, "nasiko/hr-agent", "v1").await;
    assert!(stored.is_err(), "nothing may be stored on failure");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn credential_for_another_host_is_never_sent() {
    let server = common::TestServer::start().await;
    let reg = start_registry().await;
    let oci = oci_state(&server).await;
    let host = host_of(&reg);
    let image = format!("{host}/demo/hr-agent:v1");

    let err = mirror_image(&oci, &settings("ghcr.io", PASS), &image, "hr-agent")
        .await
        .unwrap_err();

    assert!(matches!(err, MirrorError::Auth { .. }), "got {err}");
    assert_eq!(reg.token_requests.load(Ordering::SeqCst), 1);
    assert_eq!(
        reg.credentialed_token_requests.load(Ordering::SeqCst),
        0,
        "the ghcr.io credential must not be offered to this registry"
    );
    server.cleanup().await;
}

/// A stand-in for a hostile token service: counts every request it receives.
async fn start_attacker() -> (String, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let app = Router::new().route(
        "/token",
        get(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                axum::Json(serde_json::json!({"token": "stolen"}))
            }
        }),
    );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, hits)
}

#[tokio::test]
#[serial]
async fn credential_is_never_sent_to_a_token_realm_on_another_host() {
    let server = common::TestServer::start().await;
    let (attacker, attacker_hits) = start_attacker().await;
    let reg = start_registry_with_realm(Some(format!("{attacker}/token"))).await;
    let oci = oci_state(&server).await;
    let host = host_of(&reg);
    let image = format!("{host}/demo/hr-agent:v1");

    let err = mirror_image(&oci, &settings(&host, PASS), &image, "hr-agent")
        .await
        .unwrap_err();

    assert!(matches!(err, MirrorError::Auth { .. }), "got {err}");
    assert!(
        err.to_string().contains("untrusted token realm"),
        "got {err}"
    );
    assert_eq!(
        attacker_hits.load(Ordering::SeqCst),
        0,
        "the credential must never reach a realm on another host"
    );
    server.cleanup().await;
}

/// Opt-in check against a **real** registry (skipped by default and in CI):
///
/// ```sh
/// SEED_MIRROR_LIVE_IMAGE=<user>/<image>:<tag> \
///   cargo test -p nasiko-server --test seed_mirror -- --ignored live_registry
/// # private registries also take:
/// #   SEED_MIRROR_LIVE_HOST=<bare host, empty = Docker Hub>
/// #   SEED_MIRROR_LIVE_USERNAME=... SEED_MIRROR_LIVE_PASSWORD=...
/// ```
///
/// Exercises what the fake registry cannot: the real token-realm layout of
/// that registry (which `realm_is_trusted` must accept), digest verification
/// of a real multi-platform index, and real blob sizes.
#[tokio::test]
#[serial]
#[ignore = "pulls from a real registry; set SEED_MIRROR_LIVE_IMAGE"]
async fn live_registry_mirror() {
    let image = std::env::var("SEED_MIRROR_LIVE_IMAGE").expect("set SEED_MIRROR_LIVE_IMAGE");
    let credential = match (
        std::env::var("SEED_MIRROR_LIVE_USERNAME"),
        std::env::var("SEED_MIRROR_LIVE_PASSWORD"),
    ) {
        (Ok(username), Ok(password)) => Some(RegistryCredential {
            host: std::env::var("SEED_MIRROR_LIVE_HOST")
                .ok()
                .filter(|h| !h.is_empty()),
            username,
            password,
        }),
        _ => None,
    };
    let settings = MirrorSettings {
        credential,
        platform: "linux/amd64".to_owned(),
        internal_registry: String::new(),
    };
    let server = common::TestServer::start().await;
    let oci = oci_state(&server).await;
    let name = "live-agent";

    let first = mirror_image(&oci, &settings, &image, name)
        .await
        .unwrap_or_else(|e| panic!("mirror of {image} failed: {e}"));
    let tag = first.internal_ref.rsplit(':').next().unwrap().to_owned();
    let stored = nasiko_oci::ops::get_manifest(&oci, "nasiko/live-agent", &tag)
        .await
        .expect("manifest stored");
    eprintln!(
        "LIVE {image} -> {} repointed={} manifest={} media_type={}",
        first.internal_ref, first.repointed, stored.digest, stored.media_type
    );

    let second = mirror_image(&oci, &settings, &image, name)
        .await
        .expect("second mirror");
    assert!(!second.repointed, "unchanged tag must not re-copy");
    server.cleanup().await;
}
