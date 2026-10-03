//! Configuration system for request classifiers.
//!
//! This module provides comprehensive configuration management including:
//! - Environment variable loading
//! - TOML file configuration
//! - Validation of all parameters
//! - Environment-specific profiles (dev, staging, production)
//! - Sensitive value redaction for logging
//!
//! # Examples
//!
//! ## Loading from environment variables
//!
//! ```no_run
//! use nasiko_llm_router::classifier::config::ClassifierConfig;
//!
//! let config = ClassifierConfig::from_env().expect("Failed to load config");
//! config.log_config();
//! ```
//!
//! ## Loading from TOML file
//!
//! ```no_run
//! use std::path::PathBuf;
//! use nasiko_llm_router::classifier::config::ClassifierConfig;
//!
//! let config = ClassifierConfig::from_file(&PathBuf::from("config.toml"))
//!     .expect("Failed to load config");
//! ```
//!
//! ## Creating a custom configuration programmatically
//!
//! ```
//! use nasiko_llm_router::classifier::config::{ClassifierConfig, BackendConfig, HostedBackendConfig};
//! use std::time::Duration;
//!
//! let config = ClassifierConfig {
//!     primary: BackendConfig::Hosted(HostedBackendConfig {
//!         model_id: "gpt-5.6-luna".into(),
//!         region: "us-east-1".into(),
//!         timeout: Duration::from_millis(3000),
//!         prompt_mode: PromptMode::FewShot,
//!         few_shot_examples: vec![],
//!         batch_concurrency: 10,
//!     }),
//!     ..Default::default()
//! };
//!
//! config.validate().expect("Invalid configuration");
//! ```

use crate::classifier::ClassifierError;
use crate::classifier::hosted::prompt::{FewShotExample, PromptMode};
use crate::routing::RequestType;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tracing::{info, warn};

/// Top-level configuration for the classifier system.
///
/// This structure contains all configuration needed to initialize and run
/// request classifiers, including primary backend selection, fallback chain,
/// batch processing settings, and circuit breaker configuration.
///
/// # Validation
///
/// All configurations should be validated using [`validate()`](Self::validate)
/// before use. Invalid configurations will return detailed error messages.
///
/// # Environment-Specific Profiles
///
/// Use [`for_environment()`](Self::for_environment) to load profile-specific
/// configurations optimized for different deployment environments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassifierConfig {
    /// Primary classifier backend.
    pub primary: BackendConfig,

    /// Fallback chain configuration.
    pub fallback: FallbackConfig,

    /// Batch processing settings.
    pub batch: BatchConfig,

    /// Circuit breaker settings for fault tolerance.
    pub circuit_breaker: CircuitBreakerConfig,

    /// Environment profile (dev, staging, production).
    #[serde(default = "default_environment")]
    pub environment: Environment,
}

/// Backend configuration options.
///
/// Classifiers can use different backend implementations with different
/// configuration requirements. This enum provides a type-safe way to
/// configure each backend type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum BackendConfig {
    /// Hosted LLM backend (AWS Bedrock).
    Hosted(HostedBackendConfig),

    /// Regex-based pattern matching backend.
    Regex {
        /// Default request type for unmatched requests.
        default_type: RequestType,
    },
}

/// Configuration for the hosted LLM backend using AWS Bedrock.
///
/// This backend provides high-accuracy classification using large language
/// models hosted on AWS Bedrock infrastructure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostedBackendConfig {
    /// AWS Bedrock model ID (e.g., "gpt-5.6-luna").
    pub model_id: String,

    /// AWS region (e.g., "us-east-1").
    pub region: String,

    /// Request timeout.
    ///
    /// Classification requests will fail if they exceed this duration.
    #[serde(with = "serde_duration_millis")]
    pub timeout: Duration,

    /// Prompt mode: zero-shot or few-shot.
    pub prompt_mode: PromptMode,

    /// Example pairs for few-shot prompting.
    ///
    /// Only used when `prompt_mode` is `FewShot`. Limited to 5 examples
    /// per classification request.
    #[serde(default)]
    pub few_shot_examples: Vec<FewShotExample>,

    /// Batch processing concurrency limit.
    ///
    /// Maximum number of concurrent requests to Bedrock during batch
    /// classification operations.
    pub batch_concurrency: usize,
}

