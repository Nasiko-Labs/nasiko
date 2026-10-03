//! Core trait for request classification implementations.

use async_trait::async_trait;

use super::types::{ClassificationResult, ClassifierError, ClassifierMetadata, HealthStatus, InferenceContext};

/// Core trait for request classification implementations.
///
/// All classifiers must be `Send + Sync` for safe concurrent usage in async contexts.
///
/// # Examples
///
/// ```ignore
/// use async_trait::async_trait;
/// use nasiko_llm_router::classifier::{
///     RequestClassifier, InferenceContext, ClassificationResult,
///     ClassifierError, HealthStatus, ClassifierMetadata, BackendType
/// };
/// use std::collections::HashMap;
/// use std::time::Instant;
///
/// struct SimpleClassifier;
///
/// #[async_trait]
/// impl RequestClassifier for SimpleClassifier {
///     async fn classify(
///         &self,
///         context: &InferenceContext,
///     ) -> Result<ClassificationResult, ClassifierError> {
///         // Implementation here
///         todo!()
///     }
///
///     async fn classify_batch(
///         &self,
///         contexts: &[InferenceContext],
///     ) -> Result<Vec<ClassificationResult>, ClassifierError> {
///         // Default implementation: call classify for each
///         let mut results = Vec::with_capacity(contexts.len());
///         for ctx in contexts {
///             results.push(self.classify(ctx).await?);
///         }
///         Ok(results)
///     }
///
///     async fn health_check(&self) -> Result<HealthStatus, ClassifierError> {
///         Ok(HealthStatus {
///             healthy: true,
///             message: "Healthy".into(),
///             last_check: chrono::Utc::now(),
///         })
///     }
///
///     fn metadata(&self) -> ClassifierMetadata {
///         ClassifierMetadata {
///             name: "simple-classifier".into(),
///             version: "1.0.0".into(),
///             backend_type: BackendType::RuleBased,
///             capabilities: vec!["async".into()],
///         }
///     }
/// }
/// ```
#[async_trait]
pub trait RequestClassifier: Send + Sync {
    /// Classify a single request and return the predicted type with confidence.
    ///
    /// # Arguments
    ///
    /// * `context` - The inference context containing request text and metadata
    ///
    /// # Returns
    ///
    /// * `Ok(ClassificationResult)` - Successful classification with type and confidence
    /// * `Err(ClassifierError)` - Classification failed
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let context = InferenceContext {
    ///     request_text: "Write a function to sort an array".into(),
    ///     user_context: None,
    ///     conversation_history: None,
    ///     metadata: HashMap::new(),
    ///     received_at: Instant::now(),
    /// };
    ///
    /// let result = classifier.classify(&context).await?;
    /// println!("Type: {:?}, Confidence: {}", result.request_type, result.confidence);
    /// ```
    async fn classify(
        &self,
        context: &InferenceContext,
    ) -> Result<ClassificationResult, ClassifierError>;

    /// Classify multiple requests concurrently.
    ///
    /// Implementations should respect internal concurrency limits and batch efficiently.
    /// The default implementation calls `classify` for each context sequentially.
    ///
    /// # Arguments
    ///
    /// * `contexts` - Vector of inference contexts to classify
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<ClassificationResult>)` - Results in same order as input
    /// * `Err(ClassifierError)` - Batch classification failed
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let contexts = vec![
    ///     InferenceContext { request_text: "Explain this code".into(), .. },
    ///     InferenceContext { request_text: "Fix this bug".into(), .. },
    /// ];
    ///
    /// let results = classifier.classify_batch(&contexts).await?;
    /// for (i, result) in results.iter().enumerate() {
    ///     println!("Request {}: {:?}", i, result.request_type);
    /// }
    /// ```
    async fn classify_batch(
        &self,
        contexts: &[InferenceContext],
    ) -> Result<Vec<ClassificationResult>, ClassifierError>;

    /// Check if the classifier is healthy and ready to accept requests.
    ///
    /// This method should perform a lightweight check to verify connectivity,
    /// dependencies, and readiness. For classifiers that depend on external services,
    /// this might involve a test request or connection check.
    ///
    /// # Returns
    ///
    /// * `Ok(HealthStatus)` - Classifier health information
    /// * `Err(ClassifierError)` - Health check failed
    ///
    /// # Examples
    ///
    /// ```ignore
    /// match classifier.health_check().await {
    ///     Ok(status) if status.healthy => {
    ///         println!("Classifier ready: {}", status.message);
    ///     }
    ///     Ok(status) => {
    ///         eprintln!("Classifier unhealthy: {}", status.message);
    ///     }
    ///     Err(e) => {
    ///         eprintln!("Health check failed: {}", e);
    ///     }
    /// }
    /// ```
    async fn health_check(&self) -> Result<HealthStatus, ClassifierError>;

    /// Get classifier metadata including name, version, and backend type.
    ///
    /// This method returns static information about the classifier that can be used
    /// for logging, monitoring, and debugging. It should be cheap to call.
    ///
    /// # Returns
    ///
    /// * `ClassifierMetadata` - Static information about this classifier
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let meta = classifier.metadata();
    /// println!("Classifier: {} v{}", meta.name, meta.version);
    /// println!("Backend: {:?}", meta.backend_type);
    /// println!("Capabilities: {:?}", meta.capabilities);
    /// ```
    fn metadata(&self) -> ClassifierMetadata;
}
