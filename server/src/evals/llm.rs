//! Model calls the eval pipeline makes itself (rewriting a request, screening a task, the
//! preflight check): through this server's own router, as the caller's eval identity, so each is
//! priced and filed under its tag like any replayed agent's call.

use std::time::Duration;

use anyhow::{Context, Result};
use nasiko_evals::prompt::ChatPrompt;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::state::AppState;

/// How long one completion may take (a slow local model writing a long review).
const CALL_TIMEOUT: Duration = Duration::from_secs(900);

/// One chat completion on `config_id` (one of `user_id`'s LLM configs), filed under `tag`.
pub async fn complete(
    state: &AppState,
    user_id: Uuid,
    config_id: Uuid,
    tag: &str,
    prompt: &ChatPrompt,
) -> Result<String> {
    let agent = super::identity::ensure(&state.db, user_id).await?;
    let token = super::identity::attempt_token(
        agent,
        user_id,
        tag,
        config_id,
        CALL_TIMEOUT.as_secs() + 60,
    )?;
    let body = json!({
        "model": "router",
        "temperature": 0,
        "messages": [
            {"role": "system", "content": prompt.system},
            {"role": "user", "content": prompt.user},
        ],
    });
    let response = state
        .http_client
        .post(format!(
            "{}/v1/chat/completions",
            self_url(&state.config.bind)
        ))
        .bearer_auth(token)
        .timeout(CALL_TIMEOUT)
        .json(&body)
        .send()
        .await
        .context("the router could not be reached")?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    anyhow::ensure!(
        status.is_success(),
        "the router answered {status}: {}",
        nasiko_runtime::batch::tail(&text, 500)
    );
    reply_text(&text)
}

/// This server's own address for calls to itself: its bind address, with an unspecified host
/// (`0.0.0.0`, `[::]`) read as loopback.
pub fn self_url(bind: &str) -> String {
    let port = bind.rsplit(':').next().unwrap_or("8080");
    format!("http://127.0.0.1:{port}")
}

fn reply_text(body: &str) -> Result<String> {
    let value: Value = serde_json::from_str(body).context("the router's reply is not JSON")?;
    value["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_string)
        .context("the router's reply has no message content")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_calls_itself_on_loopback() {
        assert_eq!(self_url("0.0.0.0:8080"), "http://127.0.0.1:8080");
        assert_eq!(self_url("[::]:8082"), "http://127.0.0.1:8082");
    }

    #[test]
    fn reads_the_first_choice() {
        let body = r#"{"choices":[{"message":{"content":"hi"}}]}"#;
        assert_eq!(reply_text(body).unwrap(), "hi");
        assert!(reply_text("{}").is_err());
    }
}