/// Fallback chain configuration.
///
/// When the primary classifier fails, the system will attempt classification
/// using backup classifiers in the order specified by the chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackConfig {
    /// Enable fallback chain.
    ///
    /// When `false`, classification failures will immediately return errors
    /// rather than attempting fallback classifiers.
    pub enabled: bool,

    /// Ordered list of fallback backends.
    ///
    /// Classifiers will be tried in order until one succeeds or the chain
    /// is exhausted.
    pub chain: Vec<BackendConfig>,
}

/// Batch processing configuration.
///
/// Controls how the classifier handles batch classification requests for
/// optimal throughput and resource utilization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchConfig {
    /// Maximum batch size.
    ///
    /// Batch requests larger than this will be split into multiple batches.
    pub max_size: usize,

    /// Concurrency limit for batch processing.
    ///
    /// Maximum number of concurrent classification operations during batch
    /// processing. Higher values increase throughput but also resource usage.
    pub concurrency: usize,
}

/// Circuit breaker configuration for fault tolerance.
///
/// The circuit breaker prevents cascading failures by temporarily disabling
/// classifiers that are experiencing errors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening the circuit.
    pub failure_threshold: usize,

    /// Time to wait before attempting to close an open circuit.
    #[serde(with = "serde_duration_secs")]
    pub reset_timeout: Duration,
}

/// Deployment environment profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Environment {
    /// Development environment with relaxed constraints.
    Dev,

    /// Staging environment mimicking production.
    Staging,

    /// Production environment with strict reliability requirements.
    Production,
}

impl ClassifierConfig {
    /// Load configuration from environment variables.
    ///
    /// This method reads configuration from the following environment variables:
    ///
    /// ## Primary Backend Configuration
    /// - `CLASSIFIER_BACKEND`: Backend type ("hosted" or "regex", default: "hosted")
    /// - `BEDROCK_MODEL_ID`: Model ID for hosted backend (default: "gpt-5.6-luna")
    /// - `AWS_REGION`: AWS region (default: "us-east-1")
    /// - `BEDROCK_TIMEOUT_MS`: Request timeout in milliseconds (default: 5000)
    /// - `PROMPT_MODE`: Prompting mode ("zero-shot" or "few-shot", default: "zero-shot")
    /// - `BEDROCK_BATCH_CONCURRENCY`: Batch concurrency limit (default: 10)
    ///
    /// ## Fallback Configuration
    /// - `CLASSIFIER_FALLBACK_ENABLED`: Enable fallback chain (default: true)
    ///
    /// ## Batch Configuration
    /// - `CLASSIFIER_BATCH_MAX_SIZE`: Maximum batch size (default: 100)
    /// - `CLASSIFIER_BATCH_CONCURRENCY`: Batch concurrency (default: 10)
    ///
    /// ## Circuit Breaker Configuration
    /// - `CLASSIFIER_CB_FAILURE_THRESHOLD`: Failure threshold (default: 5)
    /// - `CLASSIFIER_CB_RESET_TIMEOUT_SECS`: Reset timeout in seconds (default: 60)
    ///
    /// ## Environment
    /// - `CLASSIFIER_ENVIRONMENT`: Environment profile ("dev", "staging", "production", default: "dev")
    ///
    /// # Errors
    ///
    /// Returns [`ClassifierError::ConfigError`] if:
    /// - Required environment variables are missing
    /// - Environment variable values cannot be parsed
    /// - Configuration validation fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use nasiko_llm_router::classifier::config::ClassifierConfig;
    ///
    /// let config = ClassifierConfig::from_env().expect("Failed to load config");
    /// ```
    pub fn from_env() -> Result<Self, ClassifierError> {
        let environment = Self::load_environment_from_env();
        let primary = Self::load_primary_from_env()?;
        let fallback = Self::load_fallback_from_env()?;

        let config = Self {
            primary,
            fallback,
            batch: BatchConfig {
                max_size: Self::env_var_or("CLASSIFIER_BATCH_MAX_SIZE", 100)?,
                concurrency: Self::env_var_or("CLASSIFIER_BATCH_CONCURRENCY", 10)?,
            },
            circuit_breaker: CircuitBreakerConfig {
                failure_threshold: Self::env_var_or("CLASSIFIER_CB_FAILURE_THRESHOLD", 5)?,
                reset_timeout: Duration::from_secs(Self::env_var_or(
                    "CLASSIFIER_CB_RESET_TIMEOUT_SECS",
                    60,
                )?),
            },
            environment,
        };

        config.validate()?;
        Ok(config)
    }

