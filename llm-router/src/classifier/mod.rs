//! P2 Request Classifier - Production-grade classification system.
//!
//! This module provides a trait-based architecture for categorizing incoming requests
//! into predefined `RequestType` values using multiple backend implementations.
//! The design emphasizes reliability through fallback chains, observability through
//! comprehensive metrics and logging, and maintainability through clear module
//! boundaries and async Rust patterns.
//!
//! # Architecture
//!
//! The classifier system is built around the [`RequestClassifier`] trait, which
//! defines a common interface for all classification implementations. This enables:
//!
//! - **Polymorphic usage**: Swap implementations without changing caller code
//! - **Fallback chains**: Automatically failover to backup classifiers
//! - **Testing**: Easy mocking and testing with trait objects
//!
//! # Core Components
//!
//! - [`RequestClassifier`] - The main trait all classifiers implement
//! - [`InferenceContext`] - Input data for classification
//! - [`ClassificationResult`] - Output of classification with metadata
//! - [`ClassifierError`] - Unified error handling
//!
//! # Examples
//!
//! ## Basic Classification
//!
//! ```ignore
//! use nasiko_llm_router::classifier::{RequestClassifier, InferenceContext};
//! use std::collections::HashMap;
//! use std::time::Instant;
//!
//! async fn classify_request(classifier: &dyn RequestClassifier) {
//!     let context = InferenceContext {
//!         request_text: "Write a function to sort an array".into(),
//!         user_context: None,
//!         conversation_history: None,
//!         metadata: HashMap::new(),
//!         received_at: Instant::now(),
//!     };
//!
//!     match classifier.classify(&context).await {
//!         Ok(result) => {
//!             println!("Type: {:?}", result.request_type);
//!             println!("Confidence: {:.2}", result.confidence);
//!             println!("Latency: {:?}", result.latency);
//!         }
//!         Err(e) => eprintln!("Classification failed: {}", e),
//!     }
//! }
//! ```
//!
//! ## Batch Classification
//!
//! ```ignore
//! use nasiko_llm_router::classifier::{RequestClassifier, InferenceContext};
//!
//! async fn classify_batch(
//!     classifier: &dyn RequestClassifier,
//!     requests: Vec<String>,
//! ) {
//!     let contexts: Vec<InferenceContext> = requests
//!         .into_iter()
//!         .map(|text| InferenceContext {
//!             request_text: text,
//!             user_context: None,
//!             conversation_history: None,
//!             metadata: HashMap::new(),
//!             received_at: Instant::now(),
//!         })
//!         .collect();
//!
//!     match classifier.classify_batch(&contexts).await {
//!         Ok(results) => {
//!             for (i, result) in results.iter().enumerate() {
//!                 println!("Request {}: {:?}", i, result.request_type);
//!             }
//!         }
//!         Err(e) => eprintln!("Batch classification failed: {}", e),
//!     }
//! }
//! ```
//!
//! ## Health Checking
//!
//! ```ignore
//! use nasiko_llm_router::classifier::RequestClassifier;
//!
//! async fn check_classifier_health(classifier: &dyn RequestClassifier) {
//!     match classifier.health_check().await {
//!         Ok(status) if status.healthy => {
//!             println!("✓ Classifier healthy: {}", status.message);
//!         }
//!         Ok(status) => {
//!             eprintln!("✗ Classifier unhealthy: {}", status.message);
//!         }
//!         Err(e) => {
//!             eprintln!("✗ Health check failed: {}", e);
//!         }
//!     }
//! }
//! ```
//!
//! # Future Implementations
//!
//! This module will be extended with:
//!
//! - `hosted/` - AWS Bedrock integration for LLM-powered classification
//! - `regex/` - Pattern-matching baseline adapter
//! - `chain.rs` - Fallback chain orchestration
//! - `circuit_breaker.rs` - Circuit breaker for reliability
//! - `config.rs` - Configuration types and validation
//!
//! # Requirements Coverage
//!
//! This module implements:
//! - Requirement 1.1-1.8: Trait Architecture and API Design
//! - Foundation for Requirements 2-10 (implemented in submodules)

mod traits;
mod types;
pub mod hosted;

// Re-export public API
pub use traits::RequestClassifier;
pub use types::{
    BackendType, ClassificationResult, ClassifierError, ClassifierMetadata, ConversationTurn,
    HealthStatus, InferenceContext,
};
