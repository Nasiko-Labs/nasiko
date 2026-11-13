//! Who pays for a call made through an agent by a user other than its owner.

use async_trait::async_trait;
use uuid::Uuid;

use crate::error::GatewayError;
use crate::resolver::{ConfigSubject, RegistryStore};

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

/// Whose config and keys a call by `acting_user` through `agent_id` (owned by `owner_id`) runs on,
/// and, when the payer sent the caller to their own config because an allowance is spent, the
/// seconds until it renews.
///
/// Only a call by someone other than the owner can reach the payer, and only when that caller
/// has not chosen a config for the agent: a choice is theirs to spend and is never counted.
pub async fn subject_for_call<'a>(
    store: &dyn RegistryStore,
    payer: &dyn SharedCallPayer,
    agent_id: &str,
    owner_id: &'a str,
    acting_user: Option<Uuid>,
) -> Result<(ConfigSubject<'a>, Option<u64>), GatewayError> {
    let subject = ConfigSubject::for_call(owner_id, acting_user);
    // `for_call` yields `SharedUser` only for a parsable owner, so the second pattern always matches.
    let (ConfigSubject::SharedUser(caller), Ok(owner)) = (subject, Uuid::parse_str(owner_id))
    else {
        return Ok((subject, None));
    };
    let agent = Uuid::parse_str(agent_id)
        .map_err(|_| GatewayError::NoRegistryEntry(agent_id.to_string()))?;
    let chose = store
        .has_user_agent_config(caller, agent)
        .await
        .map_err(|e| GatewayError::Internal(format!("shared user choice read failed: {e}")))?;
    if chose {
        return Ok((subject, None));
    }
    Ok(match payer.payer(owner, caller).await? {
        Payer::Owner => (ConfigSubject::Owner(owner_id), None),
        Payer::Caller => (subject, None),
        Payer::CallerAfterAllowance { retry_after_secs } => (subject, Some(retry_after_secs)),
    })
}

/// A shared caller with no config of their own, sent to it by an allowance the payer spent, is
/// told about the allowance; any other result is unchanged.
pub fn allowance_error(error: GatewayError, retry_after_secs: Option<u64>) -> GatewayError {
    match (error, retry_after_secs) {
        (GatewayError::NoSharedUserConfig, Some(retry_after_secs)) => {
            GatewayError::OwnerAllowanceSpent { retry_after_secs }
        }
        (error, _) => error,
    }
}

/// Test double: answers a fixed [`Payer`] (`None` fails, as a down counter store would) and
/// records every `(owner, caller)` it was asked about.
#[cfg(test)]
pub(crate) struct RecordingPayer {
    answer: Option<Payer>,
    pub calls: std::sync::Mutex<Vec<(Uuid, Uuid)>>,
}

#[cfg(test)]
impl RecordingPayer {
    pub fn answering(answer: Option<Payer>) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            answer,
            calls: Default::default(),
        })
    }
}

#[cfg(test)]
#[async_trait]
impl SharedCallPayer for RecordingPayer {
    async fn payer(&self, owner: Uuid, caller: Uuid) -> Result<Payer, GatewayError> {
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((owner, caller));
        self.answer
            .ok_or_else(|| GatewayError::Internal("counter store down".into()))
    }
}
