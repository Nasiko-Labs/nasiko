use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;

use crate::error::OidcError;
use crate::jwks::Jwks;

/// Claims we actually care about after signature/iss/aud/exp validation.
/// `sub`, `iss`, `aud`, `exp`, `nbf` are already enforced by `jsonwebtoken`
/// itself during `decode` (against the raw JWT payload, independent of this
/// struct's shape) — they're intentionally not re-declared/re-checked here.
///
/// `oid` (Entra's stable per-tenant user object id) and `tid` (tenant id) are
/// Microsoft-specific but harmless `None` on any other OIDC provider; `sub`
/// is always present and is the fallback identifier.
#[derive(Debug, Clone, Deserialize)]
pub struct IdTokenClaims {
    pub sub: String,
    #[serde(default)]
    pub oid: Option<String>,
    #[serde(default)]
    pub tid: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub preferred_username: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// Entra "App Roles" claim, when the app manifest defines any — captured
    /// for future role-mapping, not acted on today (see the enterprise OIDC SSO guide).
    #[serde(default, deserialize_with = "string_or_seq_opt")]
    pub roles: Option<Vec<String>>,
    /// Entra Security Group Object IDs the user belongs to — only present
    /// when the App Registration has Group Claims enabled (Token
    /// configuration → Add groups claim), which is off by default. Used to
    /// auto-assign role/team/department at login; see
    /// `oidc_group_mappings` and `ee/server/src/oidc_group_mappings.rs`.
    /// `None` (not an empty list) if the claim was omitted entirely — e.g.
    /// group-overage tenants, where Entra requires a separate Graph API
    /// call instead (not implemented; see the enterprise OIDC SSO guide).
    ///
    /// Deserialized permissively (bare string OR array): several OIDC
    /// providers (confirmed against a local oidc-server-mock instance, not
    /// just a hypothetical) collapse a single-valued multi-valued claim to a
    /// scalar rather than a one-element array, which a plain
    /// `Vec<String>`/`Option<Vec<String>>` rejects outright.
    #[serde(default, deserialize_with = "string_or_seq_opt")]
    pub groups: Option<Vec<String>>,
    #[serde(default)]
    pub nonce: Option<String>,
}

/// Accepts a claim that's missing, a bare string, or an array of strings,
/// normalizing all three to `Option<Vec<String>>`. See `groups`'s doc
/// comment for why this is needed instead of a plain `Vec<String>`.
fn string_or_seq_opt<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrVec {
        Single(String),
        Multiple(Vec<String>),
    }

    Ok(
        Option::<StringOrVec>::deserialize(deserializer)?.map(|v| match v {
            StringOrVec::Single(s) => vec![s],
            StringOrVec::Multiple(v) => v,
        }),
    )
}

impl IdTokenClaims {
    /// The stable identifier to store as `user_identities.provider_id`.
    /// Prefers `oid` (Microsoft's documented stable-per-tenant identifier)
    /// over `sub`, which can be pairwise/app-scoped depending on tenant config.
    pub fn stable_subject(&self) -> &str {
        self.oid.as_deref().unwrap_or(&self.sub)
    }

    /// Best-effort display name for the local account.
    pub fn display_username(&self) -> &str {
        self.preferred_username
            .as_deref()
            .or(self.email.as_deref())
            .or(self.name.as_deref())
            .unwrap_or(&self.sub)
    }
}

