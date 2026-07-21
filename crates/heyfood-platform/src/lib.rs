//! Native operating-system adapters used by the Rust vertical proof.

#![forbid(unsafe_code)]

#[cfg(feature = "native-credentials")]
mod credential_broker;
mod persistence;
mod python_import;

use std::io::{IsTerminal, Stderr, Stdin, Stdout};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use directories::ProjectDirs;
use heyfood_application::{BoxFuture, BrowserPort, ClockPort, PortError};
use heyfood_core::{BrowserUrl, NetworkPolicy, ProxyUrl, ServiceUrl};
use tokio::sync::mpsc;

#[cfg(feature = "native-credentials")]
pub use credential_broker::{CredentialBrokerStore, run_credential_broker_if_requested};
#[cfg(all(not(windows), feature = "native-credentials"))]
pub use persistence::KeyringCredentialStore;
#[cfg(all(windows, feature = "native-credentials"))]
pub use persistence::WindowsCredentialStore;
pub use persistence::{AtomicFile, FileCredentialStore, NativeAuthStore, NativeConfigStore};
pub use python_import::PythonStateImporter;

/// The package version shared by the native workspace.
pub const VERSION: &str = heyfood_core::VERSION;

/// All Phase 0 files are rooted together so controlled fixtures never touch a
/// user's real profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativePaths {
    config: PathBuf,
    data: PathBuf,
    cache: PathBuf,
    runtime: PathBuf,
}

impl NativePaths {
    #[must_use]
    pub fn under(root: impl Into<PathBuf>) -> Self {
        let config = root.into();
        Self {
            data: config.join("data"),
            cache: config.join("cache"),
            runtime: config.join("runtime"),
            config,
        }
    }

    pub fn discover() -> Result<Self, PortError> {
        let project = ProjectDirs::from("ai", "frntr", "heyfood").ok_or_else(|| {
            PortError::new(
                "platform_paths",
                "native configuration directory is unavailable",
            )
        })?;
        Ok(Self {
            config: project.config_dir().to_owned(),
            data: project.data_dir().to_owned(),
            cache: project.cache_dir().to_owned(),
            runtime: project
                .runtime_dir()
                .unwrap_or_else(|| project.data_local_dir())
                .join("runtime"),
        })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        self.config_dir()
    }

    #[must_use]
    pub fn config_dir(&self) -> &Path {
        &self.config
    }

    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data
    }

    #[must_use]
    pub fn cache_dir(&self) -> &Path {
        &self.cache
    }

    #[must_use]
    pub fn runtime_dir(&self) -> &Path {
        &self.runtime
    }

    /// Account-local state is isolated by an opaque, validated account ID. No
    /// profile, household, conversation, or cache state is shared on switch.
    pub fn account_state_dir(&self, account_id: &heyfood_core::AccountId) -> PathBuf {
        use sha2::{Digest, Sha256};

        let digest = Sha256::digest(account_id.as_str().as_bytes());
        let mut name = String::with_capacity(32);
        for byte in &digest[..16] {
            use std::fmt::Write as _;
            let _ = write!(name, "{byte:02x}");
        }
        self.data.join("accounts").join(name)
    }
}

/// Dependency-neutral selection seam passed to the runtime adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeNetworkPolicy(NetworkPolicy);

impl NativeNetworkPolicy {
    #[must_use]
    pub const fn strict() -> Self {
        Self(NetworkPolicy::HTTPS_ONLY)
    }

    #[must_use]
    pub const fn controlled_loopback() -> Self {
        Self(NetworkPolicy::DEVELOPMENT)
    }

    #[must_use]
    pub const fn get(self) -> NetworkPolicy {
        self.0
    }

