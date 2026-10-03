//! Hosted backend implementation for LLM-based classification.
//!
//! This module provides the [`HostedBackend`] classifier which uses AWS Bedrock
//! to perform LLM-powered request classification.

pub mod prompt;
pub mod parser;

pub use prompt::{FewShotExample, PromptBuilder, PromptMode};
pub use parser::parse_classification_response;

use crate::classifier::{
    ClassificationResult, ClassifierError, ClassifierMetadata, HealthStatus, InferenceContext,
    RequestClassifier, BackendType,
};
use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_bedrockruntime::{Client as BedrockClient, types::ContentBlock};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::{timeout, sleep};
use tracing::{info, warn, error, instrument};

/// Hosted backend using AWS Bedrock for LLM inference.
pub struct HostedBackend {
    client: Arc<BedrockClient>,
    config: HostedBackendConfig,
    prompt_builder: PromptBuilder,
}

/// Configuration for the hosted backend.
#[derive(Debug, Clone)]
pub struct HostedBackendConfig {
    /// AWS Bedrock model ID (e.g., "gpt-5.6-luna")
    pub model_id: String,
    
    /// AWS region (e.g., "us-east-1")
    pub region: String,
    
    /// Request timeout
    pub timeout: Duration,
    
    /// Prompt mode: zero-shot or few-shot
    pub prompt_mode: PromptMode,
    
    /// Example pairs for few-shot prompting
    pub few_shot_examples: Vec<FewShotExample>,
    
    /// Batch processing concurrency limit
    pub batch_concurrency: usize,
}

impl HostedBackend {
    /// Create a new HostedBackend with the given configuration.
    pub async fn new(config: HostedBackendConfig) -> Result<Self, ClassifierError> {
        // Load AWS credentials from environment
        let aws_config = aws_config::defaults(BehaviorVersion::latest())
            .region(aws_config::Region::new(config.region.clone()))
            .load()
            .await;
        
        let client = BedrockClient::new(&aws_config);
        
        let prompt_builder = PromptBuilder::new(
            config.prompt_mode,
            config.few_shot_examples.clone(),
        );
        
        info!(
            region = %config.region,
            model = %config.model_id,
            "Initialized HostedBackend"
        );
        
        Ok(Self {
            client: Arc::new(client),
            config,
            prompt_builder,
        })
    }
    
    /// Invoke Bedrock with retry logic.
    #[instrument(skip(self, prompt))]
    async fn invoke_with_retry(&self, prompt: &str) -> Result<String, ClassifierError> {
        let mut attempt = 0;
        let retry_delays = [
            Duration::from_millis(100),
            Duration::from_millis(500),
            Duration::from_millis(2000),
        ];
        
        loop {
            match self.invoke_bedrock(prompt).await {
                Ok(response) => return Ok(response),
                Err(err) => {
                    if attempt >= retry_delays.len() {
                        error!(?err, "All retries exhausted");
                        return Err(err);
                    }
                    
                    let delay = retry_delays[attempt];
                    warn!(
                        ?err,
                        attempt = attempt + 1,
                        delay_ms = delay.as_millis(),
                        "Retry after transient failure"
                    );
                    
                    sleep(delay).await;
                    attempt += 1;
                }
            }
        }
    }
    
    /// Make a single Bedrock inference request.
    async fn invoke_bedrock(&self, prompt: &str) -> Result<String, ClassifierError> {
        let request = self.client
            .converse()
            .model_id(&self.config.model_id)
            .messages(
                aws_sdk_bedrockruntime::types::Message::builder()
                    .role(aws_sdk_bedrockruntime::types::ConversationRole::User)
                    .content(ContentBlock::Text(prompt.to_string()))
                    .build()
                    .map_err(|e| ClassifierError::InferenceFailed(e.to_string()))?
            );
        
        // Apply timeout
        let response = timeout(
            self.config.timeout,
            request.send()
        )
        .await
        .map_err(|_| ClassifierError::Timeout(self.config.timeout))?
        .map_err(|e| ClassifierError::AwsError(e.to_string()))?;
        
        // Extract text from response
        let output = response
            .output()
            .ok_or_else(|| ClassifierError::ParseError("No output in response".into()))?;
        
        match output {
            aws_sdk_bedrockruntime::types::ConverseOutput::Message(msg) => {
                let content = msg.content()
                    .first()
                    .ok_or_else(|| ClassifierError::ParseError("Empty message content".into()))?;
                
                match content {
                    ContentBlock::Text(text) => Ok(text.clone()),
                    _ => Err(ClassifierError::ParseError("Expected text content".into())),
                }
            }
            _ => Err(ClassifierError::ParseError("Unexpected output type".into())),
        }
    }
}

