//! Browser sign-in by assertion: `GET /api/auth/assertion/start` and `/login`.
//!
//! The workspace server runs for real (database, Redis) behind a stub dashboard that serves the
//! key document. The server caches that document in a process-wide static, so every test in this
//! binary shares ONE key pair and ONE stub, created once on a runtime of its own (a test's runtime
//! ends with the test).
mod common;

use std::sync::OnceLock;

use axum::{Json, Router, routing::get};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use nasiko_auth::assertion::{
    ASSERTION_MAX_TTL_SECS, ASSERTION_TOKEN_TYPE, AssertionClaims, AssertionSigner, Jwks,
    LOGIN_ASSERTION_TOKEN_TYPE, now_secs,
};
use nasiko_config::Config;
use redis::AsyncCommands;
use reqwest::header::{CONTENT_TYPE, COOKIE, LOCATION, SET_COOKIE};
use reqwest::redirect::Policy;
use reqwest::{Response, StatusCode};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::Value;
use serial_test::serial;

const WORKSPACE_AUDIENCE: &str = "https://workspace.test";
/// A second dashboard origin the workspace lists, as a dashboard served on an alias domain would be.
const ALIAS: &str = "http://alias.test";
const STATE_COOKIE: &str = "assertion_state";
const RETURN_COOKIE: &str = "assertion_return";

// ─── The stub dashboard ──────────────────────────────────────────────────────

struct Dashboard {
    signer: AssertionSigner,
    /// Its origin, which is also the workspace's `NASIKO_BFF_URL` and the assertions' issuer.
    url: String,
}

/// The fixed SPKI DER header for an Ed25519 public key; the raw 32 bytes follow.
const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

fn pem(label: &str, der: &[u8]) -> String {
    format!(
        "-----BEGIN {label}-----\n{}\n-----END {label}-----\n",
        STANDARD.encode(der)
    )
}

fn dashboard() -> &'static Dashboard {
    static DASHBOARD: OnceLock<Dashboard> = OnceLock::new();
    DASHBOARD.get_or_init(start_dashboard)
}

fn start_dashboard() -> Dashboard {
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
        .expect("generate an Ed25519 key");
    let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("parse the generated key");
    let spki = [ED25519_SPKI_PREFIX.as_slice(), pair.public_key().as_ref()].concat();
    let signer = AssertionSigner::from_pem(
        &pem("PRIVATE KEY", pkcs8.as_ref()),
        &pem("PUBLIC KEY", &spki),
    )
    .expect("build the signer");
    let jwks = Jwks {
        keys: vec![signer.jwk().clone()],
    };

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the stub dashboard");
    listener
        .set_nonblocking(true)
        .expect("make the stub listener non-blocking");
    let url = format!("http://{}", listener.local_addr().expect("stub address"));
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("stub runtime");
        runtime.block_on(async move {
            let app = Router::new().route(
                "/.well-known/nasiko-delegation-jwks.json",
                get(move || {
                    let jwks = jwks.clone();
                    async move { Json(jwks) }
                }),
            );
            let listener = tokio::net::TcpListener::from_std(listener).expect("stub listener");
            axum::serve(listener, app).await.expect("stub dashboard");
        });
    });
    Dashboard { signer, url }
}

// ─── Servers, clients, claims ────────────────────────────────────────────────

/// Browser sign-in on: multi-tenant, personal emails allowed, a dashboard, and an audience of the
/// workspace's own (neither the default nor the dashboard's origin).
fn enable_sign_in(config: &mut Config) {
    let dashboard = dashboard();
    config.multi_tenant_mode = true;
    config.allow_personal_emails = true;
    config.nasiko_bff_url = Some(dashboard.url.clone());
    config.delegation_audience = WORKSPACE_AUDIENCE.to_owned();
    config.cors_allowed_origins = vec![dashboard.url.clone(), ALIAS.to_owned()];
}

