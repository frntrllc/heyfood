//! Dependency-light domain and wire contracts for heyfood.

#![forbid(unsafe_code)]

pub mod agent;
pub mod auth;
pub mod config;
pub mod network;
pub mod operation;

pub use agent::{AgentChoice, AgentEvent, AgentFailure};
pub use auth::{
    AccountId, CredentialVersion, RefreshRequest, RefreshResult, SensitiveString,
    SessionCredentials, SessionSnapshot,
};
pub use config::{ClientConfig, ConfigRevision};
pub use network::{BrowserUrl, NetworkPolicy, ServiceUrl, ServiceUrlError};
pub use operation::{CommitId, GenerationId, OperationId};

/// The package version shared by the native workspace.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
