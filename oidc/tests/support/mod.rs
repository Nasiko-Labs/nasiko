//! Shared test-only fixtures for `nasiko-oidc`'s HTTP-level integration tests.
//!
//! The RSA keypair below is generated solely for these tests
//! (`openssl genrsa 2048`) — it signs nothing outside this test binary and
//! must never be reused for any real deployment.

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};

pub const TEST_KID: &str = "test-key-1";
pub const OTHER_KID: &str = "test-key-2-rotated";

const TEST_RSA_PRIVATE_KEY_PEM: &str = "-----BEGIN RSA PRIVATE KEY-----\n\
MIIEpAIBAAKCAQEAngFgNBoiZNwUpbCb+9BTqZXwiKdRmCnd2Pm++qjLzmJUWZqw\n\
XuG7cpm4vzy6kpcbpvLluL40zlsHdfPhE3hrsULCKYxH72lt4p1++N5uNCWGTVLr\n\
Ow4X6nZf8uHoWwlwiE1+hO2qP5NSOWxqh8pmoT+9OVjwjXTZooPPSps1bLQJeOPQ\n\
aPVKdi5gWpgkY+GWFzVa0WXMqRu9XDFa5ebQu8Uaq3KcN7e+awm8c0dYgfWnYOzI\n\
YH+i2UTSHipUQxWaP2TlhrKJUMKZuncVQVEjUFwxmk8ubSRJHeeFWpkK/s84qU77\n\
vcdd+8hqDYWgxGLOE5V4Owz5u8VKqC0NNQkYawIDAQABAoIBACiKM9PKbM6yBP4q\n\
HStz3TNizC9TtsSy4T/dfFm122zdn8TJwrzlcAHMXTF79GbOLIMeSUCoVMYpZvWl\n\
mDc1q3P0q/qbCo3r4AzH2h8ieuYYRqgqQT6KtCotKxsXSVWqS1w9fdu/WvIq62re\n\
XUrv7Hss7nD7V+UDeR+QcCw1PHTiKU8ltfQYXY4hnO4rKy3jyeux5rkSVj+kiQKH\n\
CNevD+JRDMH/KM/2VVlSyC7qLgUmOV/aG9Hgg8+aw8W/nzC36UePJdr26I5PZ4qW\n\
jHFjO2fvPqGU2HuDpN6F97ayvIOVkKSAxI0yYqBhClqnALha+GKoQ0T1wbKpriyf\n\
u/OvJ/kCgYEA0SacWFMR/ynlmxgmTpY1zkD4tWIdo6ISzB0LncVhKzg/9sbZlinC\n\
J31JmiDnPigeyOXzdJkCBZkO9gcatxgDQqXGM46XU6/ySGyAex5h5IdwKYm8bwEq\n\
KDVLjzKKIStyTh8S/Br4QeYlTmLxdljqpZLvyvLaBoj3Zk3fT7RIKs0CgYEAwWXr\n\
IYWdmggTcCxSa/Y23+JbunHmStOA1zJT5NJUmYqhhH3hGW73t1y0Z0sbl/p/8cTG\n\
ZB2I0wG4Y4zFVaNrd2mBBC4FFJKMilXsoSvQvQ3o10m/y8s4OTRSumPiOeq3VL68\n\
yCSo5h4EBB5i2Q36/H6BgSxaXxaOmzHDIKtCQBcCgYEAlT/XS9QjwJF2TsHh/CyG\n\
wuNsV4tnmTB793o2ouSKHZxrUL+/37921FU8o6cdPSbGKRinLapOXg5GNd0F/Gg/\n\
U10W3g3AATFKVNJQsQsSUlEwAgRPGmubWMwHWm13Uoo9bHASTSM1y1jfgFts8cYr\n\
0/HR+mJooUc2PKQPWkJNSXUCgYAMukFcJmf10Bw/YJtYAY8g8suonIBUYlDzWJuO\n\
zozEwgvZJVOgEd55kb9JoPbC7Lho19NamVr8z/srigMenK+g3y+fb8vjy7U2EWuO\n\
O8zz9Ctjp7XYmporoZbkL1ifCSRhjl/sKAV5h3YqMzm8ISBoZ4bsUlfsNBbUfdTi\n\
nIKypwKBgQDCcoGofKAjNYl5u1ReI/IN2KNSTfubXlh20kiF5RMCbfhDMZHkKqmZ\n\
4v2hyi1/Q30u8Hg4OMHotVPFhX9zaaVkRpz7+vaU3DkDZoZoILAflctswjDgQpXx\n\
SG9MPTaBjgiZpuXgkxtEcCr35VYhcLOoR5RvoXyqGWKxecnhYsrPPA==\n\
-----END RSA PRIVATE KEY-----\n";
const TEST_JWK_N: &str = "ngFgNBoiZNwUpbCb-9BTqZXwiKdRmCnd2Pm--qjLzmJUWZqwXuG7cpm4vzy6kpcbpvLluL40zlsHdfPhE3hrsULCKYxH72lt4p1--N5uNCWGTVLrOw4X6nZf8uHoWwlwiE1-hO2qP5NSOWxqh8pmoT-9OVjwjXTZooPPSps1bLQJeOPQaPVKdi5gWpgkY-GWFzVa0WXMqRu9XDFa5ebQu8Uaq3KcN7e-awm8c0dYgfWnYOzIYH-i2UTSHipUQxWaP2TlhrKJUMKZuncVQVEjUFwxmk8ubSRJHeeFWpkK_s84qU77vcdd-8hqDYWgxGLOE5V4Owz5u8VKqC0NNQkYaw";
const TEST_JWK_E: &str = "AQAB";