async fn start_server() -> common::TestServer {
    common::TestServer::start_with(enable_sign_in).await
}

/// No redirects followed and no cookie jar: the test is the browser, so it sees each hop.
fn browser() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(Policy::none())
        .build()
        .expect("browser client")
}

fn login_claims(email: &str, nonce: &str) -> AssertionClaims {
    let now = now_secs();
    AssertionClaims {
        iss: dashboard().url.clone(),
        aud: WORKSPACE_AUDIENCE.to_owned(),
        sub: "portal-user-1".to_owned(),
        email: email.to_owned(),
        email_verified: true,
        ws: "ws-1".to_owned(),
        jti: uuid::Uuid::new_v4().to_string(),
        iat: now,
        exp: now + ASSERTION_MAX_TTL_SECS,
        token_type: LOGIN_ASSERTION_TOKEN_TYPE.to_owned(),
        nonce: Some(nonce.to_owned()),
    }
}

fn sign(claims: &AssertionClaims) -> String {
    dashboard().signer.sign(claims).expect("sign the assertion")
}

fn location(response: &Response) -> String {
    response
        .headers()
        .get(LOCATION)
        .expect("a redirect has a Location")
        .to_str()
        .expect("Location is ascii")
        .to_owned()
}

fn set_cookies(response: &Response) -> Vec<String> {
    response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .map(|v| v.to_str().expect("Set-Cookie is ascii").to_owned())
        .collect()
}

/// The whole `Set-Cookie` line of the cookie called `name`, if the response set one.
fn cookie_named(cookies: &[String], name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    cookies.iter().find(|c| c.starts_with(&prefix)).cloned()
}

/// `value` of a `Set-Cookie` line `name=value; attrs`.
fn value_of(cookie: &str) -> &str {
    let pair = cookie.split(';').next().unwrap_or_default();
    pair.split_once('=').map_or("", |(_, v)| v)
}

/// What a browser would send back: `name=value` for every cookie that has a value.
fn cookie_header(cookies: &[String]) -> String {
    cookies
        .iter()
        .filter_map(|c| c.split(';').next())
        .filter(|pair| pair.split_once('=').is_some_and(|(_, v)| !v.is_empty()))
        .collect::<Vec<_>>()
        .join("; ")
}

/// One `start`: the state the workspace handed this browser and the cookies to send back.
struct Started {
    state: String,
    cookies: String,
    location: String,
    set_cookies: Vec<String>,
}

async fn start(server: &common::TestServer, return_to: Option<&str>, https: bool) -> Started {
    let mut request = browser().get(server.url("/api/auth/assertion/start"));
    if let Some(return_to) = return_to {
        request = request.query(&[("return_to", return_to)]);
    }
    if https {
        request = request.header("x-forwarded-proto", "https");
    }
    let response = request.send().await.expect("start");
    assert_eq!(response.status(), StatusCode::FOUND);
    let location = location(&response);
    let state = location
        .split_once("state=")
        .expect("the redirect carries a state")
        .1
        .to_owned();
    let set_cookies = set_cookies(&response);
    Started {
        state,
        cookies: cookie_header(&set_cookies),
        location,
        set_cookies,
    }
}

async fn present(
    server: &common::TestServer,
    assertion: Option<&str>,
    cookies: &str,
    https: bool,
) -> Response {
    let mut request = browser().get(server.url("/api/auth/assertion/login"));
    if let Some(assertion) = assertion {
        request = request.query(&[("assertion", assertion)]);
    }
    if !cookies.is_empty() {
        request = request.header(COOKIE, cookies);
    }
    if https {
        request = request.header("x-forwarded-proto", "https");
    }
    request.send().await.expect("login")
}

