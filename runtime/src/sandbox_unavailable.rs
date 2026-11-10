//! A [`SandboxProvider`] standing in for a backend this build does not link.
//!
//! Used by a binary that was asked for a provider it cannot supply (e.g. the OSS binary
//! configured with `pod`). Selecting it instead of refusing to compile or boot means a
//! misconfiguration fails loudly, with the provider's name, at the first sandbox use rather
//! than at build time.

use async_trait::async_trait;

use crate::{
    Result, RuntimeError, SandboxHandle, SandboxInfo, SandboxProvider, SandboxProviderKind,
    SandboxSpec, SandboxState, SandboxStats, SandboxTransport,
};

/// Reports the configured provider's kind and refuses every operation.
pub struct UnavailableSandboxProvider {
    configured: SandboxProviderKind,
}

impl UnavailableSandboxProvider {
    /// `configured` is the provider the operator asked for.
    pub fn new(configured: SandboxProviderKind) -> Self {
        Self { configured }
    }

    fn unsupported<T>(&self) -> Result<T> {
        Err(RuntimeError::Unsupported(format!(
            "sandbox provider '{}' is not available in this build",
            self.configured
        )))
    }
}

#[async_trait]
impl SandboxProvider for UnavailableSandboxProvider {
    fn kind(&self) -> SandboxProviderKind {
        self.configured
    }

    fn is_available(&self) -> bool {
        false
    }

    async fn acquire(&self, _spec: &SandboxSpec) -> Result<SandboxHandle> {
        self.unsupported()
    }

    async fn attach(&self, _handle: &SandboxHandle) -> Result<SandboxTransport> {
        self.unsupported()
    }

    async fn status(&self, _handle: &SandboxHandle) -> Result<SandboxState> {
        self.unsupported()
    }

    async fn suspend(&self, _handle: &SandboxHandle) -> Result<()> {
        self.unsupported()
    }

    async fn release(&self, _handle: &SandboxHandle) -> Result<()> {
        self.unsupported()
    }

    async fn resume_takes_new_env(&self, _handle: &SandboxHandle) -> Result<bool> {
        self.unsupported()
    }

    async fn stats(&self, _handle: &SandboxHandle) -> Result<SandboxStats> {
        self.unsupported()
    }

    async fn list(&self) -> Result<Vec<SandboxInfo>> {
        self.unsupported()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SandboxKey;

    fn handle() -> SandboxHandle {
        SandboxHandle {
            key: SandboxKey::Session {
                session_id: uuid::Uuid::nil(),
            },
            provider: SandboxProviderKind::Pod,
            backend_ref: "x".into(),
        }
    }

    #[tokio::test]
    async fn every_operation_is_unsupported_and_names_the_provider() {
        let p = UnavailableSandboxProvider::new(SandboxProviderKind::Pod);
        assert_eq!(p.kind(), SandboxProviderKind::Pod);
        let msg = |e: RuntimeError| match e {
            RuntimeError::Unsupported(m) => m,
            other => panic!("expected Unsupported, got {other:?}"),
        };
        let m = msg(p.suspend(&handle()).await.unwrap_err());
        assert_eq!(m, "sandbox provider 'pod' is not available in this build");
        assert!(matches!(
            p.stats(&handle()).await,
            Err(RuntimeError::Unsupported(_))
        ));
        assert!(matches!(p.list().await, Err(RuntimeError::Unsupported(_))));
        assert!(matches!(
            p.resume_takes_new_env(&handle()).await,
            Err(RuntimeError::Unsupported(_))
        ));
        assert!(matches!(
            p.attach(&handle()).await,
            Err(RuntimeError::Unsupported(_))
        ));
    }
}
