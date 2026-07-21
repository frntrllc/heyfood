//! Killable native-keyring broker. Credentials cross the process boundary only
//! through inherited anonymous pipes, never argv, environment, logs, or files.

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::time::Duration;

use heyfood_application::{BoxFuture, CredentialCommit, CredentialPort, PortError};
use heyfood_core::{CommitId, CredentialVersion, SessionCredentials};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::persistence::CredentialState;

const BROKER_MODE: &str = "__heyfood_credential_broker";
const MAX_BROKER_DOCUMENT_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug)]
pub struct CredentialBrokerStore {
    executable: PathBuf,
    root: PathBuf,
    deadline: Duration,
}

impl CredentialBrokerStore {
    pub fn open(root: impl Into<PathBuf>, deadline: Duration) -> Result<Self, PortError> {
        if deadline.is_zero() || deadline > Duration::from_secs(30) {
            return Err(PortError::new(
                "credential_broker_deadline",
                "credential broker deadline must be between 1ns and 30s",
            ));
        }
        let executable = std::env::current_exe()
            .map_err(|error| PortError::new("credential_broker_executable", error.to_string()))?;
        if !executable.is_file() {
            return Err(PortError::new(
                "credential_broker_executable",
                "current executable is not a regular file",
            ));
        }
        Ok(Self {
            executable,
            root: root.into(),
            deadline,
        })
    }

    pub async fn initialize(&self, credentials: SessionCredentials) -> Result<(), PortError> {
        let input = CredentialState::new(credentials).encode();
        self.request("initialize", input, true).await.map(|_| ())
    }

    pub async fn delete(&self) -> Result<(), PortError> {
        self.request("delete", Vec::new(), true).await.map(|_| ())
    }

    async fn request(
        &self,
        action: &'static str,
        input: Vec<u8>,
        outcome_uncertain: bool,
    ) -> Result<Vec<u8>, PortError> {
        if input.len() > MAX_BROKER_DOCUMENT_BYTES {
            return Err(PortError::new(
                "credential_broker_size",
                "credential broker input exceeds its limit",
            ));
        }
        let mut child = Command::new(&self.executable)
            .arg(BROKER_MODE)
            .arg(action)
            .arg(&self.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| PortError::new("credential_broker_spawn", error.to_string()))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| PortError::new("credential_broker_pipe", "broker stdin is missing"))?;
        stdin
            .write_all(&input)
            .await
            .map_err(|error| PortError::new("credential_broker_pipe", error.to_string()))?;
        stdin
            .shutdown()
            .await
            .map_err(|error| PortError::new("credential_broker_pipe", error.to_string()))?;
        drop(stdin);

        let output = match tokio::time::timeout(self.deadline, child.wait_with_output()).await {
            Ok(result) => result
                .map_err(|error| PortError::new("credential_broker_wait", error.to_string()))?,
            Err(_) if outcome_uncertain => {
                return Err(PortError::uncertain(
                    "credential_broker_timeout",
                    "native credential operation exceeded its deadline",
                ));
            }
            Err(_) => {
                return Err(PortError::new(
                    "credential_broker_timeout",
                    "native credential operation exceeded its deadline",
                ));
            }
        };
        if !output.status.success() {
            return Err(if outcome_uncertain {
                PortError::uncertain(
                    "credential_broker_failed",
                    "native credential operation failed",
                )
            } else {
                PortError::new(
                    "credential_broker_failed",
                    "native credential operation failed",
                )
            });
        }
        if output.stdout.len() > MAX_BROKER_DOCUMENT_BYTES {
            return Err(PortError::new(
                "credential_broker_size",
                "credential broker output exceeds its limit",
            ));
        }
        Ok(output.stdout)
    }
}

impl CredentialPort for CredentialBrokerStore {
    fn load(&self) -> BoxFuture<'_, Result<Option<SessionCredentials>, PortError>> {
        Box::pin(async move {
            let output = self.request("load", Vec::new(), false).await?;
            if output.is_empty() {
                Ok(None)
            } else {
                CredentialState::decode(&output)
                    .map(|state| Some(state.credentials))
                    .map_err(|_| {
                        PortError::new(
                            "credential_broker_response",
                            "native credential broker returned an invalid document",
                        )
                    })
            }
        })
    }

    fn commit(&self, commit: CredentialCommit) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            let mut input = format!(
                "expected={}\ncommit={}\n",
                commit.expected_version.get(),
                commit.commit_id.as_uuid()
            )
            .into_bytes();
            input.extend_from_slice(&CredentialState::new(commit.credentials).encode());
            self.request("commit", input, true).await.map(|_| ())
        })
    }

    fn mark_reconciliation_required(
        &self,
        commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            self.request(
                "mark",
                format!("{}\n", commit_id.as_uuid()).into_bytes(),
                false,
            )
            .await
            .map(|_| ())
        })
    }

    fn clear_reconciliation_required(
        &self,
        commit_id: CommitId,
    ) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            self.request(
                "clear",
                format!("{}\n", commit_id.as_uuid()).into_bytes(),
                false,
            )
            .await
            .map(|_| ())
        })
    }
}