/// Both short-lived cookies were cleared, under the names of the request's scheme.
fn assert_cleared(cookies: &[String], host_prefix: bool) {
    for base in [STATE_COOKIE, RETURN_COOKIE] {
        let name = if host_prefix {
            format!("__Host-{base}")
        } else {
            base.to_owned()
        };
        let cookie = cookie_named(cookies, &name)
            .unwrap_or_else(|| panic!("{name} was not cleared: {cookies:?}"));
        assert!(cookie.starts_with(&format!("{name}=;")), "{cookie}");
        assert!(cookie.contains("Max-Age=0"), "{cookie}");
    }
}

/// A refusal worth retrying: back to the dashboard's sign-in page with the session kept, no
/// workspace session, both cookies cleared, and never a JSON body.
async fn assert_retry(response: Response, dashboard_origin: &str, host_prefix: bool, what: &str) {
    assert_eq!(response.status(), StatusCode::FOUND, "{what}");
    assert_eq!(
        location(&response),
        format!("{dashboard_origin}/login?redirect=%2F"),
        "{what}"
    );
    let cookies = set_cookies(&response);
    assert!(
        cookie_named(&cookies, "access_token").is_none(),
        "{what}: {cookies:?}"
    );
    assert_cleared(&cookies, host_prefix);
    assert!(response.headers().get(CONTENT_TYPE).is_none(), "{what}");
    assert!(response.text().await.expect("body").is_empty(), "{what}");
}

async fn jti_consumed(server: &common::TestServer, jti: &str) -> bool {
    let mut conn = server.state.redis.connection().await.expect("redis");
    conn.exists(format!(
        "delegation:assertion:{}",
        nasiko_auth::jwt::hash_jti(jti)
    ))
    .await
    .expect("EXISTS on the single-use marker")
}

async fn user_rows(server: &common::TestServer, lowercase_email: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE LOWER(email) = $1")
        .bind(lowercase_email)
        .fetch_one(&server.db)
        .await
        .expect("count users")
}

/// What `GET /api/me` says about the session in `access_token`.
async fn me(server: &common::TestServer, access_token: &str) -> Value {
    let response = browser()
        .get(server.url("/api/me"))
        .header(COOKIE, format!("access_token={access_token}"))
        .send()
        .await
        .expect("me");
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.expect("me body")
}

