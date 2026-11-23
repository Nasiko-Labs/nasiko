//! Chat calls for prompts P1 and P3, through the Nasiko router as the `eval` identity, each
//! tagged with an attempt tag so the router counts what building the dataset cost.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use nasiko_evals::prompt::ChatPrompt;
use serde_json::{Value, json};

use super::router::{ATTEMPT_HEADER, RouterClient};
use super::store::ModelEntry;

/// Longest wait for one completion (local models can be slow).
const CALL_TIMEOUT: Duration = Duration::from_secs(900);

/// Calls one pinned model through the router.
pub struct LlmClient<'a> {
    router: &'a RouterClient,
    agent_id: String,
    http: ureq::Agent,
}

impl<'a> LlmClient<'a> {
    /// Pins the eval identity to `model` and gets ready to call it.
    pub fn pinned(router: &'a RouterClient, short: &str, model: &ModelEntry) -> Result<Self> {
        let agent_id = router.ensure_eval_agent()?;
        router.pin_model(&agent_id, short, model)?;
        Ok(Self {
            router,
            agent_id,
            http: ureq::Agent::new_with_config(
                ureq::config::Config::builder()
                    .timeout_global(Some(CALL_TIMEOUT))
                    .http_status_as_error(false)
                    .build(),
            ),
        })
    }

    /// One chat completion; `tag` is the attempt tag the router stores with the call.
    pub fn complete(&self, prompt: &ChatPrompt, tag: &str) -> Result<String> {
        let token = self.router.mint_token(&self.agent_id)?;
        let url = format!("{}/chat/completions", self.router.router_base());
        let body = json!({
            "model": "router",
            "temperature": 0,
            "messages": [
                {"role": "system", "content": prompt.system},
                {"role": "user", "content": prompt.user},
            ],
        });
        let mut resp = self
            .http
            .post(&url)
            .header("Authorization", &format!("Bearer {}", token.token))
            .header(ATTEMPT_HEADER, tag)
            .send_json(&body)
            .with_context(|| format!("could not reach the Nasiko router at {url}"))?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().read_to_string()?;
        if status >= 400 {
            bail!(
                "router answered {status}: {}",
                text.chars().take(400).collect::<String>()
            );
        }
        reply_text(&text)
    }
}

/// The assistant text of an OpenAI-style chat completion.
fn reply_text(body: &str) -> Result<String> {
    let v: Value = serde_json::from_str(body).context("the router's reply is not JSON")?;
    v["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_string)
        .context("the router's reply has no message content")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_first_choice() {
        let body =
            r#"{"choices":[{"message":{"role":"assistant","content":"{\"status\":\"OK\"}"}}]}"#;
        assert_eq!(reply_text(body).expect("text"), "{\"status\":\"OK\"}");
        assert!(reply_text("{}").is_err());
    }
}
