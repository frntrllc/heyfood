//! Killable native-keyring broker. Credentials cross the process boundary only
//! through inherited anonymous pipes, never argv, environment, logs, or files.

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::time::Duration;

use heyfood_application::{BoxFuture, CredentialCommit, CredentialPort, PortError};
use heyfood_core::{CommitId, CredentialVersion, SessionCredentials};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
        let child = Command::new(&self.executable)
            .arg(BROKER_MODE)
            .arg(action)
            .arg(&self.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| PortError::new("credential_broker_spawn", error.to_string()))?;
        run_bounded_child(child, input, self.deadline, outcome_uncertain).await
    }
}

async fn run_bounded_child(
    mut child: tokio::process::Child,
    input: Vec<u8>,
    deadline: Duration,
    outcome_uncertain: bool,
) -> Result<Vec<u8>, PortError> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| PortError::new("credential_broker_pipe", "broker stdin is missing"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| PortError::new("credential_broker_pipe", "broker stdout is missing"))?;
    let operation = async {
        stdin
            .write_all(&input)
            .await
            .map_err(|error| PortError::new("credential_broker_pipe", error.to_string()))?;
        stdin
            .shutdown()
            .await
            .map_err(|error| PortError::new("credential_broker_pipe", error.to_string()))?;
        drop(stdin);
        let mut output = Vec::new();
        let mut stdout = stdout.take((MAX_BROKER_DOCUMENT_BYTES + 1) as u64);
        let (status, _) = tokio::try_join!(child.wait(), stdout.read_to_end(&mut output),)
            .map_err(|error| PortError::new("credential_broker_wait", error.to_string()))?;
        Ok::<_, PortError>((status, output))
    };
    let (status, output) = match tokio::time::timeout(deadline, operation).await {
        Ok(result) => result?,
        Err(_) => {
            // Dropping a `kill_on_drop` child requests termination but does not
            // guarantee that the OS process has been reaped before this method
            // returns. Explicitly kill and await it so a timed-out keyring
            // prompt cannot survive as a live or zombie broker.
            child.kill().await.map_err(|_| {
                broker_error(
                    outcome_uncertain,
                    "credential_broker_reap",
                    "native credential broker could not be terminated",
                )
            })?;
            return Err(broker_error(
                outcome_uncertain,
                "credential_broker_timeout",
                "native credential operation exceeded its deadline",
            ));
        }
    };
    if !status.success() {
        return Err(broker_error(
            outcome_uncertain,
            "credential_broker_failed",
            "native credential operation failed",
        ));
    }
    if output.len() > MAX_BROKER_DOCUMENT_BYTES {
        return Err(PortError::new(
            "credential_broker_size",
            "credential broker output exceeds its limit",
        ));
    }
    Ok(output)
}

fn broker_error(outcome_uncertain: bool, code: &'static str, message: &'static str) -> PortError {
    if outcome_uncertain {
        PortError::uncertain(code, message)
    } else {
        PortError::new(code, message)
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
    if verify_broker_parent().is_err() {
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

/// Prevent the hidden broker mode from becoming a confused-deputy credential
/// oracle. Only the exact running heyfood executable may be the broker's
/// immediate parent; shells, test runners, and unrelated processes fail before
/// stdin or native credential storage is touched.
fn verify_broker_parent() -> Result<(), PortError> {
    use sysinfo::{Pid, ProcessesToUpdate, System};

    let current_pid = Pid::from_u32(std::process::id());
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[current_pid]), true);
    let parent_pid = system
        .process(current_pid)
        .and_then(sysinfo::Process::parent)
        .ok_or_else(|| {
            PortError::new(
                "credential_broker_parent",
                "credential broker parent identity is unavailable",
            )
        })?;
    system.refresh_processes(ProcessesToUpdate::Some(&[parent_pid]), true);
    let parent_executable = system
        .process(parent_pid)
        .and_then(sysinfo::Process::exe)
        .ok_or_else(|| {
            PortError::new(
                "credential_broker_parent",
                "credential broker parent executable is unavailable",
            )
        })?
        .canonicalize()
        .map_err(|error| PortError::new("credential_broker_parent", error.to_string()))?;
    let broker_executable = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|error| PortError::new("credential_broker_parent", error.to_string()))?;
    if parent_executable != broker_executable {
        return Err(PortError::new(
            "credential_broker_parent",
            "credential broker was not launched by the running heyfood executable",
        ));
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Stdio;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use sysinfo::{Pid, ProcessesToUpdate, System};
    use tokio::process::Command;

    use super::run_bounded_child;

    #[test]
    #[ignore = "spawned only by the bounded broker lifecycle test"]
    fn broker_prompt_fixture() {
        let path = std::env::var_os("HEYFOOD_BROKER_TEST_PID_FILE")
            .map(PathBuf::from)
            .expect("fixture PID path");
        std::fs::write(path, format!("{}\n", std::process::id())).expect("publish fixture PID");
        std::thread::sleep(Duration::from_secs(30));
    }

    #[tokio::test]
    async fn timeout_terminates_a_prompting_broker_without_an_orphan() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let pid_path =
            std::env::temp_dir().join(format!("heyfood-broker-pid-{}-{nonce}", std::process::id()));
        let mut child = Command::new(std::env::current_exe().expect("test executable"));
        child
            .args([
                "--exact",
                "credential_broker::tests::broker_prompt_fixture",
                "--ignored",
                "--nocapture",
            ])
            .env("HEYFOOD_BROKER_TEST_PID_FILE", &pid_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let child = child.spawn().expect("spawn prompt fixture");
        let error = run_bounded_child(child, Vec::new(), Duration::from_millis(250), true)
            .await
            .expect_err("prompting broker must time out");
        assert_eq!(error.code, "credential_broker_timeout");
        assert!(error.outcome_uncertain);

        let pid = std::fs::read_to_string(&pid_path)
            .expect("fixture published PID")
            .trim()
            .parse::<u32>()
            .expect("fixture PID");
        let pid = Pid::from_u32(pid);
        let mut system = System::new();
        for _ in 0..100 {
            system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
            if system.process(pid).is_none() {
                let _ = std::fs::remove_file(&pid_path);
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = std::fs::remove_file(&pid_path);
        panic!("timed-out broker process remained alive");
    }
}
