//! Central-dashboard mode: exchange a BFF-signed assertion for a delegated
//! token — `POST /api/auth/delegate/assertion` (ui/docs/adr/0001a §2.2).
//!
//! The dashboard (EE tenant-server) and a workspace control plane have
//! different secrets by design, so the BFF cannot mint this CP's tokens.
//! Instead it signs an **assertion** with its Ed25519 key; this CP verifies
//! it against the JWKS published at the BFF URL that was configured at
//! provision time (`NASIKO_BFF_URL`), binds it to its own audience, consumes
//! its `jti` once, maps the asserted verified email to an EXISTING local user
//! (never creating one), and only then mints an ordinary delegated token
//! through the same policy as the session path. A workspace can therefore
//! verify the BFF without being able to impersonate it, and no secret is
//! shared across workspaces.
//!
//! Needs `NASIKO_BFF_URL`; `DELEGATION_ENABLED` is a kill switch that is on
//! unless set to `0`, `false`, `no` or `off`.
//!
//! The same verification also backs a browser sign-in
//! (`GET /api/auth/assertion/start` and `/login`): the browser goes out to the
//! dashboard and comes back carrying an assertion of type
//! [`LOGIN_ASSERTION_TOKEN_TYPE`], bound to a state cookie this workspace set
//! first. Unlike the exchange, that path creates or links the user under the
//! workspace's ordinary sign-in rules.

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use axum::{
    Json,
    body::Body,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use redis::AsyncCommands;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use nasiko_auth::assertion::{
    ASSERTION_TOKEN_TYPE, Jwks, LOGIN_ASSERTION_TOKEN_TYPE, UNKNOWN_KID, VerifiedAssertion,
    verify_assertion,
};
use nasiko_config::{Config, DEFAULT_DELEGATION_AUDIENCE};

use super::delegate::{DelegateRequest, DelegateResponse, envelope};
use super::login::{request_is_https, set_token_cookie};
use crate::state::AppState;

/// Process-wide JWKS cache: refreshed at most every [`JWKS_TTL`], and at most
/// once per [`JWKS_MIN_REFRESH`] on an unknown `kid` (rotation), so a flood of
/// bad assertions cannot turn this CP into a JWKS fetch loop.
static JWKS_CACHE: RwLock<Option<(Instant, Arc<Jwks>)>> = RwLock::new(None);
const JWKS_TTL: Duration = Duration::from_secs(300);
const JWKS_MIN_REFRESH: Duration = Duration::from_secs(30);

pub fn jwks_url(bff_url: &str) -> String {
    format!(
        "{}/.well-known/nasiko-delegation-jwks.json",
        bff_url.trim_end_matches('/')
    )
}

async fn load_jwks(
    http: &reqwest::Client,
    bff_url: &str,
    force: bool,
) -> Result<Arc<Jwks>, String> {
    if let Some((at, keys)) = JWKS_CACHE.read().map_err(|_| "jwks lock")?.clone() {
        let age = at.elapsed();
        if age < JWKS_TTL && !(force && age >= JWKS_MIN_REFRESH) {
            return Ok(keys);
        }
    }
    let resp = http
        .get(jwks_url(bff_url))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("jwks fetch: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("jwks fetch returned {}", resp.status()));
    }
    let keys: Jwks = resp.json().await.map_err(|e| format!("jwks parse: {e}"))?;
    let keys = Arc::new(keys);
    *JWKS_CACHE.write().map_err(|_| "jwks lock")? = Some((Instant::now(), keys.clone()));
    Ok(keys)
}

#[derive(Debug, Deserialize)]
pub struct AssertionExchangeRequest {
    #[serde(flatten)]
    pub delegate: DelegateRequest,
}

fn configured_bff_url(config: &Config) -> Option<&str> {
    config
        .nasiko_bff_url
        .as_deref()
        .filter(|u| !u.trim().is_empty())
}

/// Why a presented assertion was not accepted. The exchange answers each with an
/// API envelope, the browser sign-in with a redirect.
#[derive(Debug)]
enum AssertionRefusal {
    /// Redis could not record the use, so single use cannot be promised: refuse.
    Unavailable,
    /// The key document is unreachable, or the signature, issuer, audience, type,
    /// lifetime or expiry is wrong.
    Invalid,
    /// The same assertion was already presented within its lifetime.
    Replayed,
}

/// Verifies `token` against the dashboard's key document for this workspace's
/// audience and `expected_type`, then consumes its `jti`: the first caller wins,
/// and a replay inside the assertion's lifetime is refused. Fails closed when
/// Redis cannot record the use.
async fn verified_once(
    state: &AppState,
    bff_url: &str,
    token: &str,
    expected_type: &str,
) -> Result<VerifiedAssertion, AssertionRefusal> {
    let expected_aud = state.config.delegation_audience.as_str();
    let issuer = bff_url.trim_end_matches('/');
    // First try the cached keys; on an unknown kid refresh once (rotation).
    let verified = match load_jwks(&state.http_client, bff_url, false).await {
        Ok(jwks) => match verify_assertion(token, &jwks, issuer, expected_aud, expected_type) {
            Err(e) if e == UNKNOWN_KID => {
                match load_jwks(&state.http_client, bff_url, true).await {
                    Ok(jwks) => verify_assertion(token, &jwks, issuer, expected_aud, expected_type),
                    Err(e) => Err(e),
                }
            }
            other => other,
        },
        Err(e) => Err(e),
    };
    let verified = match verified {
        Ok(v) => v,
        Err(reason) => {
            tracing::warn!(reason = %reason, "assertion refused");
            return Err(AssertionRefusal::Invalid);
        }
    };

    let mut conn = match state.redis.connection().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "assertion: redis unavailable (fail closed)");
            return Err(AssertionRefusal::Unavailable);
        }
    };
    let marker = format!(
        "delegation:assertion:{}",
        nasiko_auth::jwt::hash_jti(&verified.jti)
    );
    let fresh: bool = match conn
        .set_options(
            &marker,
            1u8,
            redis::SetOptions::default()
                .conditional_set(redis::ExistenceCheck::NX)
                .with_expiration(redis::SetExpiry::EX(
                    nasiko_auth::assertion::ASSERTION_MAX_TTL_SECS,
                )),
        )
        .await
    {
        Ok(redis::Value::Okay) => true,
        Ok(_) => false,
        Err(e) => {
            tracing::error!(error = %e, "assertion: redis error (fail closed)");
            return Err(AssertionRefusal::Unavailable);
        }
    };
    if !fresh {
        tracing::warn!(jti = %verified.jti, "assertion: replay refused");
        return Err(AssertionRefusal::Replayed);
    }
    Ok(verified)
}

