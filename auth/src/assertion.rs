//! BFF → workspace delegation assertions (central-dashboard mode).
//!
//! The dashboard (EE tenant-server) signs a short-lived **assertion** with an
//! Ed25519 key and publishes the public half as a JWKS. A workspace control
//! plane verifies the assertion here, then mints its own delegated token
//! through the ordinary [`crate::AuthService::issue_delegated_token`] policy.
//! Verification is pure: no I/O, no clock injection beyond `exp`, so it is
//! unit-tested exhaustively and the server only adds caching and replay checks.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The only accepted `token_type` for an assertion.
pub const ASSERTION_TOKEN_TYPE: &str = "bff-assertion";

/// Assertion claims the BFF signs. `aud` is the target workspace's
/// [`delegation audience`](https://datatracker.ietf.org/doc/html/rfc7519#section-4.1.3)
/// (its public origin), so an assertion for one workspace is worthless at
/// another; `ws` is informational (the dashboard's workspace id) and never
/// trusted for authorization on the CP side.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertionClaims {
    pub iss: String,
    pub aud: String,
    pub sub: String,
    pub email: String,
    pub email_verified: bool,
    pub ws: String,
    pub jti: String,
    pub iat: u64,
    pub exp: u64,
    pub token_type: String,
}

/// Longest an assertion may live. The BFF signs for less; the CP keeps the
/// consumed-`jti` marker for exactly this long.
pub const ASSERTION_MAX_TTL_SECS: u64 = 60;

/// Ed25519 signer for the BFF side. Holds one private key and the matching
/// public JWK; the `kid` is derived from the public key so rotation never
/// needs a coordinated name. Constructed once at boot from PEM material and
/// self-checked (sign → verify) so a mismatched key pair fails loudly there,
/// not on the first login.
pub struct AssertionSigner {
    key: EncodingKey,
    jwk: Jwk,
}

