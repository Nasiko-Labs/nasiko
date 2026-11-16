//! Who pays for a call made through an agent by a user other than its owner, and whether a
//! deployment covers an owner's own call that found no key.

use async_trait::async_trait;
use uuid::Uuid;

use crate::error::GatewayError;
use crate::resolver::{ConfigSubject, RegistryStore};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Payer {
    /// The caller's own default config and key.
    Caller,
    /// The agent's owner: the agent's attached config, else the owner's default, with the owner's key;
    /// with no stored key, the platform's own key ([`ConfigSubject::Sponsored`]).
    Owner,
    /// The caller, because the owner's allowance for them is used up until `retry_after_secs` from now.
    CallerAfterAllowance { retry_after_secs: u64 },
    /// The platform: its default provider and model on its own key, with no config read and the
    /// requested model ignored ([`ConfigSubject::Platform`]).
    Platform,
}

#[async_trait]
pub trait SharedCallPayer: Send + Sync {
    /// Who pays for a call `caller` makes through an agent `owner` owns. Asked when the two differ and the
    /// caller has not chosen a config for the agent, and, with `owner == caller`, about an owner's own call
    /// that found no key ([`cover_owner_call`]). It may count the call. [`CallerPays`] answers `Caller`,
    /// which leaves every call as it was.
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
        Payer::Owner => (ConfigSubject::Sponsored(owner_id), None),
        // The platform covers only an owner's own call (`cover_owner_call`); anyone else stays on their own.
        Payer::Caller | Payer::Platform => (subject, None),
        Payer::CallerAfterAllowance { retry_after_secs } => (subject, Some(retry_after_secs)),
    })
}

/// Whether the payer covers an owner's own call through their agent that resolved to no key
/// ([`GatewayError::NoApiKey`]): `Some` subject to resolve it again on, `None` to keep the original
/// error. Only a call attributed to the owner themselves is asked about; an unattributed call (a
/// coding agent, a session token, a flow with no user) never is.
pub async fn cover_owner_call(
    payer: &dyn SharedCallPayer,
    owner_id: &str,
    acting_user: Option<Uuid>,
) -> Result<Option<ConfigSubject<'static>>, GatewayError> {
    let owner = match (Uuid::parse_str(owner_id), acting_user) {
        (Ok(owner), Some(user)) if owner == user => owner,
        _ => return Ok(None),
    };
    match payer.payer(owner, owner).await? {
        Payer::Platform => Ok(Some(ConfigSubject::Platform)),
        Payer::CallerAfterAllowance { retry_after_secs } => {
            Err(GatewayError::OwnerAllowanceSpent { retry_after_secs })
        }
        Payer::Caller | Payer::Owner => Ok(None),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: &str = "22222222-2222-2222-2222-222222222222";
    const SOMEONE_ELSE: &str = "33333333-3333-3333-3333-333333333333";

    fn id(s: &str) -> Uuid {
        Uuid::parse_str(s).unwrap()
    }

    #[tokio::test]
    async fn an_owner_call_asks_about_the_owner_alone_and_maps_the_answer() {
        let covered = RecordingPayer::answering(Some(Payer::Platform));
        let subject = cover_owner_call(covered.as_ref(), OWNER, Some(id(OWNER))).await;
        assert_eq!(subject.unwrap(), Some(ConfigSubject::Platform));
        assert_eq!(*covered.calls.lock().unwrap(), [(id(OWNER), id(OWNER))]);

        for left in [Payer::Caller, Payer::Owner] {
            let payer = RecordingPayer::answering(Some(left));
            let subject = cover_owner_call(payer.as_ref(), OWNER, Some(id(OWNER))).await;
            assert_eq!(subject.unwrap(), None);
        }

        let spent = RecordingPayer::answering(Some(Payer::CallerAfterAllowance {
            retry_after_secs: 7,
        }));
        let err = cover_owner_call(spent.as_ref(), OWNER, Some(id(OWNER)))
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                GatewayError::OwnerAllowanceSpent {
                    retry_after_secs: 7
                }
            ),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn a_call_by_anyone_else_or_no_one_never_asks() {
        let payer = RecordingPayer::answering(Some(Payer::Platform));
        let calls = [
            (OWNER, None),
            (OWNER, Some(id(SOMEONE_ELSE))),
            ("not-a-uuid", Some(id(OWNER))),
        ];
        for (owner_id, acting_user) in calls {
            let subject = cover_owner_call(payer.as_ref(), owner_id, acting_user).await;
            assert_eq!(subject.unwrap(), None);
        }
        assert!(payer.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_payer_failure_on_an_owner_call_fails_closed() {
        let payer = RecordingPayer::answering(None);
        let err = cover_owner_call(payer.as_ref(), OWNER, Some(id(OWNER)))
            .await
            .unwrap_err();
        assert!(matches!(err, GatewayError::Internal(_)), "{err:?}");
    }
}
