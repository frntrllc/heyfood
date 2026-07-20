//! Versioned, renderer-neutral state-migration contracts.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Result of attempting the one-time Python local-state import.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PythonImportOutcome {
    NoSource,
    Imported,
    AlreadyImported,
}

/// A content-free disposition for one legacy top-level field.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PythonFieldAction {
    Imported,
    ReauthenticationRequired,
    BlockedUnbound,
    Unsupported,
    KeyringNotRead,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PythonFieldDisposition {
    pub field: String,
    pub action: PythonFieldAction,
    pub reason_code: String,
}

/// Supported local state separated by account binding.
///
/// Values may contain household and dietary data. The custom `Debug`
/// implementation intentionally exposes field names and counts only.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedPythonState {
    pub account_user_id: Option<String>,
    pub global: BTreeMap<String, Value>,
    pub account_scoped: BTreeMap<String, Value>,
}

impl fmt::Debug for ImportedPythonState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImportedPythonState")
            .field("account_bound", &self.account_user_id.is_some())
            .field("global_fields", &self.global.keys().collect::<Vec<_>>())
            .field(
                "account_scoped_fields",
                &self.account_scoped.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Redacted report returned by the migration adapter and suitable for JSON
/// diagnostics. Imported values are retrieved through a separate trusted
/// state-loading path and never appear here.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PythonImportReport {
    pub outcome: PythonImportOutcome,
    pub source_sha256: Option<String>,
    pub reauthentication_required: bool,
    pub requires_manual_action: bool,
    pub dispositions: Vec<PythonFieldDisposition>,
}

impl PythonImportReport {
    #[must_use]
    pub fn no_source() -> Self {
        Self {
            outcome: PythonImportOutcome::NoSource,
            source_sha256: None,
            reauthentication_required: false,
            requires_manual_action: false,
            dispositions: Vec::new(),
        }
    }
}
