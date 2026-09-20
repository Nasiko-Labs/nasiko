//! Server-side wiring for the LLM router (`nasiko-llm-router`).
//!
//! - [`wiring`] — deploy-time gateway env injection into agent containers.
//! - [`model_registry`] — admin API for the tier→model registry table.
//! - [`providers`] — user-facing provider/model catalog for the UI.
pub mod model_registry;
pub mod providers;
pub mod wiring;
