use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use directories::BaseDirs;
use heyfood_application::PortError;
use heyfood_core::{
    ImportedPythonState, NetworkPolicy, PythonFieldAction, PythonFieldDisposition,
    PythonImportOutcome, PythonImportReport, ServiceUrl,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::NativePaths;
use crate::persistence::{AtomicFile, FileLock, create_private_dir};

const MAXIMUM_SOURCE_BYTES: u64 = 4 * 1024 * 1024;
const IMPORT_SCHEMA_VERSION: u64 = 1;
const IMPORT_FILE_NAME: &str = "python-state-import.v1.json";
const IMPORT_LOCK_NAME: &str = "python-state-import.lock";

const GLOBAL_FIELDS: &[&str] = &[
    "active_context",
    "api_url",
    "auth_url",
    "contexts",
    "device_id",
    "voice",
];
const ACCOUNT_STRING_FIELDS: &[&str] = &["first_name", "first_name_updated_at", "welcomed_at"];
const ACCOUNT_OBJECT_FIELDS: &[&str] = &[
    "household",
    "household_local_profiles",
    "household_profile_outbox",
    "last_conversation",
    "last_recipe_search",
    "last_restaurant_search",
    "location",
];
const CREDENTIAL_FIELDS: &[&str] = &[
    "api_key",
    "credential_api_url",
    "credential_store",
    "oauth",
    "session",
];

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportDocument {
    schema_version: u64,
    source_format: String,
    report: PythonImportReport,
    state: ImportedPythonState,
}

/// Read-only, one-time importer for the final Python client's local config.
///
/// The source is never opened for writing and keyring entries are never read.
/// Credential material is deliberately omitted; callers receive an explicit
/// reauthentication disposition instead. Imported state is written atomically
/// into the private native directory and a different source cannot overwrite a
/// completed import.
pub struct PythonStateImporter {
    source_path: PathBuf,
    destination_root: PathBuf,
}

impl PythonStateImporter {
    #[must_use]
    pub fn under(source_path: impl Into<PathBuf>, destination_root: impl Into<PathBuf>) -> Self {
        Self {
            source_path: source_path.into(),
            destination_root: destination_root.into(),
        }
    }

    pub fn discover(native_paths: &NativePaths) -> Result<Self, PortError> {
        let config_root = match std::env::var_os("XDG_CONFIG_HOME") {
            Some(path) => PathBuf::from(path),
            None => BaseDirs::new()
                .map(|dirs| dirs.home_dir().join(".config"))
                .ok_or_else(|| {
                    PortError::new(
                        "python_import_paths",
                        "legacy Python configuration directory is unavailable",
                    )
                })?,
        };
        let current = config_root.join("heyfood").join("config.json");
        let legacy = config_root.join("hellofood").join("config.json");
        let source = if current.exists() || !legacy.exists() {
            current
        } else {
            legacy
        };
        Ok(Self::under(source, native_paths.root()))
    }

    #[must_use]
    pub fn destination_path(&self) -> PathBuf {
        self.destination_root.join(IMPORT_FILE_NAME)
    }

    pub fn import(&self) -> Result<PythonImportReport, PortError> {
        validate_destination_root(&self.destination_root)?;
        create_private_dir(&self.destination_root)?;
        let _lock = FileLock::acquire(&self.destination_root.join(IMPORT_LOCK_NAME), true)?;
        let destination = self.destination_path();
        let source = read_source(&self.source_path)?;

        let Some(source) = source else {
            return match read_document_if_present(&destination)? {
                Some(mut document) => {
                    document.report.outcome = PythonImportOutcome::AlreadyImported;
                    Ok(document.report)
                }
                None => Ok(PythonImportReport::no_source()),
            };
        };

        ensure_private_import_file_supported()?;
        let source_sha256 = sha256(&source);

        if let Some(mut document) = read_document_if_present(&destination)? {
            if document.report.source_sha256.as_deref() != Some(source_sha256.as_str()) {
                return Err(PortError::new(
                    "python_import_conflict",
                    "a different Python state source has already been imported",
                ));
            }
            document.report.outcome = PythonImportOutcome::AlreadyImported;
            return Ok(document.report);
        }

        let (report, state) = build_import(&source, source_sha256)?;
        let document = ImportDocument {
            schema_version: IMPORT_SCHEMA_VERSION,
            source_format: "heyfood-python-config-v0.3.2-compatible".to_owned(),
            report: report.clone(),
            state,
        };
        let mut encoded = serde_json::to_vec_pretty(&document).map_err(|_| {
            PortError::new("python_import_encode", "could not encode native import")
        })?;
        encoded.push(b'\n');
        AtomicFile::replace(&destination, &encoded)?;
        Ok(report)
    }

    /// Load imported values for trusted native migration/application code.
    /// Diagnostics should use [`Self::import`] and its redacted report instead.
    pub fn load_state(&self) -> Result<Option<ImportedPythonState>, PortError> {
        validate_destination_root(&self.destination_root)?;
        create_private_dir(&self.destination_root)?;
        let _lock = FileLock::acquire(&self.destination_root.join(IMPORT_LOCK_NAME), false)?;
        Ok(read_document_if_present(&self.destination_path())?.map(|document| document.state))
    }
}

fn read_source(path: &Path) -> Result<Option<Vec<u8>>, PortError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(PortError::new(
                "python_import_read",
                "could not inspect the Python state source",
            ));
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(PortError::new(
            "python_import_symlink",
            "the Python state source must not be a symbolic link",
        ));
    }
    if !metadata.is_file() {
        return Err(PortError::new(
            "python_import_type",
            "the Python state source must be a regular file",
        ));
    }
    if metadata.len() > MAXIMUM_SOURCE_BYTES {
        return Err(PortError::new(
            "python_import_size",
            "the Python state source exceeds the migration size limit",
        ));
    }
    let file = File::open(path).map_err(|_| {
        PortError::new(
            "python_import_read",
            "could not open the Python state source",
        )
    })?;
    let opened_metadata = file.metadata().map_err(|_| {
        PortError::new(
            "python_import_read",
            "could not inspect the opened Python state source",
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.dev() != opened_metadata.dev() || metadata.ino() != opened_metadata.ino() {
            return Err(PortError::new(
                "python_import_source_changed",
                "the Python state source changed while it was being opened",
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = opened_metadata;
    let mut bytes = Vec::new();
    file.take(MAXIMUM_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| {
            PortError::new(
                "python_import_read",
                "could not read the Python state source",
            )
        })?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAXIMUM_SOURCE_BYTES {
        return Err(PortError::new(
            "python_import_size",
            "the Python state source exceeds the migration size limit",
        ));
    }
    Ok(Some(bytes))
}

fn read_document_if_present(path: &Path) -> Result<Option<ImportDocument>, PortError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(PortError::new(
                "python_import_native_read",
                "could not inspect native import",
            ));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(PortError::new(
            "python_import_native_type",
            "native import must be a regular non-symlink file",
        ));
    }
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(MAXIMUM_SOURCE_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|_| PortError::new("python_import_native_read", "could not read native import"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAXIMUM_SOURCE_BYTES {
        return Err(PortError::new(
            "python_import_native_size",
            "native import exceeds its size limit",
        ));
    }
    let document: ImportDocument = serde_json::from_slice(&bytes).map_err(|_| {
        PortError::new(
            "python_import_native_format",
            "native import is invalid JSON",
        )
    })?;
    if document.schema_version != IMPORT_SCHEMA_VERSION
        || document.source_format != "heyfood-python-config-v0.3.2-compatible"
    {
        return Err(PortError::new(
            "python_import_native_schema",
            "native import has an unsupported schema",
        ));
    }
    if document.report.outcome != PythonImportOutcome::Imported
        || !document.report.reauthentication_required
        || !document
            .report
            .source_sha256
            .as_deref()
            .is_some_and(valid_sha256)
    {
        return Err(PortError::new(
            "python_import_native_schema",
            "native import contains invalid provenance or migration state",
        ));
    }
    Ok(Some(document))
}

fn validate_destination_root(path: &Path) -> Result<(), PortError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(PortError::new(
            "python_import_destination_symlink",
            "native import directory must not be a symbolic link",
        )),
        Ok(metadata) if !metadata.is_dir() => Err(PortError::new(
            "python_import_destination_type",
            "native import destination must be a directory",
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(PortError::new(
            "python_import_destination",
            "could not inspect native import destination",
        )),
    }
}

#[cfg(not(windows))]
fn ensure_private_import_file_supported() -> Result<(), PortError> {
    Ok(())
}

#[cfg(windows)]
fn ensure_private_import_file_supported() -> Result<(), PortError> {
    Err(PortError::new(
        "python_import_acl_unsupported",
        "Python state import is disabled until a private Windows ACL adapter is available",
    ))
}

fn build_import(
    source: &[u8],
    source_sha256: String,
) -> Result<(PythonImportReport, ImportedPythonState), PortError> {
    let value: Value = serde_json::from_slice(source).map_err(|_| {
        PortError::new(
            "python_import_format",
            "Python state is not a valid JSON document",
        )
    })?;
    let object = value.as_object().ok_or_else(|| {
        PortError::new("python_import_format", "Python state must be a JSON object")
    })?;
    let (account_user_id, account_source_valid) = account_binding(object);
    let mut state = ImportedPythonState {
        account_user_id,
        global: BTreeMap::new(),
        account_scoped: BTreeMap::new(),
    };
    let mut dispositions = Vec::new();
    let mut requires_manual_action = !account_source_valid;

    for (field, value) in object {
        let disposition = if field == "account_user_id" {
            if nonempty_string(value).is_some() {
                disposition(
                    field,
                    PythonFieldAction::Imported,
                    "account_binding_preserved",
                )
            } else {
                requires_manual_action = true;
                disposition(
                    field,
                    PythonFieldAction::Unsupported,
                    "invalid_account_binding",
                )
            }
        } else if GLOBAL_FIELDS.contains(&field.as_str()) {
            if validate_global_field(field, value) {
                state.global.insert(field.clone(), value.clone());
                disposition(field, PythonFieldAction::Imported, "supported_global_state")
            } else {
                requires_manual_action = true;
                disposition(field, PythonFieldAction::Unsupported, "invalid_field_shape")
            }
        } else if ACCOUNT_STRING_FIELDS.contains(&field.as_str())
            || ACCOUNT_OBJECT_FIELDS.contains(&field.as_str())
        {
            if state.account_user_id.is_none() {
                requires_manual_action = true;
                disposition(
                    field,
                    PythonFieldAction::BlockedUnbound,
                    "account_binding_required",
                )
            } else if validate_account_field(field, value) {
                state.account_scoped.insert(field.clone(), value.clone());
                disposition(
                    field,
                    PythonFieldAction::Imported,
                    "supported_account_state",
                )
            } else {
                requires_manual_action = true;
                disposition(field, PythonFieldAction::Unsupported, "invalid_field_shape")
            }
        } else if field == "credential_store" && value.as_str() == Some("keyring") {
            requires_manual_action = true;
            disposition(
                field,
                PythonFieldAction::KeyringNotRead,
                "python_keyring_not_accessed",
            )
        } else if CREDENTIAL_FIELDS.contains(&field.as_str()) {
            disposition(
                field,
                PythonFieldAction::ReauthenticationRequired,
                "credential_migration_not_attempted",
            )
        } else {
            requires_manual_action = true;
            disposition(
                field,
                PythonFieldAction::Unsupported,
                "unsupported_top_level_field",
            )
        };
        dispositions.push(disposition);
    }
    dispositions.push(disposition(
        "credentials",
        PythonFieldAction::ReauthenticationRequired,
        "fresh_native_login_required",
    ));
    dispositions.sort_by(|left, right| left.field.cmp(&right.field));
    let report = PythonImportReport {
        outcome: PythonImportOutcome::Imported,
        source_sha256: Some(source_sha256),
        reauthentication_required: true,
        requires_manual_action,
        dispositions,
    };
    Ok((report, state))
}

fn account_binding(object: &Map<String, Value>) -> (Option<String>, bool) {
    if let Some(value) = object.get("account_user_id") {
        return match nonempty_string(value) {
            Some(value) => (Some(value.to_owned()), true),
            None => (session_account(object), false),
        };
    }
    (session_account(object), true)
}

fn session_account(object: &Map<String, Value>) -> Option<String> {
    object
        .get("session")
        .and_then(Value::as_object)
        .and_then(|session| session.get("user_id"))
        .and_then(nonempty_string)
        .map(str::to_owned)
}

fn nonempty_string(value: &Value) -> Option<&str> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn validate_global_field(field: &str, value: &Value) -> bool {
    match field {
        "active_context" | "device_id" => nonempty_string(value).is_some(),
        "api_url" | "auth_url" => value.as_str().is_some_and(valid_service_url),
        "contexts" => validate_contexts(value),
        "voice" => value.is_object(),
        _ => false,
    }
}

fn validate_contexts(value: &Value) -> bool {
    let Some(contexts) = value.as_object() else {
        return false;
    };
    contexts.values().all(|context| {
        let Some(context) = context.as_object() else {
            return false;
        };
        ["api_url", "auth_url"].into_iter().all(|field| {
            context
                .get(field)
                .and_then(Value::as_str)
                .is_some_and(valid_service_url)
        })
    })
}

fn valid_service_url(value: &str) -> bool {
    ServiceUrl::parse(value, NetworkPolicy::DEVELOPMENT).is_ok()
}

fn validate_account_field(field: &str, value: &Value) -> bool {
    if ACCOUNT_STRING_FIELDS.contains(&field) {
        nonempty_string(value).is_some()
    } else {
        ACCOUNT_OBJECT_FIELDS.contains(&field) && value.is_object()
    }
}

fn disposition(
    field: &str,
    action: PythonFieldAction,
    reason_code: &str,
) -> PythonFieldDisposition {
    PythonFieldDisposition {
        field: field.to_owned(),
        action,
        reason_code: reason_code.to_owned(),
    }
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
