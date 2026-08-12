use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use thiserror::Error;

/// The Distribution Spec error codes a registry may emit, each paired with the
/// status the spec requires for it.
///
/// Registry clients and the OCI conformance suite branch on the code, not the
/// message, so precision matters: a missing manifest reported as `BLOB_UNKNOWN`
/// tells a client to re-upload layers, and a digest mismatch reported as
/// `MANIFEST_INVALID` tells it the bytes were malformed when only the reference
/// disagreed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OciCode {
    /// A requested blob is not in the registry.
    BlobUnknown,
    /// The upload session is unknown, expired, or already finalized.
    BlobUploadUnknown,
    /// The chunk does not continue from the session's current offset.
    BlobUploadInvalid,
    /// The digest the client supplied does not match the bytes it sent.
    DigestInvalid,
    /// A blob the pushed manifest references is absent, so the manifest would be
    /// unpullable. Distinct from `BlobUnknown`, which answers a request *for* a
    /// blob rather than rejecting a manifest push.
    ManifestBlobUnknown,
    /// The manifest is malformed or otherwise unacceptable.
    ManifestInvalid,
    /// No manifest matches the requested reference.
    ManifestUnknown,
    /// The repository does not exist.
    NameUnknown,
    /// Authentication succeeded but the caller may not do this.
    Denied,
    /// A chunk arrived out of order. Carries the same code as
    /// `BlobUploadInvalid` but the `416` the spec requires for a range problem
    /// specifically, which is how a client knows to re-read its offset and
    /// resume rather than restart the whole upload.
    RangeNotSatisfiable,
}

impl OciCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BlobUnknown => "BLOB_UNKNOWN",
            Self::BlobUploadUnknown => "BLOB_UPLOAD_UNKNOWN",
            Self::BlobUploadInvalid => "BLOB_UPLOAD_INVALID",
            Self::DigestInvalid => "DIGEST_INVALID",
            Self::ManifestBlobUnknown => "MANIFEST_BLOB_UNKNOWN",
            Self::ManifestInvalid => "MANIFEST_INVALID",
            Self::ManifestUnknown => "MANIFEST_UNKNOWN",
            Self::NameUnknown => "NAME_UNKNOWN",
            Self::Denied => "DENIED",
            Self::RangeNotSatisfiable => "BLOB_UPLOAD_INVALID",
        }
    }

    pub fn status(self) -> StatusCode {
        match self {
            Self::BlobUnknown
            | Self::BlobUploadUnknown
            | Self::ManifestBlobUnknown
            | Self::ManifestUnknown
            | Self::NameUnknown => StatusCode::NOT_FOUND,
            Self::BlobUploadInvalid | Self::DigestInvalid | Self::ManifestInvalid => {
                StatusCode::BAD_REQUEST
            }
            Self::Denied => StatusCode::FORBIDDEN,
            Self::RangeNotSatisfiable => StatusCode::RANGE_NOT_SATISFIABLE,
        }
    }
}

#[derive(Debug, Error)]
pub enum OciError {
    /// A failure carrying the exact spec code for the condition. Preferred over
    /// the coarse variants below on every `/v2` path.
    #[error("{}: {}", .0.as_str(), .1)]
    Oci(OciCode, String),

    /// Coarse not-found. Kept for non-`/v2` callers; on a `/v2` route it renders
    /// as `BLOB_UNKNOWN`, which is rarely the truthful code — prefer `Oci`.
    #[error("not found: {0}")]
    NotFound(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("forbidden: {0}")]
    Forbidden(String),
}

impl OciError {
    /// Convenience constructors for the codes used most.
    pub fn manifest_unknown(msg: impl Into<String>) -> Self {
        Self::Oci(OciCode::ManifestUnknown, msg.into())
    }
    pub fn blob_unknown(msg: impl Into<String>) -> Self {
        Self::Oci(OciCode::BlobUnknown, msg.into())
    }
    pub fn digest_invalid(msg: impl Into<String>) -> Self {
        Self::Oci(OciCode::DigestInvalid, msg.into())
    }
    pub fn manifest_invalid(msg: impl Into<String>) -> Self {
        Self::Oci(OciCode::ManifestInvalid, msg.into())
    }
    pub fn upload_unknown(msg: impl Into<String>) -> Self {
        Self::Oci(OciCode::BlobUploadUnknown, msg.into())
    }
    pub fn name_unknown(msg: impl Into<String>) -> Self {
        Self::Oci(OciCode::NameUnknown, msg.into())
    }

    /// The spec code this error renders as — so a host with its own error type
    /// can map an `OciError` across without re-deriving the classification.
    pub fn code(&self) -> OciCode {
        match self {
            Self::Oci(code, _) => *code,
            Self::NotFound(_) => OciCode::BlobUnknown,
            Self::BadRequest(_) => OciCode::ManifestInvalid,
            Self::Forbidden(_) => OciCode::Denied,
            // Server-side faults have no spec code; callers must not surface
            // these as a 4xx.
            Self::Database(_) | Self::Storage(_) => OciCode::ManifestInvalid,
        }
    }
}

impl IntoResponse for OciError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self {
            OciError::Oci(code, m) => (code.status(), code.as_str(), m.clone()),
            OciError::NotFound(m) => (
                StatusCode::NOT_FOUND,
                OciCode::BlobUnknown.as_str(),
                m.clone(),
            ),
            OciError::BadRequest(m) => (
                StatusCode::BAD_REQUEST,
                OciCode::ManifestInvalid.as_str(),
                m.clone(),
            ),
            OciError::Database(e) => {
                tracing::error!("oci registry db error: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "UNKNOWN",
                    "database error".into(),
                )
            }
            OciError::Storage(m) => {
                tracing::error!("oci registry storage error: {m}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "UNKNOWN",
                    "storage error".into(),
                )
            }
            OciError::Forbidden(m) => (StatusCode::FORBIDDEN, OciCode::Denied.as_str(), m.clone()),
        };
        (
            status,
            Json(json!({"errors": [{"code": code, "message": message}]})),
        )
            .into_response()
    }
}

pub type Result<T> = std::result::Result<T, OciError>;
