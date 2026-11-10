//! Network transports for [`AcpConnection`](crate::AcpConnection).
//!
//! The local child-process transport lives in [`crate::connection`]; this module
//! holds the ones that reach a harness running somewhere else.

mod ws;

pub use ws::connect_ws;
