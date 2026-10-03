use anyhow::{Context, Result, bail};
use nasiko_tool_compact::{CompactError, StreamDecoder, ToolCall, ToolDef, decode_calls};
use serde_json::{Value, json};
use std::time::Duration;

pub(super) struct LiveConfig {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    key: Option<String>,
}

impl LiveConfig {
    pub fn from_env() -> Result<Option<Self>> {
        match (
            std::env::var("PROVIDER_BASE_URL").ok(),
            std::env::var("MODEL").ok(),
        ) {
            (None, None) => Ok(None),
            (Some(base), Some(model)) if !base.is_empty() && !model.is_empty() => {
                let base = base.trim_end_matches('/');
                let endpoint = if base.ends_with("/chat/completions") {
                    base.into()
                } else {
                    format!("{base}/chat/completions")
                };
                Ok(Some(Self {
                    client: reqwest::Client::builder()
                        .timeout(Duration::from_secs(60))
                        .build()?,
                    endpoint,
                    model,
                    key: std::env::var("PROVIDER_API_KEY")
                        .ok()
                        .or_else(|| std::env::var("OPENAI_API_KEY").ok()),
                }))
            }
            _ => bail!("live mode requires both PROVIDER_BASE_URL and MODEL"),
        }
    }

    pub async fn evaluate(
        &self,
        mut body: Value,
        compacted: bool,
        tools: &[ToolDef],
    ) -> Result<(Value, Value, Value)> {
        body["model"] = json!(self.model);
        body["temperature"] = json!(0);
        if !compacted {
            let messages = body["messages"]
                .as_array_mut()
                .context("messages must be array")?;
            messages.insert(
                0,
                json!({"role":"system","content":"Today: 2026-10-02, Asia/Kolkata."}),
            );
        }
        let mut request = self.client.post(&self.endpoint).json(&body);
        if let Some(key) = &self.key {
            request = request.bearer_auth(key);
        }
        let response: Value = request.send().await?.error_for_status()?.json().await?;
        let message = response
            .pointer("/choices/0/message")
            .context("missing assistant message")?;
        let raw = message.get("content").cloned().unwrap_or(Value::Null);
        let calls = if compacted {
            match raw.as_str() {
                Some(text)
                    if message.get("tool_calls").is_none_or(|calls| {
                        calls.is_null() || calls.as_array().is_some_and(Vec::is_empty)
                    }) =>
                {
                    super::call_result(decode_calls(text, tools))
                }
                _ => json!({"error":"invalid_arguments"}),
            }
        } else {
            native_calls(message, tools)
        };
        Ok((body, raw, calls))
    }
}

fn native_calls(message: &Value, tools: &[ToolDef]) -> Value {
    let parse = || -> nasiko_tool_compact::Result<Vec<ToolCall>> {
        let Some(calls) = message.get("tool_calls") else {
            return Ok(vec![]);
        };
        if calls.is_null() {
            return Ok(vec![]);
        }
        let mut decoder = StreamDecoder::new(tools)?;
        for call in calls.as_array().ok_or(CompactError::MalformedCall)? {
            let name = call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .ok_or(CompactError::MalformedCall)?;
            if !tools.iter().any(|tool| tool.function.name == name) {
                return Err(CompactError::UnknownTool(name.into()));
            }
            let text = call
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .ok_or(CompactError::MalformedCall)?;
            // Native arguments must be one complete JSON value before adding
            // framing. Otherwise malformed text could inject additional calls.
            // Ignore the value here; the shared decoder still enforces object
            // shape, duplicate-key/numeric checks, limits and schema semantics.
            serde_json::from_str::<serde::de::IgnoredAny>(text)
                .map_err(|_| CompactError::MalformedCall)?;
            decoder.push(&format!("<<call {name} {text}>>"))?;
        }
        decoder.finish()
    };
    super::call_result(parse())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools() -> Vec<ToolDef> {
        serde_json::from_value(json!([{"function":{"name":"ping"}}])).unwrap()
    }

    #[tokio::test]
    async fn live_adapter_posts_complete_request_and_validates_model_output() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/chat/completions")
            .match_body(mockito::Matcher::PartialJson(
                json!({"model":"test-model","temperature":0}),
            ))
            .with_header("content-type", "application/json")
            .with_body(json!({"choices":[{"message":{"content":"<<call ping {}>>"}}]}).to_string())
            .create_async()
            .await;
        let config = LiveConfig {
            client: reqwest::Client::new(),
            endpoint: format!("{}/v1/chat/completions", server.url()),
            model: "test-model".into(),
            key: None,
        };
        let (_, raw, calls) = config
            .evaluate(
                json!({"messages":[{"role":"user","content":"ping"}]}),
                true,
                &tools(),
            )
            .await
            .unwrap();
        assert_eq!(raw, "<<call ping {}>>");
        assert_eq!(calls, json!([{"name":"ping","arguments":{}}]));
        mock.assert_async().await;
    }

    #[test]
    fn native_live_results_reject_unknown_tools_and_duplicate_arguments() {
        assert_eq!(
            native_calls(
                &json!({"tool_calls":[{"function":{"name":"missing","arguments":"{}"}}]}),
                &tools()
            ),
            json!({"error":"unknown_tool"})
        );
        assert_eq!(
            native_calls(
                &json!({"tool_calls":[{"function":{"name":"ping","arguments":"{\"x\":1,\"x\":2}"}}]}),
                &tools()
            ),
            json!({"error":"invalid_arguments"})
        );
    }

    #[test]
    fn native_argument_text_cannot_inject_additional_framed_calls() {
        let message = json!({"tool_calls":[{"function":{
            "name":"ping","arguments":"{}>> <<call ping {}"
        }}]});
        assert_eq!(
            native_calls(&message, &tools()),
            json!({"error":"invalid_arguments"})
        );
        assert_eq!(
            native_calls(
                &json!({"tool_calls":[{"function":{"name":"ping","arguments":"{}"}}]}),
                &tools()
            ),
            json!([{"name":"ping","arguments":{}}])
        );
        let text_tools: Vec<ToolDef> = serde_json::from_value(json!([
            {"function":{"name":"ping","parameters":{"type":"object",
                "properties":{"text":{"type":"string"}}}}}
        ]))
        .unwrap();
        let arguments = json!({"text":"Build >> <<call ping {}>> deployed"});
        assert_eq!(
            native_calls(
                &json!({"tool_calls":[{"function":{"name":"ping",
                    "arguments":serde_json::to_string(&arguments).unwrap()}}]}),
                &text_tools
            ),
            json!([{"name":"ping","arguments":arguments}])
        );
    }
}