// ─── start ───────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn start_binds_a_state_to_the_browser_and_goes_to_the_dashboard() {
    let server = start_server().await;
    let dashboard = &dashboard().url;

    let started = start(&server, None, false).await;
    assert_eq!(
        started.location,
        format!("{dashboard}/api/enter/assert?state={}", started.state)
    );
    // 32 random bytes as URL-safe base64: 43 characters inside the dashboard's 16 to 128.
    assert_eq!(started.state.len(), 43, "{}", started.state);
    assert!(
        started
            .state
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    );

    let state_cookie = cookie_named(&started.set_cookies, STATE_COOKIE).expect("state cookie");
    assert_eq!(value_of(&state_cookie), started.state);
    for cookie in &started.set_cookies {
        for attr in ["HttpOnly", "Path=/", "SameSite=Lax", "Max-Age=120"] {
            assert!(cookie.contains(attr), "{attr} missing from {cookie}");
        }
        assert!(
            !cookie.contains("Secure") && !cookie.contains("Domain"),
            "{cookie}"
        );
    }
    let return_cookie = cookie_named(&started.set_cookies, RETURN_COOKIE).expect("return cookie");
    let remembered = URL_SAFE_NO_PAD
        .decode(value_of(&return_cookie))
        .expect("the return target is base64url");
    assert_eq!(
        String::from_utf8(remembered).expect("utf8"),
        format!("{dashboard}/")
    );

    // Every browser gets its own state.
    assert_ne!(start(&server, None, false).await.state, started.state);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn start_follows_a_listed_return_target_and_ignores_any_other() {
    let server = start_server().await;
    let dashboard = &dashboard().url;

    let listed = start(&server, Some("http://alias.test/dash?tab=2"), false).await;
    assert!(
        listed
            .location
            .starts_with("http://alias.test/api/enter/assert?state="),
        "{}",
        listed.location
    );
    let return_cookie = cookie_named(&listed.set_cookies, RETURN_COOKIE).expect("return cookie");
    assert_eq!(
        String::from_utf8(
            URL_SAFE_NO_PAD
                .decode(value_of(&return_cookie))
                .expect("base64url")
        )
        .expect("utf8"),
        "http://alias.test/dash?tab=2"
    );

    for refused in [
        "https://evil.test/x",
        "http://alias.test.evil.test/x",
        "http://alias.test@evil.test/x",
        "/relative",
        "not a url",
    ] {
        let started = start(&server, Some(refused), false).await;
        assert!(
            started
                .location
                .starts_with(&format!("{dashboard}/api/enter/assert?state=")),
            "{refused}: {}",
            started.location
        );
    }

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn over_https_the_cookies_carry_the_host_prefix_and_secure() {
    let server = start_server().await;
    let dashboard = &dashboard().url;

    let started = start(&server, None, true).await;
    for base in [STATE_COOKIE, RETURN_COOKIE] {
        let cookie = cookie_named(&started.set_cookies, &format!("__Host-{base}"))
            .unwrap_or_else(|| panic!("__Host-{base} missing: {:?}", started.set_cookies));
        assert!(cookie.contains("Secure"), "{cookie}");
        assert!(
            cookie.contains("Path=/") && !cookie.contains("Domain"),
            "{cookie}"
        );
    }
    assert!(cookie_named(&started.set_cookies, STATE_COOKIE).is_none());

    // The same browser over plain http sends unprefixed names, which an https request does not read.
    let plain = start(&server, None, false).await;
    let claims = login_claims("https.person@corp.test", &plain.state);
    let response = present(&server, Some(&sign(&claims)), &plain.cookies, true).await;
    assert_retry(response, dashboard, true, "unprefixed cookies over https").await;
    assert!(!jti_consumed(&server, &claims.jti).await);

    // With the prefixed cookies the same request signs in, and the session cookie is Secure.
    let claims = login_claims("https.person@corp.test", &started.state);
    let response = present(&server, Some(&sign(&claims)), &started.cookies, true).await;
    assert_eq!(response.status(), StatusCode::FOUND);
    let cookies = set_cookies(&response);
    assert_cleared(&cookies, true);
    let session = cookie_named(&cookies, "access_token").expect("session cookie");
    assert!(session.contains("Secure"), "{session}");

    server.cleanup().await;
}

// ─── login ───────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn a_valid_assertion_signs_the_person_in_once() {
    let server = start_server().await;
    let started = start(&server, Some("http://alias.test/dash"), false).await;
    let claims = login_claims("New.Person@Corp.test", &started.state);
    let token = sign(&claims);

    let response = present(&server, Some(&token), &started.cookies, false).await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(location(&response), "http://alias.test/dash");
    let cookies = set_cookies(&response);
    assert_cleared(&cookies, false);
    let session = cookie_named(&cookies, "access_token").expect("a session cookie");
    assert!(
        session.contains("HttpOnly") && !session.contains("Secure"),
        "{session}"
    );

    // The cookie is a real session: it authenticates the API as the new user.
    let identity = me(&server, value_of(&session)).await;
    let user_id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind("new.person@corp.test")
        .fetch_one(&server.db)
        .await
        .expect("the user was created under the asserted address, lowercased");
    assert_eq!(identity["sub"], user_id.to_string());
    // Linked to the dashboard's user, so the next sign-in finds the same account.
    let linked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM user_identities
         WHERE user_id = $1 AND provider = 'nasiko' AND provider_id = 'portal-user-1'",
    )
    .bind(user_id)
    .fetch_one(&server.db)
    .await
    .expect("count identities");
    assert_eq!(linked, 1);

    // The same assertion again: refused, no session, and still one user.
    let replay = present(&server, Some(&token), &started.cookies, false).await;
    assert_retry(replay, "http://alias.test", false, "a replayed assertion").await;
    assert_eq!(user_rows(&server, "new.person@corp.test").await, 1);

    server.cleanup().await;
}

