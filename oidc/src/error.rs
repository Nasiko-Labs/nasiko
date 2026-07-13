use thiserror::Error;

/// Errors surfaced by the generic OIDC relying-party client. Deliberately
/// coarse-grained (no leaked raw HTTP bodies beyond a short status/reason) so
/// callers can log the `Display` form safely.
#[derive(Debug, Error)]
pub enum OidcError {
    #[error("OIDC discovery failed: {0}")]
    Discovery(String),
    #[error("JWKS fetch failed: {0}")]
    Jwks(String),
    #[error("token endpoint request failed: {0}")]
    TokenExchange(String),
    /// No JWKS key matched the ID token's `kid`, even after one forced refetch
    /// (handles routine key rotation; anything past that is a real mismatch).
    #[error("no matching signing key found for kid {0:?}")]
    UnknownKid(Option<String>),
    #[error("unsupported id_token signing algorithm: {0}")]
    UnsupportedAlgorithm(String),
    #[error("id_token failed validation: {0}")]
    InvalidToken(String),
    #[error("id_token nonce did not match the value issued at login")]
    NonceMismatch,
}