/// Sign a fixture `id_token` with the test-only RSA key under the given `kid`.
pub fn sign_id_token(claims: &Value, kid: &str) -> String {
    let key = EncodingKey::from_rsa_pem(TEST_RSA_PRIVATE_KEY_PEM.as_bytes()).unwrap();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(kid.to_string());
    encode(&header, claims, &key).unwrap()
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

pub fn valid_claims(iss: &str, aud: &str, nonce: &str) -> Value {
    let n = now();
    json!({
        "iss": iss, "aud": aud, "sub": "user-123", "oid": "oid-123", "tid": "tid-123",
        "preferred_username": "alice@example.com", "email": "alice@example.com",
        "name": "Alice Example", "nonce": nonce,
        "iat": n, "nbf": n - 5, "exp": n + 3600,
    })
}

/// JWKS document containing only `TEST_KID` — the "normal" state.
pub fn jwks_json_with_test_key() -> Value {
    json!({ "keys": [ { "kty": "RSA", "kid": TEST_KID, "n": TEST_JWK_N, "e": TEST_JWK_E } ] })
}

/// JWKS document containing a decoy key only (no `n`/`e` match for any token
/// signed with `TEST_KID` or `OTHER_KID`) — simulates the state right before
/// a rotation lands, to exercise the unknown-kid forced-refetch path.
pub fn jwks_json_decoy_only() -> Value {
    json!({ "keys": [ { "kty": "RSA", "kid": "decoy-key-not-used", "n": "AA", "e": "AQAB" } ] })
}

/// JWKS document containing `OTHER_KID` with the *same* test keypair (fine —
/// we're only testing that the client refetches and finds a `kid` match, not
/// exercising a second distinct keypair).
pub fn jwks_json_with_rotated_key() -> Value {
    json!({ "keys": [ { "kty": "RSA", "kid": OTHER_KID, "n": TEST_JWK_N, "e": TEST_JWK_E } ] })
}

pub fn discovery_json(issuer: &str, authz: &str, token: &str, jwks: &str) -> Value {
    json!({
        "issuer": issuer,
        "authorization_endpoint": authz,
        "token_endpoint": token,
        "jwks_uri": jwks,
    })
}
