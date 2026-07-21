//! Dependency-light domain and wire contracts for heyfood.

#![forbid(unsafe_code)]

pub mod agent;
pub mod auth;
pub mod config;
pub mod error;
pub mod grocery;
pub mod health;
pub mod migration;
pub mod network;
pub mod operation;
pub mod presentation;
pub mod validation;

pub use agent::{AgentChoice, AgentEvent, AgentFailure};
pub use auth::{
    AccountId, AuthCapabilities, AuthCredentialBundle, AuthorizationCapability, ChannelCredentials,
    CredentialVersion, IdentityMethod, ProfileStatus, RefreshOutcome, RefreshRequest,
    RefreshResult, RegistrationStatus, SelfRegistrationCapability, SensitiveString,
    SessionCredentials, SessionSnapshot,
};
pub use config::{CURRENT_CONFIG_SCHEMA, ClientConfig, ConfigRevision, ConfigSchemaVersion};
pub use error::{ClientError, ErrorCategory, ErrorCode};
pub use grocery::{
    ContextFingerprint, FrozenGroceryPreconditions, GroceryCapability, GroceryConfirmation,
    GroceryConfirmationCommand, GroceryConfirmationDecision, GroceryConfirmationId,
    GroceryConfirmationState, GroceryEditPatch, GroceryEntityId, GroceryError, GroceryErrorCode,
    GroceryIdempotencyKey, GroceryListVersion, GrocerySafetyStatus, HouseholdContextHashVersion,
};
pub use health::{
    HealthCapability, HealthConnectionStatus, HealthFreshness, HealthFreshnessStatus, HealthMetric,
    HealthProvider, HealthTrend, TrendDirection,
};
pub use migration::{
    ImportedPythonState, PythonFieldAction, PythonFieldDisposition, PythonImportOutcome,
    PythonImportReport,
};
pub use network::{BrowserUrl, NetworkPolicy, ProxyUrl, ServiceUrl, ServiceUrlError};
pub use operation::{CommitId, GenerationId, OperationId};
pub use presentation::{
    NoticeLevel, PresentationBlock, PresentationDocument, PresentationError, PresentationText,
};
pub use validation::{
    ValidationError, bounded_integer, bounded_number, choice, coordinates, iso_date, optional_text,
    required_text, terminal_safe_text, validate_identifier,
};

/// The package version shared by the native workspace.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