/// Over plain http the return cookie has no `__Host-` prefix, so a sibling host can plant one. The
/// workspace re-checks it against its allowed origins at login, instead of trusting what `start`
/// stored: a forged target (or one that does not even decode) lands on the dashboard's own page.
#[tokio::test]
#[serial]
async fn a_forged_return_cookie_is_not_followed_at_login() {
    let server = start_server().await;
    let dashboard = &dashboard().url;

    for (forged, what) in [
        (
            URL_SAFE_NO_PAD.encode("https://evil.test/x"),
            "an unlisted origin",
        ),
        (
            URL_SAFE_NO_PAD.encode(format!("{dashboard}@evil.test/x")),
            "a look-alike origin",
        ),
        ("!!not-base64!!".to_owned(), "a value that does not decode"),
    ] {
        let started = start(&server, None, false).await;
        let claims = login_claims("forged.cookie@corp.test", &started.state);
        let cookies = format!("{STATE_COOKIE}={}; {RETURN_COOKIE}={forged}", started.state);
        let response = present(&server, Some(&sign(&claims)), &cookies, false).await;

        assert_eq!(response.status(), StatusCode::FOUND, "{what}");
        assert_eq!(location(&response), format!("{dashboard}/"), "{what}");
        let set = set_cookies(&response);
        assert!(
            cookie_named(&set, "access_token").is_some(),
            "{what}: {set:?}"
        );
        assert_cleared(&set, false);
    }

    server.cleanup().await;
}

/// The same assertion presented twice at once (a double click, a retrying proxy): the single-use
/// marker lets exactly one through, whichever arrives first.
#[tokio::test]
#[serial]
async fn two_concurrent_presentations_of_one_assertion_sign_in_once() {
    let server = start_server().await;
    let dashboard = &dashboard().url;
    let started = start(&server, None, false).await;
    let token = sign(&login_claims("twice.person@corp.test", &started.state));

    let (first, second) = tokio::join!(
        present(&server, Some(&token), &started.cookies, false),
        present(&server, Some(&token), &started.cookies, false),
    );

    let signed_in = |r: &Response| cookie_named(&set_cookies(r), "access_token").is_some();
    assert_eq!(
        [signed_in(&first), signed_in(&second)]
            .iter()
            .filter(|s| **s)
            .count(),
        1,
        "exactly one presentation signs in"
    );
    let (winner, loser) = if signed_in(&first) {
        (first, second)
    } else {
        (second, first)
    };
    assert_eq!(winner.status(), StatusCode::FOUND);
    assert_retry(loser, dashboard, false, "the second of two at once").await;
    assert_eq!(user_rows(&server, "twice.person@corp.test").await, 1);

    server.cleanup().await;
}

