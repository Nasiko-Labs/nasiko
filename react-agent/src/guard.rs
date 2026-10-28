use std::future::Future;
use std::pin::Pin;

use nasiko_types::agent_call::AgentCallHandle;

/// What [`CallGuard::before_call`] resolves to: the call's in-flight handle, or why it is blocked.
pub type BeforeCallFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Box<dyn AgentCallHandle>, String>> + Send + 'a>>;

/// Pre-call enforcement hook for the orchestrator.
/// Implementors check ACL, flow limits, cycle detection, etc. before each agent invocation.
pub trait CallGuard: Send + Sync {
    /// Called before invoking a target agent. Return Err(reason) to block the call.
    ///
    /// On success the returned handle is held for the whole call (including a streamed reply),
    /// told whether the call produced a result, and dropped when the call is over, so the
    /// implementor can count the call as in flight for exactly that span.
    fn before_call(&self, target_agent: &str) -> BeforeCallFuture<'_>;

    /// Called after a successful invocation. Used for tracking token usage, fan-out counts, etc.
    fn after_call(
        &self,
        target_agent: &str,
        tokens_used: u64,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;

    /// Called before each model call the orchestrator itself makes. Return
    /// Err(reason) to end the run: the reason is the failure the turn reports.
    fn before_completion(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }
}