/// Verify an `id_token`'s signature, issuer, audience, expiry and nonce.
///
/// Security notes:
/// - The signing algorithm is pinned to `RS256` from the token's own header
///   *before* touching the JWKS — never trust `alg` for key selection past
///   that check (blocks alg:none / HMAC-confusion attacks).
/// - `jwks` must have been fetched from the issuer's own discovery-provided
///   `jwks_uri` — never a separately configurable URL (blocks key confusion).
/// - `nonce` is checked here, not by `jsonwebtoken` (it isn't a registered
///   claim it understands) — replay protection depends on this call site.
pub fn verify(
    id_token: &str,
    jwks: &Jwks,
    expected_issuer: &str,
    expected_client_id: &str,
    expected_nonce: &str,
) -> Result<IdTokenClaims, OidcError> {
    let header = decode_header(id_token).map_err(|e| OidcError::InvalidToken(e.to_string()))?;
    if header.alg != Algorithm::RS256 {
        return Err(OidcError::UnsupportedAlgorithm(format!("{:?}", header.alg)));
    }

    let jwk = header
        .kid
        .as_deref()
        .and_then(|kid| jwks.find_rsa(kid))
        .ok_or_else(|| OidcError::UnknownKid(header.kid.clone()))?;
    let (n, e) = match (jwk.n.as_deref(), jwk.e.as_deref()) {
        (Some(n), Some(e)) => (n, e),
        _ => return Err(OidcError::UnknownKid(header.kid.clone())),
    };
    let decoding_key = DecodingKey::from_rsa_components(n, e)
        .map_err(|e| OidcError::InvalidToken(e.to_string()))?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&[expected_client_id]);
    validation.set_issuer(&[expected_issuer]);
    validation.leeway = 60;

    let data = decode::<IdTokenClaims>(id_token, &decoding_key, &validation)
        .map_err(|e| OidcError::InvalidToken(e.to_string()))?;

    let claims = data.claims;
    if claims.nonce.as_deref() != Some(expected_nonce) {
        return Err(OidcError::NonceMismatch);
    }
    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jwks::{Jwk, Jwks};
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde_json::{Value, json};

    // TEST-ONLY 2048-bit RSA keypair, generated solely for these unit tests
    // (`openssl genrsa 2048`) — never used for any real deployment.
    const TEST_KID: &str = "test-key-1";
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

    fn test_jwks() -> Jwks {
        Jwks {
            keys: vec![Jwk {
                kty: "RSA".into(),
                kid: Some(TEST_KID.into()),
                n: Some(TEST_JWK_N.into()),
                e: Some(TEST_JWK_E.into()),
            }],
        }
    }

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    const ISS: &str = "https://login.microsoftonline.com/test-tenant/v2.0";
    const AUD: &str = "test-client-id";
    const NONCE: &str = "test-nonce-value";

    fn valid_claims() -> Value {
        let n = now();
        json!({
            "iss": ISS, "aud": AUD, "sub": "user-123", "oid": "oid-123", "tid": "tid-123",
            "preferred_username": "alice@example.com", "email": "alice@example.com",
            "name": "Alice Example", "nonce": NONCE,
            "iat": n, "nbf": n - 5, "exp": n + 3600,
        })
    }

    fn sign_rs256(claims: &Value, kid: &str) -> String {
        let key = EncodingKey::from_rsa_pem(TEST_RSA_PRIVATE_KEY_PEM.as_bytes()).unwrap();
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_string());
        encode(&header, claims, &key).unwrap()
    }

    #[test]
    fn valid_token_verifies_and_maps_claims() {
        let token = sign_rs256(&valid_claims(), TEST_KID);
        let claims = verify(&token, &test_jwks(), ISS, AUD, NONCE).expect("should verify");
        assert_eq!(claims.stable_subject(), "oid-123");
        assert_eq!(claims.display_username(), "alice@example.com");
    }

    #[test]
    fn stable_subject_falls_back_to_sub_when_oid_absent() {
        let mut claims = valid_claims();
        claims.as_object_mut().unwrap().remove("oid");
        let token = sign_rs256(&claims, TEST_KID);
        let decoded = verify(&token, &test_jwks(), ISS, AUD, NONCE).unwrap();
        assert_eq!(decoded.stable_subject(), "user-123");
    }

    #[test]
    fn tampered_signature_rejected() {
        let mut token = sign_rs256(&valid_claims(), TEST_KID);
        // Flip a character in the signature segment (last dot-separated part).
        let last = token.pop().unwrap();
        token.push(if last == 'A' { 'B' } else { 'A' });
        assert!(verify(&token, &test_jwks(), ISS, AUD, NONCE).is_err());
    }

    #[test]
    fn expired_token_rejected() {
        let mut claims = valid_claims();
        let n = now();
        claims["iat"] = json!(n - 7200);
        claims["nbf"] = json!(n - 7200);
        claims["exp"] = json!(n - 3600);
        let token = sign_rs256(&claims, TEST_KID);
        assert!(verify(&token, &test_jwks(), ISS, AUD, NONCE).is_err());
    }

    #[test]
    fn wrong_audience_rejected() {
        let mut claims = valid_claims();
        claims["aud"] = json!("some-other-client-id");
        let token = sign_rs256(&claims, TEST_KID);
        assert!(verify(&token, &test_jwks(), ISS, AUD, NONCE).is_err());
    }

    #[test]
    fn wrong_issuer_rejected() {
        let mut claims = valid_claims();
        claims["iss"] = json!("https://login.microsoftonline.com/some-other-tenant/v2.0");
        let token = sign_rs256(&claims, TEST_KID);
        assert!(verify(&token, &test_jwks(), ISS, AUD, NONCE).is_err());
    }

    #[test]
    fn nonce_mismatch_rejected() {
        let token = sign_rs256(&valid_claims(), TEST_KID);
        match verify(&token, &test_jwks(), ISS, AUD, "a-different-nonce") {
            Err(OidcError::NonceMismatch) => {}
            other => panic!("expected NonceMismatch, got {other:?}"),
        }
    }

    #[test]
    fn missing_nonce_rejected() {
        let mut claims = valid_claims();
        claims.as_object_mut().unwrap().remove("nonce");
        let token = sign_rs256(&claims, TEST_KID);
        match verify(&token, &test_jwks(), ISS, AUD, NONCE) {
            Err(OidcError::NonceMismatch) => {}
            other => panic!("expected NonceMismatch, got {other:?}"),
        }
    }

    #[test]
    fn unknown_kid_rejected() {
        let token = sign_rs256(&valid_claims(), "some-other-kid-not-in-jwks");
        match verify(&token, &test_jwks(), ISS, AUD, NONCE) {
            Err(OidcError::UnknownKid(Some(kid))) => assert_eq!(kid, "some-other-kid-not-in-jwks"),
            other => panic!("expected UnknownKid, got {other:?}"),
        }
    }

    /// Reproduces a real interop bug found testing against a local
    /// oidc-server-mock instance: a provider that has exactly one group to
    /// report sends `"groups": "grp-x"` (a bare string) rather than
    /// `["grp-x"]`, which a plain `Vec<String>` field rejects with a
    /// deserialize error (surfacing as a full login failure).
    #[test]
    fn single_valued_groups_and_roles_claims_deserialize_as_scalar() {
        let mut claims = valid_claims();
        claims["groups"] = json!("grp-solo");
        claims["roles"] = json!("Nasiko.Admin");
        let token = sign_rs256(&claims, TEST_KID);
        let decoded = verify(&token, &test_jwks(), ISS, AUD, NONCE).expect("should verify");
        assert_eq!(decoded.groups, Some(vec!["grp-solo".to_string()]));
        assert_eq!(decoded.roles, Some(vec!["Nasiko.Admin".to_string()]));
    }

    #[test]
    fn multi_valued_groups_claim_deserializes_as_array() {
        let mut claims = valid_claims();
        claims["groups"] = json!(["grp-a", "grp-b"]);
        let token = sign_rs256(&claims, TEST_KID);
        let decoded = verify(&token, &test_jwks(), ISS, AUD, NONCE).expect("should verify");
        assert_eq!(
            decoded.groups,
            Some(vec!["grp-a".to_string(), "grp-b".to_string()])
        );
    }

    /// Blocks the classic alg:none / HMAC-confusion attack: a token whose
    /// header claims HS256 must be rejected before the JWKS is ever
    /// consulted, regardless of what it's "signed" with.
    #[test]
    fn non_rs256_algorithm_rejected() {
        let key = EncodingKey::from_secret(b"attacker-controlled-secret");
        let header = Header::new(Algorithm::HS256);
        let token = encode(&header, &valid_claims(), &key).unwrap();
        match verify(&token, &test_jwks(), ISS, AUD, NONCE) {
            Err(OidcError::UnsupportedAlgorithm(_)) => {}
            other => panic!("expected UnsupportedAlgorithm, got {other:?}"),
        }
    }
}
