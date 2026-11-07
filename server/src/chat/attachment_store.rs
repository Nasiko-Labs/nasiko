use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use bytes::Bytes;
use nasiko_llm_router::attachments::{AttachmentFetchError, AttachmentRef, AttachmentStore};
use nasiko_runtime::{BlobStore, BlobStoreError};
use uuid::Uuid;

pub(crate) fn chat_file_key(session_id: &str, file_id: Uuid) -> String {
    format!("chat-files/{session_id}/{file_id}")
}

pub struct ChatUploadStore {
    blobs: Arc<dyn BlobStore>,
}

impl ChatUploadStore {
    pub fn new(blobs: Arc<dyn BlobStore>) -> Self {
        Self { blobs }
    }
}

#[async_trait]
impl AttachmentStore for ChatUploadStore {
    async fn fetch(&self, reference: &AttachmentRef) -> Result<Bytes, AttachmentFetchError> {
        let AttachmentRef::ChatUpload {
            session_id,
            file_id,
        } = reference
        else {
            return Err(AttachmentFetchError::Unsupported);
        };
        self.blobs
            .get_blob(&chat_file_key(session_id, *file_id))
            .await
            .map_err(|e| match e {
                BlobStoreError::NotFound(_) => AttachmentFetchError::NotFound,
                BlobStoreError::Backend(detail) => AttachmentFetchError::Storage(detail),
            })
    }
}

pub struct SwappableAttachmentStore(RwLock<Arc<dyn AttachmentStore>>);

impl SwappableAttachmentStore {
    pub fn new(initial: Arc<dyn AttachmentStore>) -> Self {
        Self(RwLock::new(initial))
    }

    pub fn install(&self, new: Arc<dyn AttachmentStore>) {
        *self.0.write().expect("attachment store lock poisoned") = new;
    }

    pub fn get(&self) -> Arc<dyn AttachmentStore> {
        self.0
            .read()
            .expect("attachment store lock poisoned")
            .clone()
    }
}

#[async_trait]
impl AttachmentStore for SwappableAttachmentStore {
    async fn fetch(&self, reference: &AttachmentRef) -> Result<Bytes, AttachmentFetchError> {
        let current = self.get();
        current.fetch(reference).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct Blobs {
        reply: Result<&'static [u8], BlobStoreError>,
        asked: Mutex<Vec<String>>,
    }

    impl Blobs {
        fn replying(reply: Result<&'static [u8], BlobStoreError>) -> Arc<Self> {
            Arc::new(Self {
                reply,
                asked: Mutex::new(Vec::new()),
            })
        }
    }

    #[async_trait]
    impl BlobStore for Blobs {
        async fn put_blob(&self, _: &str, _: Bytes) -> Result<i64, BlobStoreError> {
            unreachable!()
        }
        async fn get_blob(&self, digest: &str) -> Result<Bytes, BlobStoreError> {
            self.asked.lock().unwrap().push(digest.to_string());
            match &self.reply {
                Ok(bytes) => Ok(Bytes::from_static(bytes)),
                Err(BlobStoreError::NotFound(key)) => Err(BlobStoreError::NotFound(key.clone())),
                Err(BlobStoreError::Backend(detail)) => {
                    Err(BlobStoreError::Backend(detail.clone()))
                }
            }
        }
        async fn delete_blob(&self, _: &str) -> Result<(), BlobStoreError> {
            unreachable!()
        }
        async fn blob_exists(&self, _: &str) -> bool {
            unreachable!()
        }
        async fn blob_size(&self, _: &str) -> Result<i64, BlobStoreError> {
            unreachable!()
        }
        async fn presigned_get_url(&self, _: &str, _: u64) -> Result<String, BlobStoreError> {
            unreachable!()
        }
        async fn ensure_bucket(&self, _: bool) -> anyhow::Result<()> {
            unreachable!()
        }
    }

    fn chat_ref() -> AttachmentRef {
        AttachmentRef::ChatUpload {
            session_id: "sess-1".into(),
            file_id: Uuid::from_u128(7),
        }
    }

    #[tokio::test]
    async fn a_chat_upload_is_read_from_its_upload_key() {
        let blobs = Blobs::replying(Ok(b"bytes"));
        let store = ChatUploadStore::new(blobs.clone());
        assert_eq!(&store.fetch(&chat_ref()).await.unwrap()[..], b"bytes");
        assert_eq!(
            *blobs.asked.lock().unwrap(),
            vec![format!("chat-files/sess-1/{}", Uuid::from_u128(7))]
        );
    }

    #[tokio::test]
    async fn a_missing_object_is_not_found_and_a_backend_fault_is_storage() {
        let missing =
            ChatUploadStore::new(Blobs::replying(Err(BlobStoreError::NotFound("k".into()))));
        assert!(matches!(
            missing.fetch(&chat_ref()).await,
            Err(AttachmentFetchError::NotFound)
        ));
        let down =
            ChatUploadStore::new(Blobs::replying(Err(BlobStoreError::Backend("down".into()))));
        assert!(matches!(
            down.fetch(&chat_ref()).await,
            Err(AttachmentFetchError::Storage(detail)) if detail == "down"
        ));
    }

    #[tokio::test]
    async fn a_workspace_reference_is_not_served_by_the_chat_store() {
        let blobs = Blobs::replying(Ok(b"bytes"));
        let store = ChatUploadStore::new(blobs.clone());
        let workspace = AttachmentRef::Workspace {
            agent_id: Uuid::from_u128(1),
            user_id: Uuid::from_u128(2),
            blob_id: Uuid::from_u128(3),
        };
        assert!(matches!(
            store.fetch(&workspace).await,
            Err(AttachmentFetchError::Unsupported)
        ));
        assert!(blobs.asked.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_installed_store_serves_every_existing_holder() {
        let swappable = Arc::new(SwappableAttachmentStore::new(Arc::new(
            nasiko_llm_router::attachments::NoAttachmentStore,
        )));
        let holder: Arc<dyn AttachmentStore> = swappable.clone();
        assert!(matches!(
            holder.fetch(&chat_ref()).await,
            Err(AttachmentFetchError::Unsupported)
        ));
        swappable.install(Arc::new(ChatUploadStore::new(Blobs::replying(Ok(b"x")))));
        assert_eq!(&holder.fetch(&chat_ref()).await.unwrap()[..], b"x");
    }
}
