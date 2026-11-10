//! Harness shim library: bridges a harness's ACP stdio to a WebSocket.
//!
//! Split from the binary so the integration tests can run the whole shim in-process.

mod buffer;
mod child;
mod control;
pub mod files;
pub mod handshake;
mod procstats;
pub mod reaper;
pub mod server;
pub mod supervisor;
pub mod workspace;
