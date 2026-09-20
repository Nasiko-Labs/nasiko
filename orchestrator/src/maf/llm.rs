use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    pub model: String,
}

#[derive(Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: "system".into(), content: content.into() }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: "user".into(), content: content.into() }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: "assistant".into(), content: content.into() }
    }
}

#[derive(Deserialize)]
struct CompletionResponse {
    choices: Vec<Choice>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Choice {
    message: MessageContent,
}

#[derive(Deserialize)]
struct MessageContent {
    content: String,
}

#[derive(Deserialize)]
struct Usage {
    total_tokens: i64,
}

impl LlmClient {
    pub fn new(
        http: reqwest::Client,
        api_key: String,
        base_url: Option<String>,
        model: String,
    ) -> Self {
        Self {
            http,
            api_key,
            base_url: base_url.unwrap_or_else(|| "https://api.openai.com/v1".into()),
            model,
        }
    }

    /// Returns `(content, tokens_used)`.
    pub async fn chat(&self, messages: Vec<ChatMessage>) -> Result<(String, i64), String> {
        self.chat_inner(messages, None).await
    }

    /// Returns `(parsed_json, tokens_used)`. Uses `json_object` mode — valid JSON only,
    /// no schema enforcement.
    pub async fn chat_json(&self, messages: Vec<ChatMessage>) -> Result<(serde_json::Value, i64), String> {
        let fmt = serde_json::json!({"type": "json_object"});
        let (text, tokens) = self.chat_inner(messages, Some(fmt)).await?;
        let json = serde_json::from_str(&text)
            .map_err(|e| format!("LLM returned invalid JSON: {e}\nRaw: {text}"))?;
        Ok((json, tokens))
    }

    /// Returns `(parsed_json, tokens_used)`. Uses OpenAI structured outputs with the provided
    /// JSON Schema — equivalent to Python's `with_structured_output(PydanticModel)`.
    /// With `strict: true` the API guarantees the response matches the schema exactly.
    pub async fn chat_json_schema(
        &self,
        messages: Vec<ChatMessage>,
        schema_name: &str,
        schema: serde_json::Value,
    ) -> Result<(serde_json::Value, i64), String> {
        let fmt = serde_json::json!({
            "type": "json_schema",
            "json_schema": {
                "name": schema_name,
                "strict": true,
                "schema": schema
            }
        });
        let (text, tokens) = self.chat_inner(messages, Some(fmt)).await?;
        let json = serde_json::from_str(&text)
            .map_err(|e| format!("LLM returned invalid JSON: {e}\nRaw: {text}"))?;
        Ok((json, tokens))
    }

    async fn chat_inner(
        &self,
        messages: Vec<ChatMessage>,
        response_format: Option<serde_json::Value>,
    ) -> Result<(String, i64), String> {
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "temperature": 0,
        });

        if let Some(fmt) = response_format {
            body["response_format"] = fmt;
        }

        let resp = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .timeout(std::time::Duration::from_secs(120))
            .send()
            .await
            .map_err(|e| format!("LLM request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("LLM HTTP {status}: {body}"));
        }

        let parsed: CompletionResponse =
            resp.json().await.map_err(|e| format!("LLM response parse error: {e}"))?;

        let tokens = parsed.usage.map(|u| u.total_tokens).unwrap_or(0);
        let content = parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or_else(|| "LLM returned no choices".to_string())?;

        Ok((content, tokens))
    }
}
