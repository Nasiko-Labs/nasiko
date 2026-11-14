//! Model-name normalization for direct (saved-key) OpenAI calls, shared with the orchestrator.

/// True when `base_url`'s host is exactly `api.openai.com` (proxies such as
/// `api.openai.com.proxy.example` are custom endpoints and keep their model names).
pub fn is_official_openai(base_url: &str) -> bool {
    reqwest::Url::parse(base_url.trim())
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|host| host == "api.openai.com")
}

/// True when `model` is clearly another provider's name and OpenAI would reject it.
pub fn is_foreign_model(model: &str) -> bool {
    let m: String = model.trim().to_ascii_lowercase();
    m.is_empty()
        || m.contains('/')
        || [
            "gemini",
            "claude",
            "deepseek",
            "text-embedding-0",
            "llama",
            "mistral",
            "qwen",
        ]
        .iter()
        .any(|p| m.starts_with(p))
}

/// The model to send OpenAI directly: `requested` unless it is foreign, else `openai_model`.
pub fn direct_openai_model(requested: &str, openai_model: &str) -> String {
    if is_foreign_model(requested) {
        openai_model.to_owned()
    } else {
        requested.to_owned()
    }
}

/// OpenAI chat model used on the official API when neither the request nor
/// `OPENAI_MODEL` names an OpenAI model. Only direct OpenAI calls need it; routed
/// calls leave the model blank and the LLM router picks its provider's default.
pub const OPENAI_CHAT_FALLBACK: &str = "gpt-6-luna";

/// Model a direct OpenAI call swaps a foreign model to: `OPENAI_MODEL` when it is
/// an OpenAI model, else [`OPENAI_CHAT_FALLBACK`].
pub fn direct_openai_fallback() -> String {
    std::env::var("OPENAI_MODEL")
        .ok()
        .map(|m: String| m.trim().to_owned())
        .filter(|m: &String| !is_foreign_model(m))
        .unwrap_or_else(|| OPENAI_CHAT_FALLBACK.to_owned())
}

/// Chat model for a direct call to `base_url`: empty → `fallback` (any host, an empty model is
/// never valid); a foreign model (e.g. an inherited Gemini `DEFAULT_MODEL`) → `fallback` only on
/// the official `api.openai.com` host; otherwise `requested` unchanged (custom endpoints keep it).
pub fn resolve_direct_chat_model(requested: &str, base_url: &str, fallback: &str) -> String {
    let trimmed: &str = requested.trim();
    if trimmed.is_empty() || (is_official_openai(base_url) && is_foreign_model(trimmed)) {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OFFICIAL: &str = "https://api.openai.com/v1";
    const CUSTOM: &str = "https://llm.example.com/v1";

    #[test]
    fn empty_model_gets_fallback_on_any_host() {
        assert_eq!(resolve_direct_chat_model("", OFFICIAL, "gpt-x"), "gpt-x");
        assert_eq!(resolve_direct_chat_model("  ", CUSTOM, "gpt-x"), "gpt-x");
    }

    #[test]
    fn foreign_model_swapped_only_on_official_host() {
        assert_eq!(
            resolve_direct_chat_model("gemini-4-hydrogen", OFFICIAL, "gpt-x"),
            "gpt-x"
        );
        assert_eq!(
            resolve_direct_chat_model("gemini-4-hydrogen", CUSTOM, "gpt-x"),
            "gemini-4-hydrogen"
        );
        assert_eq!(
            resolve_direct_chat_model(
                "gemini-4-hydrogen",
                "https://api.openai.com.proxy.example/v1",
                "gpt-x"
            ),
            "gemini-4-hydrogen"
        );
    }

    #[test]
    fn openai_model_kept() {
        assert_eq!(
            resolve_direct_chat_model("gpt-6.1-sol", OFFICIAL, "gpt-x"),
            "gpt-6.1-sol"
        );
    }
}