/// Handle the broker mode before any terminal/tracing initialization. Returns
/// `None` for every ordinary invocation.
pub fn run_credential_broker_if_requested() -> Option<ExitCode> {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() != Some(OsStr::new(BROKER_MODE)) {
        return None;
    }
    let Some(action) = arguments.next().and_then(|value| value.into_string().ok()) else {
        return Some(ExitCode::from(2));
    };
    let Some(root) = arguments.next().map(PathBuf::from) else {
        return Some(ExitCode::from(2));
    };
    if arguments.next().is_some() {
        return Some(ExitCode::from(2));
    }
    Some(match run_broker_action(&action, &root) {
        Ok(output) => {
            if std::io::stdout().write_all(&output).is_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(_) => ExitCode::FAILURE,
    })
}

fn run_broker_action(action: &str, root: &Path) -> Result<Vec<u8>, PortError> {
    let mut input = Vec::new();
    std::io::stdin()
        .take((MAX_BROKER_DOCUMENT_BYTES + 1) as u64)
        .read_to_end(&mut input)
        .map_err(|error| PortError::new("credential_broker_read", error.to_string()))?;
    if input.len() > MAX_BROKER_DOCUMENT_BYTES {
        return Err(PortError::new(
            "credential_broker_size",
            "credential broker input exceeds its limit",
        ));
    }

    #[cfg(windows)]
    let store = crate::persistence::WindowsCredentialStore::open(root)?;
    #[cfg(not(windows))]
    let store = crate::persistence::KeyringCredentialStore::open(root)?;

    match action {
        "load" if input.is_empty() => Ok(store
            .broker_load()?
            .map_or_else(Vec::new, |value| CredentialState::new(value).encode())),
        "initialize" => {
            store.initialize(&CredentialState::decode(&input)?.credentials)?;
            Ok(Vec::new())
        }
        "commit" => {
            let expected = required_field(&input, "expected")?
                .parse::<u64>()
                .map(CredentialVersion::new)
                .map_err(|_| {
                    PortError::new("credential_broker_request", "invalid expected version")
                })?;
            let commit_id = parse_commit_id(required_field(&input, "commit")?)?;
            let credentials = CredentialState::decode(&input)?.credentials;
            store.broker_commit(CredentialCommit {
                commit_id,
                expected_version: expected,
                credentials,
            })?;
            Ok(Vec::new())
        }
        "mark" => {
            store.broker_mark(parse_commit_id(trimmed_input(&input)?)?)?;
            Ok(Vec::new())
        }
        "clear" => {
            store.broker_clear(parse_commit_id(trimmed_input(&input)?)?)?;
            Ok(Vec::new())
        }
        "delete" if input.is_empty() => {
            store.delete()?;
            Ok(Vec::new())
        }
        _ => Err(PortError::new(
            "credential_broker_request",
            "invalid credential broker request",
        )),
    }
}

fn required_field<'a>(input: &'a [u8], name: &str) -> Result<&'a str, PortError> {
    let input = std::str::from_utf8(input)
        .map_err(|_| PortError::new("credential_broker_request", "request is not UTF-8"))?;
    input
        .lines()
        .find_map(|line| line.split_once('=').filter(|(key, _)| *key == name))
        .map(|(_, value)| value)
        .ok_or_else(|| {
            PortError::new(
                "credential_broker_request",
                format!("request is missing {name}"),
            )
        })
}

fn trimmed_input(input: &[u8]) -> Result<&str, PortError> {
    std::str::from_utf8(input)
        .map(str::trim)
        .map_err(|_| PortError::new("credential_broker_request", "request is not UTF-8"))
}

fn parse_commit_id(value: &str) -> Result<CommitId, PortError> {
    serde_json::from_value(serde_json::Value::String(value.to_owned()))
        .map_err(|_| PortError::new("credential_broker_request", "invalid commit ID"))
}
