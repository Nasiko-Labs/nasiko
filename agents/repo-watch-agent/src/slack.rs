//! Slack file-upload client — posts a markdown digest as a real uploaded file (not a chat
//! message), since Slack's `mrkdwn` can't render the digest's headers/tables. Implements
//! Slack's current 3-step upload flow (the old single-call `files.upload` was retired in 2025).
//!
//! Slack always answers HTTP 200; success/failure lives in the JSON body's `"ok"` field — a
//! different contract from GitHub's status-code errors in [`crate::github`], so this client
//! checks that field itself rather than sharing GitHub's `ensure_success`.

const SLACK_API: &str = "https://slack.com/api";

// ─── Public operation ─────────────────────────────────────────────────────────

/// Uploads `markdown` as a `.md` file to the configured Slack channel, with `comment` as the
/// post's accompanying message. Reads `SLACK_BOT_TOKEN` and `SLACK_CHANNEL_ID` at call time.
pub(crate) async fn post_markdown_file(
    markdown: &str,
    filename: &str,
    comment: &str,
) -> Result<(), String> {
    let token = slack_token()?;
    let channel_id = channel_id()?;

    let upload = get_upload_url(&token, filename, markdown.len()).await?;
    upload_bytes(&upload.upload_url, markdown).await?;
    complete_upload(&token, &channel_id, &upload.file_id, filename, comment).await
}

// ─── The 3-step flow ──────────────────────────────────────────────────────────

struct UploadUrl {
    upload_url: String,
    file_id: String,
}

async fn get_upload_url(token: &str, filename: &str, length: usize) -> Result<UploadUrl, String> {
    let length = length.to_string();
    // POST with form-encoded params — Slack requires POST here, not GET.
    let response = client()
        .post(format!("{SLACK_API}/files.getUploadURLExternal"))
        .bearer_auth(token)
        .form(&[("filename", filename), ("length", length.as_str())])
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let body = ensure_ok(response, "files.getUploadURLExternal").await?;

    let upload_url = body["upload_url"]
        .as_str()
        .ok_or("missing upload_url in response")?
        .to_string();
    let file_id = body["file_id"]
        .as_str()
        .ok_or("missing file_id in response")?
        .to_string();
    Ok(UploadUrl {
        upload_url,
        file_id,
    })
}

/// Step 2: POST the file's raw bytes to the pre-signed `upload_url`. No auth token here — the
/// URL itself is the credential — and a raw binary body, per Slack's documented contract.
async fn upload_bytes(upload_url: &str, content: &str) -> Result<(), String> {
    let response = client()
        .post(upload_url)
        .header("Content-Type", "application/octet-stream")
        .body(content.as_bytes().to_vec())
        .send()
        .await
        .map_err(|e| format!("file upload request failed: {e}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("file upload {status}: {body}"));
    }
    Ok(())
}

async fn complete_upload(
    token: &str,
    channel_id: &str,
    file_id: &str,
    title: &str,
    comment: &str,
) -> Result<(), String> {
    let response = client()
        .post(format!("{SLACK_API}/files.completeUploadExternal"))
        .bearer_auth(token)
        .json(&serde_json::json!({
            "files": [{"id": file_id, "title": title}],
            "channel_id": channel_id,
            "initial_comment": comment,
        }))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    ensure_ok(response, "files.completeUploadExternal").await?;
    Ok(())
}

// ─── HTTP plumbing ────────────────────────────────────────────────────────────

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("nasiko-repo-watch-agent")
        .build()
        .expect("failed to build reqwest client")
}

fn slack_token() -> Result<String, String> {
    std::env::var("SLACK_BOT_TOKEN").map_err(|_| "SLACK_BOT_TOKEN is not set".to_string())
}

fn channel_id() -> Result<String, String> {
    std::env::var("SLACK_CHANNEL_ID").map_err(|_| "SLACK_CHANNEL_ID is not set".to_string())
}

/// Checks the response status, then Slack's `"ok"` field in the parsed body — Slack answers
/// HTTP 200 even for a logical failure, so `ok: false` is the real error signal, not the status
/// code.
async fn ensure_ok(response: reqwest::Response, step: &str) -> Result<serde_json::Value, String> {
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Slack {step} {status}: {body}"));
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("{step} parse failed: {e}"))?;
    if body["ok"].as_bool() != Some(true) {
        let error = body["error"].as_str().unwrap_or("unknown");
        return Err(format!("Slack {step} failed: {error}"));
    }
    Ok(body)
}