/// `POST /api/auth/delegate/assertion` - `Authorization: Bearer <assertion>`.
pub async fn exchange_assertion(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<AssertionExchangeRequest>,
) -> Response {
    if !state.config.delegation_enabled {
        return envelope(
            StatusCode::SERVICE_UNAVAILABLE,
            "delegation is disabled on this deployment",
        );
    }
    let Some(bff_url) = configured_bff_url(&state.config) else {
        return envelope(
            StatusCode::NOT_FOUND,
            "this control plane is not attached to a dashboard",
        );
    };
    // A fleet of workspaces on the shared default audience would accept each
    // other's assertions. Refuse rather than mint - this path is only ever
    // reached by dashboard-attached workspaces, which have a public origin.
    if state.config.delegation_audience == nasiko_config::DEFAULT_DELEGATION_AUDIENCE {
        tracing::error!(
            "assertion exchange refused: NASIKO_CP_AUDIENCE/CP_DOMAIN unset, audience is the shared default"
        );
        return envelope(
            StatusCode::SERVICE_UNAVAILABLE,
            "delegation audience is not configured on this deployment",
        );
    }
    let Some(token) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return envelope(StatusCode::UNAUTHORIZED, "missing assertion");
    };
    let verified = match verified_once(&state, bff_url, token, ASSERTION_TOKEN_TYPE).await {
        Ok(v) => v,
        Err(AssertionRefusal::Unavailable) => {
            return envelope(
                StatusCode::SERVICE_UNAVAILABLE,
                "delegation temporarily unavailable",
            );
        }
        Err(AssertionRefusal::Invalid) => {
            return envelope(StatusCode::UNAUTHORIZED, "invalid assertion");
        }
        Err(AssertionRefusal::Replayed) => {
            return envelope(StatusCode::UNAUTHORIZED, "assertion already used");
        }
    };

    // Map the verified email to an EXISTING member. No auto-create: the
    // assertion proves who the dashboard authenticated, not that they belong here.
    let row: Option<(uuid::Uuid, String, bool)> = match sqlx::query_as(
        "SELECT id, username, is_superuser FROM users
         WHERE LOWER(email) = $1 AND deleted_at IS NULL AND is_active",
    )
    .bind(&verified.email)
    .fetch_optional(&state.db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "assertion exchange: user lookup failed");
            return envelope(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not resolve the member",
            );
        }
    };
    let Some((id, username, is_superuser)) = row else {
        tracing::warn!(workspace = %verified.workspace, issuer = %verified.issuer, "assertion exchange: not a member");
        return envelope(StatusCode::FORBIDDEN, "not a member of this workspace");
    };
    let identity = nasiko_auth::Identity {
        user_id: id.to_string(),
        username,
        is_superuser,
    };
    let request = nasiko_auth::DelegationRequest {
        scopes: req.delegate.scopes,
        ttl_secs: req
            .delegate
            .ttl_secs
            .unwrap_or(nasiko_auth::DELEGATION_MAX_TTL_SECS),
        parent_jti: verified.jti.clone(),
        audience: state.config.delegation_audience.clone(),
    };
    match state.auth.issue_delegated_token(&identity, &request).await {
        Ok(issued) => {
            tracing::info!(user = %identity.user_id, jti = %issued.jti, parent = %verified.jti, portal_subject = %verified.portal_subject, scopes = ?issued.scopes, "assertion exchange: issued");
            Json(DelegateResponse {
                token: issued.token,
                expires_in: issued.expires_in,
                scopes: issued.scopes,
                aud: request.audience,
            })
            .into_response()
        }
        Err(nasiko_auth::AuthError::InvalidToken(m)) => envelope(StatusCode::BAD_REQUEST, &m),
        Err(nasiko_auth::AuthError::Unsupported) => envelope(
            StatusCode::NOT_IMPLEMENTED,
            "delegation is not supported by this auth backend",
        ),
        Err(e) => {
            tracing::error!(error = %e, "assertion exchange: failed to issue");
            envelope(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not issue a delegated token",
            )
        }
    }
}

