pub mod auth;
pub mod config;
pub mod proxy;
pub mod rate_limit;
pub mod routing;
pub mod tls;
pub mod translation;

// TODO: Move flow guard logic here from oss/server/src/flow/guard.rs
// The gateway should enforce traceparent-based cascade limits (depth, fan-out,
// token budget, timeout) on agent proxy requests before forwarding.
// Requires adding Redis to gateway deps and async flow check in Pingora filter.
