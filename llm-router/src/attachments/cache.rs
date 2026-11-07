use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use super::marker::AttachmentRef;
use super::store::{AttachmentFetchError, AttachmentStore};

pub const ATTACHMENT_CACHE_TTL: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
pub struct CachedAttachment {
    pub base64: Arc<str>,
    pub from_cache: bool,
}

pub struct AttachmentCache {
    max_bytes: usize,
    ttl: Duration,
    inner: Mutex<CacheInner>,
}

#[derive(Default)]
struct CacheInner {
    entries: HashMap<AttachmentRef, Entry>,
    bytes: usize,
    next_seq: u64,
}

struct Entry {
    base64: Arc<str>,
    stored_at: Instant,
    seq: u64,
}

impl AttachmentCache {
    pub fn new(max_bytes: usize, ttl: Duration) -> Self {
        Self {
            max_bytes,
            ttl,
            inner: Mutex::new(CacheInner::default()),
        }
    }

    pub async fn get_or_fetch(
        &self,
        reference: &AttachmentRef,
        store: &dyn AttachmentStore,
    ) -> Result<CachedAttachment, AttachmentFetchError> {
        if let Some(base64) = self.get(reference) {
            return Ok(CachedAttachment {
                base64,
                from_cache: true,
            });
        }
        let bytes = store.fetch(reference).await?;
        let base64: Arc<str> = STANDARD.encode(&bytes).into();
        self.put(reference.clone(), base64.clone());
        Ok(CachedAttachment {
            base64,
            from_cache: false,
        })
    }

    fn get(&self, reference: &AttachmentRef) -> Option<Arc<str>> {
        let mut inner = self.lock();
        let found = inner
            .entries
            .get(reference)
            .map(|entry| (entry.base64.clone(), entry.stored_at.elapsed() < self.ttl));
        match found {
            Some((base64, true)) => Some(base64),
            Some((_, false)) => {
                inner.remove(reference);
                None
            }
            None => None,
        }
    }

    fn put(&self, reference: AttachmentRef, base64: Arc<str>) {
        let size = base64.len();
        if size > self.max_bytes {
            return;
        }
        let mut inner = self.lock();
        inner.remove(&reference);
        inner.drop_expired(self.ttl);
        while inner.bytes + size > self.max_bytes && inner.drop_oldest() {}
        inner.bytes += size;
        let seq = inner.next_seq;
        inner.next_seq += 1;
        inner.entries.insert(
            reference,
            Entry {
                base64,
                stored_at: Instant::now(),
                seq,
            },
        );
    }

    fn lock(&self) -> MutexGuard<'_, CacheInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl CacheInner {
    fn remove(&mut self, reference: &AttachmentRef) {
        if let Some(entry) = self.entries.remove(reference) {
            self.bytes -= entry.base64.len();
        }
    }

    fn drop_expired(&mut self, ttl: Duration) {
        let bytes = &mut self.bytes;
        self.entries.retain(|_, entry| {
            let fresh = entry.stored_at.elapsed() < ttl;
            if !fresh {
                *bytes -= entry.base64.len();
            }
            fresh
        });
    }

