//! A [`SandboxProvider`] that splits traffic by key: session sandboxes to one backend, agent
//! sandboxes to another.

use std::sync::Arc;

use async_trait::async_trait;

use crate::{
    Result, SandboxHandle, SandboxInfo, SandboxKey, SandboxProvider, SandboxProviderKind,
    SandboxSpec, SandboxState, SandboxStats, SandboxTransport,
};

/// Sends session sandboxes to one provider and agent sandboxes to another.
///
/// New sandboxes (`acquire`) go by key: `Session` to the session side, `Agent` and
/// `AgentContext` to the agent side. Calls that take a handle go to the side whose `kind()`
/// equals `handle.provider`, because a side can hold sandboxes the other would not create (a
/// pod-era `Session` sandbox still listed by the Pod provider); only a handle neither side owns
/// falls back to its key. `list()` concatenates both sides; a side that is not available
/// contributes nothing, while an error from an available side fails the whole listing.
///
/// `kind()` reports the session side: that is the provider a `sandbox_ref` of a session row
/// names, and the wiring rejects a stored ref whose kind differs. `is_available()` is true when
/// either side is; use `is_available_for` for one key.
pub struct KeyRoutedSandboxProvider {
    sessions: Arc<dyn SandboxProvider>,
    agents: Arc<dyn SandboxProvider>,
}

impl KeyRoutedSandboxProvider {
    /// `sessions` serves `Session` keys, `agents` serves `Agent` and `AgentContext` keys.
    pub fn new(sessions: Arc<dyn SandboxProvider>, agents: Arc<dyn SandboxProvider>) -> Self {
        Self { sessions, agents }
    }

    fn by_key(&self, key: &SandboxKey) -> &Arc<dyn SandboxProvider> {
        match key {
            SandboxKey::Session { .. } => &self.sessions,
            SandboxKey::Agent { .. } | SandboxKey::AgentContext { .. } => &self.agents,
        }
    }

    fn by_handle(&self, handle: &SandboxHandle) -> &Arc<dyn SandboxProvider> {
        match (
            self.sessions.kind() == handle.provider,
            self.agents.kind() == handle.provider,
        ) {
            (true, false) => &self.sessions,
            (false, true) => &self.agents,
            // Both sides share the kind (ambiguous) or neither owns it: the key decides.
            (true, true) | (false, false) => self.by_key(&handle.key),
        }
    }
}

#[async_trait]
impl SandboxProvider for KeyRoutedSandboxProvider {
    fn kind(&self) -> SandboxProviderKind {
        self.sessions.kind()
    }

    fn is_available(&self) -> bool {
        self.sessions.is_available() || self.agents.is_available()
    }

    fn is_available_for(&self, key: &SandboxKey) -> bool {
        self.by_key(key).is_available_for(key)
    }

    async fn acquire(&self, spec: &SandboxSpec) -> Result<SandboxHandle> {
        self.by_key(&spec.key).acquire(spec).await
    }

    async fn attach(&self, handle: &SandboxHandle) -> Result<SandboxTransport> {
        self.by_handle(handle).attach(handle).await
    }

    async fn status(&self, handle: &SandboxHandle) -> Result<SandboxState> {
        self.by_handle(handle).status(handle).await
    }

    async fn suspend(&self, handle: &SandboxHandle) -> Result<()> {
        self.by_handle(handle).suspend(handle).await
    }

    async fn release(&self, handle: &SandboxHandle) -> Result<()> {
        self.by_handle(handle).release(handle).await
    }

    /// The owning side's answer, when it is also the side the next `acquire` of the key goes
    /// to. A sandbox the other side owns (a pod-era session sandbox), or one neither side owns,
    /// is never resumed: its key's next acquire creates a new sandbox on the key's side.
    async fn resume_takes_new_env(&self, handle: &SandboxHandle) -> Result<bool> {
        let owned =
            self.sessions.kind() == handle.provider || self.agents.kind() == handle.provider;
        let owner = self.by_handle(handle);
        if !owned || !Arc::ptr_eq(owner, self.by_key(&handle.key)) {
            return Ok(false);
        }
        owner.resume_takes_new_env(handle).await
    }

