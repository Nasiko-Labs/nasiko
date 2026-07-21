//! Axum route handlers — extract identity + params, run ACL, call `service::*`,
//! shape the HTTP response. No business logic or SQL here.

pub mod catalog;
pub mod connect;
pub mod connectors;
pub mod credentials;
pub mod gateway;
pub mod oauth;
pub mod permissions;
pub mod sharing;
pub mod upload;
pub mod webhooks;
