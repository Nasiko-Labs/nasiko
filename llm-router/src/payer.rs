//! Who pays for a call made through an agent by a user other than its owner.

use async_trait::async_trait;
use uuid::Uuid;

use crate::error::GatewayError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Payer {
    /// The caller's own default config and key.
    Caller,
    /// The agent's owner: the agent's attached config, else the owner's default, with the owner's key.
    Owner,
    /// The caller, because the owner's allowance for them is used up until `retry_after_secs` from now.
    CallerAfterAllowance { retry_after_secs: u64 },
}

#[async_trait]
pub trait SharedCallPayer: Send + Sync {
    /// Who pays for a call `caller` makes through an agent `owner` owns. Asked only when the two differ and the
    /// caller has not chosen a config for the agent; it may count the call.
    async fn payer(&self, owner: Uuid, caller: Uuid) -> Result<Payer, GatewayError>;
}

pub struct CallerPays;

#[async_trait]
impl SharedCallPayer for CallerPays {
    async fn payer(&self, _owner: Uuid, _caller: Uuid) -> Result<Payer, GatewayError> {
        Ok(Payer::Caller)
    }
}
