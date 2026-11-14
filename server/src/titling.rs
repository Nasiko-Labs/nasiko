//! Best-effort LLM title generation, shared by anything that turns a user's
//! first prompt into a short, human-readable label — chat sessions
//! (`chat::routes`) and, in EE, Weave's generated views.
//!
//! One non-streaming call to the cheap task model, with a plain-truncation
//! fallback so a slow or unconfigured provider never blocks the thing the
//! title is attached to.

use nasiko_orchestrator::models::{ChatCompletionRequest, ChatMessage as LlmMessage};
use nasiko_orchestrator::providers::ProviderError;
use uuid::Uuid;

use crate::state::AppState;

/// Cap on both the generated and fallback title lengths (chars).
pub const MAX_TITLE_CHARS: usize = 80;

/// Derive a short title from a non-empty, already-trimmed prompt.
///
/// Best-effort: a single LLM call summarizes the prompt into a few words. If
/// the provider is unconfigured or the call fails, this falls back to a
/// truncated form of the prompt — the caller is responsible for the
/// empty-prompt default (e.g. "New chat", "New view"), since that default is
/// context-specific and this function's contract is "given real text, title
/// it."
pub async fn title_from_prompt(state: &AppState, user_id: Uuid, prompt: &str) -> String {
    match generate_title(state, user_id, prompt).await {
        Ok(title) if !title.is_empty() => title,
        Ok(_) => truncate_title(prompt),
        Err(e) => {
            tracing::warn!(%e, "title generation failed; falling back to truncated prompt");
            truncate_title(prompt)
        }
    }
}

/// Single non-streaming LLM call that summarizes `prompt` into a title.
async fn generate_title(
    state: &AppState,
    user_id: Uuid,
    prompt: &str,
) -> Result<String, ProviderError> {
    // A user's saved OpenAI key is spent directly; otherwise the platform call goes
    // through the in-process router, which picks the provider and records usage.
    let provider = match nasiko_orchestrator::user_key::saved_openai_key(&state.db, user_id).await {
        Some(key) => {
            nasiko_orchestrator::providers::LLMProvider::from_env(state.http_client.clone())
                .with_api_key(key)
        }
        None => nasiko_orchestrator::providers::LLMProvider::from_env(state.http_client.clone())
            .with_backend(
                std::sync::Arc::clone(&state.llm_backend),
                nasiko_orchestrator::llm_backend::LlmCallContext {
                    user_id,
                    component: nasiko_orchestrator::llm_backend::PlatformComponent::Titling
                        .to_string(),
                    session_id: None,
                },
            ),
    };

    let request = ChatCompletionRequest {
        // Reuse the cheap task model already configured for capability
        // generation (`CAPABILITY_GENERATOR_MODEL`, default gpt-4o-mini) —
        // titling is a small, low-stakes summarization.
        model: state.config.capability_generator_model.clone(),
        messages: vec![
            LlmMessage {
                role: "system".to_string(),
                content: Some(
                    r#"You generate concise titles for chat conversations.

                    Your task is to summarize the USER'S INTENT, not the content they provide.

                    Rules:
                    - Generate a title of 3-6 words.
                    - Focus on what the user wants the assistant to do.
                    - If the user asks to translate text, make the title about translation (e.g. "English to Spanish Translation"), not the text being translated.
                    - If the user asks to summarize, emphasize summarization.
                    - If the user asks to write code, emphasize the coding task.
                    - If the user asks a question, summarize the question's purpose.
                    - Do not quote or repeat large parts of the user's input.
                    - Do not use prefixes like "Title:".
                    - Do not use surrounding quotes.
                    - Do not end with punctuation.
                    - Return ONLY the title."#
                        .to_string(),
                ),
            },
            LlmMessage {
                role: "user".to_string(),
                content: Some(prompt.to_string()),
            },
        ],
        stream: false,
        temperature: Some(0.2),
        max_tokens: Some(1024),
        response_format: None,
        stream_options: None,
    };

    let result = provider.chat_completion(&request).await?;
    Ok(sanitize_title(&result.content))
}

/// Trim whitespace, strip a single layer of surrounding quotes the model may add,
/// and cap the length.
fn sanitize_title(raw: &str) -> String {
    let trimmed = raw.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(trimmed)
        .trim();
    truncate_title(unquoted)
}

/// Cap a title at `MAX_TITLE_CHARS` on a char boundary (never mid-codepoint).
pub fn truncate_title(s: &str) -> String {
    let s = s.trim();
    match s.char_indices().nth(MAX_TITLE_CHARS) {
        Some((idx, _)) => s[..idx].trim_end().to_string(),
        None => s.to_string(),
    }
}
