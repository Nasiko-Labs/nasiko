use axum::{Json, Router, extract::Multipart, extract::State, routing::post};
use serde::Serialize;

use crate::state::AppState;

#[derive(Serialize)]
pub struct TranscriptionResponse {
    pub text: String,
}

async fn transcribe(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<TranscriptionResponse>, (axum::http::StatusCode, String)> {
    let api_key = state.config.openai_api_key.as_deref().ok_or((
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "Transcription not configured".into(),
    ))?;

    let base_url = state
        .config
        .openai_base_url
        .as_deref()
        .unwrap_or("https://api.openai.com");

    let mut audio_data: Option<Vec<u8>> = None;
    let mut filename = "audio.webm".to_string();

    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            if let Some(name) = field.file_name() {
                filename = name.to_string();
            }
            audio_data = Some(
                field
                    .bytes()
                    .await
                    .map_err(|e| {
                        tracing::error!(%e, "failed to read uploaded audio file");
                        (
                            axum::http::StatusCode::BAD_REQUEST,
                            "failed to read file".to_string(),
                        )
                    })?
                    .to_vec(),
            );
        }
    }

    let audio = audio_data.ok_or((
        axum::http::StatusCode::BAD_REQUEST,
        "No audio file provided".into(),
    ))?;

    let part = reqwest::multipart::Part::bytes(audio)
        .file_name(filename)
        .mime_str("audio/webm")
        .unwrap();

    let form = reqwest::multipart::Form::new()
        .text("model", "whisper-1")
        .text("response_format", "json")
        .part("file", part);

    // `OPENAI_BASE_URL` is commonly written with the `/v1` already on it (that
    // is how cp.nasiko.dev has it), so strip it before appending our own —
    // otherwise this posts to `.../v1/v1/audio/transcriptions` and 404s.
    let url = format!(
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
                axum::http::StatusCode::BAD_GATEWAY,
                "transcription request failed".to_string(),
            )
        })?;

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        tracing::error!(%status, %body, "transcription API returned an error");
        return Err((
            axum::http::StatusCode::BAD_GATEWAY,
            "transcription API error".to_string(),
        ));
    }

    let result: serde_json::Value = res.json().await.map_err(|e| {
        tracing::error!(%e, "invalid transcription response");
        (
            axum::http::StatusCode::BAD_GATEWAY,
            "invalid transcription response".to_string(),
        )
    })?;

    let text = result
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok(Json(TranscriptionResponse { text }))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/transcribe", post(transcribe))
}