    /// Load configuration from a TOML file.
    ///
    /// The TOML file should have the following structure:
    ///
    /// ```toml
    /// environment = "production"
    ///
    /// [primary]
    /// type = "hosted"
    /// model_id = "gpt-5.6-luna"
    /// region = "us-east-1"
    /// timeout = 5000  # milliseconds
    /// prompt_mode = "few-shot"
    /// batch_concurrency = 10
    ///
    /// [[primary.few_shot_examples]]
    /// request_text = "Write a function to sort an array"
    /// request_type = "CodeGeneration"
    /// explanation = "Request asks to write/create code"
    ///
    /// [fallback]
    /// enabled = true
    ///
    /// [[fallback.chain]]
    /// type = "regex"
    /// default_type = "General"
    ///
    /// [batch]
    /// max_size = 100
    /// concurrency = 10
    ///
    /// [circuit_breaker]
    /// failure_threshold = 5
    /// reset_timeout = 60  # seconds
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`ClassifierError::ConfigError`] if:
    /// - File cannot be read
    /// - TOML syntax is invalid
    /// - Configuration structure is invalid
    /// - Configuration validation fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::path::PathBuf;
    /// use nasiko_llm_router::classifier::config::ClassifierConfig;
    ///
    /// let config = ClassifierConfig::from_file(&PathBuf::from("classifier.toml"))
    ///     .expect("Failed to load config file");
    /// ```
    pub fn from_file(path: &PathBuf) -> Result<Self, ClassifierError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            ClassifierError::ConfigError(format!("Failed to read config file: {}", e))
        })?;

        let config: Self = toml::from_str(&content).map_err(|e| {
            ClassifierError::ConfigError(format!("Failed to parse TOML config: {}", e))
        })?;

        config.validate()?;
        Ok(config)
    }

    /// Load configuration for a specific environment profile.
    ///
    /// This method provides optimized defaults for different deployment
    /// environments:
    ///
    /// - **Development**: Fast timeouts, minimal retries, verbose logging
    /// - **Staging**: Production-like settings for testing
    /// - **Production**: Strict reliability, longer timeouts, full fallback chain
    ///
    /// # Examples
    ///
    /// ```
    /// use nasiko_llm_router::classifier::config::{ClassifierConfig, Environment};
    ///
    /// let prod_config = ClassifierConfig::for_environment(Environment::Production);
    /// let dev_config = ClassifierConfig::for_environment(Environment::Dev);
    /// ```
    pub fn for_environment(env: Environment) -> Self {
        match env {
            Environment::Dev => Self {
                primary: BackendConfig::Hosted(HostedBackendConfig {
                    model_id: "gpt-5.6-luna".into(),
                    region: "us-east-1".into(),
                    timeout: Duration::from_millis(3000), // Faster timeout for dev
                    prompt_mode: PromptMode::ZeroShot,
                    few_shot_examples: vec![],
                    batch_concurrency: 5, // Lower concurrency for dev
                }),
                fallback: FallbackConfig {
                    enabled: true,
                    chain: vec![BackendConfig::Regex {
                        default_type: RequestType::General,
                    }],
                },
                batch: BatchConfig {
                    max_size: 50,
                    concurrency: 5,
                },
                circuit_breaker: CircuitBreakerConfig {
                    failure_threshold: 3, // More aggressive circuit breaking
                    reset_timeout: Duration::from_secs(30),
                },
                environment: Environment::Dev,
            },
            Environment::Staging => Self {
                primary: BackendConfig::Hosted(HostedBackendConfig {
                    model_id: "gpt-5.6-luna".into(),
                    region: "us-east-1".into(),
                    timeout: Duration::from_millis(5000),
                    prompt_mode: PromptMode::FewShot,
                    few_shot_examples: vec![],
                    batch_concurrency: 10,
                }),
                fallback: FallbackConfig {
                    enabled: true,
                    chain: vec![BackendConfig::Regex {
                        default_type: RequestType::General,
                    }],
                },
                batch: BatchConfig {
                    max_size: 100,
                    concurrency: 10,
                },
                circuit_breaker: CircuitBreakerConfig {
                    failure_threshold: 5,
                    reset_timeout: Duration::from_secs(60),
                },
                environment: Environment::Staging,
            },
            Environment::Production => Self {
                primary: BackendConfig::Hosted(HostedBackendConfig {
                    model_id: "gpt-5.6-luna".into(),
                    region: "us-east-1".into(),
                    timeout: Duration::from_millis(5000),
                    prompt_mode: PromptMode::FewShot,
                    few_shot_examples: vec![],
                    batch_concurrency: 15, // Higher concurrency for production
                }),
                fallback: FallbackConfig {
                    enabled: true,
                    chain: vec![BackendConfig::Regex {
                        default_type: RequestType::General,
                    }],
                },
                batch: BatchConfig {
                    max_size: 200, // Larger batches for production
                    concurrency: 15,
                },
                circuit_breaker: CircuitBreakerConfig {
                    failure_threshold: 5,
                    reset_timeout: Duration::from_secs(60),
                },
                environment: Environment::Production,
            },
        }
    }

    /// Validate configuration values.
    ///
    /// Checks that all configuration parameters are within valid ranges and
    /// that the configuration as a whole is internally consistent.
    ///
    /// # Errors
    ///
    /// Returns [`ClassifierError::ConfigError`] if any validation check fails,
    /// with a descriptive message indicating the invalid parameter.
    ///
    /// # Examples
    ///
    /// ```
    /// use nasiko_llm_router::classifier::config::ClassifierConfig;
    ///
    /// let config = ClassifierConfig::default();
    /// assert!(config.validate().is_ok());
    /// ```
    pub fn validate(&self) -> Result<(), ClassifierError> {
        // Validate batch settings
        if self.batch.max_size == 0 {
            return Err(ClassifierError::ConfigError(
                "batch.max_size must be greater than 0".into(),
            ));
        }

        if self.batch.concurrency == 0 {
            return Err(ClassifierError::ConfigError(
                "batch.concurrency must be greater than 0".into(),
            ));
        }

        // Validate circuit breaker settings
        if self.circuit_breaker.failure_threshold == 0 {
            return Err(ClassifierError::ConfigError(
                "circuit_breaker.failure_threshold must be greater than 0".into(),
            ));
        }

        if self.circuit_breaker.reset_timeout.as_secs() == 0 {
            return Err(ClassifierError::ConfigError(
                "circuit_breaker.reset_timeout must be greater than 0".into(),
            ));
        }

        // Validate hosted backend settings if present
        match &self.primary {
            BackendConfig::Hosted(config) => {
                if config.model_id.is_empty() {
                    return Err(ClassifierError::ConfigError(
                        "hosted backend model_id cannot be empty".into(),
                    ));
                }

                if config.region.is_empty() {
                    return Err(ClassifierError::ConfigError(
                        "hosted backend region cannot be empty".into(),
                    ));
                }

                if config.timeout.as_millis() == 0 {
                    return Err(ClassifierError::ConfigError(
                        "hosted backend timeout must be greater than 0".into(),
                    ));
                }

                if config.batch_concurrency == 0 {
                    return Err(ClassifierError::ConfigError(
                        "hosted backend batch_concurrency must be greater than 0".into(),
                    ));
                }

                // Warn if few-shot mode is enabled but no examples provided
                if config.prompt_mode == PromptMode::FewShot && config.few_shot_examples.is_empty()
                {
                    warn!("Few-shot mode enabled but no examples provided");
                }
            }
            BackendConfig::Regex { .. } => {
                // Regex backend has no additional validation requirements
            }
        }

        // Validate fallback chain
        if self.fallback.enabled && self.fallback.chain.is_empty() {
            warn!("Fallback enabled but fallback chain is empty");
        }

        Ok(())
    }

    /// Log the active configuration with sensitive values redacted.
    ///
    /// This method logs the current configuration at INFO level, ensuring
    /// that sensitive information (like AWS credentials) is not exposed in logs.
    ///
    /// # Examples
    ///
    /// ```
    /// use nasiko_llm_router::classifier::config::ClassifierConfig;
    ///
    /// let config = ClassifierConfig::default();
    /// config.log_config();
    /// ```
    pub fn log_config(&self) {
        info!(
            environment = ?self.environment,
            primary_backend = ?self.primary_backend_type(),
            fallback_enabled = self.fallback.enabled,
            fallback_count = self.fallback.chain.len(),
            batch_max_size = self.batch.max_size,
            batch_concurrency = self.batch.concurrency,
            circuit_breaker_threshold = self.circuit_breaker.failure_threshold,
            circuit_breaker_timeout_secs = self.circuit_breaker.reset_timeout.as_secs(),
            "Classifier configuration loaded"
        );

        // Log additional details based on backend type
        match &self.primary {
            BackendConfig::Hosted(config) => {
                info!(
                    model_id = %config.model_id,
                    region = "[REDACTED]", // Redact region to avoid exposing infrastructure details
                    timeout_ms = config.timeout.as_millis(),
                    prompt_mode = ?config.prompt_mode,
                    few_shot_examples_count = config.few_shot_examples.len(),
                    batch_concurrency = config.batch_concurrency,
                    "Hosted backend configuration"
                );
            }
            BackendConfig::Regex { default_type } => {
                info!(
                    default_type = ?default_type,
                    "Regex backend configuration"
                );
            }
        }
    }

    /// Get the backend type of the primary classifier.
    fn primary_backend_type(&self) -> &str {
        match &self.primary {
            BackendConfig::Hosted(_) => "hosted",
            BackendConfig::Regex { .. } => "regex",
        }
    }

    // Helper methods for environment variable loading

    fn load_environment_from_env() -> Environment {
        std::env::var("CLASSIFIER_ENVIRONMENT")
            .ok()
            .and_then(|s| match s.to_lowercase().as_str() {
                "dev" | "development" => Some(Environment::Dev),
                "staging" => Some(Environment::Staging),
                "prod" | "production" => Some(Environment::Production),
                _ => None,
            })
            .unwrap_or(Environment::Dev)
    }

    fn load_primary_from_env() -> Result<BackendConfig, ClassifierError> {
        let backend_type = std::env::var("CLASSIFIER_BACKEND").unwrap_or_else(|_| "hosted".into());

        match backend_type.to_lowercase().as_str() {
            "hosted" => Ok(BackendConfig::Hosted(HostedBackendConfig {
                model_id: std::env::var("BEDROCK_MODEL_ID")
                    .unwrap_or_else(|_| "gpt-5.6-luna".into()),
                region: std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".into()),
                timeout: Duration::from_millis(Self::env_var_or("BEDROCK_TIMEOUT_MS", 5000)?),
                prompt_mode: match std::env::var("PROMPT_MODE")
                    .unwrap_or_default()
                    .to_lowercase()
                    .as_str()
                {
                    "few-shot" | "fewshot" => PromptMode::FewShot,
                    _ => PromptMode::ZeroShot,
                },
                few_shot_examples: Vec::new(), // Loaded from separate file or config
                batch_concurrency: Self::env_var_or("BEDROCK_BATCH_CONCURRENCY", 10)?,
            })),
            "regex" => Ok(BackendConfig::Regex {
                default_type: RequestType::General,
            }),
            _ => Err(ClassifierError::ConfigError(format!(
                "Unknown backend type: {}. Expected 'hosted' or 'regex'",
                backend_type
            ))),
        }
    }

    fn load_fallback_from_env() -> Result<FallbackConfig, ClassifierError> {
        let enabled = std::env::var("CLASSIFIER_FALLBACK_ENABLED")
            .map(|v| v.to_lowercase() == "true")
            .unwrap_or(true);

        let chain = if enabled {
            // Default fallback chain: regex baseline
            vec![BackendConfig::Regex {
                default_type: RequestType::General,
            }]
        } else {
            Vec::new()
        };

        Ok(FallbackConfig { enabled, chain })
    }

    fn env_var_or<T: std::str::FromStr>(key: &str, default: T) -> Result<T, ClassifierError> {
        std::env::var(key)
            .ok()
            .and_then(|v| v.parse().ok())
            .or(Some(default))
            .ok_or_else(|| {
                ClassifierError::ConfigError(format!("Invalid value for environment variable {}", key))
            })
    }
}