// ─── Browser sign-in ─────────────────────────────────────────────────────────

const STATE_COOKIE: &str = "assertion_state";
const RETURN_COOKIE: &str = "assertion_return";
/// Long enough for the round trip through the dashboard, short enough that a
/// stale pair is useless.
const COOKIE_TTL_SECS: u64 = 120;
/// A longer `return_to` is ignored: it rides in a cookie and a `Location`
/// header, and a proxy answers a header that is too big with an error page.
const MAX_RETURN_TARGET_LEN: usize = 1024;

/// `scheme://host[:port]` of an http(s) URL, normalised the way a browser
/// compares origins; `None` for anything else.
fn http_origin(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    origin_of(&parsed)
}

fn origin_of(url: &reqwest::Url) -> Option<String> {
    matches!(url.scheme(), "http" | "https").then(|| url.origin().ascii_serialization())
}

/// Whether browser sign-in is on for these settings. Requiring
/// `allow_personal_emails` stands in for a per-request admission check: every
/// multi-tenant install sets it, and one that turns it off refuses the whole
/// path instead of skipping a rule. The audience must be this workspace's own:
/// neither the shared default nor the dashboard's origin (what an install
/// without `NASIKO_CP_AUDIENCE` falls back to), or every workspace would accept
/// every other's assertions.
fn sign_in_allowed(
    multi_tenant_mode: bool,
    allow_personal_emails: bool,
    delegation_enabled: bool,
    bff_url: Option<&str>,
    audience: &str,
) -> bool {
    let Some(dashboard_origin) = bff_url.and_then(http_origin) else {
        return false;
    };
    multi_tenant_mode
        && allow_personal_emails
        && delegation_enabled
        && audience != DEFAULT_DELEGATION_AUDIENCE
        && http_origin(audience).as_deref() != Some(dashboard_origin.as_str())
}

