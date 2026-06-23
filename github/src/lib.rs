//! GitHub integration for the Nasiko platform.
//!
//! Provides GitHub OAuth (connect flow only), repository listing, and repository
//! cloning as an in-memory `tar.gz` archive.  This crate contains **no database
//! access** — token persistence and the user-authentication login flow belong to
//! the calling control plane.
//!
//! ## Caller contract
//!
//! After a successful [`service::GitHubService::exchange_code`] call the caller
//! must:
//! 1. Encrypt the returned `access_token` at rest (use `nasiko-secrets`).
//! 2. Persist it keyed by `user_id`.
//! 3. Pass the decrypted token into subsequent calls
//!    (`verify_token`, `list_repos`, `clone_to_archive`).
//! 4. Delete the stored credential on logout.
//!
//! ## Route integration
//!
//! Enable the `routes` feature to get Axum route handlers.  Wire them into the
//! protected router *after* the `require_auth` middleware so that `X-User-Id`
//! is already present.
//!
//! ```rust,ignore
//! let app = Router::new()
//!     .merge(nasiko_github::routes::github_router(Arc::new(svc)))
//!     .layer(middleware::from_fn_with_state(state.clone(), auth::require_auth));
//! ```

pub mod config;
pub mod error;
pub mod models;
pub mod service;

#[cfg(feature = "routes")]
pub mod routes;

pub(crate) mod http;

pub use config::GitHubConfig;
pub use error::{Error, Result};
pub use models::{AccessToken, CloneArchive, GitHubRepo, GitHubUser, OAuthStateClaims};
pub use service::GitHubService;
