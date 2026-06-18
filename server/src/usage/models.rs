use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

/// Comprehensive token usage record capturing all provider metrics
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct TokenUsage {
    pub id: Uuid,
    pub user_id: Uuid,
    pub agent_id: Option<Uuid>,
    pub operation_type: String,
    pub request_id: Option<String>,
    pub session_id: Option<String>,
    pub provider: String,
    pub model: String,
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub total_tokens: i32,
    pub cache_creation_input_tokens: Option<i32>,
    pub cache_read_input_tokens: Option<i32>,
    pub cached_tokens: Option<i32>,
    pub audio_tokens: Option<i32>,
    pub reasoning_tokens: Option<i32>,
    pub accepted_prediction_tokens: Option<i32>,
    pub rejected_prediction_tokens: Option<i32>,
    pub completion_tokens_details: Option<sqlx::types::Json<serde_json::Value>>,
    pub prompt_tokens_details: Option<sqlx::types::Json<serde_json::Value>>,
    pub cost_usd: Option<Decimal>,
    pub latency_ms: Option<i32>,
    pub ttft_ms: Option<i32>,
    pub streaming: bool,
    pub finish_reason: Option<String>,
    pub metadata: sqlx::types::Json<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

/// Builder for creating token usage records
#[derive(Debug, Clone)]
pub struct TokenUsageBuilder {
    user_id: Uuid,
    agent_id: Option<Uuid>,
    operation_type: String,
    request_id: Option<String>,
    session_id: Option<String>,
    provider: String,
    model: String,
    input_tokens: i32,
    output_tokens: i32,
    cache_creation_input_tokens: Option<i32>,
    cache_read_input_tokens: Option<i32>,
    cached_tokens: Option<i32>,
    audio_tokens: Option<i32>,
    reasoning_tokens: Option<i32>,
    accepted_prediction_tokens: Option<i32>,
    rejected_prediction_tokens: Option<i32>,
    completion_tokens_details: Option<serde_json::Value>,
    prompt_tokens_details: Option<serde_json::Value>,
    latency_ms: Option<i32>,
    ttft_ms: Option<i32>,
    streaming: bool,
    finish_reason: Option<String>,
    metadata: serde_json::Value,
}

impl TokenUsageBuilder {
    pub fn new(
        user_id: Uuid,
        operation_type: impl Into<String>,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            user_id,
            agent_id: None,
            operation_type: operation_type.into(),
            request_id: None,
            session_id: None,
            provider: provider.into(),
            model: model.into(),
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            cached_tokens: None,
            audio_tokens: None,
            reasoning_tokens: None,
            accepted_prediction_tokens: None,
            rejected_prediction_tokens: None,
            completion_tokens_details: None,
            prompt_tokens_details: None,
            latency_ms: None,
            ttft_ms: None,
            streaming: false,
            finish_reason: None,
            metadata: serde_json::json!({}),
        }
    }

    pub fn agent_id(mut self, agent_id: Uuid) -> Self {
        self.agent_id = Some(agent_id);
        self
    }

    pub fn request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }

    pub fn session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn tokens(mut self, input: i32, output: i32) -> Self {
        self.input_tokens = input;
        self.output_tokens = output;
        self
    }

    pub fn cache_creation_tokens(mut self, tokens: i32) -> Self {
        self.cache_creation_input_tokens = Some(tokens);
        self
    }

    pub fn cache_read_tokens(mut self, tokens: i32) -> Self {
        self.cache_read_input_tokens = Some(tokens);
        self
    }

    pub fn cached_tokens(mut self, tokens: i32) -> Self {
        self.cached_tokens = Some(tokens);
        self
    }

    pub fn audio_tokens(mut self, tokens: i32) -> Self {
        self.audio_tokens = Some(tokens);
        self
    }

    pub fn reasoning_tokens(mut self, tokens: i32) -> Self {
        self.reasoning_tokens = Some(tokens);
        self
    }

    pub fn predicted_tokens(mut self, accepted: i32, rejected: i32) -> Self {
        self.accepted_prediction_tokens = Some(accepted);
        self.rejected_prediction_tokens = Some(rejected);
        self
    }

    pub fn completion_details(mut self, details: serde_json::Value) -> Self {
        self.completion_tokens_details = Some(details);
        self
    }

    pub fn prompt_details(mut self, details: serde_json::Value) -> Self {
        self.prompt_tokens_details = Some(details);
        self
    }

    pub fn latency_ms(mut self, ms: i32) -> Self {
        self.latency_ms = Some(ms);
        self
    }

    pub fn ttft_ms(mut self, ms: i32) -> Self {
        self.ttft_ms = Some(ms);
        self
    }

    pub fn streaming(mut self, streaming: bool) -> Self {
        self.streaming = streaming;
        self
    }

    pub fn finish_reason(mut self, reason: impl Into<String>) -> Self {
        self.finish_reason = Some(reason.into());
        self
    }

    pub fn metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }

    pub fn build(self) -> CreateTokenUsage {
        CreateTokenUsage {
            user_id: self.user_id,
            agent_id: self.agent_id,
            operation_type: self.operation_type,
            request_id: self.request_id,
            session_id: self.session_id,
            provider: self.provider,
            model: self.model,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            total_tokens: self.input_tokens + self.output_tokens,
            cache_creation_input_tokens: self.cache_creation_input_tokens,
            cache_read_input_tokens: self.cache_read_input_tokens,
            cached_tokens: self.cached_tokens,
            audio_tokens: self.audio_tokens,
            reasoning_tokens: self.reasoning_tokens,
            accepted_prediction_tokens: self.accepted_prediction_tokens,
            rejected_prediction_tokens: self.rejected_prediction_tokens,
            completion_tokens_details: self.completion_tokens_details,
            prompt_tokens_details: self.prompt_tokens_details,
            latency_ms: self.latency_ms,
            ttft_ms: self.ttft_ms,
            streaming: self.streaming,
            finish_reason: self.finish_reason,
            metadata: self.metadata,
        }
    }
}