    fn drop_oldest(&mut self) -> bool {
        let oldest = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.seq)
            .map(|(reference, _)| reference.clone());
        match oldest {
            Some(reference) => {
                self.remove(&reference);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attachments::store::NoAttachmentStore;
    use async_trait::async_trait;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use bytes::Bytes;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use uuid::Uuid;

    struct CountingStore {
        calls: AtomicUsize,
        bytes: Option<Bytes>,
    }

    impl CountingStore {
        fn serving(bytes: &'static [u8]) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                bytes: Some(Bytes::from_static(bytes)),
            }
        }

        fn failing() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                bytes: None,
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl AttachmentStore for CountingStore {
        async fn fetch(&self, _reference: &AttachmentRef) -> Result<Bytes, AttachmentFetchError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.bytes.clone().ok_or(AttachmentFetchError::NotFound)
        }
    }

    fn file(n: u128) -> AttachmentRef {
        AttachmentRef::ChatUpload {
            session_id: "sess".into(),
            file_id: Uuid::from_u128(n),
        }
    }

    const SIX_BYTES: &[u8] = b"abcdef";
    const SIX_BYTES_B64_LEN: usize = 8;

    #[tokio::test]
    async fn a_second_read_of_the_same_file_comes_from_the_cache() {
        let cache = AttachmentCache::new(1024, ATTACHMENT_CACHE_TTL);
        let store = CountingStore::serving(SIX_BYTES);
        let first = cache.get_or_fetch(&file(1), &store).await.unwrap();
        let second = cache.get_or_fetch(&file(1), &store).await.unwrap();
        assert_eq!(&*first.base64, STANDARD.encode(SIX_BYTES));
        assert!(!first.from_cache);
        assert!(second.from_cache);
        assert_eq!(second.base64, first.base64);
        assert_eq!(store.calls(), 1);
    }

    #[tokio::test]
    async fn different_files_are_fetched_separately() {
        let cache = AttachmentCache::new(1024, ATTACHMENT_CACHE_TTL);
        let store = CountingStore::serving(SIX_BYTES);
        cache.get_or_fetch(&file(1), &store).await.unwrap();
        cache.get_or_fetch(&file(2), &store).await.unwrap();
        assert_eq!(store.calls(), 2);
    }

    #[tokio::test]
    async fn an_expired_entry_is_fetched_again() {
        let cache = AttachmentCache::new(1024, Duration::ZERO);
        let store = CountingStore::serving(SIX_BYTES);
        cache.get_or_fetch(&file(1), &store).await.unwrap();
        let again = cache.get_or_fetch(&file(1), &store).await.unwrap();
        assert!(!again.from_cache);
        assert_eq!(store.calls(), 2);
        assert_eq!(cache.lock().bytes, SIX_BYTES_B64_LEN);
        assert_eq!(cache.lock().entries.len(), 1);
    }

    #[tokio::test]
    async fn a_full_cache_drops_the_oldest_file_first() {
        let cache = AttachmentCache::new(2 * SIX_BYTES_B64_LEN, ATTACHMENT_CACHE_TTL);
        let store = CountingStore::serving(SIX_BYTES);
        for n in 1..=3 {
            cache.get_or_fetch(&file(n), &store).await.unwrap();
        }
        assert_eq!(store.calls(), 3);
        assert_eq!(cache.lock().bytes, 2 * SIX_BYTES_B64_LEN);
        assert!(
            cache
                .get_or_fetch(&file(3), &store)
                .await
                .unwrap()
                .from_cache
        );
        assert!(
            cache
                .get_or_fetch(&file(2), &store)
                .await
                .unwrap()
                .from_cache
        );
        assert_eq!(store.calls(), 3);
        assert!(
            !cache
                .get_or_fetch(&file(1), &store)
                .await
                .unwrap()
                .from_cache
        );
        assert_eq!(store.calls(), 4);
        assert_eq!(cache.lock().bytes, 2 * SIX_BYTES_B64_LEN);
    }

    #[tokio::test]
    async fn a_file_bigger_than_the_cache_is_served_but_not_kept() {
        let cache = AttachmentCache::new(SIX_BYTES_B64_LEN - 1, ATTACHMENT_CACHE_TTL);
        let store = CountingStore::serving(SIX_BYTES);
        let first = cache.get_or_fetch(&file(1), &store).await.unwrap();
        assert_eq!(&*first.base64, STANDARD.encode(SIX_BYTES));
        cache.get_or_fetch(&file(1), &store).await.unwrap();
        assert_eq!(store.calls(), 2);
        assert_eq!(cache.lock().bytes, 0);
    }

    #[tokio::test]
    async fn a_failed_fetch_is_not_cached() {
        let cache = AttachmentCache::new(1024, ATTACHMENT_CACHE_TTL);
        let store = CountingStore::failing();
        for _ in 0..2 {
            let err = cache.get_or_fetch(&file(1), &store).await.unwrap_err();
            assert!(matches!(err, AttachmentFetchError::NotFound));
        }
        assert_eq!(store.calls(), 2);
        assert_eq!(cache.lock().bytes, 0);
    }

    #[tokio::test]
    async fn without_a_store_every_file_is_unsupported() {
        let cache = AttachmentCache::new(1024, ATTACHMENT_CACHE_TTL);
        let err = cache
            .get_or_fetch(&file(1), &NoAttachmentStore)
            .await
            .unwrap_err();
        assert!(matches!(err, AttachmentFetchError::Unsupported));
    }

    #[tokio::test]
    async fn workspace_and_chat_files_with_the_same_ids_do_not_collide() {
        let cache = AttachmentCache::new(1024, ATTACHMENT_CACHE_TTL);
        let store = CountingStore::serving(SIX_BYTES);
        let workspace = AttachmentRef::Workspace {
            agent_id: Uuid::from_u128(1),
            user_id: Uuid::from_u128(1),
            blob_id: Uuid::from_u128(1),
        };
        cache.get_or_fetch(&file(1), &store).await.unwrap();
        assert!(
            !cache
                .get_or_fetch(&workspace, &store)
                .await
                .unwrap()
                .from_cache
        );
        assert_eq!(store.calls(), 2);
    }
}