impl AssertionSigner {
    /// `private_pkcs8_pem` from `openssl genpkey -algorithm ed25519`,
    /// `public_spki_pem` from `openssl pkey -in key.pem -pubout`.
    pub fn from_pem(private_pkcs8_pem: &str, public_spki_pem: &str) -> Result<Self, String> {
        let key = EncodingKey::from_ed_pem(private_pkcs8_pem.as_bytes())
            .map_err(|e| format!("delegation private key: {e}"))?;
        let raw = raw_public_key_from_spki_pem(public_spki_pem)?;
        let x = URL_SAFE_NO_PAD.encode(raw);
        let kid: String = Sha256::digest(raw)[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let jwk = Jwk {
            kty: "OKP".into(),
            crv: "Ed25519".into(),
            kid,
            x,
            alg: Some("EdDSA".into()),
        };
        let signer = Self { key, jwk };
        signer.self_check()?;
        Ok(signer)
    }

    pub fn kid(&self) -> &str {
        &self.jwk.kid
    }

    pub fn jwk(&self) -> &Jwk {
        &self.jwk
    }

    pub fn sign(&self, claims: &AssertionClaims) -> Result<String, String> {
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some(self.jwk.kid.clone());
        encode(&header, claims, &self.key).map_err(|e| format!("sign assertion: {e}"))
    }

    fn self_check(&self) -> Result<(), String> {
        let now = now_secs();
        let probe = AssertionClaims {
            iss: "self-check".into(),
            aud: "self-check".into(),
            sub: "self-check".into(),
            email: "self-check@invalid".into(),
            email_verified: true,
            ws: String::new(),
            jti: "self-check".into(),
            iat: now,
            exp: now + 5,
            token_type: ASSERTION_TOKEN_TYPE.into(),
        };
        let token = self.sign(&probe)?;
        let jwks = Jwks {
            keys: vec![self.jwk.clone()],
        };
        verify_assertion(&token, &jwks, "self-check", "self-check")
            .map(|_| ())
            .map_err(|e| format!("delegation key pair does not match: {e}"))
    }
}

/// Current signer plus (during rotation) the previous one, whose public key
/// stays published until every assertion it signed has expired.
pub struct AssertionKeyRing {
    pub current: AssertionSigner,
    pub previous: Option<AssertionSigner>,
}

impl AssertionKeyRing {
    pub fn jwks(&self) -> Jwks {
        let mut keys = vec![self.current.jwk().clone()];
        if let Some(prev) = &self.previous {
            keys.push(prev.jwk().clone());
        }
        Jwks { keys }
    }
}

/// The 32 raw bytes of an Ed25519 public key from its SPKI PEM. The SPKI DER
/// for Ed25519 is a fixed 12-byte prefix followed by the key, so the key is
/// simply the trailing 32 bytes; anything else is not an Ed25519 SPKI.
pub fn raw_public_key_from_spki_pem(pem: &str) -> Result<[u8; 32], String> {
    const PREFIX: [u8; 12] = [
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];
    let body: String = pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .map(str::trim)
        .collect();
    let der = STANDARD
        .decode(body)
        .map_err(|e| format!("delegation public key: {e}"))?;
    if der.len() != 44 || der[..12] != PREFIX {
        return Err("delegation public key is not an Ed25519 SPKI".into());
    }
    let mut raw = [0u8; 32];
    raw.copy_from_slice(&der[12..]);
    Ok(raw)
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// One Ed25519 public key as published in the BFF's JWKS.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Jwk {
    pub kty: String,
    pub crv: String,
    pub kid: String,
    pub x: String,
    #[serde(default)]
    pub alg: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Jwks {
    pub keys: Vec<Jwk>,
}

#[derive(Debug, Clone)]
pub struct VerifiedAssertion {
    pub jti: String,
    pub email: String,
    pub workspace: String,
    pub portal_subject: String,
    pub expires_at: u64,
    pub issuer: String,
}

/// Returned when the header names a `kid` the JWKS does not contain; the
/// caller may refresh the key set once and retry (key rotation).
pub const UNKNOWN_KID: &str = "unknown kid";

/// Pure verification: signature by a key in `jwks` (selected by the header's
/// `kid`), EdDSA only, `iss` and `aud` exact, `exp` literal, verified email.
pub fn verify_assertion(
    token: &str,
    jwks: &Jwks,
    expected_iss: &str,
    expected_aud: &str,
) -> Result<VerifiedAssertion, String> {
    let header = jsonwebtoken::decode_header(token).map_err(|e| format!("header: {e}"))?;
    if header.alg != Algorithm::EdDSA {
        return Err("assertion must be EdDSA".into());
    }
    let kid = header.kid.ok_or("assertion has no kid")?;
    let jwk = jwks
        .keys
        .iter()
        .find(|k| k.kid == kid && k.kty == "OKP" && k.crv == "Ed25519")
        .ok_or(UNKNOWN_KID)?;
    let key = DecodingKey::from_ed_components(&jwk.x).map_err(|e| format!("jwk: {e}"))?;
    let mut validation = Validation::new(Algorithm::EdDSA);
    validation.leeway = 0;
    validation.set_issuer(&[expected_iss]);
    validation.set_audience(&[expected_aud]);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    let data =
        decode::<AssertionClaims>(token, &key, &validation).map_err(|e| format!("verify: {e}"))?;
    let c = data.claims;
    if c.token_type != ASSERTION_TOKEN_TYPE {
        return Err("not a BFF assertion".into());
    }
    if !c.email_verified {
        return Err("asserted email is not verified".into());
    }
    if c.jti.is_empty() || c.email.trim().is_empty() {
        return Err("assertion missing jti/email".into());
    }
    Ok(VerifiedAssertion {
        jti: c.jti,
        email: c.email.trim().to_lowercase(),
        workspace: c.ws,
        portal_subject: c.sub,
        expires_at: c.exp,
        issuer: c.iss,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // A throwaway Ed25519 keypair generated for these tests only (PKCS#8 PEM
    // and the raw public key). Not used anywhere else.
    const TEST_PRIVATE_PEM: &str = include_str!("testdata/delegation-test-ed25519.pem");
    const TEST_PUBLIC_X: &str = include_str!("testdata/delegation-test-ed25519.pub.x");

    fn jwks(kid: &str) -> Jwks {
        Jwks {
            keys: vec![Jwk {
                kty: "OKP".into(),
                crv: "Ed25519".into(),
                kid: kid.into(),
                x: TEST_PUBLIC_X.trim().into(),
                alg: Some("EdDSA".into()),
            }],
        }
    }

    fn sign(kid: &str, claims: serde_json::Value) -> String {
        let mut h = Header::new(Algorithm::EdDSA);
        h.kid = Some(kid.into());
        encode(
            &h,
            &claims,
            &EncodingKey::from_ed_pem(TEST_PRIVATE_PEM.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    fn claims(exp_offset: i64) -> serde_json::Value {
        let now = chrono::Utc::now().timestamp();
        serde_json::json!({
            "iss": "https://nasiko.dev", "aud": "https://acme.nasiko.dev", "sub": "portal-1",
            "email": "Satya@Acme.test", "email_verified": true, "ws": "ws-1", "jti": "j-1",
            "iat": now, "exp": now + exp_offset, "token_type": "bff-assertion"
        })
    }

    #[test]
    fn a_valid_assertion_verifies_and_lowercases_the_email() {
        let t = sign("k1", claims(60));
        let v = verify_assertion(
            &t,
            &jwks("k1"),
            "https://nasiko.dev",
            "https://acme.nasiko.dev",
        )
        .unwrap();
        assert_eq!(v.email, "satya@acme.test");
        assert_eq!(v.jti, "j-1");
        assert_eq!(v.workspace, "ws-1");
    }

    #[test]
    fn audience_issuer_kid_and_expiry_are_all_enforced() {
        let t = sign("k1", claims(60));
        assert!(
            verify_assertion(
                &t,
                &jwks("k1"),
                "https://nasiko.dev",
                "https://other.nasiko.dev"
            )
            .is_err()
        );
        assert!(
            verify_assertion(
                &t,
                &jwks("k1"),
                "https://evil.dev",
                "https://acme.nasiko.dev"
            )
            .is_err()
        );
        assert_eq!(
            verify_assertion(
                &t,
                &jwks("k2"),
                "https://nasiko.dev",
                "https://acme.nasiko.dev"
            )
            .unwrap_err(),
            UNKNOWN_KID
        );
        let expired = sign("k1", claims(-5));
        assert!(
            verify_assertion(
                &expired,
                &jwks("k1"),
                "https://nasiko.dev",
                "https://acme.nasiko.dev"
            )
            .is_err()
        );
    }

    #[test]
    fn unverified_email_and_wrong_type_are_refused() {
        let mut c = claims(60);
        c["email_verified"] = serde_json::json!(false);
        assert!(
            verify_assertion(
                &sign("k1", c),
                &jwks("k1"),
                "https://nasiko.dev",
                "https://acme.nasiko.dev"
            )
            .is_err()
        );
        let mut c = claims(60);
        c["token_type"] = serde_json::json!("user");
        assert!(
            verify_assertion(
                &sign("k1", c),
                &jwks("k1"),
                "https://nasiko.dev",
                "https://acme.nasiko.dev"
            )
            .is_err()
        );
    }

    #[test]
    fn an_hs256_token_is_never_accepted_as_an_assertion() {
        let t = encode(
            &Header::default(),
            &claims(60),
            &EncodingKey::from_secret(b"s"),
        )
        .unwrap();
        assert!(
            verify_assertion(
                &t,
                &jwks("k1"),
                "https://nasiko.dev",
                "https://acme.nasiko.dev"
            )
            .is_err()
        );
    }

    const TEST_PUBLIC_PEM: &str = include_str!("testdata/delegation-test-ed25519.pub.pem");

    #[test]
    fn signer_round_trips_through_verify_with_a_derived_kid() {
        let signer = AssertionSigner::from_pem(TEST_PRIVATE_PEM, TEST_PUBLIC_PEM).unwrap();
        assert_eq!(signer.jwk().x, TEST_PUBLIC_X.trim());
        assert_eq!(signer.kid().len(), 16);
        let now = now_secs();
        let claims = AssertionClaims {
            iss: "https://nasiko.dev".into(),
            aud: "https://acme.nasiko.dev".into(),
            sub: "portal-1".into(),
            email: "a@acme.test".into(),
            email_verified: true,
            ws: "ws-1".into(),
            jti: "j-2".into(),
            iat: now,
            exp: now + 30,
            token_type: ASSERTION_TOKEN_TYPE.into(),
        };
        let token = signer.sign(&claims).unwrap();
        let ring = AssertionKeyRing {
            current: signer,
            previous: None,
        };
        let v = verify_assertion(
            &token,
            &ring.jwks(),
            "https://nasiko.dev",
            "https://acme.nasiko.dev",
        )
        .unwrap();
        assert_eq!(v.jti, "j-2");
    }

    #[test]
    fn a_mismatched_key_pair_fails_at_construction() {
        let other_pub = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\n-----END PUBLIC KEY-----\n";
        assert!(AssertionSigner::from_pem(TEST_PRIVATE_PEM, other_pub).is_err());
        assert!(
            raw_public_key_from_spki_pem(
                "-----BEGIN PUBLIC KEY-----\nAAAA\n-----END PUBLIC KEY-----"
            )
            .is_err()
        );
    }
}
