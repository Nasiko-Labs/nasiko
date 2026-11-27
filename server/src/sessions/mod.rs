//! Harness sessions: a user's long-lived conversation with a platform-built coding-agent
//! image (OpenCode, Claude Code, Codex) that runs in its own suspendable sandbox.
//!
//! The session row is the `chat_sessions` row whose `session_id` is the A2A contextId; this
//! module adds the harness read model ([`catalog`]), the row access the lifecycle needs
//! ([`model`]), the credentials a sandbox carries ([`identity`]), the HTTP surface
//! ([`routes`]) and the idle reaper ([`reaper`]). Starting and driving turns is the
//! [`turn`] runs one turn on the sandbox, reusing the `coding` module's lease, permission and
//! streaming pieces; this module also builds the sandbox spec and owns suspension.

pub mod admission;
pub mod catalog;
pub(crate) mod handshake;
pub mod identity;
pub mod keys;
pub mod live;
pub mod model;
pub mod pending;
pub mod profile;
pub mod reaper;
pub mod routes;
pub mod scratch;
mod suspend;
pub mod tasks;
pub mod teardown;
pub mod token;
pub mod turn;

pub use catalog::{Harness, HarnessCatalog, PLATFORM_HARNESS_AGENT, spec_for_session};
pub use model::{SessionRow, SessionRuntimeState, SessionStore};
pub use reaper::spawn_reaper;
pub use routes::router;