/// Each case differs from a valid sign-in in exactly one thing, and a valid one runs last on the
/// same fixtures, so a refusal cannot come from the setup. The single-use marker tells apart a
/// refusal before the assertion was accepted (not recorded) from one after (recorded).
#[tokio::test]
#[serial]
async fn every_refusal_goes_to_the_retry_page_and_leaves_no_session_or_user() {
    let server = start_server().await;
    let dashboard = &dashboard().url;
    let email = "refused.person@corp.test";

    type Tamper = fn(&mut AssertionClaims);
    let cases: [(&str, Tamper, bool); 6] = [
        (
            "a state that is not this browser's",
            |c| c.nonce = Some("another-browsers-state".to_owned()),
            true,
        ),
        ("no state in the assertion", |c| c.nonce = None, true),
        (
            "another workspace's audience",
            |c| c.aud = "https://other.test".to_owned(),
            false,
        ),
        (
            "a delegation assertion instead of a login one",
            |c| c.token_type = ASSERTION_TOKEN_TYPE.to_owned(),
            false,
        ),
        (
            "a lifetime of 61 seconds",
            |c| c.exp = c.iat + ASSERTION_MAX_TTL_SECS + 1,
            false,
        ),
        ("an unverified address", |c| c.email_verified = false, false),
    ];
    for (what, tamper, accepted_before_refusal) in cases {
        // Once for the dashboard's own origin, once for an alias, so the retry page follows the
        // return target the workspace remembered.
        for (return_to, origin) in [
            (None, dashboard.as_str()),
            (Some("http://alias.test/x"), ALIAS),
        ] {
            let started = start(&server, return_to, false).await;
            let mut claims = login_claims(email, &started.state);
            tamper(&mut claims);
            let response = present(&server, Some(&sign(&claims)), &started.cookies, false).await;
            assert_retry(response, origin, false, what).await;
            assert_eq!(
                jti_consumed(&server, &claims.jti).await,
                accepted_before_refusal,
                "{what}"
            );
        }
    }

    // No state cookie at all (blocked, expired or never set): refused before the assertion is read.
    let started = start(&server, None, false).await;
    let claims = login_claims(email, &started.state);
    let response = present(&server, Some(&sign(&claims)), "", false).await;
    assert_retry(response, dashboard, false, "no cookies").await;
    assert!(!jti_consumed(&server, &claims.jti).await);

    // A state cookie that is another browser's.
    let other = start(&server, None, false).await;
    let claims = login_claims(email, &started.state);
    let response = present(&server, Some(&sign(&claims)), &other.cookies, false).await;
    assert_retry(response, dashboard, false, "another browser's cookies").await;
    assert!(jti_consumed(&server, &claims.jti).await);

    // Not an assertion at all, and no assertion.
    for token in [Some("not.a.jwt"), None] {
        let started = start(&server, None, false).await;
        let response = present(&server, token, &started.cookies, false).await;
        assert_retry(
            response,
            dashboard,
            false,
            "a missing or malformed assertion",
        )
        .await;
    }

    assert_eq!(user_rows(&server, email).await, 0);

    // Positive control: the untampered assertion on fresh cookies signs in.
    let started = start(&server, None, false).await;
    let token = sign(&login_claims(email, &started.state));
    let response = present(&server, Some(&token), &started.cookies, false).await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(location(&response), format!("{dashboard}/"));
    assert!(cookie_named(&set_cookies(&response), "access_token").is_some());
    assert_eq!(user_rows(&server, email).await, 1);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_address_stored_with_capitals_is_the_account_that_signs_in() {
    let server = start_server().await;
    let seeded: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO users (username, email, is_active) VALUES ('jane', 'Jane@Corp.com', true)
         RETURNING id",
    )
    .fetch_one(&server.db)
    .await
    .expect("seed user");

    let started = start(&server, None, false).await;
    // The dashboard's assertion carries the address however the person typed it.
    let token = sign(&login_claims("jane@corp.com", &started.state));
    let response = present(&server, Some(&token), &started.cookies, false).await;
    assert_eq!(response.status(), StatusCode::FOUND);
    let session = cookie_named(&set_cookies(&response), "access_token").expect("session cookie");

    assert_eq!(
        me(&server, value_of(&session)).await["sub"],
        seeded.to_string()
    );
    // One row, still under its stored spelling: no second, empty account beside it.
    assert_eq!(user_rows(&server, "jane@corp.com").await, 1);
    let stored: String = sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
        .bind(seeded)
        .fetch_one(&server.db)
        .await
        .expect("stored email");
    assert_eq!(stored, "Jane@Corp.com");

    server.cleanup().await;
}

