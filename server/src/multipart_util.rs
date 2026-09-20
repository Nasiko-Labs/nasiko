//! Shared multipart-to-disk streaming, used by both the agent-upload path
//! (`agents::upload::upload_and_deploy`) and the MCP-server-upload path
//! (`mcp::handlers::upload::upload_zip`) — extracted so the size-capped,
//! stream-not-buffer logic exists exactly once.

use std::path::PathBuf;

use tokio::io::AsyncWriteExt;

/// What went wrong while streaming a multipart field to disk. Kept generic
/// (not tied to either caller's own error-response shape) — each caller maps
/// this to its own convention (a bare `impl IntoResponse` for agents, an
/// `ApiError`/`McpError` for MCP).
#[derive(Debug)]
pub enum StreamUploadError {
    /// Failed to create the temp directory or destination file.
    Io(String),
    /// The stream exceeded the caller-provided byte limit.
    TooLarge,
    /// The multipart stream itself errored while reading a chunk.
    ReadFailed(String),
}

/// Creates a fresh UUID-keyed temp directory — never named by caller-supplied
/// data (two concurrent uploads of the same name must never share one dir, or
/// one's cleanup can delete the other's still-in-use source) — and streams
/// `field`'s bytes into `file_name` inside it, enforcing `max_bytes` while
/// streaming, not after buffering the whole body in memory.
pub async fn stream_field_to_fresh_temp_file(
    dir_prefix: &str,
    file_name: &str,
    mut field: axum::extract::multipart::Field<'_>,
    max_bytes: u64,
) -> Result<PathBuf, StreamUploadError> {
    let upload_dir = std::env::temp_dir().join(format!("{dir_prefix}-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&upload_dir)
        .await
        .map_err(|e| StreamUploadError::Io(format!("create upload dir: {e}")))?;
    let path = upload_dir.join(file_name);

    let mut f = tokio::fs::File::create(&path)
        .await
        .map_err(|e| StreamUploadError::Io(format!("create destination file: {e}")))?;

    let mut total_bytes: u64 = 0;
    loop {
        match field.chunk().await {
            Ok(Some(chunk)) => {
                total_bytes += chunk.len() as u64;
                if total_bytes > max_bytes {
                    let _ = tokio::fs::remove_dir_all(&upload_dir).await;
                    return Err(StreamUploadError::TooLarge);
                }
                if let Err(e) = f.write_all(&chunk).await {
                    let _ = tokio::fs::remove_dir_all(&upload_dir).await;
                    return Err(StreamUploadError::Io(format!("write chunk: {e}")));
                }
            }
            Ok(None) => break,
            Err(e) => {
                let _ = tokio::fs::remove_dir_all(&upload_dir).await;
                return Err(StreamUploadError::ReadFailed(e.to_string()));
            }
        }
    }
    Ok(path)
}
