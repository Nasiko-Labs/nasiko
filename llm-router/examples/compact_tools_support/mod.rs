use std::time::{Duration, Instant};

use nasiko_tool_compact::{ToolCall, ToolDef, decode_calls};
use serde::Deserialize;
use serde_json::{Value, json};

type Error = Box<dyn std::error::Error>;

#[derive(Deserialize)]
pub struct EvalSet {
    pub tools: Vec<Value>,
    pub cases: Vec<Case>,
    #[serde(default)]
    pub decoder_cases: Vec<DecoderCase>,
}

#[derive(Deserialize)]
pub struct Case {
    pub id: String,
    pub tools: Vec<String>,
    pub messages: Vec<Value>,
    pub expected: Vec<ToolCall>,
}

#[derive(Deserialize)]
pub struct DecoderCase {
    pub id: String,
    pub tools: Vec<String>,
    pub chunks: Vec<String>,
}

pub fn select_tools(catalog: &[Value], names: &[String]) -> Result<Vec<Value>, Error> {
    names
        .iter()
        .map(|name| {
            catalog
                .iter()
                .find(|tool| tool.pointer("/function/name").and_then(Value::as_str) == Some(name))
                .cloned()
                .ok_or_else(|| format!("evaluation references missing tool: {name}").into())
        })
        .collect()
}

pub fn native_request(dataset: &EvalSet, case: &Case) -> Result<Value, Error> {
    Ok(json!({"messages":case.messages,"tools":select_tools(&dataset.tools, &case.tools)?}))
}

pub struct LiveConfig {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    temperature: f64,
    reasoning_effort: Option<String>,
    key: Option<String>,
    baseline: bool,
}

impl LiveConfig {
    pub fn configure_request(&self, request: &mut Value) {
        self.configure(request);
        if let Some(messages) = request["messages"].as_array_mut() {
            messages.insert(
                0,
                json!({"role":"system","content":"Today is 2026-10-03. Timezone: Asia/Kolkata."}),
            );
        }
    }

    pub fn from_env() -> Result<Option<Self>, Error> {
        let base = std::env::var("PROVIDER_BASE_URL")
            .ok()
            .filter(|s| !s.is_empty());
        let model = std::env::var("MODEL").ok().filter(|s| !s.is_empty());
        let (base, model) = match (base, model) {
            (None, None) => return Ok(None),
            (Some(base), Some(model)) => (base, model),
            _ => return Err("live mode requires both PROVIDER_BASE_URL and MODEL".into()),
        };
        let temperature = match std::env::var("LIVE_TEMPERATURE") {
            Ok(value) => value.parse::<f64>()?,
            Err(_) => 0.0,
        };
        if !temperature.is_finite() || !(0.0..=2.0).contains(&temperature) {
            return Err("LIVE_TEMPERATURE must be a finite number from 0 through 2".into());
        }
        let reasoning_effort = std::env::var("LIVE_REASONING_EFFORT")
            .ok()
            .filter(|value| !value.is_empty());
        if reasoning_effort.as_deref().is_some_and(|value| {
            !matches!(value, "none" | "low" | "medium" | "high" | "xhigh")
        }) {
            return Err("LIVE_REASONING_EFFORT must be none, low, medium, high or xhigh".into());
        }
        let url = reqwest::Url::parse(&base)?;
        if url.scheme() != "https"
            && !(url.scheme() == "http"
                && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")))
        {
            return Err("use HTTPS for a hosted endpoint; HTTP is allowed only on loopback".into());
        }
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("endpoint must not contain credentials, a query, or a fragment".into());
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let endpoint = format!("{}/chat/completions", base.trim_end_matches('/'));
        let key = std::env::var("PROVIDER_API_KEY")
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .ok()
            .filter(|key| !key.is_empty());
        Ok(Some(Self {
            client,
            endpoint,
            model,
            temperature,
            reasoning_effort,
            key,
            baseline: std::env::var("LIVE_BASELINE").as_deref() == Ok("1"),
        }))
    }

    pub async fn add_outputs(
        &self,
        record: &mut Value,
        native: &Value,
        tools: &[ToolDef],
    ) -> Result<(), Error> {
        self.configure(&mut record["compact_request"]);
        let (body, elapsed) = self.send(&record["compact_request"]).await?;
        record["live_latency_ms"] = json!(elapsed);
        record["live_usage"] = body.get("usage").cloned().unwrap_or(Value::Null);
        record["raw_output"] = body
            .pointer("/choices/0/message/content")
            .cloned()
            .unwrap_or(Value::Null);
        record["live_calls"] = if record["compacted"] == true {
            if body
                .get("choices")
                .and_then(Value::as_array)
                .is_none_or(|choices| choices.len() != 1)
                || body
                    .pointer("/choices/0/message/tool_calls")
                    .and_then(Value::as_array)
                    .is_some_and(|calls| !calls.is_empty())
                || !matches!(record["raw_output"], Value::String(_) | Value::Null)
            {
                json!({"error":"malformed_output"})
            } else if body
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)
                != Some("stop")
            {
                json!({"error":"incomplete_call"})
            } else {
                let output = record["raw_output"].as_str().unwrap_or("");
                match decode_calls(output, tools) {
                    Ok(calls) => json!({"calls":calls}),
                    Err(error) => json!({"error":error.code()}),
                }
            }
        } else {
            native_calls(&body)?
        };
        if self.baseline {
            let mut request = native.clone();
            self.configure(&mut request);
            let (body, elapsed) = self.send(&request).await?;
            record["baseline_calls"] = native_calls(&body)?;
            record["baseline_usage"] = body.get("usage").cloned().unwrap_or(Value::Null);
            record["baseline_latency_ms"] = json!(elapsed);
        }
        Ok(())
    }

    fn configure(&self, request: &mut Value) {
        request["model"] = json!(self.model);
        request["temperature"] = json!(self.temperature);
        request["max_completion_tokens"] = json!(1024);
        if let Some(reasoning_effort) = &self.reasoning_effort {
            request["reasoning_effort"] = json!(reasoning_effort);
        }
    }

    async fn send(&self, request: &Value) -> Result<(Value, u128), Error> {
        let mut call = self.client.post(&self.endpoint).json(request);
        if let Some(key) = &self.key {
            call = call.bearer_auth(key);
        }
        let started = Instant::now();
        let response = call
            .send()
            .await
            .map_err(|_| "live provider connection failed")?;
        if !response.status().is_success() {
            return Err(
                format!("live provider returned HTTP {}", response.status().as_u16()).into(),
            );
        }
        let body = response
            .json()
            .await
            .map_err(|_| "invalid live provider JSON")?;
        Ok((body, started.elapsed().as_millis()))
    }
}

fn native_calls(body: &Value) -> Result<Value, Error> {
    let Some(message) = body.pointer("/choices/0/message") else {
        return Ok(json!({"error":"malformed_output"}));
    };
    if !matches!(
        body.pointer("/choices/0/finish_reason")
            .and_then(Value::as_str),
        Some("stop" | "tool_calls")
    ) {
        return Ok(json!({"error":"incomplete_call"}));
    }
    let mut calls = Vec::new();
    if let Some(native) = message.get("tool_calls").and_then(Value::as_array) {
        for call in native {
            let name = call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .ok_or("native response missing tool name")?;
            let arguments = call
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .ok_or("native response missing arguments")?;
            let arguments: Value = serde_json::from_str(arguments)?;
            calls.push(json!({"name":name,"arguments":arguments}));
        }
    }
    Ok(json!({"calls":calls}))
}