impl Default for ClassifierConfig {
    /// Create a default configuration suitable for development.
    ///
    /// Equivalent to [`ClassifierConfig::for_environment(Environment::Dev)`].
    fn default() -> Self {
        Self::for_environment(Environment::Dev)
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

impl Default for FallbackConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            chain: vec![BackendConfig::Regex {
                default_type: RequestType::General,
            }],
        }
    }
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            max_size: 100,
            concurrency: 10,
        }
    }
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            reset_timeout: Duration::from_secs(60),
        }
    }
}

fn default_environment() -> Environment {
    Environment::Dev
}

/// Custom serialization for Duration as milliseconds.
mod serde_duration_millis {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        duration.as_millis().serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let millis = u64::deserialize(deserializer)?;
        Ok(Duration::from_millis(millis))
    }
}

/// Custom serialization for Duration as seconds.
mod serde_duration_secs {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        duration.as_secs().serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = u64::deserialize(deserializer)?;
        Ok(Duration::from_secs(secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_is_valid() {
        let config = ClassifierConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_environment_configs_are_valid() {
        assert!(ClassifierConfig::for_environment(Environment::Dev)
            .validate()
            .is_ok());
        assert!(ClassifierConfig::for_environment(Environment::Staging)
            .validate()
            .is_ok());
        assert!(ClassifierConfig::for_environment(Environment::Production)
            .validate()
            .is_ok());
    }

    #[test]
    fn test_zero_batch_size_fails_validation() {
        let mut config = ClassifierConfig::default();
        config.batch.max_size = 0;

        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("batch.max_size must be greater than 0"));
    }

    #[test]
    fn test_zero_batch_concurrency_fails_validation() {
        let mut config = ClassifierConfig::default();
        config.batch.concurrency = 0;

        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("batch.concurrency must be greater than 0"));
    }

    #[test]
    fn test_zero_circuit_breaker_threshold_fails_validation() {
        let mut config = ClassifierConfig::default();
        config.circuit_breaker.failure_threshold = 0;

        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("circuit_breaker.failure_threshold must be greater than 0"));
    }

