//! Where else a turn's files can come from. Open source has no such source; an
//! edition that does installs one at startup. Inert by default: no rows, no
//! chips, no download schemes beyond the ones this crate already owns.
//!
//! Generic on purpose — "files a turn produced elsewhere" and "serve a download
//! whose storage scheme I do not own" — so nothing edition-specific leaks into
//! the open-source crate. Installed through a swappable cell for exactly the
//! reason [`crate::prompt_context::SwappablePromptContext`] exists: a plain
//! `Arc<dyn Trait>` field reassignment never reaches an `AppState` clone a
//! background task captured before the composition root ran.

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use sqlx::PgPool;
use uuid::Uuid;

pub use super::models::ChatMessageFile;

/// A source of files a completed turn produced outside this crate's own chat
/// store — and the way to serve them back on download.
#[async_trait]
pub trait TurnArtifactSource: Send + Sync {
    /// Chip parts for files this turn produced elsewhere. The source inserts its
    /// own `chat_message_files` rows (with a `storage_uri` scheme it owns, so
    /// [`Self::resolve_download`] can route them back to it) and returns the
    /// parts the reply should carry. An empty vector means there is nothing to
    /// add, and the caller then falls back to its own capture path unchanged.
    async fn capture(
        &self,
        db: &PgPool,
        session_id: &str,
        message_id: Uuid,
    ) -> Vec<serde_json::Value>;

    /// Serve a `chat_message_files` row whose `storage_uri` scheme this source
    /// owns. `None` means "not mine": the caller continues with its own schemes.
    async fn resolve_download(
        &self,
        db: &PgPool,
        requester: Uuid,
        file: &ChatMessageFile,
    ) -> Option<DownloadTarget>;
}

/// How a download whose storage scheme the chat crate does not own should be
/// served to the user's browser.
pub enum DownloadTarget {
    /// Redirect the browser to a short-lived URL (e.g. a presigned object URL).
    Redirect(String),
    /// Stream the bytes through the control plane, with the headers to set.
    Stream {
        /// The body, already mapped to I/O errors for `Body::from_stream`.
        body: BoxStream<'static, std::io::Result<Bytes>>,
        /// `Content-Type`.
        mime: String,
        /// `Content-Length`, when the exact stored size is known.
        size: i64,
        /// The full `Content-Disposition` header value.
        content_disposition: String,
    },
    /// The file existed but is gone — the caller answers `410 Gone`.
    Gone,
    /// The requester may not have it. The caller renders this as `404` so a
    /// denied read is never an existence oracle.
    Denied,
}

/// The open-source default: no turn-artifact source is wired up, so capture adds
/// nothing and no download scheme is claimed.
pub struct NoTurnArtifacts;

#[async_trait]
impl TurnArtifactSource for NoTurnArtifacts {
    async fn capture(
        &self,
        _db: &PgPool,
        _session_id: &str,
        _message_id: Uuid,
    ) -> Vec<serde_json::Value> {
        Vec::new()
    }

    async fn resolve_download(
        &self,
        _db: &PgPool,
        _requester: Uuid,
        _file: &ChatMessageFile,
    ) -> Option<DownloadTarget> {
        None
    }
}

/// Holds a [`TurnArtifactSource`] behind a lock so the implementation can be
/// swapped after `AppState` has already been cloned — needed for exactly the
/// reason [`crate::prompt_context::SwappablePromptContext`] exists (see its doc
/// comment): the composition root installs the real implementation after
/// background tasks have already captured earlier `AppState` clones, and a plain
/// field reassignment would never reach them. Installing *through* this shared
/// cell makes the swap visible to every existing and future clone.
pub struct SwappableTurnArtifactSource(RwLock<Arc<dyn TurnArtifactSource>>);

impl SwappableTurnArtifactSource {
    /// Creates the cell holding `initial`.
    pub fn new(initial: Arc<dyn TurnArtifactSource>) -> Self {
        Self(RwLock::new(initial))
    }

    /// Replaces the underlying implementation for every clone, past and future,
    /// of the `AppState` this cell lives in.
    pub fn install(&self, new: Arc<dyn TurnArtifactSource>) {
        *self.0.write().expect("turn artifact source lock poisoned") = new;
    }

    /// The currently-installed implementation.
    pub fn get(&self) -> Arc<dyn TurnArtifactSource> {
        self.0
            .read()
            .expect("turn artifact source lock poisoned")
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct OneChip;

    #[async_trait]
    impl TurnArtifactSource for OneChip {
        async fn capture(
            &self,
            _db: &PgPool,
            _session_id: &str,
            _message_id: Uuid,
        ) -> Vec<serde_json::Value> {
            vec![serde_json::json!({"id": "x", "name": "x.txt"})]
        }

        async fn resolve_download(
            &self,
            _db: &PgPool,
            _requester: Uuid,
            _file: &ChatMessageFile,
        ) -> Option<DownloadTarget> {
            Some(DownloadTarget::Denied)
        }
    }

    /// Reproduces the swap-visibility contract the cell exists for: a clone taken
    /// before `install` still observes the installed implementation, because
    /// `AppState::clone()` only clones the `Arc` pointer to this one cell.
    #[test]
    fn install_is_visible_to_a_clone_taken_before_the_install_call() {
        let cell = Arc::new(SwappableTurnArtifactSource::new(Arc::new(NoTurnArtifacts)));
        let pre_install_clone = cell.clone();

        cell.install(Arc::new(OneChip));

        // The earlier clone — not `cell` itself — must see the installed source.
        let got = tokio_test_block_on(async {
            pre_install_clone
                .get()
                .capture(&never_pool(), "s", Uuid::nil())
                .await
        });
        assert_eq!(got.len(), 1, "a clone taken before install() sees the swap");
    }

    /// A `PgPool` is never touched by the test sources above, so a lazily-created
    /// (never-connected) pool is enough to satisfy the signatures.
    fn never_pool() -> PgPool {
        PgPool::connect_lazy("postgres://unused").expect("lazy pool")
    }

    fn tokio_test_block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime")
            .block_on(f)
    }
}
