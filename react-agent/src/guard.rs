use std::future::Future;
use std::pin::Pin;

/// Pre-call enforcement hook for the orchestrator.
/// Implementors check ACL, flow limits, cycle detection, etc. before each agent invocation.
pub trait CallGuard: Send + Sync {
    /// Called before invoking a target agent. Return Err(reason) to block the call.
    fn before_call(
        &self,
        target_agent: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;

    /// Called after a successful invocation. Used for tracking token usage, fan-out counts, etc.
    fn after_call(
        &self,
        target_agent: &str,
        tokens_used: u64,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}