    pub fn service_url(self, value: &str) -> Result<ServiceUrl, PortError> {
        ServiceUrl::parse(value, self.0)
            .map_err(|error| PortError::new("network_policy", error.to_string()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProxyConfiguration {
    pub http_proxy: Option<ProxyUrl>,
    pub https_proxy: Option<ProxyUrl>,
    pub no_proxy: Option<String>,
}

impl ProxyConfiguration {
    pub fn from_values(
        http_proxy: Option<&str>,
        https_proxy: Option<&str>,
        no_proxy: Option<&str>,
    ) -> Result<Self, PortError> {
        let parse = |name: &'static str, value: Option<&str>| {
            value
                .filter(|value| !value.is_empty())
                .map(|value| {
                    let parsed = ProxyUrl::parse(value)
                        .map_err(|error| PortError::new(name, error.to_string()))?;
                    Ok(parsed)
                })
                .transpose()
        };
        let no_proxy = no_proxy
            .filter(|value| !value.is_empty())
            .map(|value| {
                if value.len() > 8 * 1024 || value.chars().any(char::is_control) {
                    return Err(PortError::new("no_proxy", "NO_PROXY is invalid"));
                }
                Ok(value.to_owned())
            })
            .transpose()?;
        Ok(Self {
            http_proxy: parse("http_proxy", http_proxy)?,
            https_proxy: parse("https_proxy", https_proxy)?,
            no_proxy,
        })
    }

    pub fn from_environment() -> Result<Self, PortError> {
        fn environment(name: &'static str) -> Result<Option<String>, PortError> {
            std::env::var_os(name)
                .map(|value| {
                    value
                        .into_string()
                        .map_err(|_| PortError::new(name, format!("{name} is not valid Unicode")))
                })
                .transpose()
        }

        let http = environment("HTTP_PROXY")?;
        let https = environment("HTTPS_PROXY")?;
        let no_proxy = environment("NO_PROXY")?;
        Self::from_values(http.as_deref(), https.as_deref(), no_proxy.as_deref())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TlsRootPolicy {
    pub custom_ca_bundle: Option<PathBuf>,
}

impl TlsRootPolicy {
    pub fn system() -> Self {
        Self {
            custom_ca_bundle: None,
        }
    }

    pub fn with_custom_ca(path: impl Into<PathBuf>) -> Result<Self, PortError> {
        let path = path.into();
        let metadata = std::fs::metadata(&path)
            .map_err(|error| PortError::new("ca_bundle", error.to_string()))?;
        if !metadata.is_file() {
            return Err(PortError::new(
                "ca_bundle",
                "custom CA bundle must be a regular file",
            ));
        }
        Ok(Self {
            custom_ca_bundle: Some(path),
        })
    }

    pub fn from_environment() -> Result<Self, PortError> {
        match std::env::var_os("HEYFOOD_CA_BUNDLE") {
            None => Ok(Self::system()),
            Some(value) => value
                .into_string()
                .map_err(|_| PortError::new("ca_bundle", "HEYFOOD_CA_BUNDLE is not valid Unicode"))
                .and_then(Self::with_custom_ca),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalKind {
    Interactive,
    Redirected,
    RemoteInteractive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TtyCapabilities {
    pub stdin: bool,
    pub stdout: bool,
    pub stderr: bool,
    pub remote_session: bool,
}

impl TtyCapabilities {
    #[must_use]
    pub fn detect() -> Self {
        Self {
            stdin: Stdin::is_terminal(&std::io::stdin()),
            stdout: Stdout::is_terminal(&std::io::stdout()),
            stderr: Stderr::is_terminal(&std::io::stderr()),
            remote_session: std::env::var_os("SSH_CONNECTION").is_some()
                || std::env::var_os("SSH_TTY").is_some(),
        }
    }

    #[must_use]
    pub const fn kind(self) -> TerminalKind {
        if self.stdin && self.stdout {
            if self.remote_session {
                TerminalKind::RemoteInteractive
            } else {
                TerminalKind::Interactive
            }
        } else {
            TerminalKind::Redirected
        }
    }

    #[must_use]
    pub const fn supports_loopback_browser_flow(self) -> bool {
        matches!(self.kind(), TerminalKind::Interactive)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignalEvent {
    Interrupt,
    Terminate,
    Hangup,
    ConsoleClose,
}

/// Native signal receiver. `heyfood-bin` owns the translation into UI actions.
pub struct NativeSignalSource {
    receiver: mpsc::Receiver<SignalEvent>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl NativeSignalSource {
    pub fn install() -> Result<Self, PortError> {
        let (sender, receiver) = mpsc::channel(8);
        let tasks = install_signal_tasks(sender)?;
        Ok(Self { receiver, tasks })
    }

    pub async fn next(&mut self) -> Option<SignalEvent> {
        self.receiver.recv().await
    }

    /// Stops and joins every native signal listener within the supplied bound.
    ///
    /// Callers that own a runtime should use this explicit lifecycle boundary
    /// instead of relying on `Drop`, which can only request cancellation.
    pub async fn shutdown(&mut self, timeout: Duration) -> Result<(), PortError> {
        for task in &self.tasks {
            task.abort();
        }
        let tasks = std::mem::take(&mut self.tasks);
        tokio::time::timeout(timeout, async move {
            for task in tasks {
                match task.await {
                    Ok(()) => {}
                    Err(error) if error.is_cancelled() => {}
                    Err(error) => {
                        return Err(PortError::new("signal_join", error.to_string()));
                    }
                }
            }
            Ok(())
        })
        .await
        .map_err(|_| PortError::new("signal_join_timeout", "signal listeners did not stop"))?
    }
}

impl Drop for NativeSignalSource {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

#[cfg(unix)]
fn install_signal_tasks(
    sender: mpsc::Sender<SignalEvent>,
) -> Result<Vec<tokio::task::JoinHandle<()>>, PortError> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut tasks = Vec::new();
    for (kind, event) in [
        (SignalKind::interrupt(), SignalEvent::Interrupt),
        (SignalKind::terminate(), SignalEvent::Terminate),
        (SignalKind::hangup(), SignalEvent::Hangup),
    ] {
        let mut stream =
            signal(kind).map_err(|error| PortError::new("signal_install", error.to_string()))?;
        let sender = sender.clone();
        tasks.push(tokio::spawn(async move {
            while stream.recv().await.is_some() {
                if sender.send(event).await.is_err() {
                    break;
                }
            }
        }));
    }
    Ok(tasks)
}

#[cfg(windows)]
fn install_signal_tasks(
    sender: mpsc::Sender<SignalEvent>,
) -> Result<Vec<tokio::task::JoinHandle<()>>, PortError> {
    use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close, ctrl_logoff, ctrl_shutdown};

    let mut ctrl_c_stream =
        ctrl_c().map_err(|error| PortError::new("signal_install", error.to_string()))?;
    let mut ctrl_break_stream =
        ctrl_break().map_err(|error| PortError::new("signal_install", error.to_string()))?;
    let mut ctrl_close_stream =
        ctrl_close().map_err(|error| PortError::new("signal_install", error.to_string()))?;
    let mut ctrl_logoff_stream =
        ctrl_logoff().map_err(|error| PortError::new("signal_install", error.to_string()))?;
    let mut ctrl_shutdown_stream =
        ctrl_shutdown().map_err(|error| PortError::new("signal_install", error.to_string()))?;
    let mut tasks = Vec::new();
    let interrupt_sender = sender.clone();
    tasks.push(tokio::spawn(async move {
        loop {
            tokio::select! {
                value = ctrl_c_stream.recv() => if value.is_none() { break; },
                value = ctrl_break_stream.recv() => if value.is_none() { break; },
            }
            if interrupt_sender.send(SignalEvent::Interrupt).await.is_err() {
                break;
            }
        }
    }));
    tasks.push(tokio::spawn(async move {
        loop {
            tokio::select! {
                value = ctrl_close_stream.recv() => if value.is_none() { break; },
                value = ctrl_logoff_stream.recv() => if value.is_none() { break; },
                value = ctrl_shutdown_stream.recv() => if value.is_none() { break; },
            }
            if sender.send(SignalEvent::ConsoleClose).await.is_err() {
                break;
            }
        }
    }));
    Ok(tasks)
}

#[cfg(not(any(unix, windows)))]
fn install_signal_tasks(
    _sender: mpsc::Sender<SignalEvent>,
) -> Result<Vec<tokio::task::JoinHandle<()>>, PortError> {
    Err(PortError::new(
        "signal_unsupported",
        "native signals are not supported on this platform",
    ))
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeClock;

impl ClockPort for NativeClock {
    fn unix_timestamp(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| {
                i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
            })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeBrowser;

impl BrowserPort for NativeBrowser {
    fn open(&self, url: BrowserUrl) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move {
            webbrowser::open(url.as_url().as_str())
                .map_err(|error| PortError::new("browser_open", error.to_string()))
        })
    }
}
