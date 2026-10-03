//! Server-side wiring for the LLM router (`nasiko-llm-router`).
//!
//! - [`wiring`] — deploy-time gateway env injection into agent containers.
//! - [`model_registry`] — admin API for the tier→model registry table.
//! - [`providers`] — user-facing provider/model catalog for the UI.
//! - [`custom_providers`] — admin API for DB-registered custom LLM endpoints.
//! - [`classifier`] — status and side-effect-free preview for the Level 3 request classifier.
pub mod classifier;
pub mod custom_providers;
pub mod model_registry;
pub mod providers;
pub mod wiring;