    async fn stats(&self, handle: &SandboxHandle) -> Result<SandboxStats> {
        self.by_handle(handle).stats(handle).await
    }

    async fn list(&self) -> Result<Vec<SandboxInfo>> {
        let mut all = Vec::new();
        for side in [&self.sessions, &self.agents] {
            if side.is_available() {
                all.extend(side.list().await?);
            }
        }
        Ok(all)
    }
}

#[cfg(all(test, feature = "simulated", feature = "testing"))]
mod tests {
    use super::*;
    use crate::conformance::spec_for;
    use crate::sandbox_fake::FakeSandboxProvider;
    use crate::{RuntimeError, TransportKind, UnavailableSandboxProvider};
    use uuid::Uuid;

    fn session_key() -> SandboxKey {
        SandboxKey::Session {
            session_id: Uuid::new_v4(),
        }
    }

    fn agent_key() -> SandboxKey {
        SandboxKey::Agent {
            agent_id: Uuid::new_v4(),
        }
    }

    fn pair() -> (
        Arc<FakeSandboxProvider>,
        Arc<FakeSandboxProvider>,
        KeyRoutedSandboxProvider,
    ) {
        let sessions = Arc::new(FakeSandboxProvider::default());
        let agents = Arc::new(FakeSandboxProvider::default());
        let routed = KeyRoutedSandboxProvider::new(sessions.clone(), agents.clone());
        (sessions, agents, routed)
    }