/// The unique index on email is case-sensitive, so rows differing only in case can exist; the one
/// spelled exactly as the assertion carries it is the account.
#[tokio::test]
#[serial]
async fn with_two_rows_differing_only_in_case_the_exact_spelling_is_the_account() {
    let server = start_server().await;
    let mut ids = Vec::new();
    for (username, email) in [
        ("jane-mixed", "Jane@Corp.com"),
        ("jane-lower", "jane@corp.com"),
    ] {
        let id: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO users (username, email, is_active) VALUES ($1, $2, true) RETURNING id",
        )
        .bind(username)
        .bind(email)
        .fetch_one(&server.db)
        .await
        .expect("seed user");
        ids.push(id);
    }

    let started = start(&server, None, false).await;
    let token = sign(&login_claims("jane@corp.com", &started.state));
    let response = present(&server, Some(&token), &started.cookies, false).await;
    let session = cookie_named(&set_cookies(&response), "access_token").expect("session cookie");

    assert_eq!(
        me(&server, value_of(&session)).await["sub"],
        ids[1].to_string()
    );
    assert_eq!(user_rows(&server, "jane@corp.com").await, 2);

    server.cleanup().await;
}

/// A failure while saving the user is not retried: the sign-in page shows its own code and no
/// session is set.
#[tokio::test]
#[serial]
async fn a_database_failure_while_saving_the_user_ends_on_the_refused_page() {
    let server = start_server().await;
    sqlx::query("ALTER TABLE users ADD CONSTRAINT refuse_boom CHECK (email <> 'boom@corp.test')")
        .execute(&server.db)
        .await
        .expect("add the failing constraint");

    let started = start(&server, None, false).await;
    let token = sign(&login_claims("boom@corp.test", &started.state));
    let response = present(&server, Some(&token), &started.cookies, false).await;

    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(
        location(&response),
        format!(
            "{}/login?signin=external#error=user_save_failed",
            dashboard().url
        )
    );
    let cookies = set_cookies(&response);
    assert!(
        cookie_named(&cookies, "access_token").is_none(),
        "{cookies:?}"
    );
    assert_cleared(&cookies, false);
    assert_eq!(user_rows(&server, "boom@corp.test").await, 0);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_deactivated_account_is_refused_with_its_own_code() {
    let server = start_server().await;
    sqlx::query(
        "INSERT INTO users (username, email, is_active) VALUES ('gone', 'gone@corp.test', false)",
    )
    .execute(&server.db)
    .await
    .expect("seed user");

    let started = start(&server, None, false).await;
    let token = sign(&login_claims("gone@corp.test", &started.state));
    let response = present(&server, Some(&token), &started.cookies, false).await;

    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(
        location(&response),
        format!(
            "{}/login?signin=external#error=account_deactivated",
            dashboard().url
        )
    );
    let cookies = set_cookies(&response);
    assert!(
        cookie_named(&cookies, "access_token").is_none(),
        "{cookies:?}"
    );
    assert_cleared(&cookies, false);

    server.cleanup().await;
}

// ─── the exchange, which shares the verification step ────────────────────────

async fn exchange(server: &common::TestServer, bearer: Option<&str>) -> (StatusCode, Value) {
    let mut request = browser()
        .post(server.url("/api/auth/delegate/assertion"))
        .json(&serde_json::json!({ "scopes": ["me:read"] }));
    if let Some(token) = bearer {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.expect("exchange");
    let status = response.status();
    (status, response.json().await.expect("exchange body"))
}

/// The delegation exchange keeps every answer it gave before the verification step was shared with
/// the browser sign-in, and refuses a login assertion outright.
#[tokio::test]
#[serial]
async fn the_delegation_exchange_answers_as_before() {
    let server = start_server().await;
    sqlx::query(
        "INSERT INTO users (username, email, is_active) VALUES ('member', 'member@corp.test', true)",
    )
    .execute(&server.db)
    .await
    .expect("seed user");
    let delegation = |email: &str| {
        let mut claims = login_claims(email, "unused");
        claims.token_type = ASSERTION_TOKEN_TYPE.to_owned();
        claims.nonce = None;
        claims
    };

    let (status, body) = exchange(&server, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["message"], "missing assertion");

    let claims = delegation("member@corp.test");
    let token = sign(&claims);
    let (status, body) = exchange(&server, Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["aud"], WORKSPACE_AUDIENCE);
    assert_eq!(body["scopes"], serde_json::json!(["me:read"]));
    assert!(body["token"].as_str().is_some_and(|t| !t.is_empty()));

    let (status, body) = exchange(&server, Some(&token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["message"], "assertion already used");

    // A login assertion is not a delegation assertion, and is refused before it is spent.
    let mut login = delegation("member@corp.test");
    login.token_type = LOGIN_ASSERTION_TOKEN_TYPE.to_owned();
    let (status, body) = exchange(&server, Some(&sign(&login))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["message"], "invalid assertion");
    assert!(!jti_consumed(&server, &login.jti).await);

    let (status, body) = exchange(&server, Some("not.a.jwt")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["message"], "invalid assertion");

    // Verified and spent, but not a member: the exchange never creates a user.
    let (status, body) = exchange(&server, Some(&sign(&delegation("stranger@corp.test")))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["message"], "not a member of this workspace");
    assert_eq!(user_rows(&server, "stranger@corp.test").await, 0);

    server.cleanup().await;
}

/// With Redis unreachable the single use of an assertion cannot be recorded, so nothing is
/// admitted: the browser is sent to retry, the exchange answers 503, and no user is created. The
/// stub dashboard still serves the key document, so the refusal comes from Redis alone.
#[tokio::test]
#[serial]
async fn without_redis_no_assertion_is_admitted() {
    let server = common::TestServer::start_with(|c| {
        enable_sign_in(c);
        // Nothing listens on port 1.
        c.redis_url = "redis://127.0.0.1:1".to_owned();
    })
    .await;
    let dashboard = &dashboard().url;
    sqlx::query(
        "INSERT INTO users (username, email, is_active) VALUES ('member', 'member@corp.test', true)",
    )
    .execute(&server.db)
    .await
    .expect("seed user");

    let started = start(&server, None, false).await;
    let token = sign(&login_claims("new.person@corp.test", &started.state));
    let response = present(&server, Some(&token), &started.cookies, false).await;
    assert_retry(response, dashboard, false, "redis down").await;
    assert_eq!(user_rows(&server, "new.person@corp.test").await, 0);

    let mut claims = login_claims("member@corp.test", "unused");
    claims.token_type = ASSERTION_TOKEN_TYPE.to_owned();
    claims.nonce = None;
    let (status, body) = exchange(&server, Some(&sign(&claims))).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["message"], "delegation temporarily unavailable");

    server.cleanup().await;
}

// ─── off ─────────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn both_routes_answer_404_when_sign_in_is_off() {
    type Off = fn(&mut Config);
    let variants: [(&str, Off); 3] = [
        ("no dashboard URL", |c| c.nasiko_bff_url = None),
        ("personal emails not allowed", |c| {
            c.allow_personal_emails = false
        }),
        ("the audience is the dashboard's origin", |c| {
            c.delegation_audience = dashboard().url.clone()
        }),
    ];
    for (what, turn_off) in variants {
        let server = common::TestServer::start_with(|c| {
            enable_sign_in(c);
            turn_off(c);
        })
        .await;

        let cookies = "assertion_state=abc; assertion_return=def";
        let start = browser()
            .get(server.url("/api/auth/assertion/start"))
            .send()
            .await
            .expect("start");
        assert_eq!(start.status(), StatusCode::NOT_FOUND, "{what}");
        assert!(set_cookies(&start).is_empty(), "{what}");

        let token = sign(&login_claims("off.person@corp.test", "abc"));
        let login = present(&server, Some(&token), cookies, false).await;
        assert_eq!(login.status(), StatusCode::NOT_FOUND, "{what}");
        assert!(set_cookies(&login).is_empty(), "{what}");
        assert_eq!(
            user_rows(&server, "off.person@corp.test").await,
            0,
            "{what}"
        );

        server.cleanup().await;
    }
}