fn sign_in_enabled(config: &Config) -> bool {
    sign_in_allowed(
        config.multi_tenant_mode,
        config.allow_personal_emails,
        config.delegation_enabled,
        configured_bff_url(config),
        &config.delegation_audience,
    )
}

/// The dashboard URL when browser sign-in is on; `None` makes a route answer 404.
fn sign_in_bff_url(config: &Config) -> Option<&str> {
    sign_in_enabled(config)
        .then(|| configured_bff_url(config))
        .flatten()
}

/// Where the browser lands after signing in: `raw` when it is an http(s) URL
/// whose origin is one of `allowed_origins` (parsed equality, never a prefix, so
/// `https://app.example.com.evil.test` is not `https://app.example.com`), else
/// `fallback`. The result is the parsed URL's own serialisation, so it is plain
/// ASCII and safe to put in a header.
fn return_target(raw: Option<&str>, allowed_origins: &[String], fallback: &str) -> String {
    raw.filter(|r| r.len() <= MAX_RETURN_TARGET_LEN)
        .and_then(|r| reqwest::Url::parse(r).ok())
        .filter(|url| url.username().is_empty() && url.password().is_none())
        .filter(|url| {
            origin_of(url).is_some_and(|origin| {
                allowed_origins
                    .iter()
                    .any(|allowed| http_origin(allowed).as_deref() == Some(origin.as_str()))
            })
        })
        .map_or_else(|| fallback.to_owned(), |url| url.to_string())
}

/// The return target and the dashboard origin every browser redirect of this
/// flow goes to. `None` only when the dashboard URL itself is unusable, which
/// `sign_in_enabled` has already excluded.
fn choose_target(config: &Config, bff_url: &str, raw: Option<&str>) -> Option<(String, String)> {
    let fallback = format!("{}/", bff_url.trim_end_matches('/'));
    let target = return_target(raw, &config.cors_allowed_origins, &fallback);
    let origin = http_origin(&target)?;
    Some((target, origin))
}

/// `__Host-` makes the browser refuse a cookie not set over https with
/// `Path=/` and no `Domain`, so a sibling workspace cannot plant one.
fn cookie_name(base: &str, secure: bool) -> String {
    if secure {
        format!("__Host-{base}")
    } else {
        base.to_owned()
    }
}

/// Host-only (no `Domain`), `HttpOnly`, and `Lax` so it rides the top-level
/// navigation that brings the browser back from the dashboard.
fn set_cookie(base: &str, value: &str, secure: bool, max_age_secs: u64) -> String {
    let secure_attr = if secure { " Secure;" } else { "" };
    format!(
        "{}={value}; HttpOnly;{secure_attr} Path=/; SameSite=Lax; Max-Age={max_age_secs}",
        cookie_name(base, secure)
    )
}

fn clearing_cookies(secure: bool) -> Vec<String> {
    [STATE_COOKIE, RETURN_COOKIE]
        .map(|base| set_cookie(base, "", secure, 0))
        .to_vec()
}

fn cookie_value<'h>(headers: &'h HeaderMap, name: &str) -> Option<&'h str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|part| part.trim().strip_prefix(name)?.strip_prefix('='))
        .filter(|v| !v.is_empty())
}

/// 32 random bytes as URL-safe base64 (43 characters), inside the 16 to 128
/// `[A-Za-z0-9_-]` the dashboard accepts for a state.
fn new_browser_state() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Compares digests, not the strings, so there is no byte-by-byte early exit.
fn same_secret(a: &str, b: &str) -> bool {
    Sha256::digest(a) == Sha256::digest(b)
}

