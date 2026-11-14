//! Server-side wiring for the LLM router (`nasiko-llm-router`).
//!
//! - [`wiring`] — deploy-time gateway env injection into agent containers.
//! - [`model_registry`] — admin API for the tier→model registry table.
//! - [`providers`] — user-facing provider/model catalog for the UI.
//! - [`custom_providers`] — admin API for DB-registered custom LLM endpoints.
pub mod custom_providers;
pub mod model_registry;
pub mod platform_backend;
pub mod providers;
pub mod wiring;
