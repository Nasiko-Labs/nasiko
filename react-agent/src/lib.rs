// TODO: Extract an Orchestrator trait so OSS and cloud can have different implementations.
// Cloud version can add: cost limits, team-scoped routing, fallback chains, advanced observability.
mod a2a;
mod context;
mod error;
mod events;
mod guard;
mod react_loop;
mod registry;
mod tool;

pub use a2a::{A2aClient, A2aClientError, A2aResponse};
pub use context::{ContextConfig, ContextManager, ContextWindow};
pub use error::OrchestratorError;
pub use events::OrchestratorEvent;
pub use guard::CallGuard;
pub use react_loop::{OrchestrationResult, Orchestrator, OrchestratorConfig, TurnTrace};
pub use registry::{AgentInfo, AgentRegistry, AgentSkill, RegistrySource};
pub use tool::{A2aTool, DelegationContext};
