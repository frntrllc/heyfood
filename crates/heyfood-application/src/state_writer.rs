//! The sole serialized boundary for application state mutations.

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use heyfood_core::{
    AccountId, AgentEvent, ClientConfig, CommitId, CredentialVersion, GenerationId, OperationId,
    SessionCredentials, SessionSnapshot,
};
use tokio::sync::Mutex;

use crate::ports::{ConfigCommit, ConfigMutation, ConfigPort, CredentialCommit, CredentialPort};

/// Immutable inputs captured before a stateful workflow starts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationSnapshot {
    pub operation_id: OperationId,
    pub generation: GenerationId,
    pub config: ClientConfig,
    pub session: SessionSnapshot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutationClass {
    GenerationScoped,
    ServerAcceptedDurable,
    LocalFirstDurable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationMetadata {
    pub operation_id: OperationId,
    pub generation: GenerationId,
    pub class: MutationClass,
    pub expected_account: Option<AccountId>,
    pub expected_credential_version: Option<CredentialVersion>,
    pub credential_version: Option<CredentialVersion>,
    pub commit_id: CommitId,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Mutation {
    Presentation(AgentEvent),
    ConversationPointer(Option<String>),
    CredentialRotation(SessionCredentials),
    Config(ClientConfig),
    LocalFirstRecord { kind: String, payload: Vec<u8> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct MutationProposal {
    pub metadata: MutationMetadata,
    pub mutation: Mutation,
}

impl MutationProposal {
    #[must_use]
    pub fn presentation(snapshot: &OperationSnapshot, event: AgentEvent) -> Self {
        Self {
            metadata: MutationMetadata {
                operation_id: snapshot.operation_id,
                generation: snapshot.generation,
                class: MutationClass::GenerationScoped,
                expected_account: Some(snapshot.session.credentials.account_id.clone()),
                expected_credential_version: None,
                credential_version: Some(snapshot.session.credentials.version),
                commit_id: CommitId::new(),
            },
            mutation: Mutation::Presentation(event),
        }
    }

    #[must_use]
    pub fn conversation_pointer(snapshot: &OperationSnapshot, value: Option<String>) -> Self {
        Self {
            metadata: MutationMetadata {
                operation_id: snapshot.operation_id,
                generation: snapshot.generation,
                class: MutationClass::GenerationScoped,
                expected_account: Some(snapshot.session.credentials.account_id.clone()),
                expected_credential_version: None,
                credential_version: Some(snapshot.session.credentials.version),
                commit_id: CommitId::new(),
            },
            mutation: Mutation::ConversationPointer(value),
        }
    }

    #[must_use]
    pub fn credential_rotation(
        snapshot: &OperationSnapshot,
        commit_id: CommitId,
        credentials: SessionCredentials,
    ) -> Self {
        Self {
            metadata: MutationMetadata {
                operation_id: snapshot.operation_id,
                generation: snapshot.generation,
                class: MutationClass::ServerAcceptedDurable,
                expected_account: Some(snapshot.session.credentials.account_id.clone()),
                expected_credential_version: Some(snapshot.session.credentials.version),
                credential_version: Some(credentials.version),
                commit_id,
            },
            mutation: Mutation::CredentialRotation(credentials),
        }
    }

    #[must_use]
    pub fn local_first(
        snapshot: &OperationSnapshot,
        commit_id: CommitId,
        kind: impl Into<String>,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            metadata: MutationMetadata {
                operation_id: snapshot.operation_id,
                generation: snapshot.generation,
                class: MutationClass::LocalFirstDurable,
                expected_account: Some(snapshot.session.credentials.account_id.clone()),
                expected_credential_version: None,
                credential_version: Some(snapshot.session.credentials.version),
                commit_id,
            },
            mutation: Mutation::LocalFirstRecord {
                kind: kind.into(),
                payload,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitOutcome {
    Applied,
    Duplicate,
    RejectedStaleGeneration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommitError {
    InvalidProposal(&'static str),
    Port(crate::PortError),
    ReconciliationRequired(crate::PortError),
    ReconciliationMarkerWrite {
        operation: crate::PortError,
        marker: crate::PortError,
    },
    ReconciliationMarkerClear(crate::PortError),
}

impl fmt::Display for CommitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProposal(message) => formatter.write_str(message),
            Self::Port(error) => write!(formatter, "state commit failed: {error}"),
            Self::ReconciliationRequired(error) => {
                write!(
                    formatter,
                    "state commit outcome requires reconciliation: {error}"
                )
            }
            Self::ReconciliationMarkerWrite { operation, marker } => write!(
                formatter,
                "state outcome is uncertain ({operation}) and its reconciliation marker could not be written: {marker}"
            ),
            Self::ReconciliationMarkerClear(error) => write!(
                formatter,
                "durable state was committed but its reconciliation marker could not be cleared: {error}"
            ),
        }
    }
}

impl std::error::Error for CommitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidProposal(_) => None,
            Self::Port(error)
            | Self::ReconciliationRequired(error)
            | Self::ReconciliationMarkerClear(error) => Some(error),
            Self::ReconciliationMarkerWrite { marker, .. } => Some(marker),
        }
    }
}

struct WriterState {
    generation: GenerationId,
    account_id: Option<AccountId>,
    credential_version: Option<CredentialVersion>,
    applied_commits: HashSet<CommitId>,
}

/// Serializes every config/auth commit and owns the current presentation generation.
pub struct SerializedStateWriter {
    credential_port: Arc<dyn CredentialPort>,
    config_port: Arc<dyn ConfigPort>,
    state: Mutex<WriterState>,
}

impl SerializedStateWriter {
    #[must_use]
    pub fn new(
        credential_port: Arc<dyn CredentialPort>,
        config_port: Arc<dyn ConfigPort>,
        initial_generation: GenerationId,
        session: Option<&SessionCredentials>,
    ) -> Self {
        Self {
            credential_port,
            config_port,
            state: Mutex::new(WriterState {
                generation: initial_generation,
                account_id: session.map(|value| value.account_id.clone()),
                credential_version: session.map(|value| value.version),
                applied_commits: HashSet::new(),
            }),
        }
    }

    pub async fn current_generation(&self) -> GenerationId {
        self.state.lock().await.generation
    }

    /// Advance the generation only after the cancelled workflow has been joined.
    pub async fn advance_generation(&self) -> GenerationId {
        let mut state = self.state.lock().await;
        state.generation = state.generation.next();
        state.generation
    }

    /// Apply one proposal under the serialized commit lock.
    ///
    /// No cancellation token enters this method: once a durable proposal is
    /// made, its bounded adapter commit is deliberately non-cancellable.
    pub async fn commit(&self, proposal: MutationProposal) -> Result<CommitOutcome, CommitError> {
        validate_shape(&proposal)?;
        let retain_commit_id = !matches!(&proposal.mutation, Mutation::Presentation(_));
        let mut state = self.state.lock().await;

        if state.applied_commits.contains(&proposal.metadata.commit_id) {
            if matches!(proposal.mutation, Mutation::CredentialRotation(_)) {
                self.credential_port
                    .clear_reconciliation_required(proposal.metadata.commit_id)
                    .await
                    .map_err(CommitError::ReconciliationMarkerClear)?;
            }
            return Ok(CommitOutcome::Duplicate);
        }
        if proposal.metadata.class == MutationClass::GenerationScoped
            && proposal.metadata.generation != state.generation
        {
            return Ok(CommitOutcome::RejectedStaleGeneration);
        }
        if let (Some(expected), Some(actual)) = (
            proposal.metadata.expected_account.as_ref(),
            state.account_id.as_ref(),
        ) && expected != actual
        {
            return Err(CommitError::InvalidProposal("account snapshot is stale"));
        }

        match proposal.mutation {
            Mutation::Presentation(_) => {}
            Mutation::ConversationPointer(value) => {
                self.config_port
                    .commit(ConfigCommit {
                        commit_id: proposal.metadata.commit_id,
                        mutation: ConfigMutation::ConversationPointer(value),
                    })
                    .await
                    .map_err(CommitError::Port)?;
            }
            Mutation::CredentialRotation(credentials) => {
                let expected_version = proposal.metadata.expected_credential_version.ok_or(
                    CommitError::InvalidProposal(
                        "credential rotation is missing its original expected version",
                    ),
                )?;
                if credentials.version <= expected_version {
                    return Err(CommitError::InvalidProposal(
                        "credential rotation must advance the stored version",
                    ));
                }
                let commit = CredentialCommit {
                    commit_id: proposal.metadata.commit_id,
                    expected_version,
                    credentials: credentials.clone(),
                };
                if let Err(operation) = self.credential_port.commit(commit).await {
                    if let Err(marker) = self
                        .credential_port
                        .mark_reconciliation_required(proposal.metadata.commit_id)
                        .await
                    {
                        return Err(CommitError::ReconciliationMarkerWrite { operation, marker });
                    }
                    return Err(CommitError::ReconciliationRequired(operation));
                }
                self.credential_port
                    .clear_reconciliation_required(proposal.metadata.commit_id)
                    .await
                    .map_err(CommitError::ReconciliationMarkerClear)?;
                if state
                    .credential_version
                    .is_none_or(|current| credentials.version > current)
                {
                    state.account_id = Some(credentials.account_id);
                    state.credential_version = Some(credentials.version);
                }
            }
            Mutation::Config(config) => {
                self.config_port
                    .commit(ConfigCommit {
                        commit_id: proposal.metadata.commit_id,
                        mutation: ConfigMutation::Replace(config),
                    })
                    .await
                    .map_err(CommitError::Port)?;
            }
            Mutation::LocalFirstRecord { kind, payload } => {
                self.config_port
                    .commit(ConfigCommit {
                        commit_id: proposal.metadata.commit_id,
                        mutation: ConfigMutation::LocalFirstRecord { kind, payload },
                    })
                    .await
                    .map_err(CommitError::Port)?;
            }
        }

        if retain_commit_id {
            state.applied_commits.insert(proposal.metadata.commit_id);
        }
        Ok(CommitOutcome::Applied)
    }

    /// Persist an uncertain network outcome before returning control to the UI.
    pub async fn mark_reconciliation_required(
        &self,
        commit_id: CommitId,
        operation: crate::PortError,
    ) -> Result<(), CommitError> {
        let _state = self.state.lock().await;
        self.credential_port
            .mark_reconciliation_required(commit_id)
            .await
            .map_err(|marker| CommitError::ReconciliationMarkerWrite { operation, marker })
    }
}

fn validate_shape(proposal: &MutationProposal) -> Result<(), CommitError> {
    let valid = matches!(
        (&proposal.metadata.class, &proposal.mutation),
        (
            MutationClass::GenerationScoped,
            Mutation::Presentation(_) | Mutation::ConversationPointer(_)
        ) | (
            MutationClass::ServerAcceptedDurable,
            Mutation::CredentialRotation(_) | Mutation::Config(_)
        ) | (
            MutationClass::LocalFirstDurable,
            Mutation::LocalFirstRecord { .. }
        )
    );
    if !valid {
        return Err(CommitError::InvalidProposal(
            "mutation payload does not match its durability class",
        ));
    }

    if let Mutation::CredentialRotation(credentials) = &proposal.mutation {
        if proposal.metadata.expected_account.as_ref() != Some(&credentials.account_id) {
            return Err(CommitError::InvalidProposal(
                "credential rotation account does not match metadata",
            ));
        }
        if proposal.metadata.credential_version != Some(credentials.version) {
            return Err(CommitError::InvalidProposal(
                "credential rotation version does not match metadata",
            ));
        }
        if proposal
            .metadata
            .expected_credential_version
            .is_none_or(|expected| credentials.version <= expected)
        {
            return Err(CommitError::InvalidProposal(
                "credential rotation must preserve an earlier expected version",
            ));
        }
    }

    Ok(())
}