/// Input for creating a token usage record
#[derive(Debug, Clone)]
pub struct CreateTokenUsage {
    pub user_id: Uuid,
    pub agent_id: Option<Uuid>,
    pub operation_type: String,
    pub request_id: Option<String>,
    pub session_id: Option<String>,
    pub provider: String,
    pub model: String,
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub total_tokens: i32,
    pub cache_creation_input_tokens: Option<i32>,
    pub cache_read_input_tokens: Option<i32>,
    pub cached_tokens: Option<i32>,
    pub audio_tokens: Option<i32>,
    pub reasoning_tokens: Option<i32>,
    pub accepted_prediction_tokens: Option<i32>,
    pub rejected_prediction_tokens: Option<i32>,
    pub completion_tokens_details: Option<serde_json::Value>,
    pub prompt_tokens_details: Option<serde_json::Value>,
    pub latency_ms: Option<i32>,
    pub ttft_ms: Option<i32>,
    pub streaming: bool,
    pub finish_reason: Option<String>,
    pub metadata: serde_json::Value,
}

/// Router request log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterRequestLog {
    pub id: Uuid,
    pub request_id: String,
    pub user_id: Uuid,
    pub session_id: String,
    pub query: String,
    pub agents_considered: i32,
    pub selected_agent_id: Option<Uuid>,
    pub selected_agent_name: Option<String>,
    pub selection_reasoning: Option<String>,
    pub fallback_used: bool,
    pub selection_token_usage_id: Option<Uuid>,
    pub total_latency_ms: i32,
    pub registry_fetch_ms: Option<i32>,
    pub vector_store_ms: Option<i32>,
    pub selection_llm_ms: Option<i32>,
    pub agent_call_ms: Option<i32>,
    pub success: bool,
    pub error_message: Option<String>,
    pub finish_reason: Option<String>,
    pub streaming: bool,
    pub file_count: i32,
    pub metadata: sqlx::types::Json<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

/// Builder for router request logs
#[derive(Debug, Clone)]
pub struct RouterRequestLogBuilder {
    request_id: String,
    user_id: Uuid,
    session_id: String,
    query: String,
    agents_considered: i32,
    selected_agent_id: Option<Uuid>,
    selected_agent_name: Option<String>,
    selection_reasoning: Option<String>,
    fallback_used: bool,
    selection_token_usage_id: Option<Uuid>,
    total_latency_ms: i32,
    registry_fetch_ms: Option<i32>,
    vector_store_ms: Option<i32>,
    selection_llm_ms: Option<i32>,
    agent_call_ms: Option<i32>,
    success: bool,
    error_message: Option<String>,
    finish_reason: Option<String>,
    streaming: bool,
    file_count: i32,
    metadata: serde_json::Value,
}

