use async_trait::async_trait;
use bytes::Bytes;

use super::marker::AttachmentRef;

#[derive(Debug, thiserror::Error)]
pub enum AttachmentFetchError {
    #[error("the attached file is no longer stored")]
    NotFound,
    #[error("this server cannot read that attached file")]
    Unsupported,
    #[error("attachment storage failed: {0}")]
    Storage(String),
}

#[async_trait]
pub trait AttachmentStore: Send + Sync {
    async fn fetch(&self, reference: &AttachmentRef) -> Result<Bytes, AttachmentFetchError>;
}

pub struct NoAttachmentStore;

#[async_trait]
impl AttachmentStore for NoAttachmentStore {
    async fn fetch(&self, _reference: &AttachmentRef) -> Result<Bytes, AttachmentFetchError> {
        Err(AttachmentFetchError::Unsupported)
    }
}
