//! Harness sessions: a user's long-lived conversation with a platform-built coding-agent
//! image (OpenCode, Claude Code, Codex) that runs in its own suspendable sandbox, bound to the
//! user's own harness agent ([`own_agent`]).
//!
//! The session row is the `chat_sessions` row whose `session_id` is the A2A contextId; this
//! module adds the harness read model ([`catalog`]), the row access the lifecycle needs
//! ([`model`]), the credentials a sandbox carries ([`identity`]), the HTTP surface
//! ([`routes`]), the start of a new session's sandbox ([`start`]) and the idle reaper
//! ([`reaper`]). [`turn`] runs one turn on the sandbox, reusing the `coding` module's lease,
//! permission and streaming pieces; this module also builds the sandbox spec and owns suspension.

pub mod admission;
pub mod catalog;
pub(crate) mod handshake;
pub mod identity;
pub mod keys;
pub mod live;
pub mod model;
pub mod own_agent;
pub mod pending;
pub mod profile;
pub mod readiness;
pub mod reaper;
pub mod routes;
pub mod scratch;
pub mod start;
mod suspend;
pub mod tasks;
pub mod teardown;
pub mod token;
pub mod turn;

pub use catalog::{Harness, HarnessCatalog, spec_for_session};
pub use model::{SessionRow, SessionRuntimeState, SessionStore};
pub use reaper::spawn_reaper;
pub use routes::router;
