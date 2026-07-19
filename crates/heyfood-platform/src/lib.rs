//! Native operating-system adapters used by the Rust vertical proof.

#![forbid(unsafe_code)]

mod persistence;

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use directories::ProjectDirs;
use heyfood_application::{BoxFuture, BrowserPort, ClockPort, PortError};
use heyfood_core::{BrowserUrl, NetworkPolicy, ServiceUrl};
use tokio::sync::mpsc;

#[cfg(all(windows, feature = "native-credentials"))]
pub use persistence::WindowsCredentialStore;
pub use persistence::{AtomicFile, FileCredentialStore, NativeConfigStore};

/// The package version shared by the native workspace.
pub const VERSION: &str = heyfood_core::VERSION;

/// All Phase 0 files are rooted together so controlled fixtures never touch a
/// user's real profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativePaths {
    root: PathBuf,
}

impl NativePaths {
    #[must_use]
    pub fn under(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn discover() -> Result<Self, PortError> {
        let project = ProjectDirs::from("ai", "frntr", "heyfood").ok_or_else(|| {
            PortError::new(
                "platform_paths",
                "native configuration directory is unavailable",
            )
        })?;
        Ok(Self::under(project.config_dir()))
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
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