    #[tokio::test]
    async fn session_keys_go_to_the_session_side() {
        let (sessions, agents, routed) = pair();
        routed
            .acquire(&spec_for(session_key(), TransportKind::WebSocket))
            .await
            .unwrap();
        assert_eq!(sessions.list().await.unwrap().len(), 1);
        assert!(agents.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn agent_keys_go_to_the_agent_side() {
        let (sessions, agents, routed) = pair();
        routed
            .acquire(&spec_for(agent_key(), TransportKind::Http))
            .await
            .unwrap();
        assert!(sessions.list().await.unwrap().is_empty());
        assert_eq!(agents.list().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn agent_context_keys_go_to_the_agent_side() {
        let (sessions, agents, routed) = pair();
        let key = SandboxKey::AgentContext {
            agent_id: Uuid::new_v4(),
            context_id: "ctx".into(),
        };
        routed
            .acquire(&spec_for(key, TransportKind::Http))
            .await
            .unwrap();
        assert!(sessions.list().await.unwrap().is_empty());
        assert_eq!(agents.list().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn list_concatenates_both_sides() {
        let (_s, _a, routed) = pair();
        routed
            .acquire(&spec_for(session_key(), TransportKind::WebSocket))
            .await
            .unwrap();
        routed
            .acquire(&spec_for(agent_key(), TransportKind::Http))
            .await
            .unwrap();
        assert_eq!(routed.list().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn session_handle_calls_reach_the_session_side_only() {
        let (sessions, agents, routed) = pair();
        let handle = routed
            .acquire(&spec_for(session_key(), TransportKind::WebSocket))
            .await
            .unwrap();
        assert_eq!(routed.status(&handle).await.unwrap(), SandboxState::Running);
        routed.attach(&handle).await.unwrap();
        routed.stats(&handle).await.unwrap();
        routed.suspend(&handle).await.unwrap();
        assert_eq!(
            sessions.status(&handle).await.unwrap(),
            SandboxState::Suspended
        );
        assert_eq!(agents.status(&handle).await.unwrap(), SandboxState::Absent);
        routed.release(&handle).await.unwrap();
        assert_eq!(
            sessions.status(&handle).await.unwrap(),
            SandboxState::Absent
        );
    }

    /// A fake that reports a chosen kind and stamps it on its handles, so the two sides can be
    /// told apart (handle calls route by `handle.provider`).
    struct Kinded(SandboxProviderKind, FakeSandboxProvider);

    #[async_trait]
    impl SandboxProvider for Kinded {
        fn kind(&self) -> SandboxProviderKind {
            self.0
        }
        fn is_available(&self) -> bool {
            self.1.is_available()
        }
        async fn acquire(&self, spec: &SandboxSpec) -> Result<SandboxHandle> {
            let mut handle = self.1.acquire(spec).await?;
            handle.provider = self.0;
            Ok(handle)
        }
        async fn attach(&self, h: &SandboxHandle) -> Result<SandboxTransport> {
            self.1.attach(h).await
        }
        async fn status(&self, h: &SandboxHandle) -> Result<SandboxState> {
            self.1.status(h).await
        }
        async fn suspend(&self, h: &SandboxHandle) -> Result<()> {
            self.1.suspend(h).await
        }
        async fn release(&self, h: &SandboxHandle) -> Result<()> {
            self.1.release(h).await
        }
        async fn resume_takes_new_env(&self, h: &SandboxHandle) -> Result<bool> {
            self.1.resume_takes_new_env(h).await
        }
        async fn stats(&self, h: &SandboxHandle) -> Result<SandboxStats> {
            self.1.stats(h).await
        }
        async fn list(&self) -> Result<Vec<SandboxInfo>> {
            self.1.list().await
        }
    }

    fn kinded_pair() -> (Arc<Kinded>, Arc<Kinded>, KeyRoutedSandboxProvider) {
        let sessions = Arc::new(Kinded(
            SandboxProviderKind::SharedPod,
            FakeSandboxProvider::default(),
        ));
        let agents = Arc::new(Kinded(
            SandboxProviderKind::Pod,
            FakeSandboxProvider::default(),
        ));
        let routed = KeyRoutedSandboxProvider::new(sessions.clone(), agents.clone());
        (sessions, agents, routed)
    }

    #[tokio::test]
    async fn agent_handle_calls_reach_the_agent_side_only() {
        let (sessions, agents, routed) = kinded_pair();
        let handle = routed
            .acquire(&spec_for(agent_key(), TransportKind::Http))
            .await
            .unwrap();
        routed.suspend(&handle).await.unwrap();
        assert_eq!(
            agents.status(&handle).await.unwrap(),
            SandboxState::Suspended
        );
        assert_eq!(
            sessions.status(&handle).await.unwrap(),
            SandboxState::Absent
        );
    }

    #[tokio::test]
    async fn a_handle_both_sides_could_own_routes_by_key() {
        let sessions = Arc::new(Kinded(
            SandboxProviderKind::Pod,
            FakeSandboxProvider::default(),
        ));
        let agents = Arc::new(Kinded(
            SandboxProviderKind::Pod,
            FakeSandboxProvider::default(),
        ));
        let routed = KeyRoutedSandboxProvider::new(sessions.clone(), agents.clone());
        let handle = routed
            .acquire(&spec_for(agent_key(), TransportKind::Http))
            .await
            .unwrap();
        routed.suspend(&handle).await.unwrap();
        assert_eq!(
            agents.status(&handle).await.unwrap(),
            SandboxState::Suspended
        );
        assert_eq!(
            sessions.status(&handle).await.unwrap(),
            SandboxState::Absent
        );
    }

    #[tokio::test]
    async fn a_pod_kind_session_handle_goes_to_the_pod_side() {
        let (sessions, agents, routed) = kinded_pair();
        // A pod-era session sandbox: Session key, owned by the Pod provider.
        let handle = agents
            .acquire(&spec_for(session_key(), TransportKind::WebSocket))
            .await
            .unwrap();
        assert_eq!(handle.provider, SandboxProviderKind::Pod);
        routed.suspend(&handle).await.unwrap();
        assert_eq!(
            agents.status(&handle).await.unwrap(),
            SandboxState::Suspended
        );
        assert_eq!(
            sessions.status(&handle).await.unwrap(),
            SandboxState::Absent
        );
    }

    #[tokio::test]
    async fn resume_takes_new_env_is_the_answer_of_the_side_that_resumes_the_sandbox() {
        let (sessions, agents, routed) = kinded_pair();
        sessions.1.set_resume_takes_new_env(true);
        let session = routed
            .acquire(&spec_for(session_key(), TransportKind::WebSocket))
            .await
            .unwrap();
        let agent = routed
            .acquire(&spec_for(agent_key(), TransportKind::Http))
            .await
            .unwrap();
        assert!(routed.resume_takes_new_env(&session).await.unwrap());
        assert!(!routed.resume_takes_new_env(&agent).await.unwrap());

        agents.1.set_resume_takes_new_env(true);
        assert!(routed.resume_takes_new_env(&agent).await.unwrap());
        // A pod-era session sandbox: the pod side owns it, but its key's next acquire goes to the
        // session side, so nothing resumes it.
        let pod_era = agents
            .acquire(&spec_for(session_key(), TransportKind::WebSocket))
            .await
            .unwrap();
        assert!(!routed.resume_takes_new_env(&pod_era).await.unwrap());
    }

    #[tokio::test]
    async fn a_handle_neither_side_owns_is_never_resumed() {
        let (sessions, _agents, routed) = kinded_pair();
        sessions.1.set_resume_takes_new_env(true);
        let mut foreign = routed
            .acquire(&spec_for(session_key(), TransportKind::WebSocket))
            .await
            .unwrap();
        foreign.provider = SandboxProviderKind::Docker;
        assert!(!routed.resume_takes_new_env(&foreign).await.unwrap());
    }

    #[test]
    fn kind_is_the_session_side() {
        let routed = KeyRoutedSandboxProvider::new(
            Arc::new(UnavailableSandboxProvider::new(
                SandboxProviderKind::SharedPod,
            )),
            Arc::new(FakeSandboxProvider::default()),
        );
        assert_eq!(routed.kind(), SandboxProviderKind::SharedPod);
    }

    #[test]
    fn availability_is_per_key_and_overall_is_either_side() {
        let agents = Arc::new(FakeSandboxProvider::default());
        let routed = KeyRoutedSandboxProvider::new(
            Arc::new(UnavailableSandboxProvider::new(
                SandboxProviderKind::SharedPod,
            )),
            agents.clone(),
        );
        assert!(!routed.is_available_for(&session_key()));
        assert!(routed.is_available_for(&agent_key()));
        assert!(routed.is_available());

        agents.set_unavailable(true);
        assert!(!routed.is_available_for(&agent_key()));
        assert!(!routed.is_available());

        let sessions_up = KeyRoutedSandboxProvider::new(
            Arc::new(FakeSandboxProvider::default()),
            Arc::new(UnavailableSandboxProvider::new(SandboxProviderKind::Pod)),
        );
        assert!(sessions_up.is_available_for(&session_key()));
        assert!(!sessions_up.is_available_for(&agent_key()));
        assert!(sessions_up.is_available());
    }

    #[tokio::test]
    async fn list_skips_an_unavailable_side() {
        let (sessions, agents, routed) = pair();
        routed
            .acquire(&spec_for(session_key(), TransportKind::WebSocket))
            .await
            .unwrap();
        routed
            .acquire(&spec_for(agent_key(), TransportKind::Http))
            .await
            .unwrap();
        agents.set_unavailable(true);
        assert_eq!(routed.list().await.unwrap().len(), 1);
        agents.set_unavailable(false);
        sessions.set_unavailable(true);
        assert_eq!(routed.list().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn list_fails_when_an_available_side_errors() {
        let (sessions, agents, routed) = pair();
        sessions.fail_next_list(RuntimeError::Internal("boom".into()));
        assert!(routed.list().await.is_err());
        agents.fail_next_list(RuntimeError::Internal("boom".into()));
        assert!(routed.list().await.is_err());
    }
}
