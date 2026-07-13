//! Generic OIDC relying-party (RP) client.
//!
//! Standards-compliant enough to work with any OIDC-compliant IdP — Microsoft
//! Entra ID, Okta, Auth0, Keycloak, Google — configured purely through
//! [`OidcConfig::issuer_url`]. Provider-specific glue (which claims map to
//! which local user fields, cookie/session handling, route wiring) lives in
//! the server crate that uses this client, not here.

pub mod client;
pub mod config;
pub mod discovery;
pub mod error;
pub mod id_token;
pub mod jwks;
pub mod pkce;

pub use client::{OidcClient, TokenResponse};
pub use config::OidcConfig;
pub use discovery::DiscoveryDocument;
pub use error::OidcError;
pub use id_token::IdTokenClaims;
pub use jwks::{Jwk, Jwks};