fn not_found() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

/// A `302` to `location` that sets `set_cookies`. Every input is built from
/// base64url text and constants, so a failure is a bug: it answers 500 with an
/// empty body rather than panic.
fn found(location: &str, set_cookies: Vec<String>) -> Response {
    let mut builder = Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, location);
    for cookie in set_cookies {
        builder = builder.header(header::SET_COOKIE, cookie);
    }
    builder.body(Body::empty()).unwrap_or_else(|e| {
        tracing::error!(error = %e, "assertion sign-in: could not build the redirect");
        StatusCode::INTERNAL_SERVER_ERROR.into_response()
    })
}

/// The dashboard page that tries the sign-in again, session kept.
fn retry_url(dashboard_origin: &str) -> String {
    format!("{dashboard_origin}/login?redirect=%2F")
}

/// The sign-in page's error shape (`signin=external` and `#error=<code>`).
fn refused_url(dashboard_origin: &str, code: &str) -> String {
    format!("{dashboard_origin}/login?signin=external#error={code}")
}

#[derive(Debug, Deserialize)]
pub struct SignInStartQuery {
    return_to: Option<String>,
}

/// `GET /api/auth/assertion/start`: remembers a random state in this browser,
/// then sends it to the dashboard to be vouched for. The dashboard signs the
/// state into the assertion, so only the browser that started can finish.
pub async fn sign_in_start(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SignInStartQuery>,
) -> Response {
    let Some(bff_url) = sign_in_bff_url(&state.config) else {
        return not_found();
    };
    let Some((target, origin)) = choose_target(&state.config, bff_url, query.return_to.as_deref())
    else {
        return not_found();
    };
    let secure = request_is_https(&headers);
    let browser_state = new_browser_state();
    found(
        &format!("{origin}/api/enter/assert?state={browser_state}"),
        vec![
            set_cookie(STATE_COOKIE, &browser_state, secure, COOKIE_TTL_SECS),
            set_cookie(
                RETURN_COOKIE,
                &URL_SAFE_NO_PAD.encode(&target),
                secure,
                COOKIE_TTL_SECS,
            ),
        ],
    )
}

/// No `Debug` derive: the assertion is a credential and must never reach a log.
#[derive(Deserialize)]
pub struct SignInQuery {
    assertion: Option<String>,
}

