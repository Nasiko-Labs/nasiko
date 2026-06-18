use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::post,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::Claims;
use crate::router::providers::LLMProvider;
use crate::state::AppState;

use super::generator::{CapabilityGenerator, GeneratedCard, GeneratorError};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/capabilities/generate", post(generate))
        .route("/capabilities/apply/{agent_id}", post(generate_and_apply))
}

#[derive(Debug, Deserialize)]
struct GenerateRequest {
    source_code: String,
    agent_name: Option<String>,
}

#[derive(Debug, Serialize)]
struct GenerateResponse {
    card: GeneratedCard,
    tokens_used: i32,
    latency_ms: i32,
}

async fn generate(
    State(state): State<AppState>,
    _claims: Claims,
    Json(body): Json<GenerateRequest>,
) -> impl IntoResponse {
    let generator = make_generator(&state);
    let agent_name = body.agent_name.as_deref().unwrap_or("unnamed-agent");

    match generator.generate(&body.source_code, agent_name).await {
        Ok((card, result)) => (
            StatusCode::OK,
            Json(GenerateResponse {
                card,
                tokens_used: result.usage.total_tokens,
                latency_ms: result.latency_ms,
            }),
        )
            .into_response(),
        Err(e) => error_response(e),
    }
}

#[derive(Debug, Deserialize)]
struct ApplyRequest {
    source_code: Option<String>,
    build_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
struct ApplyResponse {
    card: GeneratedCard,
    applied: bool,
    agent_id: Uuid,
}

async fn generate_and_apply(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
    Json(body): Json<ApplyRequest>,
) -> impl IntoResponse {
    let owner_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid user id").into_response(),
    };

    // Fetch agent name and verify ownership
    let agent_name: Option<String> =
        sqlx::query_scalar("SELECT name FROM agents WHERE id = $1 AND owner_id = $2")
            .bind(agent_id)
            .bind(owner_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();

    let agent_name = match agent_name {
        Some(n) => n,
        None => {
            return (StatusCode::NOT_FOUND, "agent not found or not owned by you").into_response()
        }
    };

    // Get source code: from request body, or from S3 via build_id
    let source_code = match resolve_source(&state, &body, agent_id).await {
        Ok(src) => src,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };

    let generator = make_generator(&state);

    let (card, _result) = match generator.generate(&source_code, &agent_name).await {
        Ok(r) => r,
        Err(e) => return error_response(e),
    };

    // Apply to database
    let skills_json = serde_json::to_value(&card.skills).unwrap_or_default();
    let caps_json = serde_json::to_value(&card.capabilities).unwrap_or_default();
    let input_modes = serde_json::to_value(&card.default_input_modes).unwrap_or_default();
    let output_modes = serde_json::to_value(&card.default_output_modes).unwrap_or_default();

    let applied = sqlx::query(
        r#"UPDATE agents
           SET description = $2,
               skills = $3,
               tags = $4,
               capabilities = $5,
               default_input_modes = $6,
               default_output_modes = $7,
               updated_at = now()
           WHERE id = $1"#,
    )
    .bind(agent_id)
    .bind(&card.description)
    .bind(&skills_json)
    .bind(&card.tags)
    .bind(&caps_json)
    .bind(&input_modes)
    .bind(&output_modes)
    .execute(&state.db)
    .await
    .is_ok();

    Json(ApplyResponse {
        card,
        applied,
        agent_id,
    })
    .into_response()
}

async fn resolve_source(
    state: &AppState,
    body: &ApplyRequest,
    agent_id: Uuid,
) -> Result<String, String> {
    if let Some(ref src) = body.source_code {
        return Ok(src.clone());
    }

    if let Some(build_id) = body.build_id {
        let version_tag: Option<String> = sqlx::query_scalar::<_, String>(
            "SELECT version_tag FROM agent_builds WHERE id = $1 AND agent_id = $2",
        )
        .bind(build_id)
        .bind(agent_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| e.to_string())?;

        let version_tag =
            version_tag.ok_or_else(|| "build not found for this agent".to_string())?;

        let key = format!("sources/{agent_id}/{version_tag}.zip");
        let data = state
            .oci_storage
            .get_blob(&key)
            .await
            .map_err(|e| format!("failed to fetch source from S3: {e}"))?;

        let text = extract_text_from_zip(&data)
            .map_err(|e| format!("failed to read source ZIP: {e}"))?;
        return Ok(text);
    }

    Err("either source_code or build_id is required".into())
}

fn extract_text_from_zip(data: &[u8]) -> Result<String, zip::result::ZipError> {
    use std::io::Read;

    let cursor = std::io::Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor)?;
    let mut combined = String::new();

    let code_extensions = [
        "py", "rs", "ts", "js", "go", "java", "rb", "ex", "exs", "toml", "yaml", "yml", "json",
        "md", "txt", "dockerfile", "sh",
    ];

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        if file.is_dir() {
            continue;
        }

        let name = file.name().to_string();
        let ext = name
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_lowercase();

        if !code_extensions.contains(&ext.as_str())
            && !name.to_lowercase().contains("dockerfile")
        {
            continue;
        }

        // Skip large files (>50KB)
        if file.size() > 50_000 {
            combined.push_str(&format!("\n--- {name} (skipped, too large) ---\n"));
            continue;
        }

        let mut contents = String::new();
        if file.read_to_string(&mut contents).is_ok() {
            combined.push_str(&format!("\n--- {name} ---\n"));
            combined.push_str(&contents);
        }
    }

    Ok(combined)
}

fn make_generator(state: &AppState) -> CapabilityGenerator {
    let provider = LLMProvider::from_env(state.http_client.clone());
    let model = std::env::var("CAPABILITY_GENERATOR_MODEL")
        .unwrap_or_else(|_| "deepseek-v4-flash".into());
    CapabilityGenerator::new(provider, model)
}

fn error_response(e: GeneratorError) -> axum::response::Response {
    let status = match &e {
        GeneratorError::Provider(_) => StatusCode::BAD_GATEWAY,
        GeneratorError::ParseError(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, e.to_string()).into_response()
}
