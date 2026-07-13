use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};

/// RFC 7636 §4.1 unreserved character set — safe unescaped in a URL query value.
const UNRESERVED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";

fn random_unreserved_string(len: usize) -> String {
    use rand::TryRngCore;
    use rand::rngs::OsRng;
    let mut bytes = vec![0u8; len];
    OsRng.try_fill_bytes(&mut bytes).expect("OS CSPRNG unavailable");
    bytes
        .iter()
        .map(|&b| UNRESERVED[b as usize % UNRESERVED.len()] as char)
        .collect()
}

/// A PKCE `code_verifier`: RFC 7636 requires 43-128 chars from the unreserved
/// set. 64 comfortably satisfies that with plenty of entropy (64 * ~6 bits).
pub fn generate_code_verifier() -> String {
    random_unreserved_string(64)
}

/// RFC 7636 S256 `code_challenge = BASE64URL-ENCODE(SHA256(code_verifier))`.
pub fn code_challenge_s256(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

/// Random URL-safe token for the OAuth `state` (CSRF) and OIDC `nonce`
/// (ID-token replay protection) parameters — same charset as the verifier,
/// just doesn't need to satisfy PKCE's specific length rule.
pub fn generate_random_token() -> String {
    random_unreserved_string(32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_verifier_has_valid_length_and_charset() {
        let v = generate_code_verifier();
        assert!(v.len() >= 43 && v.len() <= 128, "len {} out of RFC 7636 range", v.len());
        assert!(v.bytes().all(|b| UNRESERVED.contains(&b)));
    }

    #[test]
    fn code_verifier_is_random() {
        assert_ne!(generate_code_verifier(), generate_code_verifier());
    }

    #[test]
    fn challenge_is_deterministic_function_of_verifier() {
        let v = "a-fixed-test-verifier-value-1234567890";
        assert_eq!(code_challenge_s256(v), code_challenge_s256(v));
    }

    #[test]
    fn challenge_differs_for_different_verifiers() {
        assert_ne!(
            code_challenge_s256(&generate_code_verifier()),
            code_challenge_s256(&generate_code_verifier())
        );
    }

    /// RFC 7636 Appendix B worked example.
    #[test]
    fn challenge_matches_rfc7636_appendix_b_vector() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(code_challenge_s256(verifier), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn random_token_is_random_and_nonempty() {
        let a = generate_random_token();
        let b = generate_random_token();
        assert!(!a.is_empty());
        assert_ne!(a, b);
    }
}