/// How a sign-in attempt ended; `sign_in` turns each into a redirect.
enum SignInOutcome {
    /// Retrying may work: the dashboard re-enters on its own, session kept.
    Retry,
    /// Retrying cannot help: the sign-in page shows the code.
    Refused(&'static str),
    /// Carries the new workspace session token.
    SignedIn(String),
}

/// `GET /api/auth/assertion/login?assertion=<jwt>`: a browser navigation, so no
/// outcome is a JSON body. Both short-lived cookies are cleared on every one.
pub async fn sign_in(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SignInQuery>,
) -> Response {
    let Some(bff_url) = sign_in_bff_url(&state.config) else {
        return not_found();
    };
    let secure = request_is_https(&headers);
    let remembered_target = cookie_value(&headers, &cookie_name(RETURN_COOKIE, secure))
        .and_then(|v| URL_SAFE_NO_PAD.decode(v).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok());
    // Re-validated, not trusted: the cookie outlives the config it was checked against.
    let Some((target, origin)) =
        choose_target(&state.config, bff_url, remembered_target.as_deref())
    else {
        return not_found();
    };

    let outcome = complete_sign_in(
        &state,
        bff_url,
        cookie_value(&headers, &cookie_name(STATE_COOKIE, secure)),
        query.assertion.as_deref(),
        secure,
    )
    .await;

    let clearing = clearing_cookies(secure);
    match outcome {
        SignInOutcome::Retry => found(&retry_url(&origin), clearing),
        SignInOutcome::Refused(code) => found(&refused_url(&origin, code), clearing),
        SignInOutcome::SignedIn(token) => {
            let mut response = found(&target, clearing);
            response
                .headers_mut()
                .append(header::SET_COOKIE, set_token_cookie(&token, secure));
            response
        }
    }
}

async fn complete_sign_in(
    state: &AppState,
    bff_url: &str,
    browser_state: Option<&str>,
    assertion: Option<&str>,
    secure: bool,
) -> SignInOutcome {
    let (Some(assertion), Some(browser_state)) = (assertion, browser_state) else {
        tracing::warn!(
            has_assertion = assertion.is_some(),
            has_state_cookie = browser_state.is_some(),
            secure,
            "assertion sign-in: missing assertion or state cookie"
        );
        return SignInOutcome::Retry;
    };
    let verified = match verified_once(state, bff_url, assertion, LOGIN_ASSERTION_TOKEN_TYPE).await
    {
        Ok(v) => v,
        Err(refusal) => {
            tracing::warn!(?refusal, "assertion sign-in: assertion refused");
            return SignInOutcome::Retry;
        }
    };
    if !verified
        .nonce
        .as_deref()
        .is_some_and(|nonce| same_secret(nonce, browser_state))
    {
        tracing::warn!(jti = %verified.jti, "assertion sign-in: state does not match this browser");
        return SignInOutcome::Retry;
    }

    let email = match stored_spelling(&state.db, &verified.email).await {
        Ok(email) => email,
        Err(e) => {
            tracing::error!(error = %e, "assertion sign-in: user lookup failed");
            return SignInOutcome::Refused("user_save_failed");
        }
    };
    // An assertion carries no IdP claims, so the OIDC callback's steps that
    // consume them (provider metadata, group and role reconcile, directory
    // refresh) have no counterpart here.
    match state
        .auth
        .upsert_oauth_user("nasiko", &verified.portal_subject, &email, Some(&email))
        .await
    {
        Ok(result) => {
            tracing::info!(
                user = %result.user_id,
                email_domain = email.rsplit('@').next().unwrap_or_default(),
                workspace = %verified.workspace,
                "assertion sign-in: signed in"
            );
            SignInOutcome::SignedIn(result.token)
        }
        Err(nasiko_auth::AuthError::NotInvited) => SignInOutcome::Refused("not_invited"),
        Err(nasiko_auth::AuthError::Disabled) => SignInOutcome::Refused("account_deactivated"),
        // No error text: `Conflict` says "no free username derived from '<username>'",
        // and the username here is the full address. A soft-deleted row that still
        // holds the address makes every insert conflict.
        Err(nasiko_auth::AuthError::Conflict(_)) => {
            tracing::error!("assertion sign-in: no free username for the new user (conflict)");
            SignInOutcome::Refused("user_save_failed")
        }
        // `%e`, not `?e`: every other variant's Display is a fixed message or a
        // reason code (`Database` and `Internal` are scrubbed on purpose), while a
        // unique-violation's Debug carries the address it rejected.
        Err(e) => {
            tracing::error!(error = %e, "assertion sign-in: upsert_oauth_user failed");
            SignInOutcome::Refused("user_save_failed")
        }
    }
}

/// The address as this workspace stores it. The upsert and the members-only
/// check match email exactly, and a row made by another sign-in method can hold
/// capitals: asserting the lowercase spelling would create a second, empty
/// account. Two rows that differ only in case can exist (the unique index is
/// case-sensitive), so the exact lowercase one wins, then the lowest id.
async fn stored_spelling(db: &sqlx::PgPool, lowercase: &str) -> Result<String, sqlx::Error> {
    let stored: Option<String> = sqlx::query_scalar(
        "SELECT email FROM users WHERE LOWER(email) = $1 AND deleted_at IS NULL
         ORDER BY (email = $1) DESC, id LIMIT 1",
    )
    .bind(lowercase)
    .fetch_optional(db)
    .await?;
    Ok(stored.unwrap_or_else(|| lowercase.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jwks_url_is_derived_from_the_configured_bff_url_only() {
        assert_eq!(
            jwks_url("https://app.nasiko.com/"),
            "https://app.nasiko.com/.well-known/nasiko-delegation-jwks.json"
        );
    }

    /// The one test that touches the process-wide key cache in this binary, so its order of cases
    /// is its own: every failure comes before the one success that fills the cache.
    #[tokio::test]
    async fn an_unusable_key_document_is_an_error_and_is_never_cached() {
        let http = reqwest::Client::new();

        // Nothing listens on port 1: the connection is refused at once.
        let unreachable = load_jwks(&http, "http://127.0.0.1:1", false).await;
        assert!(
            unreachable
                .as_ref()
                .is_err_and(|e| e.starts_with("jwks fetch:")),
            "{unreachable:?}"
        );

        let mut server = mockito::Server::new_async().await;
        let path = "/.well-known/nasiko-delegation-jwks.json";
        let failing = server
            .mock("GET", path)
            .with_status(500)
            .create_async()
            .await;
        assert_eq!(
            load_jwks(&http, &server.url(), false).await.unwrap_err(),
            "jwks fetch returned 500 Internal Server Error"
        );
        failing.remove_async().await;

        let garbage = server
            .mock("GET", path)
            .with_status(200)
            .with_body("not json")
            .create_async()
            .await;
        let parse = load_jwks(&http, &server.url(), false).await;
        assert!(
            parse.as_ref().is_err_and(|e| e.starts_with("jwks parse:")),
            "{parse:?}"
        );
        garbage.remove_async().await;

        // Positive control: the same address serves a real document once the failures are gone, so
        // none of them was cached as an answer.
        let _good = server
            .mock("GET", path)
            .with_status(200)
            .with_body(r#"{"keys":[]}"#)
            .create_async()
            .await;
        assert!(load_jwks(&http, &server.url(), false).await.is_ok());
    }

    const DASHBOARD: &str = "https://app.example.com";
    const OWN_AUDIENCE: &str = "https://acme.app.example.com";

    #[test]
    fn sign_in_is_on_when_every_condition_holds() {
        assert!(sign_in_allowed(
            true,
            true,
            true,
            Some(DASHBOARD),
            OWN_AUDIENCE
        ));
    }

    #[test]
    fn sign_in_is_off_when_the_audience_is_the_dashboard_origin() {
        // The fallback of an install without its own audience, however it is spelled.
        for audience in [
            "https://app.example.com",
            "https://app.example.com/",
            "https://APP.example.com",
        ] {
            assert!(
                !sign_in_allowed(
                    true,
                    true,
                    true,
                    Some("https://app.example.com/dash"),
                    audience
                ),
                "{audience}"
            );
        }
        assert!(!sign_in_allowed(
            true,
            true,
            true,
            Some(DASHBOARD),
            DEFAULT_DELEGATION_AUDIENCE
        ));
    }

    #[test]
    fn sign_in_is_off_without_allow_personal_emails() {
        assert!(!sign_in_allowed(
            true,
            false,
            true,
            Some(DASHBOARD),
            OWN_AUDIENCE
        ));
    }

    #[test]
    fn sign_in_is_off_without_the_other_conditions() {
        assert!(!sign_in_allowed(
            false,
            true,
            true,
            Some(DASHBOARD),
            OWN_AUDIENCE
        ));
        assert!(!sign_in_allowed(
            true,
            true,
            false,
            Some(DASHBOARD),
            OWN_AUDIENCE
        ));
        assert!(!sign_in_allowed(true, true, true, None, OWN_AUDIENCE));
        assert!(!sign_in_allowed(
            true,
            true,
            true,
            Some("not a url"),
            OWN_AUDIENCE
        ));
        assert!(!sign_in_allowed(
            true,
            true,
            true,
            Some("ftp://app.example.com"),
            OWN_AUDIENCE
        ));
    }

    const FALLBACK: &str = "https://app.example.com/";

    fn allowed() -> Vec<String> {
        vec![
            "https://app.example.com".to_owned(),
            "*".to_owned(),
            "http://localhost:5173".to_owned(),
        ]
    }

    #[test]
    fn return_target_keeps_an_allowed_origin() {
        for kept in [
            "https://app.example.com/workspaces/1?tab=2#x",
            "https://app.example.com/",
            "http://localhost:5173/dash",
        ] {
            assert_eq!(return_target(Some(kept), &allowed(), FALLBACK), kept);
        }
        // An allowed origin spelled with a trailing slash still matches by parsed equality.
        assert_eq!(
            return_target(
                Some("https://app.example.com/x"),
                &["https://app.example.com/".to_owned()],
                FALLBACK
            ),
            "https://app.example.com/x"
        );
    }

    #[test]
    fn return_target_falls_back_for_everything_else() {
        let too_long = format!(
            "https://app.example.com/{}",
            "a".repeat(MAX_RETURN_TARGET_LEN)
        );
        for refused in [
            "https://other.example.com/",
            "https://app.example.com.evil.test/",
            "https://app.example.com@evil.test/",
            "https://evil.test\\@app.example.com/",
            "http://app.example.com/",
            "https://app.example.com:8443/",
            "https://user:pw@app.example.com/",
            "//app.example.com/x",
            "/relative/path",
            "not a url",
            "javascript:alert(1)",
            "",
            too_long.as_str(),
        ] {
            assert_eq!(
                return_target(Some(refused), &allowed(), FALLBACK),
                FALLBACK,
                "{refused:?}"
            );
        }
        assert_eq!(return_target(None, &allowed(), FALLBACK), FALLBACK);
        // Nothing listed means nothing is allowed.
        assert_eq!(
            return_target(Some("https://app.example.com/"), &[], "https://f.test/"),
            "https://f.test/"
        );
    }

    #[test]
    fn return_target_is_header_safe() {
        let target = return_target(
            Some("https://app.example.com/a\r\nSet-Cookie: x=1"),
            &allowed(),
            FALLBACK,
        );
        assert!(
            reqwest::header::HeaderValue::from_str(&target).is_ok(),
            "{target:?}"
        );
    }

    #[test]
    fn cookies_are_host_only_http_only_and_prefixed_over_https() {
        assert_eq!(
            set_cookie(STATE_COOKIE, "abc", true, 120),
            "__Host-assertion_state=abc; HttpOnly; Secure; Path=/; SameSite=Lax; Max-Age=120"
        );
        assert_eq!(
            set_cookie(RETURN_COOKIE, "abc", false, 120),
            "assertion_return=abc; HttpOnly; Path=/; SameSite=Lax; Max-Age=120"
        );
        for secure in [true, false] {
            for cookie in clearing_cookies(secure) {
                assert!(cookie.contains("Max-Age=0"), "{cookie}");
                assert!(!cookie.contains("Domain"), "{cookie}");
            }
            assert!(!set_cookie(STATE_COOKIE, "abc", secure, 120).contains("Domain"));
        }
    }

    #[test]
    fn cookie_value_matches_the_exact_name_only() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "access_token=t; __Host-assertion_state=xyz; assertion_state_x=no; assertion_return="
                .parse()
                .unwrap(),
        );
        assert_eq!(
            cookie_value(&headers, "__Host-assertion_state"),
            Some("xyz")
        );
        // Neither the unprefixed name nor a longer name with the same start.
        assert_eq!(cookie_value(&headers, "assertion_state"), None);
        // An empty value is no value.
        assert_eq!(cookie_value(&headers, "assertion_return"), None);
    }

    #[test]
    fn a_browser_state_is_random_and_in_the_range_the_dashboard_accepts() {
        let (a, b) = (new_browser_state(), new_browser_state());
        assert_ne!(a, b);
        assert!((16..=128).contains(&a.len()), "{a}");
        assert!(
            a.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }

    #[test]
    fn same_secret_compares_whole_values() {
        assert!(same_secret("abc", "abc"));
        assert!(!same_secret("abc", "abd"));
        assert!(!same_secret("abc", "abcd"));
        assert!(!same_secret("", "abc"));
    }
}
