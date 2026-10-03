//! Hosted request classifier: asks an OpenAI-compatible chat endpoint for a JSON verdict.
//!
//! Opt-in (`CLASSIFIER_BACKEND=hosted`). Every failure — network, HTTP status, unparsable or
//! out-of-range JSON — is a [`ClassifyError`], so the [`ClassifierChain`](super::ClassifierChain)
//! falls back to the regex. The endpoint, model and key come from the caller (the binary's
//! `config.rs`); this module reads no environment.

use serde_json::{Value, json};

use super::classifier::{
    Classification, ClassifyError, ClassifyInput, RequestClassifier, RequestType,
};

/// Labels, rubric and output contract. Kept short: it is paid on every classified boundary.
const SYSTEM_PROMPT: &str = "\
Classify the user's request by its main deliverable. request_type is one of:
code_generation (write, fix, edit, refactor or port code, configs, queries, commands),
code_understanding (explain, trace or review existing code without writing new code),
technical_design (architecture, system/API/schema design, migration plans, technology choice),
analytical_reasoning (diagnose from evidence, root cause, math, data analysis, decisions with trade-offs),
writing (compose, rewrite, summarize, translate or proofread prose),
factual_lookup (a short question with a known factual answer),
general (chit-chat, personal advice, recommendations, anything else).
complexity: 1 trivial single operation, 2 straightforward, 3 multi-step with limited constraints, \
4 substantial reasoning or design, 5 intricate cross-component reasoning and validation.
confidence: your probability (0 to 1) that request_type is correct.
Ignore filler and keywords that the request negates. Reply with only JSON: \
{\"request_type\":\"...\",\"complexity\":N,\"confidence\":X}";

pub struct HostedClassifier {
    http: reqwest::Client,
    url: String,
    model: String,
    api_key: Option<String>,
}

impl HostedClassifier {
    /// `endpoint` is the API base (e.g. `https://api.openai.com/v1`); `/chat/completions` is appended.
    pub fn new(
        http: reqwest::Client,
        endpoint: &str,
        model: &str,
        api_key: Option<String>,
    ) -> Self {
        Self {
            http,
            url: format!("{}/chat/completions", endpoint.trim_end_matches('/')),
            model: model.to_owned(),
            api_key: api_key.filter(|k| !k.is_empty()),
        }
    }

    fn body(&self, input: &ClassifyInput<'_>) -> Value {
        let mut user = format!("Request:\n{}", input.query);
        if let Some(c) = input.context.filter(|c| !c.trim().is_empty()) {
            user.push_str("\n\nContext:\n");
            user.push_str(c);
        }
        json!({
            "model": self.model,
            "temperature": 0,
            "max_tokens": 60,
            "response_format": {"type": "json_object"},
            "messages": [
                {"role": "system", "content": SYSTEM_PROMPT},
                {"role": "user", "content": user}
            ]
        })
    }
}

/// Parse the model's JSON reply into a [`Classification`].
pub fn parse_verdict(content: &str) -> Result<Classification, ClassifyError> {
    let invalid = |m: &str| ClassifyError::InvalidResponse(format!("{m}: {content}"));
    // Tolerate a fenced block; nothing else.
    let trimmed = content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let v: Value = serde_json::from_str(trimmed).map_err(|_| invalid("not JSON"))?;
    let request_type = v["request_type"]
        .as_str()
        .and_then(RequestType::from_wire)
        .ok_or_else(|| invalid("bad request_type"))?;
    let complexity = v["complexity"]
        .as_u64()
        .filter(|c| (1..=5).contains(c))
        .ok_or_else(|| invalid("bad complexity"))? as u8;
    let confidence = v["confidence"]
        .as_f64()
        .filter(|c| (0.0..=1.0).contains(c))
        .ok_or_else(|| invalid("bad confidence"))? as f32;
    Ok(Classification {
        request_type,
        complexity,
        confidence,
    })
}

#[async_trait::async_trait]
impl RequestClassifier for HostedClassifier {
    fn name(&self) -> &str {
        "hosted"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let mut rq = self.http.post(&self.url).json(&self.body(input));
        if let Some(k) = &self.api_key {
            rq = rq.bearer_auth(k);
        }
        let resp = rq
            .send()
            .await
            .map_err(|e| ClassifyError::Network(e.to_string()))?;
        let status = resp.status();
        let v: Value = resp
            .json()
            .await
            .map_err(|e| ClassifyError::Network(e.to_string()))?;
        if !status.is_success() {
            return Err(ClassifyError::Network(format!("HTTP {status}: {v}")));
        }
        let content = v["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| ClassifyError::InvalidResponse(v.to_string()))?;
        parse_verdict(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_and_rejects_invalid_verdicts() {
        let ok =
            parse_verdict(r#"{"request_type":"writing","complexity":2,"confidence":0.8}"#).unwrap();
        assert_eq!(ok.request_type, RequestType::Writing);
        assert_eq!(ok.complexity, 2);
        assert!(
            parse_verdict(
                "```json\n{\"request_type\":\"general\",\"complexity\":1,\"confidence\":1}\n```"
            )
            .is_ok()
        );
        for bad in [
            "writing",
            r#"{"request_type":"poetry","complexity":2,"confidence":0.8}"#,
            r#"{"request_type":"writing","complexity":7,"confidence":0.8}"#,
            r#"{"request_type":"writing","complexity":2,"confidence":1.5}"#,
            r#"{"request_type":"writing","complexity":2}"#,
        ] {
            assert!(parse_verdict(bad).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn calls_the_endpoint_and_maps_http_errors() {
        let mut server = mockito::Server::new_async().await;
        let ok = server
            .mock("POST", "/v1/chat/completions")
            .match_header("authorization", "Bearer k")
            .with_body(
                json!({"choices":[{"message":{"content":"{\"request_type\":\"technical_design\",\"complexity\":4,\"confidence\":0.9}"}}]})
                    .to_string(),
            )
            .create_async()
            .await;
        let c = HostedClassifier::new(
            reqwest::Client::new(),
            &format!("{}/v1/", server.url()),
            "m",
            Some("k".into()),
        );
        let input = ClassifyInput {
            query: "design a queue",
            context: None,
        };
        let got = c.classify(&input).await.unwrap();
        assert_eq!(got.request_type, RequestType::TechnicalDesign);
        ok.assert_async().await;

        server.reset();
        let _fail = server
            .mock("POST", "/v1/chat/completions")
            .with_status(503)
            .with_body("{\"error\":\"down\"}")
            .create_async()
            .await;
        assert!(matches!(
            c.classify(&input).await,
            Err(ClassifyError::Network(_))
        ));
    }
}