    #[test]
    fn test_zero_circuit_breaker_timeout_fails_validation() {
        let mut config = ClassifierConfig::default();
        config.circuit_breaker.reset_timeout = Duration::from_secs(0);

        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("circuit_breaker.reset_timeout must be greater than 0"));
    }

    #[test]
    fn test_empty_model_id_fails_validation() {
        let mut config = ClassifierConfig::default();
        if let BackendConfig::Hosted(ref mut hosted) = config.primary {
            hosted.model_id = String::new();
        }

        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("model_id cannot be empty"));
    }

    #[test]
    fn test_empty_region_fails_validation() {
        let mut config = ClassifierConfig::default();
        if let BackendConfig::Hosted(ref mut hosted) = config.primary {
            hosted.region = String::new();
        }

        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("region cannot be empty"));
    }

    #[test]
    fn test_zero_timeout_fails_validation() {
        let mut config = ClassifierConfig::default();
        if let BackendConfig::Hosted(ref mut hosted) = config.primary {
            hosted.timeout = Duration::from_millis(0);
        }

        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("timeout must be greater than 0"));
    }

    #[test]
    fn test_zero_batch_concurrency_hosted_fails_validation() {
        let mut config = ClassifierConfig::default();
        if let BackendConfig::Hosted(ref mut hosted) = config.primary {
            hosted.batch_concurrency = 0;
        }

        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("batch_concurrency must be greater than 0"));
    }

    #[test]
    fn test_regex_backend_validates() {
        let config = ClassifierConfig {
            primary: BackendConfig::Regex {
                default_type: RequestType::General,
            },
            ..Default::default()
        };

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_hosted_backend_config_default_values() {
        let config = HostedBackendConfig::default();

        assert_eq!(config.model_id, "gpt-5.6-luna");
        assert_eq!(config.region, "us-east-1");
        assert_eq!(config.timeout, Duration::from_millis(5000));
        assert_eq!(config.prompt_mode, PromptMode::ZeroShot);
        assert!(config.few_shot_examples.is_empty());
        assert_eq!(config.batch_concurrency, 10);
    }

    #[test]
    fn test_fallback_config_default_values() {
        let config = FallbackConfig::default();

        assert!(config.enabled);
        assert_eq!(config.chain.len(), 1);
        assert!(matches!(
            config.chain[0],
            BackendConfig::Regex {
                default_type: RequestType::General
            }
        ));
    }

    #[test]
    fn test_batch_config_default_values() {
        let config = BatchConfig::default();

        assert_eq!(config.max_size, 100);
        assert_eq!(config.concurrency, 10);
    }

    #[test]
    fn test_circuit_breaker_config_default_values() {
        let config = CircuitBreakerConfig::default();

        assert_eq!(config.failure_threshold, 5);
        assert_eq!(config.reset_timeout, Duration::from_secs(60));
    }

    #[test]
    fn test_environment_profile_differences() {
        let dev = ClassifierConfig::for_environment(Environment::Dev);
        let staging = ClassifierConfig::for_environment(Environment::Staging);
        let prod = ClassifierConfig::for_environment(Environment::Production);

        // Dev has faster timeout
        if let BackendConfig::Hosted(ref dev_hosted) = dev.primary {
            assert_eq!(dev_hosted.timeout, Duration::from_millis(3000));
        }

        // Production has higher batch sizes
        assert_eq!(prod.batch.max_size, 200);
        assert!(prod.batch.max_size > dev.batch.max_size);

        // Dev has more aggressive circuit breaking
        assert_eq!(dev.circuit_breaker.failure_threshold, 3);
        assert!(dev.circuit_breaker.failure_threshold < prod.circuit_breaker.failure_threshold);

        // All environments have fallback enabled
        assert!(dev.fallback.enabled);
        assert!(staging.fallback.enabled);
        assert!(prod.fallback.enabled);
    }

    #[test]
    fn test_prompt_mode_serialization() {
        let zero_shot = PromptMode::ZeroShot;
        let few_shot = PromptMode::FewShot;

        let zero_shot_json = serde_json::to_string(&zero_shot).unwrap();
        let few_shot_json = serde_json::to_string(&few_shot).unwrap();

        // PromptMode uses default serde naming (PascalCase)
        assert_eq!(zero_shot_json, r#""ZeroShot""#);
        assert_eq!(few_shot_json, r#""FewShot""#);
    }

    #[test]
    fn test_environment_serialization() {
        let dev = Environment::Dev;
        let staging = Environment::Staging;
        let prod = Environment::Production;

        let dev_json = serde_json::to_string(&dev).unwrap();
        let staging_json = serde_json::to_string(&staging).unwrap();
        let prod_json = serde_json::to_string(&prod).unwrap();

        assert_eq!(dev_json, r#""dev""#);
        assert_eq!(staging_json, r#""staging""#);
        assert_eq!(prod_json, r#""production""#);
    }

    #[test]
    fn test_few_shot_example_creation() {
        let example = FewShotExample {
            request_text: "Write a Python function".into(),
            request_type: RequestType::CodeGeneration,
            explanation: "Request asks to write code".into(),
        };

        assert_eq!(example.request_text, "Write a Python function");
        assert_eq!(example.request_type, RequestType::CodeGeneration);
    }

    #[test]
    fn test_config_serialization_roundtrip() {
        let config = ClassifierConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: ClassifierConfig = serde_json::from_str(&json).unwrap();

        assert!(deserialized.validate().is_ok());
        assert_eq!(config.environment, deserialized.environment);
    }

    #[test]
    fn test_backend_config_serialization() {
        let hosted = BackendConfig::Hosted(HostedBackendConfig::default());
        let json = serde_json::to_string(&hosted).unwrap();
        assert!(json.contains(r#""type":"hosted""#));

        let regex = BackendConfig::Regex {
            default_type: RequestType::General,
        };
        let json = serde_json::to_string(&regex).unwrap();
        assert!(json.contains(r#""type":"regex""#));
    }
}