#[async_trait]
impl RequestClassifier for HostedBackend {
    #[instrument(skip(self, context))]
    async fn classify(
        &self,
        context: &InferenceContext,
    ) -> Result<ClassificationResult, ClassifierError> {
        let start = Instant::now();
        
        // Build prompt
        let prompt = self.prompt_builder.build_prompt(context)?;
        
        // Sanitize request text for logging (truncate long requests)
        let sanitized = if context.request_text.len() > 100 {
            format!("{}...", &context.request_text[..100])
        } else {
            context.request_text.clone()
        };
        
        info!(
            request_text = %sanitized,
            "Starting classification"
        );
        
        // Invoke with retry
        let response_text = self.invoke_with_retry(&prompt).await?;
        
        // Parse response
        let (request_type, confidence, reasoning) = 
            parse_classification_response(&response_text)?;
        
        let latency = start.elapsed();
        
        info!(
            request_type = ?request_type,
            confidence = confidence,
            latency_ms = latency.as_millis(),
            "Classification complete"
        );
        
        Ok(ClassificationResult {
            request_type,
            confidence,
            reasoning,
            classifier_name: self.metadata().name,
            latency,
            timestamp: chrono::Utc::now(),
        })
    }
    
    async fn classify_batch(
        &self,
        contexts: &[InferenceContext],
    ) -> Result<Vec<ClassificationResult>, ClassifierError> {
        use futures::stream::{self, StreamExt};
        
        info!(batch_size = contexts.len(), "Starting batch classification");
        
        // Process batch with concurrency limit
        let results = stream::iter(contexts)
            .map(|ctx| self.classify(ctx))
            .buffer_unordered(self.config.batch_concurrency)
            .collect::<Vec<_>>()
            .await;
        
        // Check for errors
        let mut outputs = Vec::with_capacity(results.len());
        for result in results {
            outputs.push(result?);
        }
        
        Ok(outputs)
    }
    
    async fn health_check(&self) -> Result<HealthStatus, ClassifierError> {
        // Attempt a simple classification to verify connectivity
        let test_context = InferenceContext {
            request_text: "health check".into(),
            user_context: None,
            conversation_history: None,
            metadata: std::collections::HashMap::new(),
            received_at: Instant::now(),
        };
        
        match timeout(Duration::from_secs(5), self.classify(&test_context)).await {
            Ok(Ok(_)) => Ok(HealthStatus {
                healthy: true,
                message: "Bedrock connection healthy".into(),
                last_check: chrono::Utc::now(),
            }),
            Ok(Err(e)) => Ok(HealthStatus {
                healthy: false,
                message: format!("Classification failed: {}", e),
                last_check: chrono::Utc::now(),
            }),
            Err(_) => Ok(HealthStatus {
                healthy: false,
                message: "Health check timed out".into(),
                last_check: chrono::Utc::now(),
            }),
        }
    }
    
    fn metadata(&self) -> ClassifierMetadata {
        ClassifierMetadata {
            name: "hosted-backend".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            backend_type: BackendType::HostedLLM,
            capabilities: vec![
                "async".into(),
                "batch".into(),
                "few-shot".into(),
                "reasoning".into(),
            ],
        }
    }
}

impl Default for HostedBackendConfig {
    fn default() -> Self {
        Self {
            model_id: "gpt-5.6-luna".into(),
            region: "us-east-1".into(),
            timeout: Duration::from_millis(5000),
            prompt_mode: PromptMode::ZeroShot,
            few_shot_examples: Vec::new(),
            batch_concurrency: 10,
        }
    }
}
