//! Single-flight workflow supervisor with cancel-and-join generation changes.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use heyfood_core::{ClientConfig, OperationId, SessionSnapshot};
use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;

use crate::{OperationSnapshot, SerializedStateWriter};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SupervisorError {
    WorkflowActive,
    UnknownOperation,
    JoinTimeout,
}

impl fmt::Display for SupervisorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkflowActive => formatter.write_str("a stateful workflow is already active"),
            Self::UnknownOperation => formatter.write_str("the workflow is no longer active"),
            Self::JoinTimeout => {
                formatter.write_str("the workflow did not stop before its deadline")
            }
        }
    }
}

impl std::error::Error for SupervisorError {}

struct ActiveWorkflow {
    operation_id: OperationId,
    cancellation: CancellationToken,
    completed: watch::Receiver<bool>,
}

#[derive(Debug)]
pub struct WorkflowLease {
    pub snapshot: OperationSnapshot,
    pub cancellation: CancellationToken,
    completed: watch::Sender<bool>,
}

impl WorkflowLease {
    /// Mark all network, stream-close, and durable-commit work complete.
    pub fn finish(self) {}
}

impl Drop for WorkflowLease {
    fn drop(&mut self) {
        let _ = self.completed.send(true);
    }
}

pub struct OperationSupervisor {
    writer: Arc<SerializedStateWriter>,
    active: Mutex<Option<ActiveWorkflow>>,
}

impl OperationSupervisor {
    #[must_use]
    pub fn new(writer: Arc<SerializedStateWriter>) -> Self {
        Self {
            writer,
            active: Mutex::new(None),
        }
    }

    pub async fn begin(
        &self,
        config: ClientConfig,
        session: SessionSnapshot,
    ) -> Result<WorkflowLease, SupervisorError> {
        let mut active = self.active.lock().await;
        if active.is_some() {
            return Err(SupervisorError::WorkflowActive);
        }
        let operation_id = OperationId::new();
        let cancellation = CancellationToken::new();
        let (completed, receiver) = watch::channel(false);
        let snapshot = OperationSnapshot {
            operation_id,
            generation: self.writer.current_generation().await,
            config,
            session,
        };
        *active = Some(ActiveWorkflow {
            operation_id,
            cancellation: cancellation.clone(),
            completed: receiver,
        });
        Ok(WorkflowLease {
            snapshot,
            cancellation,
            completed,
        })
    }

    pub async fn join(
        &self,
        operation_id: OperationId,
        deadline: Duration,
    ) -> Result<(), SupervisorError> {
        self.join_inner(operation_id, deadline, false).await
    }

    pub async fn cancel_and_join(
        &self,
        operation_id: OperationId,
        deadline: Duration,
    ) -> Result<(), SupervisorError> {
        self.join_inner(operation_id, deadline, true).await
    }

    async fn join_inner(
        &self,
        operation_id: OperationId,
        deadline: Duration,
        cancel: bool,
    ) -> Result<(), SupervisorError> {
        let mut completed = {
            let active = self.active.lock().await;
            let active = active.as_ref().ok_or(SupervisorError::UnknownOperation)?;
            if active.operation_id != operation_id {
                return Err(SupervisorError::UnknownOperation);
            }
            if cancel {
                active.cancellation.cancel();
            }
            active.completed.clone()
        };

        if !*completed.borrow() {
            tokio::time::timeout(deadline, completed.wait_for(|value| *value))
                .await
                .map_err(|_| SupervisorError::JoinTimeout)?
                .map_err(|_| SupervisorError::JoinTimeout)?;
        }

        let mut active = self.active.lock().await;
        if active.as_ref().map(|value| value.operation_id) != Some(operation_id) {
            return Err(SupervisorError::UnknownOperation);
        }
        *active = None;
        self.writer.advance_generation().await;
        Ok(())
    }

    pub async fn has_active_workflow(&self) -> bool {
        self.active.lock().await.is_some()
    }
}
