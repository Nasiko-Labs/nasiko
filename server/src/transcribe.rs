//! `POST /transcribe`: speech to text for voice input.
//!
//! A user with a saved OpenAI key is billed on that key (direct Whisper call). Everyone
//! else goes through the in-process LLM router, which picks the platform provider
//! (OpenAI Whisper, or Gemini/Vertex with inline audio) and writes `trace_usage`.

use axum::http::StatusCode;
use axum::{
    Json, Router, extract::DefaultBodyLimit, extract::Multipart, extract::State, routing::post,
};
use serde::Serialize;

use nasiko_llm_router::platform::MAX_TRANSCRIBE_BYTES;
use nasiko_orchestrator::llm_backend::LlmCallContext;

use crate::state::AppState;

/// Usage label for transcription rows in `trace_usage`.
const COMPONENT: &str = nasiko_orchestrator::llm_backend::PlatformComponent::Transcription.as_str();
const DEFAULT_MIME: &str = "audio/webm";

#[derive(Serialize)]
pub struct TranscriptionResponse {
    pub text: String,
}

type ApiError = (StatusCode, String);

/// The uploaded audio and its content type.
#[derive(Debug)]
struct Upload {
    audio: Vec<u8>,
    filename: String,
    mime_type: String,
}

/// Content type to send upstream: the part's own type when it is audio, else `audio/webm`
/// (what browsers record; some clients send `application/octet-stream`).
fn upload_mime(content_type: Option<&str>) -> String {
    match content_type.map(str::trim) {
        Some(ct) if ct.starts_with("audio/") => ct.to_string(),
        _ => DEFAULT_MIME.to_string(),
    }
}

/// Rejects empty and oversized audio before any upstream call.
fn check_size(len: usize) -> Result<(), ApiError> {
    if len == 0 {
        return Err((StatusCode::BAD_REQUEST, "audio file is empty".into()));
    }
    if len > MAX_TRANSCRIBE_BYTES {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "audio is larger than {} MB",
                MAX_TRANSCRIBE_BYTES / (1024 * 1024)
            ),
        ));
    }
    Ok(())
}

async fn read_upload(mut multipart: Multipart) -> Result<Upload, ApiError> {
    while let Some(field) = multipart.next_field().await.map_err(|e| {
        tracing::warn!(%e, "invalid multipart body");
        (
            StatusCode::BAD_REQUEST,
            "invalid multipart body".to_string(),
        )
    })? {
        if field.name() != Some("file") {
            continue;
        }
        let filename: String = field.file_name().unwrap_or("audio.webm").to_string();
        let mime_type: String = upload_mime(field.content_type());
        let audio: Vec<u8> = field
            .bytes()
            .await
            .map_err(|e| {
                tracing::error!(%e, "failed to read uploaded audio file");
                (StatusCode::BAD_REQUEST, "failed to read file".to_string())
            })?
            .to_vec();
        check_size(audio.len())?;
        return Ok(Upload {
            audio,
            filename,
            mime_type,
        });
    }
    Err((StatusCode::BAD_REQUEST, "No audio file provided".into()))
}

async fn transcribe(
    State(state): State<AppState>,
    claims: crate::auth::Claims,
    multipart: Multipart,
) -> Result<Json<TranscriptionResponse>, ApiError> {
    let user_id = claims
        .user_uuid()
        .map_err(|(status, msg)| (status, msg.to_string()))?;
    let upload: Upload = read_upload(multipart).await?;

    let text: String =
        match nasiko_orchestrator::user_key::saved_openai_key(&state.db, user_id).await {
            Some(key) => whisper_with_user_key(&state, &key, upload).await?,
            None => {
                let call = LlmCallContext {
                    user_id,
                    component: COMPONENT.to_string(),
                    session_id: None,
                };
                state
                    .llm_backend
                    .transcribe(&call, upload.audio, &upload.mime_type)
                    .await
                    .map_err(|e| {
                        tracing::error!(error = %e, "routed transcription failed");
                        (StatusCode::BAD_GATEWAY, "transcription failed".to_string())
                    })?
            }
        };
    Ok(Json(TranscriptionResponse { text }))
}

/// Whisper on the user's own OpenAI key; not routed, so not in `trace_usage`.
async fn whisper_with_user_key(
    state: &AppState,
    api_key: &str,
    upload: Upload,
) -> Result<String, ApiError> {
    let base_url: &str = state
        .config
        .openai_base_url
        .as_deref()
        .unwrap_or("https://api.openai.com");
    let part = reqwest::multipart::Part::bytes(upload.audio)
        .file_name(upload.filename)
        .mime_str(&upload.mime_type)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid audio type: {e}")))?;
    let form = reqwest::multipart::Form::new()
        .text("model", "whisper-1")
        .text("response_format", "json")
        .part("file", part);
    // `OPENAI_BASE_URL` often already ends in `/v1`; strip it so we don't post to `/v1/v1`.
    let url: String = format!(
        "{}/v1/audio/transcriptions",
        nasiko_config::openai_base_url_without_v1(base_url)
    );
    let res = state
        .http_client
        .post(&url)
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| {
            tracing::error!(%e, "transcription request failed");
            (
                StatusCode::BAD_GATEWAY,
                "transcription request failed".to_string(),
            )
        })?;
    if !res.status().is_success() {
        let status = res.status();
        let body: String = res.text().await.unwrap_or_default();
        tracing::error!(%status, %body, "transcription API returned an error");
        let message = if matches!(status.as_u16(), 401 | 403) {
            "OpenAI rejected the API key used for transcription."
        } else {
            "transcription API error"
        };
        return Err((axum::http::StatusCode::BAD_GATEWAY, message.to_string()));
    }
    let result: serde_json::Value = res.json().await.map_err(|e| {
        tracing::error!(%e, "invalid transcription response");
        (
            StatusCode::BAD_GATEWAY,
            "invalid transcription response".to_string(),
        )
    })?;
    Ok(result
        .get("text")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string())
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/transcribe", post(transcribe))
        // Room for the audio cap plus multipart framing.
        .layer(DefaultBodyLimit::max(MAX_TRANSCRIBE_BYTES + 64 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_mime_keeps_audio_types() {
        assert_eq!(upload_mime(Some("audio/ogg")), "audio/ogg");
        assert_eq!(
            upload_mime(Some("audio/webm;codecs=opus")),
            "audio/webm;codecs=opus"
        );
    }

    #[test]
    fn upload_mime_defaults_non_audio_to_webm() {
        assert_eq!(upload_mime(None), "audio/webm");
        assert_eq!(upload_mime(Some("application/octet-stream")), "audio/webm");
    }

    #[test]
    fn size_limits() {
        assert_eq!(check_size(0).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(
            check_size(MAX_TRANSCRIBE_BYTES + 1).unwrap_err().0,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert!(check_size(1024).is_ok());
    }
}