impl RouterRequestLogBuilder {
    pub fn new(request_id: String, user_id: Uuid, session_id: String, query: String) -> Self {
        Self {
            request_id,
            user_id,
            session_id,
            query,
            agents_considered: 0,
            selected_agent_id: None,
            selected_agent_name: None,
            selection_reasoning: None,
            fallback_used: false,
            selection_token_usage_id: None,
            total_latency_ms: 0,
            registry_fetch_ms: None,
            vector_store_ms: None,
            selection_llm_ms: None,
            agent_call_ms: None,
            success: false,
            error_message: None,
            finish_reason: None,
            streaming: false,
            file_count: 0,
            metadata: serde_json::json!({}),
        }
    }

    pub fn agents_considered(mut self, count: i32) -> Self {
        self.agents_considered = count;
        self
    }

    pub fn selected_agent(mut self, id: Uuid, name: String) -> Self {
        self.selected_agent_id = Some(id);
        self.selected_agent_name = Some(name);
        self
    }

    pub fn selection_reasoning(mut self, reasoning: String) -> Self {
        self.selection_reasoning = Some(reasoning);
        self
    }

    pub fn fallback_used(mut self, used: bool) -> Self {
        self.fallback_used = used;
        self
    }

    pub fn selection_token_usage(mut self, id: Uuid) -> Self {
        self.selection_token_usage_id = Some(id);
        self
    }

    pub fn total_latency_ms(mut self, ms: i32) -> Self {
        self.total_latency_ms = ms;
        self
    }

    pub fn registry_fetch_ms(mut self, ms: i32) -> Self {
        self.registry_fetch_ms = Some(ms);
        self
    }

    pub fn vector_store_ms(mut self, ms: i32) -> Self {
        self.vector_store_ms = Some(ms);
        self
    }

    pub fn selection_llm_ms(mut self, ms: i32) -> Self {
        self.selection_llm_ms = Some(ms);
        self
    }

    pub fn agent_call_ms(mut self, ms: i32) -> Self {
        self.agent_call_ms = Some(ms);
        self
    }

    pub fn success(mut self, success: bool) -> Self {
        self.success = success;
        self
    }

    pub fn error_message(mut self, error: String) -> Self {
        self.error_message = Some(error);
        self
    }

    pub fn finish_reason(mut self, reason: String) -> Self {
        self.finish_reason = Some(reason);
        self
    }

    pub fn streaming(mut self, streaming: bool) -> Self {
        self.streaming = streaming;
        self
    }

    pub fn file_count(mut self, count: i32) -> Self {
        self.file_count = count;
        self
    }

    pub fn metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }

    pub fn build(self) -> CreateRouterRequestLog {
        CreateRouterRequestLog {
            request_id: self.request_id,
            user_id: self.user_id,
            session_id: self.session_id,
            query: self.query,
            agents_considered: self.agents_considered,
            selected_agent_id: self.selected_agent_id,
            selected_agent_name: self.selected_agent_name,
            selection_reasoning: self.selection_reasoning,
            fallback_used: self.fallback_used,
            selection_token_usage_id: self.selection_token_usage_id,
            total_latency_ms: self.total_latency_ms,
            registry_fetch_ms: self.registry_fetch_ms,
            vector_store_ms: self.vector_store_ms,
            selection_llm_ms: self.selection_llm_ms,
            agent_call_ms: self.agent_call_ms,
            success: self.success,
            error_message: self.error_message,
            finish_reason: self.finish_reason,
            streaming: self.streaming,
            file_count: self.file_count,
            metadata: self.metadata,
        }
    }
}

/// Input for creating a router request log
#[derive(Debug, Clone)]
pub struct CreateRouterRequestLog {
    pub request_id: String,
    pub user_id: Uuid,
    pub session_id: String,
    pub query: String,
    pub agents_considered: i32,
    pub selected_agent_id: Option<Uuid>,
    pub selected_agent_name: Option<String>,
    pub selection_reasoning: Option<String>,
    pub fallback_used: bool,
    pub selection_token_usage_id: Option<Uuid>,
    pub total_latency_ms: i32,
    pub registry_fetch_ms: Option<i32>,
    pub vector_store_ms: Option<i32>,
    pub selection_llm_ms: Option<i32>,
    pub agent_call_ms: Option<i32>,
    pub success: bool,
    pub error_message: Option<String>,
    pub finish_reason: Option<String>,
    pub streaming: bool,
    pub file_count: i32,
    pub metadata: serde_json::Value,
}
