use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fmt;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use directories::BaseDirs;
use heyfood_application::{
    HouseholdInitialize, HouseholdLoad, HouseholdRepositoryResolutionV1, PortError,
    resolve_household_initialize_v1,
};
use heyfood_core::{
    AccountId, AgeEvidenceSourceV1, AgeEvidenceV1, AppliedCommitOutcomeV1, CanonicalDateV1,
    CanonicalDigestV1, CanonicalJsonValueV1, CanonicalTimestampV1, CommitId,
    CompatibilityJsonLimitsV1, DateOfBirthV1, DisplayName, HOUSEHOLD_STATE_SCHEMA_VERSION,
    HouseholdEffectFingerprintV1, HouseholdEffectV1, HouseholdLifecycleV1, HouseholdMemberV1,
    HouseholdOutboxRecordV1, HouseholdOwnerV1, HouseholdProfileDocumentV1,
    HouseholdProfileOutboxEntryV1, HouseholdProfileRecordV1, HouseholdProfileStateV1,
    HouseholdRevision, HouseholdScope, HouseholdStateV1, HouseholdSubjectId,
    ImportedCompatibilityFieldV1, ImportedCompatibilityStateV1, ImportedPythonState,
    LegacyOutboxSourceKindV1, LegacyPythonSnapshotProvenanceV1, LegacyRemoteProfileReferenceV1,
    LegacySourceIdentityV1, LegacyTimestampDispositionV1, LegacyTimestampRecordV1,
    MAX_HOUSEHOLD_MEMBERS, MAX_HOUSEHOLD_OUTBOX_ENTRIES, MAX_HOUSEHOLD_PROFILES,
    MAX_IMPORTED_COMPATIBILITY_FIELDS, MAX_LEGACY_APPLIED_MUTATION_IDS,
    MAX_MIGRATION_CANDIDATE_BYTES, MAX_MIGRATION_DISPOSITIONS, MemberId,
    MigrationDispositionKindV1, MigrationDispositionManifestV1, MigrationDispositionV1,
    MigrationProvenanceV1, NetworkPolicy, OutboxRevision, ProfileRevision, PythonFieldAction,
    PythonFieldDisposition, PythonImportOutcome, PythonImportReport, RelationshipSourceV1,
    RelationshipV1, ServiceUrl, canonical_sha256_v1, canonicalize_json_value_v1,
    classify_legacy_outbox_v1, derive_minor_status_v1, domain_hash_v1, encode_lower_hex,
    normalize_legacy_timestamp_v1, parse_bounded_json_object_v1,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::NativePaths;
use crate::credential_broker::{
    HouseholdBrokerOperationV1, HouseholdMigrationGuardDocument, HouseholdMigrationGuardStateV1,
    HouseholdMigrationInitializationPhaseV1, HouseholdMigrationPresentSourceKindV1,
    HouseholdMigrationSourceIdentityV1,
};
#[cfg(unix)]
use crate::credential_broker::{LEGACY_PYTHON_KEYRING_SERVICE, LegacyPythonKeyringLocatorV1};
use crate::household_vault::{
    AcquiredNarrowerVaultLease, HouseholdAccountSlotV1, HouseholdLifecycleLease, HouseholdVault,
    HouseholdVaultLease, HouseholdVaultLeaseModeV1,
};
use crate::persistence::{AtomicFile, FileLock, create_private_dir};

#[cfg(test)]
thread_local! {
    static MIXED_SOURCE_READ_PROBE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

const MAXIMUM_SOURCE_BYTES: u64 = 4 * 1024 * 1024;
const IMPORT_SCHEMA_VERSION: u64 = 1;
const IMPORT_FILE_NAME: &str = "python-state-import.v1.json";
const IMPORT_LOCK_NAME: &str = "python-state-import.lock";
const IMPORT_SOURCE_FORMAT: &str = "heyfood-python-config-v0.3.2-compatible";
const NORMALIZED_STATE_DOMAIN: &[u8] = b"heyfood.log.python-state.v1";
const SOURCE_SET_DOMAIN: &[u8] = b"heyfood.log.python-source-set.v2";
const SELECTED_LOCATOR_DOMAIN: &[u8] = b"heyfood.log.python-selected-locator.v2";
const D2_SOURCE_BUNDLE_KIND: &str = "legacy_python_source_bundle_v1";
const D2_SOURCE_MANIFEST_CONTRACT: &str = "heyfood.household.legacy-source-bundle.v1";
const D2_SOURCE_SET_CONTRACT: &str = "heyfood.household.legacy-source-set.v1";
const D2_CURRENT_CONFIG_KIND: &str = "legacy_python_config_current_v1";
const D2_LEGACY_CONFIG_KIND: &str = "legacy_python_config_legacy_v1";
const D2_CURRENT_KEYRING_KIND: &str = "legacy_python_keyring_current_v1";
const D2_LEGACY_KEYRING_KIND: &str = "legacy_python_keyring_legacy_v1";
const D2_SNAPSHOT_KIND: &str = "native_import_snapshot_v1";
const D2_KEYRING_HOUSEHOLD_STATE: &str = "household.state";
const D2_KEYRING_LOCAL_PROFILES: &str = "household.local_profiles";
const D2_KEYRING_PROFILE_OUTBOX: &str = "household.profile_outbox";
const D2_KEYRING_EVIDENCE_CONTRACT: &str = "heyfood.household.legacy-keyring-evidence.v1";
const D2_DESTINATION_FRAGMENT_CONTRACT: &str =
    "heyfood.household.migration-destination-fragment.v1";
#[cfg(any(unix, test))]
const UNIX_FILE_ID_DOMAIN: &[u8] = b"heyfood.log.unix-file-id.v1";
#[cfg(any(windows, test))]
const WINDOWS_FILE_ID_DOMAIN: &[u8] = b"heyfood.log.windows-file-id.v1";

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

/// The two frozen Python configuration locators, resolved with
/// `Path.resolve(strict=False)` parity before any historical keyring locator is
/// derived.
#[derive(Clone, Eq, PartialEq)]
pub struct LegacyPythonConfigRootV1 {
    requested_root: PathBuf,
    resolved_root: PathBuf,
    current_config: PathBuf,
    legacy_config: PathBuf,
}

impl LegacyPythonConfigRootV1 {
    /// Resolve the legacy Python root without consulting the process working
    /// directory. An unset or empty XDG value selects `<home>/.config`; a
    /// nonempty relative value is ambiguous and is never converted into a
    /// no-source observation.
    pub fn from_environment_values(
        xdg_config_home: Option<&OsStr>,
        home: Option<&Path>,
    ) -> Result<Self, PortError> {
        let requested_root = match xdg_config_home {
            Some(value) if !value.is_empty() => {
                let value = PathBuf::from(value);
                if !value.is_absolute() {
                    return Err(legacy_root_ambiguous());
                }
                value
            }
            _ => home
                .filter(|value| value.is_absolute())
                .map(|value| value.join(".config"))
                .ok_or_else(legacy_root_ambiguous)?,
        };
        Self::from_absolute_root(requested_root)
    }

    pub fn from_absolute_root(root: impl Into<PathBuf>) -> Result<Self, PortError> {
        let requested_root = root.into();
        if !requested_root.is_absolute() {
            return Err(legacy_root_ambiguous());
        }
        let resolved_root = resolve_strict_false(&requested_root)?;
        if !resolved_root.is_absolute() {
            return Err(legacy_root_ambiguous());
        }
        let current_config = resolved_root.join("heyfood").join("config.json");
        let legacy_config = resolved_root.join("hellofood").join("config.json");
        Ok(Self {
            requested_root,
            resolved_root,
            current_config,
            legacy_config,
        })
    }

    fn revalidate(&self) -> Result<(), PortError> {
        let current = resolve_strict_false(&self.requested_root)?;
        if current != self.resolved_root {
            return Err(legacy_root_ambiguous());
        }
        Ok(())
    }

    #[must_use]
    pub fn config_path(&self, kind: LegacyPythonConfigKindV1) -> &Path {
        match kind {
            LegacyPythonConfigKindV1::Current => &self.current_config,
            LegacyPythonConfigKindV1::Legacy => &self.legacy_config,
        }
    }
}

impl fmt::Debug for LegacyPythonConfigRootV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LegacyPythonConfigRootV1")
            .field("resolved", &true)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyPythonConfigKindV1 {
    Current,
    Legacy,
}

impl LegacyPythonConfigKindV1 {
    const fn source_kind(self) -> &'static str {
        match self {
            Self::Current => D2_CURRENT_CONFIG_KIND,
            Self::Legacy => D2_LEGACY_CONFIG_KIND,
        }
    }

    const fn keyring_kind(self) -> &'static str {
        match self {
            Self::Current => D2_CURRENT_KEYRING_KIND,
            Self::Legacy => D2_LEGACY_KEYRING_KIND,
        }
    }
}

/// Purpose-limited result returned by the credential broker for one historical
/// Python keyring target.
///
/// This is deliberately only an outcome. A migration cannot consume it until
/// [`LegacyPythonKeyringProbeSetV1::bind`] has attached and digested the exact
/// account slot, historical locator, broker operation, and response bytes.
#[derive(Clone, Eq, PartialEq)]
pub enum LegacyPythonKeyringProbeOutcomeV1 {
    AuthoritativeMissing,
    Present(Vec<u8>),
    Unavailable,
}

impl fmt::Debug for LegacyPythonKeyringProbeOutcomeV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::AuthoritativeMissing => "LegacyPythonKeyringProbeOutcomeV1::AuthoritativeMissing",
            Self::Present(_) => "LegacyPythonKeyringProbeOutcomeV1::Present([REDACTED])",
            Self::Unavailable => "LegacyPythonKeyringProbeOutcomeV1::Unavailable",
        })
    }
}

#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct LegacyPythonKeyringEvidenceBindingV1 {
    contract: &'static str,
    config_kind: LegacyPythonConfigKindV1,
    operation: &'static str,
    account_digest: String,
    native_root_instance_digest: String,
    account_locator_digest: String,
    legacy_locator_digest: String,
    outcome: &'static str,
    payload_digest: Option<String>,
}

#[derive(Clone, Eq, PartialEq)]
pub struct LegacyPythonKeyringProbeV1 {
    binding: LegacyPythonKeyringEvidenceBindingV1,
    evidence_digest: CanonicalDigestV1,
    outcome: LegacyPythonKeyringProbeOutcomeV1,
}

impl fmt::Debug for LegacyPythonKeyringProbeV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LegacyPythonKeyringProbeV1")
            .field("config_kind", &self.binding.config_kind)
            .field("operation", &self.binding.operation)
            .field("outcome", &self.binding.outcome)
            .field("evidence_digest", &self.evidence_digest)
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct LegacyPythonKeyringProbeSetV1 {
    current: LegacyPythonKeyringProbeV1,
    legacy: LegacyPythonKeyringProbeV1,
}

impl LegacyPythonKeyringProbeSetV1 {
    /// Bind two independently obtained broker outcomes to this exact account
    /// slot and the two frozen historical Python keyring locators.
    pub fn bind(
        account_slot: &HouseholdAccountSlotV1,
        config_root: &LegacyPythonConfigRootV1,
        current: LegacyPythonKeyringProbeOutcomeV1,
        legacy: LegacyPythonKeyringProbeOutcomeV1,
    ) -> Result<Self, PortError> {
        Ok(Self {
            current: bind_keyring_probe(
                LegacyPythonConfigKindV1::Current,
                account_slot,
                config_root,
                current,
            )?,
            legacy: bind_keyring_probe(
                LegacyPythonConfigKindV1::Legacy,
                account_slot,
                config_root,
                legacy,
            )?,
        })
    }

    pub fn authoritative_missing(
        account_slot: &HouseholdAccountSlotV1,
        config_root: &LegacyPythonConfigRootV1,
    ) -> Result<Self, PortError> {
        Self::bind(
            account_slot,
            config_root,
            LegacyPythonKeyringProbeOutcomeV1::AuthoritativeMissing,
            LegacyPythonKeyringProbeOutcomeV1::AuthoritativeMissing,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum LegacyCredentialStoreV1 {
    File,
    Keyring,
}

#[derive(Clone, Eq, PartialEq)]
struct StrictKeyringDocumentV1 {
    document_digest: CanonicalDigestV1,
    household: Option<Map<String, Value>>,
    local_profiles: Option<Map<String, Value>>,
    profile_outbox: Option<Map<String, Value>>,
}

impl StrictKeyringDocumentV1 {
    fn has_household_data(&self) -> bool {
        self.household.is_some() || self.local_profiles.is_some() || self.profile_outbox.is_some()
    }
}

#[derive(Clone, Eq, PartialEq)]
struct StrictConfigDocumentV1 {
    kind: LegacyPythonConfigKindV1,
    bytes_digest: CanonicalDigestV1,
    bytes: Vec<u8>,
    object: Map<String, Value>,
}

impl StrictConfigDocumentV1 {
    fn has_household_data(&self) -> bool {
        self.object.contains_key("household")
            || self.object.contains_key("household_local_profiles")
            || self.object.contains_key("household_profile_outbox")
    }
}

#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct LegacySourceBundleManifestV1 {
    contract: &'static str,
    account_digest: String,
    native_root_instance_digest: String,
    account_locator_digest: String,
    selected_locator_kind: &'static str,
    selected_locator_digest: String,
    config_file_digest: String,
    matching_snapshot_digest: Option<String>,
    matching_snapshot_locator_digest: String,
    matching_snapshot_normalized_state_digest: Option<String>,
    credential_store: LegacyCredentialStoreV1,
    household_digest: Option<String>,
    local_profiles_digest: Option<String>,
    profile_outbox_digest: Option<String>,
    ignored_sources: Vec<LegacyIgnoredSourceV1>,
}

#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct LegacyIgnoredSourceV1 {
    kind: &'static str,
    locator_digest: String,
    state: &'static str,
    content_digest: Option<String>,
}

#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct LegacyNoSourceProbeV1 {
    kind: &'static str,
    locator_digest: String,
    state: &'static str,
    evidence_digest: Option<String>,
    content_digest: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LegacyDocumentSourceV1 {
    File,
    Keyring,
}

#[derive(Clone, Eq, PartialEq)]
struct CredentialFreeLegacyObjectV1 {
    object: Map<String, Value>,
}

impl CredentialFreeLegacyObjectV1 {
    fn new(object: Map<String, Value>) -> Result<Self, PortError> {
        if contains_prohibited_credential_field(&Value::Object(object.clone())) {
            return Err(PortError::new(
                "legacy_python_credential_material",
                "legacy household candidate contains credential-shaped material and cannot be retained for migration",
            ));
        }
        Ok(Self { object })
    }

    const fn as_map(&self) -> &Map<String, Value> {
        &self.object
    }
}

impl fmt::Debug for CredentialFreeLegacyObjectV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialFreeLegacyObjectV1")
            .field("field_count", &self.object.len())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LegacyConfigFieldRoleV1 {
    AccountBinding,
    OwnerName,
    CompatibilityMigrated,
    CompatibilityRetired,
    Household,
    LocalProfiles,
    ProfileOutbox,
    CredentialMarker,
    Credential,
    Unknown,
}

#[derive(Clone, Eq, PartialEq)]
struct LegacyConfigFieldEvidenceV1 {
    field_name: String,
    role: LegacyConfigFieldRoleV1,
    source_digest: Option<CanonicalDigestV1>,
}

#[derive(Clone, Eq, PartialEq)]
struct LegacyBoundDocumentV1 {
    value: CredentialFreeLegacyObjectV1,
    source: LegacyDocumentSourceV1,
    source_digest: CanonicalDigestV1,
}

#[derive(Clone, Eq, PartialEq)]
pub struct LegacyPythonPresentCandidateV1 {
    account: AccountId,
    selected_kind: LegacyPythonConfigKindV1,
    credential_store: LegacyCredentialStoreV1,
    source_digest: CanonicalDigestV1,
    snapshot_evidence: Option<LegacyPythonSnapshotEvidenceV1>,
    first_name: Option<CanonicalJsonValueV1>,
    compatibility_fields: BTreeMap<String, CanonicalJsonValueV1>,
    config_field_evidence: Vec<LegacyConfigFieldEvidenceV1>,
    household: Option<LegacyBoundDocumentV1>,
    local_profiles: Option<LegacyBoundDocumentV1>,
    profile_outbox: Option<LegacyBoundDocumentV1>,
}

impl fmt::Debug for LegacyPythonPresentCandidateV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LegacyPythonPresentCandidateV1")
            .field("selected_kind", &self.selected_kind)
            .field("source_digest", &self.source_digest)
            .field("config_field_count", &self.config_field_evidence.len())
            .field("household_present", &self.household.is_some())
            .field("local_profiles_present", &self.local_profiles.is_some())
            .field("profile_outbox_present", &self.profile_outbox.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub enum LegacyPythonPhaseAResultV1 {
    Present(Box<LegacyPythonPresentCandidateV1>),
    NoSource {
        account: AccountId,
        source_set_fingerprint: CanonicalDigestV1,
    },
}

impl LegacyPythonPhaseAResultV1 {
    #[must_use]
    pub fn source_identity(&self) -> LegacySourceIdentityV1 {
        match self {
            Self::Present(candidate) => LegacySourceIdentityV1::Present {
                source_kind: D2_SOURCE_BUNDLE_KIND.to_owned(),
                source_digest: candidate.source_digest,
            },
            Self::NoSource {
                source_set_fingerprint,
                ..
            } => LegacySourceIdentityV1::NoSource {
                source_set_fingerprint: *source_set_fingerprint,
            },
        }
    }

    #[must_use]
    pub fn account(&self) -> &AccountId {
        match self {
            Self::Present(candidate) => &candidate.account,
            Self::NoSource { account, .. } => account,
        }
    }

    #[must_use]
    pub const fn is_no_source(&self) -> bool {
        matches!(self, Self::NoSource { .. })
    }

    /// Content-free provenance for reserving the exact import snapshot in the
    /// durable migration guard. No-source observations and present sources
    /// without a prior importer snapshot return `None`.
    #[must_use]
    pub fn snapshot_provenance(&self) -> Option<LegacyPythonSnapshotProvenanceV1> {
        match self {
            Self::Present(candidate) => candidate.snapshot_evidence.clone(),
            Self::NoSource { .. } => None,
        }
    }
}

impl fmt::Debug for LegacyPythonPhaseAResultV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Present(candidate) => formatter
                .debug_tuple("LegacyPythonPhaseAResultV1::Present")
                .field(candidate)
                .finish(),
            Self::NoSource {
                source_set_fingerprint,
                ..
            } => formatter
                .debug_struct("LegacyPythonPhaseAResultV1::NoSource")
                .field("source_set_fingerprint", source_set_fingerprint)
                .finish(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyPythonPhaseBContextV1 {
    migration_frozen_at: CanonicalTimestampV1,
    migration_id: Uuid,
    initialization_id: Uuid,
    initial_commit_id: CommitId,
    owner_display_name: DisplayName,
}

impl LegacyPythonPhaseBContextV1 {
    /// Construct Phase B input only from a durable guard that has reserved this
    /// exact Phase A source identity for this exact account slot.
    pub fn from_reserved_guard(
        phase_a: &LegacyPythonPhaseAResultV1,
        account_slot: &HouseholdAccountSlotV1,
        guard: &HouseholdMigrationGuardDocument,
        owner_display_name: DisplayName,
    ) -> Result<Self, PortError> {
        validate_account_slot_binding(phase_a.account(), account_slot)?;
        guard.validate_for(account_slot)?;
        if guard.state() != HouseholdMigrationGuardStateV1::Initializing
            || !matches!(
                guard.initialization_phase(),
                Some(
                    HouseholdMigrationInitializationPhaseV1::ReservedSource
                        | HouseholdMigrationInitializationPhaseV1::ReadyToInitialize
                )
            )
            || !guard_source_matches_phase_a(guard.source_identity(), phase_a)
            || guard.legacy_python_snapshot() != phase_a.snapshot_provenance().as_ref()
        {
            return Err(PortError::new(
                "legacy_python_guard_reservation_mismatch",
                "legacy household conversion requires an exact reserved-source migration guard",
            ));
        }
        Ok(Self {
            migration_frozen_at: guard.migration_frozen_at().clone(),
            migration_id: guard.migration_id(),
            initialization_id: guard.initialization_id(),
            initial_commit_id: CommitId::from_uuid(guard.initial_commit_id()),
            owner_display_name,
        })
    }
}

fn guard_source_matches_phase_a(
    guard: &HouseholdMigrationSourceIdentityV1,
    phase_a: &LegacyPythonPhaseAResultV1,
) -> bool {
    guard_source_matches_identity(guard, &phase_a.source_identity())
}

fn guard_source_matches_identity(
    guard: &HouseholdMigrationSourceIdentityV1,
    source: &LegacySourceIdentityV1,
) -> bool {
    match (guard, source) {
        (
            HouseholdMigrationSourceIdentityV1::Present {
                source_kind: HouseholdMigrationPresentSourceKindV1::LegacyPythonSourceBundleV1,
                source_digest: guard_digest,
            },
            LegacySourceIdentityV1::Present {
                source_kind,
                source_digest,
            },
        ) => {
            source_kind == D2_SOURCE_BUNDLE_KIND
                && guard_digest.as_bytes() == source_digest.as_bytes()
        }
        (
            HouseholdMigrationSourceIdentityV1::NoSource {
                source_set_fingerprint: guard_digest,
            },
            LegacySourceIdentityV1::NoSource {
                source_set_fingerprint,
            },
        ) => guard_digest.as_bytes() == source_set_fingerprint.as_bytes(),
        _ => false,
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct LegacyPythonPhaseBResultV1 {
    pub state: HouseholdStateV1,
    pub semantic_candidate_digest: CanonicalDigestV1,
    snapshot_evidence: Option<LegacyPythonSnapshotEvidenceV1>,
}

impl fmt::Debug for LegacyPythonPhaseBResultV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LegacyPythonPhaseBResultV1")
            .field("state", &self.state)
            .field("semantic_candidate_digest", &self.semantic_candidate_digest)
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct LegacyLocationRestartV1 {
    canonical: CanonicalJsonValueV1,
    label: String,
    latitude: CanonicalJsonValueV1,
    longitude: CanonicalJsonValueV1,
}

impl LegacyLocationRestartV1 {
    #[must_use]
    pub const fn canonical(&self) -> &CanonicalJsonValueV1 {
        &self.canonical
    }

    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    #[must_use]
    pub const fn latitude(&self) -> &CanonicalJsonValueV1 {
        &self.latitude
    }

    #[must_use]
    pub const fn longitude(&self) -> &CanonicalJsonValueV1 {
        &self.longitude
    }
}

impl fmt::Debug for LegacyLocationRestartV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LegacyLocationRestartV1")
            .field("canonical_sha256", &self.canonical.canonical_sha256())
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct LegacyRestaurantSearchRestartV1 {
    canonical: CanonicalJsonValueV1,
    restaurant_names: Vec<String>,
}

impl LegacyRestaurantSearchRestartV1 {
    #[must_use]
    pub const fn canonical(&self) -> &CanonicalJsonValueV1 {
        &self.canonical
    }

    #[must_use]
    pub fn restaurant_names(&self) -> &[String] {
        &self.restaurant_names
    }
}

impl fmt::Debug for LegacyRestaurantSearchRestartV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LegacyRestaurantSearchRestartV1")
            .field("canonical_sha256", &self.canonical.canonical_sha256())
            .field("restaurant_count", &self.restaurant_names.len())
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyPythonRestartStateV1 {
    pub location: Option<LegacyLocationRestartV1>,
    pub last_restaurant_search: Option<LegacyRestaurantSearchRestartV1>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyPythonVaultReadbackVerificationV1 {
    pub canonical_state_digest: CanonicalDigestV1,
    pub disposition_manifest_digest: CanonicalDigestV1,
    pub restart_state: LegacyPythonRestartStateV1,
    snapshot_evidence: Option<LegacyPythonSnapshotEvidenceV1>,
}

pub type LegacyPythonSnapshotEvidenceV1 = LegacyPythonSnapshotProvenanceV1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyPythonSnapshotRetirementV1 {
    NotPresent,
    Retired,
}

#[derive(Clone, Eq, PartialEq)]
pub struct LegacyPythonResolvedInitializationV1 {
    pub command: HouseholdInitialize,
    pub resolved_state: HouseholdStateV1,
    pub canonical_state_digest: CanonicalDigestV1,
    pub initial_effect_fingerprint: HouseholdEffectFingerprintV1,
}

impl fmt::Debug for LegacyPythonResolvedInitializationV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LegacyPythonResolvedInitializationV1")
            .field("command", &self.command)
            .field("canonical_state_digest", &self.canonical_state_digest)
            .field(
                "initial_effect_fingerprint",
                &self.initial_effect_fingerprint,
            )
            .finish_non_exhaustive()
    }
}

impl LegacyPythonPhaseBResultV1 {
    /// Resolve the application initialization command before guard
    /// `ready_to_initialize`. The returned digest is the post-ledger state
    /// digest that must be written into the guard and later verified.
    pub fn resolve_initialization(
        &self,
    ) -> Result<LegacyPythonResolvedInitializationV1, PortError> {
        let effect = match &self.state.migration_provenance.source_identity {
            LegacySourceIdentityV1::Present { .. } => HouseholdEffectV1::Migration,
            LegacySourceIdentityV1::NoSource { .. } => HouseholdEffectV1::Initialize,
        };
        let command = HouseholdInitialize::new(
            self.state.account_binding.clone(),
            self.state.migration_provenance.initial_commit_id,
            self.state.clone(),
            effect,
            self.state.updated_at.clone(),
        )?;
        self.resolve_initialization_command(command)
    }

    pub fn resolve_initialization_command(
        &self,
        command: HouseholdInitialize,
    ) -> Result<LegacyPythonResolvedInitializationV1, PortError> {
        if command.semantic_candidate_state != self.state
            || command.account != self.state.account_binding
            || command.commit_id != self.state.migration_provenance.initial_commit_id
            || command.normalized_typed_effect
                != match &self.state.migration_provenance.source_identity {
                    LegacySourceIdentityV1::Present { .. } => HouseholdEffectV1::Migration,
                    LegacySourceIdentityV1::NoSource { .. } => HouseholdEffectV1::Initialize,
                }
            || command.claimed_effect_fingerprint.as_digest().as_bytes()
                == self.semantic_candidate_digest.as_bytes()
        {
            return Err(PortError::new(
                "legacy_python_initialization_mismatch",
                "household initialization command does not match the exact migration candidate",
            ));
        }
        let resolution = resolve_household_initialize_v1(None, &command)?;
        let HouseholdRepositoryResolutionV1::Write { state, outcome } = resolution else {
            return Err(PortError::new(
                "legacy_python_initialization_mismatch",
                "new household migration unexpectedly resolved as a replay",
            ));
        };
        if outcome.outcome != AppliedCommitOutcomeV1::Initialized
            || state.bounded_applied_commits.len() != 1
        {
            return Err(PortError::new(
                "legacy_python_initialization_mismatch",
                "resolved household initialization does not contain exactly one initial applied-commit record",
            ));
        }
        let record = &state.bounded_applied_commits[0];
        if record.commit_id != command.commit_id
            || record.fingerprint.as_bytes()
                != command.claimed_effect_fingerprint.as_digest().as_bytes()
            || record.resulting_revision != state.revision
            || record.outcome != AppliedCommitOutcomeV1::Initialized
            || record.committed_at != command.frozen_commit_timestamp
        {
            return Err(PortError::new(
                "legacy_python_initialization_mismatch",
                "resolved household initialization record does not match its exact command",
            ));
        }
        verify_only_initial_ledger_delta(&self.state, &state)?;
        let canonical = state.canonical_bytes().map_err(canonical_phase_b_error)?;
        Ok(LegacyPythonResolvedInitializationV1 {
            canonical_state_digest: digest_bytes(&canonical),
            initial_effect_fingerprint: command.claimed_effect_fingerprint,
            command,
            resolved_state: *state,
        })
    }

    /// Verify the authenticated state returned by the vault, including every
    /// independently derived destination-fragment digest, before callers make
    /// the migrated state or its restart contexts live.
    pub fn verify_vault_readback(
        &self,
        initialization: &HouseholdInitialize,
        readback: &HouseholdStateV1,
    ) -> Result<LegacyPythonVaultReadbackVerificationV1, PortError> {
        let resolved = self.resolve_initialization_command(initialization.clone())?;
        readback.validate().map_err(vault_readback_error)?;
        let canonical = readback.canonical_bytes().map_err(vault_readback_error)?;
        let canonical_state_digest = digest_bytes(&canonical);
        if canonical_state_digest != resolved.canonical_state_digest
            || readback != &resolved.resolved_state
        {
            return Err(PortError::new(
                "legacy_python_vault_readback_mismatch",
                "authenticated household vault readback does not match the reserved migration result",
            ));
        }
        verify_destination_disposition_digests(readback)?;
        let disposition_manifest_digest =
            canonical_sha256_v1(&readback.migration_dispositions).map_err(vault_readback_error)?;
        Ok(LegacyPythonVaultReadbackVerificationV1 {
            canonical_state_digest,
            disposition_manifest_digest,
            restart_state: build_restart_state(readback).map_err(vault_readback_error)?,
            snapshot_evidence: readback.migration_provenance.legacy_python_snapshot.clone(),
        })
    }
}

fn verify_only_initial_ledger_delta(
    semantic_candidate: &HouseholdStateV1,
    resolved: &HouseholdStateV1,
) -> Result<(), PortError> {
    let mut without_ledger = resolved.clone();
    without_ledger.bounded_applied_commits.clear();
    if &without_ledger != semantic_candidate {
        return Err(PortError::new(
            "legacy_python_initialization_mismatch",
            "resolved household initialization changed migration semantics outside the initial applied-commit ledger",
        ));
    }
    Ok(())
}

/// Strict D2 migration adapter. Phase A performs no clock-dependent
/// interpretation. Phase B first repeats phase A and proves the exact source
/// identity unchanged, then applies the guard-frozen time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyPythonHouseholdMigrationV1 {
    config_root: LegacyPythonConfigRootV1,
    snapshot_path: PathBuf,
}

#[cfg(test)]
#[derive(Clone)]
struct LegacySourceLockDropObserver {
    label: &'static str,
    events: std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>,
}

#[cfg(test)]
#[derive(Clone)]
struct LegacySourceLockAcquisitionObserver {
    attempt: Option<std::sync::mpsc::Sender<()>>,
    drop_observer: LegacySourceLockDropObserver,
}

#[cfg(test)]
static LEGACY_SOURCE_LOCK_ACQUISITION_OBSERVERS: std::sync::OnceLock<
    std::sync::Mutex<BTreeMap<PathBuf, LegacySourceLockAcquisitionObserver>>,
> = std::sync::OnceLock::new();

#[cfg(test)]
fn register_legacy_source_lock_acquisition_observer(
    path: PathBuf,
    label: &'static str,
    events: std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>,
    attempt: Option<std::sync::mpsc::Sender<()>>,
) {
    LEGACY_SOURCE_LOCK_ACQUISITION_OBSERVERS
        .get_or_init(|| std::sync::Mutex::new(BTreeMap::new()))
        .lock()
        .expect("legacy source lock observer registry must not be poisoned")
        .insert(
            path,
            LegacySourceLockAcquisitionObserver {
                attempt,
                drop_observer: LegacySourceLockDropObserver { label, events },
            },
        );
}

#[cfg(test)]
fn take_legacy_source_lock_acquisition_observer(
    path: &Path,
) -> Option<LegacySourceLockAcquisitionObserver> {
    LEGACY_SOURCE_LOCK_ACQUISITION_OBSERVERS
        .get_or_init(|| std::sync::Mutex::new(BTreeMap::new()))
        .lock()
        .expect("legacy source lock observer registry must not be poisoned")
        .remove(path)
}

#[cfg(test)]
fn unregister_legacy_source_lock_acquisition_observer(path: &Path) {
    let _ = LEGACY_SOURCE_LOCK_ACQUISITION_OBSERVERS
        .get_or_init(|| std::sync::Mutex::new(BTreeMap::new()))
        .lock()
        .expect("legacy source lock observer registry must not be poisoned")
        .remove(path);
}

struct LegacySourceFileLock {
    lock: Option<FileLock>,
    #[cfg(test)]
    drop_observer: Option<LegacySourceLockDropObserver>,
}

impl LegacySourceFileLock {
    fn new(lock: FileLock) -> Self {
        Self {
            lock: Some(lock),
            #[cfg(test)]
            drop_observer: None,
        }
    }

    #[cfg(test)]
    fn observe_drop(
        &mut self,
        label: &'static str,
        events: std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>,
    ) {
        self.drop_observer = Some(LegacySourceLockDropObserver { label, events });
    }
}

impl Drop for LegacySourceFileLock {
    fn drop(&mut self) {
        drop(self.lock.take());
        #[cfg(test)]
        if let Some(observer) = &self.drop_observer
            && let Ok(mut events) = observer.events.lock()
        {
            events.push(observer.label);
        }
    }
}

/// Retained lock authority for one exact legacy source bundle.
///
/// Acquisition consumes the already-held per-account lifecycle lease and
/// retains it through Phase A. [`LegacyPythonHouseholdMigrationV1::acquire_source_vault_lease`]
/// performs the public one-way transfer into an opaque composite without
/// releasing any source lock. The three file locks remain held across Phase A,
/// guard reservation, Phase B, vault verification, and exact snapshot
/// retirement.
pub struct LegacyPythonSourceLeaseV1 {
    _snapshot_lock: LegacySourceFileLock,
    _legacy_config_lock: LegacySourceFileLock,
    _current_config_lock: LegacySourceFileLock,
    lifecycle: Option<HouseholdLifecycleLease>,
    account_slot: HouseholdAccountSlotV1,
    current_config_locator_digest: CanonicalDigestV1,
    legacy_config_locator_digest: CanonicalDigestV1,
    snapshot_locator_digest: CanonicalDigestV1,
}

/// Exact current/legacy Python `config.lock` authority for the released
/// pre-native logout path.
///
/// This deliberately omits a lifecycle and snapshot lock: the compatibility
/// path is reachable only after proving that no D2 account provenance exists,
/// so creating an account directory merely to borrow D2 lifecycle authority
/// would manufacture the evidence that disables the path. Native auth remains
/// present until both locked file targets and both derived keyring targets
/// have been scrubbed or authoritatively classified complete.
#[cfg(feature = "native-credentials")]
pub(crate) struct LegacyPythonCredentialSourceLeaseV1 {
    _legacy_config_lock: LegacySourceFileLock,
    _current_config_lock: LegacySourceFileLock,
    account_slot: HouseholdAccountSlotV1,
    current_config_locator_digest: CanonicalDigestV1,
    legacy_config_locator_digest: CanonicalDigestV1,
}

/// Snapshot-only crash-recovery authority used after a committed native vault
/// has made every mixed Python config/keyring source permanently ineligible.
/// It intentionally has no config-root or keyring field.
pub struct LegacyPythonSnapshotLeaseV1 {
    _snapshot_lock: LegacySourceFileLock,
    lifecycle: Option<HouseholdLifecycleLease>,
    account_slot: HouseholdAccountSlotV1,
    snapshot_locator_digest: CanonicalDigestV1,
}

/// One exact legacy-source/vault critical section. The acquired wrapper is
/// already composite at the blocking-worker boundary, so cancellation cannot
/// expose a raw source/vault tuple before validated migration binding.
pub(crate) type LegacyPythonVaultLeaseTransactionV1<S> = AcquiredNarrowerVaultLease<S>;
pub(crate) type LegacyPythonSourceVaultLeaseTransactionV1 =
    LegacyPythonVaultLeaseTransactionV1<LegacyPythonSourceLeaseV1>;
pub(crate) type LegacyPythonSnapshotVaultLeaseTransactionV1 =
    LegacyPythonVaultLeaseTransactionV1<LegacyPythonSnapshotLeaseV1>;

/// Opaque public authority for Phase B. Its public API exposes shared borrows
/// only; downstream callers can neither extract nor replace the retained vault
/// lease and therefore cannot release lifecycle before legacy source locks.
pub struct LegacyPythonSourceVaultLeaseV1 {
    transaction: Option<LegacyPythonSourceVaultLeaseTransactionV1>,
}

impl LegacyPythonSourceVaultLeaseV1 {
    #[must_use]
    pub fn source_lease(&self) -> &LegacyPythonSourceLeaseV1 {
        self.transaction
            .as_ref()
            .expect("active source/vault lease retains its transaction")
            .source_lease()
    }

    #[must_use]
    pub fn vault_lease(&self) -> &HouseholdVaultLease {
        self.transaction
            .as_ref()
            .expect("active source/vault lease retains its transaction")
            .vault_lease()
    }

    // Unit-only mutation lets migration tests seed guarded state without
    // exposing replaceable composite authority in production builds.
    #[cfg(test)]
    pub(crate) fn vault_lease_mut(&mut self) -> &mut HouseholdVaultLease {
        self.transaction
            .as_mut()
            .expect("active source/vault lease retains its transaction")
            .vault_lease_mut()
    }
}

impl fmt::Debug for LegacyPythonSourceVaultLeaseV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LegacyPythonSourceVaultLeaseV1")
            .field("transaction_retained", &self.transaction.is_some())
            .finish_non_exhaustive()
    }
}

/// Opaque snapshot/vault authority for committed crash recovery. Its public
/// API exposes shared borrows only, preventing replacement of either retained
/// authority.
pub struct LegacyPythonSnapshotVaultLeaseV1 {
    transaction: Option<LegacyPythonSnapshotVaultLeaseTransactionV1>,
}

impl LegacyPythonSnapshotVaultLeaseV1 {
    #[must_use]
    pub fn snapshot_lease(&self) -> &LegacyPythonSnapshotLeaseV1 {
        self.transaction
            .as_ref()
            .expect("active snapshot/vault lease retains its transaction")
            .source_lease()
    }

    #[must_use]
    pub fn vault_lease(&self) -> &HouseholdVaultLease {
        self.transaction
            .as_ref()
            .expect("active snapshot/vault lease retains its transaction")
            .vault_lease()
    }
}

impl fmt::Debug for LegacyPythonSnapshotVaultLeaseV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LegacyPythonSnapshotVaultLeaseV1")
            .field("transaction_retained", &self.transaction.is_some())
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for LegacyPythonSnapshotLeaseV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LegacyPythonSnapshotLeaseV1")
            .field("account_slot", &self.account_slot)
            .field("lifecycle_retained", &self.lifecycle.is_some())
            .field("snapshot_lock_held", &true)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyPythonCommittedSnapshotRetirementV1 {
    canonical_state_digest: CanonicalDigestV1,
    snapshot_evidence: Option<LegacyPythonSnapshotProvenanceV1>,
}

impl fmt::Debug for LegacyPythonSourceLeaseV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LegacyPythonSourceLeaseV1")
            .field("account_slot", &self.account_slot)
            .field("lifecycle_retained", &self.lifecycle.is_some())
            .field("source_locks_held", &true)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "native-credentials")]
impl fmt::Debug for LegacyPythonCredentialSourceLeaseV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LegacyPythonCredentialSourceLeaseV1")
            .field("account_slot", &self.account_slot)
            .field("credential_source_locks_held", &true)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "native-credentials")]
impl LegacyPythonCredentialSourceLeaseV1 {
    pub(crate) fn account_slot_for_target(
        &self,
        config_kind: LegacyPythonConfigKindV1,
        resolved_config_path: &Path,
    ) -> Result<&HouseholdAccountSlotV1, PortError> {
        let expected = match config_kind {
            LegacyPythonConfigKindV1::Current => self.current_config_locator_digest,
            LegacyPythonConfigKindV1::Legacy => self.legacy_config_locator_digest,
        };
        if path_locator_digest(resolved_config_path)? != expected {
            return Err(PortError::new(
                "legacy_python_credential_lease_mismatch",
                "legacy Python credential operation names a different frozen config locator",
            ));
        }
        Ok(&self.account_slot)
    }
}

impl LegacyPythonHouseholdMigrationV1 {
    #[must_use]
    pub fn new(config_root: LegacyPythonConfigRootV1, snapshot_path: impl Into<PathBuf>) -> Self {
        Self {
            config_root,
            snapshot_path: snapshot_path.into(),
        }
    }

    pub fn discover(native_paths: &NativePaths) -> Result<Self, PortError> {
        let base_dirs = BaseDirs::new().ok_or_else(legacy_root_ambiguous)?;
        let config_root = LegacyPythonConfigRootV1::from_environment_values(
            std::env::var_os("XDG_CONFIG_HOME").as_deref(),
            Some(base_dirs.home_dir()),
        )?;
        Ok(Self::new(
            config_root,
            native_paths.root().join(IMPORT_FILE_NAME),
        ))
    }

    #[must_use]
    pub fn snapshot_path(&self) -> &Path {
        &self.snapshot_path
    }

    #[must_use]
    pub fn config_path(&self, kind: LegacyPythonConfigKindV1) -> &Path {
        self.config_root.config_path(kind)
    }

    pub async fn acquire_source_lease(
        &self,
        lifecycle: HouseholdLifecycleLease,
        cancellation: CancellationToken,
    ) -> Result<LegacyPythonSourceLeaseV1, PortError> {
        migration_checkpoint(&cancellation).await?;
        let account_slot = lifecycle.account_slot().clone();
        lifecycle.validate_for(&account_slot)?;
        self.config_root.revalidate()?;
        let current_path = self
            .config_root
            .config_path(LegacyPythonConfigKindV1::Current);
        let legacy_path = self
            .config_root
            .config_path(LegacyPythonConfigKindV1::Legacy);
        let current_lock_path = sibling_config_lock_path(current_path)?;
        let legacy_lock_path = sibling_config_lock_path(legacy_path)?;
        let snapshot_lock_path = self
            .snapshot_path
            .parent()
            .ok_or_else(legacy_root_ambiguous)?
            .join(IMPORT_LOCK_NAME);
        let current_config_locator_digest = path_locator_digest(current_path)?;
        let legacy_config_locator_digest = path_locator_digest(legacy_path)?;
        let snapshot_locator_digest = path_locator_digest(&self.snapshot_path)?;

        // Fixed lower-rank order under account-lifecycle.lock: current Python
        // config.lock, legacy Python config.lock, then import/snapshot lock.
        // One blocking worker owns lifecycle and every acquired lock until it
        // returns the completed, drop-ordered lease. Cancelling or dropping
        // this outer future therefore cannot detach a higher-rank lock worker
        // from the lower-rank authorities it depends on.
        let worker_cancellation = cancellation.clone();
        let lease = tokio::task::spawn_blocking(move || {
            let current_lock =
                acquire_source_file_lock_blocking(&current_lock_path, &worker_cancellation)?;
            let legacy_lock =
                acquire_source_file_lock_blocking(&legacy_lock_path, &worker_cancellation)?;
            let snapshot_lock =
                acquire_source_file_lock_blocking(&snapshot_lock_path, &worker_cancellation)?;
            Ok(LegacyPythonSourceLeaseV1 {
                _snapshot_lock: snapshot_lock,
                _legacy_config_lock: legacy_lock,
                _current_config_lock: current_lock,
                lifecycle: Some(lifecycle),
                account_slot,
                current_config_locator_digest,
                legacy_config_locator_digest,
                snapshot_locator_digest,
            })
        })
        .await
        .map_err(legacy_source_lock_task_error)??;
        self.validate_source_lease_for_phase_a(&lease)?;
        migration_checkpoint(&cancellation).await?;
        Ok(lease)
    }

    /// Move a Phase-A source lease directly into one opaque Phase-B
    /// source/vault authority. No lifecycle or vault ownership is exposed at
    /// the transition boundary.
    pub async fn acquire_source_vault_lease(
        &self,
        mut source_lease: LegacyPythonSourceLeaseV1,
        vault: &HouseholdVault,
        mode: HouseholdVaultLeaseModeV1,
        cancellation: CancellationToken,
    ) -> Result<LegacyPythonSourceVaultLeaseV1, PortError> {
        let lifecycle = self.take_lifecycle_for_vault(&mut source_lease)?;
        let acquired = vault
            .acquire_vault_lease_after_narrower(source_lease, lifecycle, mode, cancellation)
            .await?;
        let transaction = self.bind_source_vault_transaction(acquired)?;
        Ok(LegacyPythonSourceVaultLeaseV1 {
            transaction: Some(transaction),
        })
    }

    #[cfg(feature = "native-credentials")]
    pub(crate) async fn acquire_credential_source_lease(
        &self,
        account_slot: HouseholdAccountSlotV1,
        cancellation: CancellationToken,
    ) -> Result<LegacyPythonCredentialSourceLeaseV1, PortError> {
        migration_checkpoint(&cancellation).await?;
        self.config_root.revalidate()?;
        let current_path = self
            .config_root
            .config_path(LegacyPythonConfigKindV1::Current);
        let legacy_path = self
            .config_root
            .config_path(LegacyPythonConfigKindV1::Legacy);
        let current_lock_path = sibling_config_lock_path(current_path)?;
        let legacy_lock_path = sibling_config_lock_path(legacy_path)?;
        let current_config_locator_digest = path_locator_digest(current_path)?;
        let legacy_config_locator_digest = path_locator_digest(legacy_path)?;

        // The released Python order is current config then legacy config. The
        // worker returns only the completed drop-ordered credential lease.
        let worker_cancellation = cancellation.clone();
        let lease = tokio::task::spawn_blocking(move || {
            let current_lock =
                acquire_source_file_lock_blocking(&current_lock_path, &worker_cancellation)?;
            let legacy_lock =
                acquire_source_file_lock_blocking(&legacy_lock_path, &worker_cancellation)?;
            Ok(LegacyPythonCredentialSourceLeaseV1 {
                _legacy_config_lock: legacy_lock,
                _current_config_lock: current_lock,
                account_slot,
                current_config_locator_digest,
                legacy_config_locator_digest,
            })
        })
        .await
        .map_err(legacy_source_lock_task_error)??;
        self.validate_credential_source_lease(&lease)?;
        migration_checkpoint(&cancellation).await?;
        Ok(lease)
    }

    #[cfg(feature = "native-credentials")]
    pub(crate) fn validate_credential_source_lease(
        &self,
        lease: &LegacyPythonCredentialSourceLeaseV1,
    ) -> Result<(), PortError> {
        self.config_root.revalidate()?;
        if lease.current_config_locator_digest
            != path_locator_digest(
                self.config_root
                    .config_path(LegacyPythonConfigKindV1::Current),
            )?
            || lease.legacy_config_locator_digest
                != path_locator_digest(
                    self.config_root
                        .config_path(LegacyPythonConfigKindV1::Legacy),
                )?
        {
            return Err(PortError::new(
                "legacy_python_credential_lease_mismatch",
                "legacy Python credential lease does not match its frozen config locators",
            ));
        }
        Ok(())
    }

    pub async fn acquire_snapshot_retirement_lease(
        &self,
        lifecycle: HouseholdLifecycleLease,
        cancellation: CancellationToken,
    ) -> Result<LegacyPythonSnapshotLeaseV1, PortError> {
        migration_checkpoint(&cancellation).await?;
        let account_slot = lifecycle.account_slot().clone();
        lifecycle.validate_for(&account_slot)?;
        let snapshot_lock_path = self
            .snapshot_path
            .parent()
            .ok_or_else(legacy_root_ambiguous)?
            .join(IMPORT_LOCK_NAME);
        let snapshot_locator_digest = path_locator_digest(&self.snapshot_path)?;
        let worker_cancellation = cancellation.clone();
        let lease = tokio::task::spawn_blocking(move || {
            let snapshot_lock =
                acquire_source_file_lock_blocking(&snapshot_lock_path, &worker_cancellation)?;
            Ok(LegacyPythonSnapshotLeaseV1 {
                _snapshot_lock: snapshot_lock,
                lifecycle: Some(lifecycle),
                account_slot,
                snapshot_locator_digest,
            })
        })
        .await
        .map_err(legacy_source_lock_task_error)??;
        self.validate_snapshot_lease_for_phase_a(&lease)?;
        migration_checkpoint(&cancellation).await?;
        Ok(lease)
    }

    /// Move a snapshot-retirement lease directly into one opaque
    /// snapshot/vault authority.
    pub async fn acquire_snapshot_vault_lease(
        &self,
        mut snapshot_lease: LegacyPythonSnapshotLeaseV1,
        vault: &HouseholdVault,
        mode: HouseholdVaultLeaseModeV1,
        cancellation: CancellationToken,
    ) -> Result<LegacyPythonSnapshotVaultLeaseV1, PortError> {
        let lifecycle = self.take_snapshot_lifecycle_for_vault(&mut snapshot_lease)?;
        let acquired = vault
            .acquire_vault_lease_after_narrower(snapshot_lease, lifecycle, mode, cancellation)
            .await?;
        let transaction = self.bind_snapshot_vault_transaction(acquired)?;
        Ok(LegacyPythonSnapshotVaultLeaseV1 {
            transaction: Some(transaction),
        })
    }

    fn validate_snapshot_lease_binding(
        &self,
        lease: &LegacyPythonSnapshotLeaseV1,
    ) -> Result<(), PortError> {
        if lease.snapshot_locator_digest != path_locator_digest(&self.snapshot_path)? {
            return Err(PortError::new(
                "legacy_python_snapshot_lease_mismatch",
                "legacy Python snapshot lease names a different exact locator",
            ));
        }
        Ok(())
    }

    fn validate_snapshot_lease_for_phase_a(
        &self,
        lease: &LegacyPythonSnapshotLeaseV1,
    ) -> Result<(), PortError> {
        let lifecycle = lease.lifecycle.as_ref().ok_or_else(|| {
            PortError::new(
                "legacy_python_snapshot_lease_mismatch",
                "legacy Python snapshot lease has transferred its lifecycle authority",
            )
        })?;
        lifecycle.validate_for(&lease.account_slot)?;
        self.validate_snapshot_lease_binding(lease)
    }

    pub(crate) fn take_snapshot_lifecycle_for_vault(
        &self,
        lease: &mut LegacyPythonSnapshotLeaseV1,
    ) -> Result<HouseholdLifecycleLease, PortError> {
        self.validate_snapshot_lease_for_phase_a(lease)?;
        lease.lifecycle.take().ok_or_else(|| {
            PortError::new(
                "legacy_python_snapshot_lease_mismatch",
                "legacy Python snapshot lifecycle authority was already transferred",
            )
        })
    }

    pub(crate) fn bind_snapshot_vault_transaction(
        &self,
        transaction: LegacyPythonSnapshotVaultLeaseTransactionV1,
    ) -> Result<LegacyPythonSnapshotVaultLeaseTransactionV1, PortError> {
        if transaction.source_lease().lifecycle.is_some() {
            return Err(PortError::new(
                "legacy_python_snapshot_lease_mismatch",
                "legacy Python snapshot lease retained duplicate lifecycle authority",
            ));
        }
        self.validate_snapshot_lease_for_vault(
            transaction.source_lease(),
            transaction.vault_lease(),
        )?;
        Ok(transaction)
    }

    pub(crate) async fn release_snapshot_vault_transaction(
        &self,
        mut transaction: LegacyPythonSnapshotVaultLeaseTransactionV1,
        cancellation: CancellationToken,
    ) -> Result<HouseholdLifecycleLease, PortError> {
        let operation = transaction
            .vault_lease()
            .acquire_operation(&cancellation)
            .await?;
        self.validate_snapshot_lease_for_vault(
            transaction.source_lease(),
            transaction.vault_lease(),
        )?;
        let account_slot = transaction.source_lease().account_slot.clone();
        let Some((snapshot_lease, vault_lease)) = transaction.take_parts() else {
            drop(operation);
            return Err(PortError::new(
                "legacy_python_snapshot_lease_mismatch",
                "legacy Python snapshot/vault transaction is incomplete",
            ));
        };
        let lifecycle = vault_lease.into_lifecycle_after_vault_drop_for_cleanup();
        drop(operation);
        drop(snapshot_lease);
        lifecycle.validate_for(&account_slot)?;
        Ok(lifecycle)
    }

    fn validate_snapshot_lease_for_vault(
        &self,
        lease: &LegacyPythonSnapshotLeaseV1,
        vault_lease: &HouseholdVaultLease,
    ) -> Result<(), PortError> {
        vault_lease.validate_for(&lease.account_slot)?;
        self.validate_snapshot_lease_binding(lease)
    }

    fn validate_source_lease_binding(
        &self,
        lease: &LegacyPythonSourceLeaseV1,
    ) -> Result<(), PortError> {
        self.config_root.revalidate()?;
        if lease.current_config_locator_digest
            != path_locator_digest(
                self.config_root
                    .config_path(LegacyPythonConfigKindV1::Current),
            )?
            || lease.legacy_config_locator_digest
                != path_locator_digest(
                    self.config_root
                        .config_path(LegacyPythonConfigKindV1::Legacy),
                )?
            || lease.snapshot_locator_digest != path_locator_digest(&self.snapshot_path)?
        {
            return Err(PortError::new(
                "legacy_python_source_lease_mismatch",
                "legacy Python source lease does not match its exact account, config roots, and snapshot locator",
            ));
        }
        Ok(())
    }

    pub fn validate_source_lease_for_phase_a(
        &self,
        source_lease: &LegacyPythonSourceLeaseV1,
    ) -> Result<(), PortError> {
        let lifecycle = source_lease.lifecycle.as_ref().ok_or_else(|| {
            PortError::new(
                "legacy_python_source_lease_mismatch",
                "legacy Python source lease has already transferred its lifecycle authority to a vault lease",
            )
        })?;
        lifecycle.validate_for(&source_lease.account_slot)?;
        self.validate_source_lease_binding(source_lease)
    }

    pub fn validate_source_lease_for_vault(
        &self,
        source_lease: &LegacyPythonSourceLeaseV1,
        vault_lease: &HouseholdVaultLease,
    ) -> Result<(), PortError> {
        vault_lease.validate_for(&source_lease.account_slot)?;
        self.validate_source_lease_binding(source_lease)
    }

    pub(crate) fn take_lifecycle_for_vault(
        &self,
        source_lease: &mut LegacyPythonSourceLeaseV1,
    ) -> Result<HouseholdLifecycleLease, PortError> {
        self.validate_source_lease_for_phase_a(source_lease)?;
        source_lease.lifecycle.take().ok_or_else(|| {
            PortError::new(
                "legacy_python_source_lease_mismatch",
                "legacy Python source lease lifecycle authority was already transferred",
            )
        })
    }

    pub(crate) fn bind_source_vault_transaction(
        &self,
        transaction: LegacyPythonSourceVaultLeaseTransactionV1,
    ) -> Result<LegacyPythonSourceVaultLeaseTransactionV1, PortError> {
        if transaction.source_lease().lifecycle.is_some() {
            return Err(PortError::new(
                "legacy_python_source_lease_mismatch",
                "legacy Python source lease retained duplicate lifecycle authority",
            ));
        }
        self.validate_source_lease_for_vault(
            transaction.source_lease(),
            transaction.vault_lease(),
        )?;
        Ok(transaction)
    }

    pub(crate) async fn release_source_vault_transaction(
        &self,
        mut transaction: LegacyPythonSourceVaultLeaseTransactionV1,
        cancellation: CancellationToken,
    ) -> Result<HouseholdLifecycleLease, PortError> {
        let operation = transaction
            .vault_lease()
            .acquire_operation(&cancellation)
            .await?;
        self.validate_source_lease_for_vault(
            transaction.source_lease(),
            transaction.vault_lease(),
        )?;
        let account_slot = transaction.source_lease().account_slot.clone();
        let Some((source_lease, vault_lease)) = transaction.take_parts() else {
            drop(operation);
            return Err(PortError::new(
                "legacy_python_source_lease_mismatch",
                "legacy Python source/vault transaction is incomplete",
            ));
        };
        let lifecycle = vault_lease.into_lifecycle_after_vault_drop_for_cleanup();
        drop(operation);
        drop(source_lease);
        lifecycle.validate_for(&account_slot)?;
        Ok(lifecycle)
    }

    /// Release vault authority while restoring lifecycle inside the consumed
    /// source lease. The returned value is again a Phase-A authority; no split
    /// `(source, lifecycle)` state is observable by the caller.
    pub async fn release_source_vault_lease(
        &self,
        mut lease: LegacyPythonSourceVaultLeaseV1,
        cancellation: CancellationToken,
    ) -> Result<LegacyPythonSourceLeaseV1, PortError> {
        let mut transaction = lease.transaction.take().ok_or_else(|| {
            PortError::new(
                "legacy_python_source_lease_mismatch",
                "legacy Python source/vault lease is incomplete",
            )
        })?;
        let operation = transaction
            .vault_lease()
            .acquire_operation(&cancellation)
            .await?;
        self.validate_source_lease_for_vault(
            transaction.source_lease(),
            transaction.vault_lease(),
        )?;
        let Some((mut source_lease, vault_lease)) = transaction.take_parts() else {
            drop(operation);
            return Err(PortError::new(
                "legacy_python_source_lease_mismatch",
                "legacy Python source/vault transaction is incomplete",
            ));
        };
        let lifecycle = vault_lease.into_lifecycle_after_vault_drop_for_cleanup();
        drop(operation);
        if source_lease.lifecycle.is_some() {
            std::mem::forget(lifecycle);
            return Err(PortError::new(
                "legacy_python_source_lease_mismatch",
                "legacy Python source lease retained duplicate lifecycle authority",
            ));
        }
        let validation = lifecycle
            .validate_for(&source_lease.account_slot)
            .and_then(|()| self.validate_source_lease_binding(&source_lease));
        source_lease.lifecycle = Some(lifecycle);
        validation?;
        Ok(source_lease)
    }

    pub fn lifecycle_for_phase_a<'a>(
        &self,
        source_lease: &'a LegacyPythonSourceLeaseV1,
    ) -> Result<&'a HouseholdLifecycleLease, PortError> {
        self.validate_source_lease_for_phase_a(source_lease)?;
        source_lease.lifecycle.as_ref().ok_or_else(|| {
            PortError::new(
                "legacy_python_source_lease_mismatch",
                "legacy Python source lease does not retain lifecycle authority",
            )
        })
    }

    pub fn bind_keyring_probes(
        &self,
        account_slot: &HouseholdAccountSlotV1,
        current: LegacyPythonKeyringProbeOutcomeV1,
        legacy: LegacyPythonKeyringProbeOutcomeV1,
    ) -> Result<LegacyPythonKeyringProbeSetV1, PortError> {
        LegacyPythonKeyringProbeSetV1::bind(account_slot, &self.config_root, current, legacy)
    }

    pub fn authoritative_missing_keyring_probes(
        &self,
        account_slot: &HouseholdAccountSlotV1,
    ) -> Result<LegacyPythonKeyringProbeSetV1, PortError> {
        LegacyPythonKeyringProbeSetV1::authoritative_missing(account_slot, &self.config_root)
    }

    pub async fn phase_a(
        &self,
        account: &AccountId,
        account_slot: &HouseholdAccountSlotV1,
        source_lease: &LegacyPythonSourceLeaseV1,
        keyring: &LegacyPythonKeyringProbeSetV1,
        cancellation: CancellationToken,
    ) -> Result<LegacyPythonPhaseAResultV1, PortError> {
        self.validate_source_lease_for_phase_a(source_lease)?;
        self.phase_a_locked(account, account_slot, source_lease, keyring, cancellation)
            .await
    }

    async fn phase_a_locked(
        &self,
        account: &AccountId,
        account_slot: &HouseholdAccountSlotV1,
        source_lease: &LegacyPythonSourceLeaseV1,
        keyring: &LegacyPythonKeyringProbeSetV1,
        cancellation: CancellationToken,
    ) -> Result<LegacyPythonPhaseAResultV1, PortError> {
        migration_checkpoint(&cancellation).await?;
        self.validate_source_lease_binding(source_lease)?;
        if source_lease.account_slot != *account_slot {
            return Err(PortError::new(
                "legacy_python_source_lease_mismatch",
                "legacy Python source lease belongs to a different account slot",
            ));
        }
        validate_account_slot_binding(account, account_slot)?;
        self.config_root.revalidate()?;
        validate_keyring_probe_set(keyring, account_slot, &self.config_root)?;
        migration_checkpoint(&cancellation).await?;
        let current = read_strict_config(
            self.config_root
                .config_path(LegacyPythonConfigKindV1::Current),
            LegacyPythonConfigKindV1::Current,
        )?;
        migration_checkpoint(&cancellation).await?;
        let legacy = read_strict_config(
            self.config_root
                .config_path(LegacyPythonConfigKindV1::Legacy),
            LegacyPythonConfigKindV1::Legacy,
        )?;
        migration_checkpoint(&cancellation).await?;
        let current_keyring = parse_keyring_probe(
            &keyring.current,
            LegacyPythonConfigKindV1::Current,
            account_slot,
            &self.config_root,
        )?;
        migration_checkpoint(&cancellation).await?;
        let legacy_keyring = parse_keyring_probe(
            &keyring.legacy,
            LegacyPythonConfigKindV1::Legacy,
            account_slot,
            &self.config_root,
        )?;
        migration_checkpoint(&cancellation).await?;
        let snapshot = read_strict_snapshot(&self.snapshot_path)?;
        self.config_root.revalidate()?;
        migration_checkpoint(&cancellation).await?;

        let selected = current.as_ref().or(legacy.as_ref());
        let Some(selected) = selected else {
            if current_keyring
                .as_ref()
                .is_some_and(StrictKeyringDocumentV1::has_household_data)
                || legacy_keyring
                    .as_ref()
                    .is_some_and(StrictKeyringDocumentV1::has_household_data)
                || snapshot.is_some()
            {
                return Err(PortError::new(
                    "legacy_household_source_unbound",
                    "legacy household state has no exact account-binding configuration; repair the legacy source before migration",
                ));
            }
            return no_source_result(
                account,
                account_slot,
                &self.config_root,
                &self.snapshot_path,
                keyring,
            );
        };

        validate_selected_account(selected, account)?;
        if current
            .as_ref()
            .zip(legacy.as_ref())
            .is_some_and(|(_, ignored)| ignored.has_household_data())
        {
            return Err(PortError::new(
                "legacy_python_source_conflict",
                "current and legacy Python configuration sources both contain household state; repair the legacy sources before migration",
            ));
        }
        validate_snapshot_binding(
            snapshot.as_ref(),
            selected,
            account,
            account_slot,
            &self.snapshot_path,
        )?;
        migration_checkpoint(&cancellation).await?;

        let selected_keyring = match selected.kind {
            LegacyPythonConfigKindV1::Current => current_keyring.as_ref(),
            LegacyPythonConfigKindV1::Legacy => legacy_keyring.as_ref(),
        };
        let ignored_keyring = match selected.kind {
            LegacyPythonConfigKindV1::Current => legacy_keyring.as_ref(),
            LegacyPythonConfigKindV1::Legacy => current_keyring.as_ref(),
        };
        if ignored_keyring.is_some_and(StrictKeyringDocumentV1::has_household_data) {
            return Err(PortError::new(
                "legacy_python_source_conflict",
                "a nonselected historical keyring target contains household state; repair the legacy sources before migration",
            ));
        }

        let credential_store = parse_credential_store(&selected.object)?;
        let MergedHouseholdDocumentsV1 {
            household,
            local_profiles,
            profile_outbox,
        } = merge_household_documents(selected, selected_keyring, credential_store)?;
        let LegacyCandidateConfigProjectionV1 {
            first_name,
            compatibility_fields,
            config_field_evidence,
        } = build_secret_free_candidate_config(&selected.object)?;
        let manifest = build_present_manifest(
            selected,
            snapshot.as_ref(),
            credential_store,
            household.as_ref(),
            local_profiles.as_ref(),
            profile_outbox.as_ref(),
            &current,
            &legacy,
            current_keyring.as_ref(),
            legacy_keyring.as_ref(),
            &self.config_root,
            account_slot,
            &self.snapshot_path,
        )?;
        let source_digest = canonical_sha256_v1(&manifest).map_err(canonical_phase_a_error)?;
        migration_checkpoint(&cancellation).await?;
        Ok(LegacyPythonPhaseAResultV1::Present(Box::new(
            LegacyPythonPresentCandidateV1 {
                account: account.clone(),
                selected_kind: selected.kind,
                credential_store,
                source_digest,
                snapshot_evidence: snapshot.map(|snapshot| LegacyPythonSnapshotEvidenceV1 {
                    locator_digest: snapshot.path_locator_digest,
                    content_digest: snapshot.bytes_digest,
                }),
                first_name,
                compatibility_fields,
                config_field_evidence,
                household,
                local_profiles,
                profile_outbox,
            },
        )))
    }

    // The arguments are separate authority-bearing values. Keeping them
    // explicit makes accidental substitution visible at every call site.
    #[allow(clippy::too_many_arguments)]
    pub async fn phase_b(
        &self,
        phase_a: &LegacyPythonPhaseAResultV1,
        context: &LegacyPythonPhaseBContextV1,
        account_slot: &HouseholdAccountSlotV1,
        vault_lease: &HouseholdVaultLease,
        source_lease: &LegacyPythonSourceLeaseV1,
        keyring: &LegacyPythonKeyringProbeSetV1,
        cancellation: CancellationToken,
    ) -> Result<LegacyPythonPhaseBResultV1, PortError> {
        migration_checkpoint(&cancellation).await?;
        self.validate_source_lease_for_vault(source_lease, vault_lease)?;
        let replayed = self
            .phase_a_locked(
                phase_a.account(),
                account_slot,
                source_lease,
                keyring,
                cancellation.clone(),
            )
            .await?;
        migration_checkpoint(&cancellation).await?;
        if replayed.source_identity() != phase_a.source_identity() {
            return Err(PortError::new(
                "legacy_python_source_changed",
                "legacy household source identity changed after migration reservation",
            ));
        }
        let result = match (&replayed, phase_a) {
            (
                LegacyPythonPhaseAResultV1::Present(replayed),
                LegacyPythonPhaseAResultV1::Present(expected),
            ) if replayed == expected => build_present_phase_b(replayed, context),
            (
                LegacyPythonPhaseAResultV1::NoSource { .. },
                LegacyPythonPhaseAResultV1::NoSource { .. },
            ) => build_no_source_phase_b(phase_a, context),
            _ => Err(PortError::new(
                "legacy_python_source_changed",
                "legacy household source changed after migration reservation",
            )),
        }?;
        migration_checkpoint(&cancellation).await?;
        Ok(result)
    }

    pub fn committed_snapshot_retirement_authority(
        &self,
        snapshot_lease: &LegacyPythonSnapshotLeaseV1,
        vault_lease: &HouseholdVaultLease,
        guard: &HouseholdMigrationGuardDocument,
        load: &HouseholdLoad,
    ) -> Result<LegacyPythonCommittedSnapshotRetirementV1, PortError> {
        self.validate_snapshot_lease_for_vault(snapshot_lease, vault_lease)?;
        guard.validate_for(&snapshot_lease.account_slot)?;
        load.state.validate().map_err(canonical_phase_b_error)?;
        validate_account_slot_binding(&load.state.account_binding, &snapshot_lease.account_slot)?;
        let canonical = load
            .state
            .canonical_bytes()
            .map_err(canonical_phase_b_error)?;
        let canonical_state_digest = digest_bytes(&canonical);
        let provenance = &load.state.migration_provenance;
        let source_and_state_match = matches!(
            (
                guard.state(),
                guard.source_identity(),
                &provenance.source_identity,
            ),
            (
                HouseholdMigrationGuardStateV1::Migrated,
                HouseholdMigrationSourceIdentityV1::Present { .. },
                LegacySourceIdentityV1::Present { .. },
            ) | (
                HouseholdMigrationGuardStateV1::InitializedNoSource,
                HouseholdMigrationSourceIdentityV1::NoSource { .. },
                LegacySourceIdentityV1::NoSource { .. },
            )
        ) && guard_source_matches_identity(
            guard.source_identity(),
            &provenance.source_identity,
        );
        let initial_state_digest = guard.initial_state_digest();
        if !source_and_state_match
            || guard.initialization_phase().is_some()
            || guard.legacy_python_snapshot() != provenance.legacy_python_snapshot.as_ref()
            || provenance.migration_id != guard.migration_id()
            || provenance.initialization_id != guard.initialization_id()
            || provenance.initial_commit_id.as_uuid() != guard.initial_commit_id()
            || provenance.migration_frozen_at != *guard.migration_frozen_at()
            || canonical_state_digest != load.state_digest
            || initial_state_digest.is_none()
            || (load.state.revision.get() == 1
                && initial_state_digest != Some(*canonical_state_digest.as_bytes()))
        {
            return Err(PortError::new(
                "legacy_python_committed_snapshot_authority_mismatch",
                "committed guard and authenticated household vault do not prove one exact snapshot-retirement authority",
            ));
        }
        let mut initial_records = load
            .state
            .bounded_applied_commits
            .iter()
            .filter(|record| record.commit_id == provenance.initial_commit_id);
        let initial = initial_records.next().ok_or_else(|| {
            PortError::new(
                "legacy_python_committed_snapshot_authority_mismatch",
                "committed household state no longer retains its exact initial migration record",
            )
        })?;
        if initial.commit_id != provenance.initial_commit_id
            || initial_records.next().is_some()
            || initial.resulting_revision.get() != 1
            || initial.outcome != AppliedCommitOutcomeV1::Initialized
            || guard.initial_effect_fingerprint() != Some(*initial.fingerprint.as_bytes())
            || load
                .state
                .bounded_applied_commits
                .iter()
                .filter(|record| record.outcome == AppliedCommitOutcomeV1::Initialized)
                .count()
                != 1
        {
            return Err(PortError::new(
                "legacy_python_committed_snapshot_authority_mismatch",
                "committed household initial ledger does not match its migration guard",
            ));
        }
        Ok(LegacyPythonCommittedSnapshotRetirementV1 {
            canonical_state_digest,
            snapshot_evidence: provenance.legacy_python_snapshot.clone(),
        })
    }

    pub async fn retire_verified_snapshot(
        &self,
        source_lease: &LegacyPythonSourceLeaseV1,
        verification: &LegacyPythonVaultReadbackVerificationV1,
        cancellation: CancellationToken,
    ) -> Result<LegacyPythonSnapshotRetirementV1, PortError> {
        migration_checkpoint(&cancellation).await?;
        self.validate_source_lease_binding(source_lease)?;
        if source_lease.snapshot_locator_digest != path_locator_digest(&self.snapshot_path)? {
            return Err(PortError::new(
                "legacy_python_snapshot_retirement_mismatch",
                "legacy import snapshot retirement authority names a different locator",
            ));
        }
        self.retire_snapshot_with_evidence(
            source_lease.snapshot_locator_digest,
            verification.snapshot_evidence.as_ref(),
            cancellation,
        )
        .await
    }

    pub async fn retire_committed_snapshot(
        &self,
        snapshot_lease: &LegacyPythonSnapshotLeaseV1,
        authority: &LegacyPythonCommittedSnapshotRetirementV1,
        cancellation: CancellationToken,
    ) -> Result<LegacyPythonSnapshotRetirementV1, PortError> {
        migration_checkpoint(&cancellation).await?;
        self.validate_snapshot_lease_binding(snapshot_lease)?;
        if authority
            .canonical_state_digest
            .as_bytes()
            .iter()
            .all(|byte| *byte == 0)
        {
            return Err(PortError::new(
                "legacy_python_committed_snapshot_authority_mismatch",
                "committed snapshot retirement authority is invalid",
            ));
        }
        self.retire_snapshot_with_evidence(
            snapshot_lease.snapshot_locator_digest,
            authority.snapshot_evidence.as_ref(),
            cancellation,
        )
        .await
    }

    async fn retire_snapshot_with_evidence(
        &self,
        expected_locator_digest: CanonicalDigestV1,
        evidence: Option<&LegacyPythonSnapshotProvenanceV1>,
        cancellation: CancellationToken,
    ) -> Result<LegacyPythonSnapshotRetirementV1, PortError> {
        let Some(expected) = evidence else {
            match fs::symlink_metadata(&self.snapshot_path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(LegacyPythonSnapshotRetirementV1::NotPresent);
                }
                _ => {
                    return Err(PortError::new(
                        "legacy_python_snapshot_retirement_mismatch",
                        "an unverified legacy import snapshot appeared after vault verification",
                    ));
                }
            }
        };
        if expected.locator_digest != expected_locator_digest {
            return Err(PortError::new(
                "legacy_python_snapshot_retirement_mismatch",
                "verified legacy import snapshot evidence names a different locator",
            ));
        }
        let bytes = read_source(&self.snapshot_path)?.ok_or_else(|| {
            PortError::new(
                "legacy_python_snapshot_retirement_mismatch",
                "verified legacy import snapshot disappeared before exact retirement",
            )
        })?;
        if digest_bytes(&bytes) != expected.content_digest {
            return Err(PortError::new(
                "legacy_python_snapshot_retirement_mismatch",
                "legacy import snapshot bytes changed before exact retirement",
            ));
        }
        migration_checkpoint(&cancellation).await?;
        fs::remove_file(&self.snapshot_path).map_err(|_| {
            PortError::new(
                "legacy_python_snapshot_retirement",
                "verified legacy import snapshot could not be removed",
            )
        })?;
        sync_snapshot_parent(
            self.snapshot_path
                .parent()
                .ok_or_else(legacy_root_ambiguous)?,
        )?;
        match fs::symlink_metadata(&self.snapshot_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => {
                return Err(PortError::uncertain(
                    "legacy_python_snapshot_retirement_uncertain",
                    "verified legacy import snapshot removal could not be confirmed",
                ));
            }
        }
        migration_checkpoint(&cancellation).await?;
        Ok(LegacyPythonSnapshotRetirementV1::Retired)
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportDocument {
    schema_version: u64,
    source_format: String,
    report: PythonImportReport,
    state: ImportedPythonState,
}

/// A canonical SHA-256 value whose `Debug` output never exposes its preimage.
#[derive(Clone, Eq, PartialEq)]
pub struct Sha256Digest(String);

impl Sha256Digest {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Sha256Digest([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PythonStateSourceKind {
    CurrentConfig,
    LegacyConfig,
}

impl PythonStateSourceKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::CurrentConfig => "current_config",
            Self::LegacyConfig => "legacy_config",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtectedHouseholdReason {
    UninspectedMixedSource,
    PriorImporterSkippedKeyring,
}

#[derive(Clone, Eq, PartialEq)]
struct CheckedSourceEntry {
    kind: &'static str,
    locator_digest: Sha256Digest,
    state: &'static str,
    file_type: Option<&'static str>,
    byte_len: Option<u64>,
    modified_ns: Option<String>,
    file_identity: Option<Sha256Digest>,
    content_digest: Option<Sha256Digest>,
}

impl fmt::Debug for CheckedSourceEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CheckedSourceEntry")
            .field("kind", &self.kind)
            .field("state", &self.state)
            .field("file_type", &self.file_type)
            .field("byte_len", &self.byte_len)
            .field("modified_ns_present", &self.modified_ns.is_some())
            .field("file_identity_present", &self.file_identity.is_some())
            .field("content_digest_present", &self.content_digest.is_some())
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct PythonSourceSetFingerprint {
    digest: Sha256Digest,
    current_config: CheckedSourceEntry,
    legacy_config: CheckedSourceEntry,
    native_snapshot: CheckedSourceEntry,
}

impl PythonSourceSetFingerprint {
    #[must_use]
    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }
}

impl fmt::Debug for PythonSourceSetFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PythonSourceSetFingerprint")
            .field("digest", &self.digest)
            .field("current_config", &self.current_config)
            .field("legacy_config", &self.legacy_config)
            .field("native_snapshot", &self.native_snapshot)
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub enum PythonStatePreview {
    SafeSnapshot {
        state: ImportedPythonState,
        native_snapshot_digest: Sha256Digest,
        normalized_state_digest: Sha256Digest,
        reported_source_digest: Sha256Digest,
        selected_source_kind: Option<PythonStateSourceKind>,
        checked_source_set: PythonSourceSetFingerprint,
    },
    NoSource {
        checked_source_set: PythonSourceSetFingerprint,
    },
    ProtectedUninspectedMixedSource {
        checked_source_set: PythonSourceSetFingerprint,
        selected_locator_digest: Option<Sha256Digest>,
        native_snapshot_digest: Option<Sha256Digest>,
        reported_source_digest: Option<Sha256Digest>,
        normalized_snapshot_digest: Option<Sha256Digest>,
        reason: ProtectedHouseholdReason,
    },
}

impl PythonStatePreview {
    #[must_use]
    pub fn checked_source_set(&self) -> &PythonSourceSetFingerprint {
        match self {
            Self::SafeSnapshot {
                checked_source_set, ..
            }
            | Self::NoSource { checked_source_set }
            | Self::ProtectedUninspectedMixedSource {
                checked_source_set, ..
            } => checked_source_set,
        }
    }
}

impl fmt::Debug for PythonStatePreview {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SafeSnapshot {
                native_snapshot_digest,
                normalized_state_digest,
                reported_source_digest,
                selected_source_kind,
                checked_source_set,
                ..
            } => formatter
                .debug_struct("PythonStatePreview::SafeSnapshot")
                .field("native_snapshot_digest", native_snapshot_digest)
                .field("normalized_state_digest", normalized_state_digest)
                .field("reported_source_digest", reported_source_digest)
                .field("selected_source_kind", selected_source_kind)
                .field("checked_source_set", checked_source_set)
                .finish(),
            Self::NoSource { checked_source_set } => formatter
                .debug_struct("PythonStatePreview::NoSource")
                .field("checked_source_set", checked_source_set)
                .finish(),
            Self::ProtectedUninspectedMixedSource {
                checked_source_set,
                selected_locator_digest,
                native_snapshot_digest,
                reported_source_digest,
                normalized_snapshot_digest,
                reason,
            } => formatter
                .debug_struct("PythonStatePreview::ProtectedUninspectedMixedSource")
                .field("checked_source_set", checked_source_set)
                .field("selected_locator_digest", selected_locator_digest)
                .field("native_snapshot_digest", native_snapshot_digest)
                .field("reported_source_digest", reported_source_digest)
                .field("normalized_snapshot_digest", normalized_snapshot_digest)
                .field("reason", reason)
                .finish(),
        }
    }
}

/// State exposed only after the exact preview binding has been rechecked.
pub struct VerifiedPythonState {
    state: Option<ImportedPythonState>,
    pending_import_report: Option<PythonImportReport>,
    destination_root: PathBuf,
}

impl VerifiedPythonState {
    #[must_use]
    pub fn state(&self) -> Option<&ImportedPythonState> {
        self.state.as_ref()
    }

    /// Persist a newly verified protected import only after the caller has
    /// completed authenticated account binding and strict household
    /// validation. A concurrent snapshot must be exactly equivalent.
    pub fn commit_validated(self) -> Result<(), PortError> {
        let Some(report) = self.pending_import_report else {
            return Ok(());
        };
        let state = self.state.ok_or_else(|| {
            PortError::new(
                "python_import_conflict",
                "verified Python import lost its normalized state",
            )
        })?;
        validate_destination_root(&self.destination_root)?;
        create_private_dir(&self.destination_root)?;
        let _lock = FileLock::acquire(&self.destination_root.join(IMPORT_LOCK_NAME), true)?;
        let destination = self.destination_root.join(IMPORT_FILE_NAME);
        if let Some(document) = read_document_if_present(&destination)? {
            let expected = ImportDocument {
                schema_version: IMPORT_SCHEMA_VERSION,
                source_format: IMPORT_SOURCE_FORMAT.to_owned(),
                report,
                state,
            };
            if document != expected {
                return Err(PortError::new(
                    "python_import_conflict",
                    "a different Python state snapshot appeared before commit",
                ));
            }
            return Ok(());
        }
        write_import_document(&destination, report, state)
    }
}

impl fmt::Debug for VerifiedPythonState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedPythonState")
            .field("state_present", &self.state.is_some())
            .field("pending_import", &self.pending_import_report.is_some())
            .finish()
    }
}

/// Read-only, one-time importer for the final Python client's local config.
///
/// The source is never opened for writing and keyring entries are never read.
/// Credential material is deliberately omitted; callers receive an explicit
/// reauthentication disposition instead. Imported state is written atomically
/// into the private native directory and a different source cannot overwrite a
/// completed import.
pub struct PythonStateImporter {
    current_source_path: PathBuf,
    legacy_source_path: Option<PathBuf>,
    destination_root: PathBuf,
}

impl PythonStateImporter {
    #[must_use]
    pub fn under(source_path: impl Into<PathBuf>, destination_root: impl Into<PathBuf>) -> Self {
        Self {
            current_source_path: source_path.into(),
            legacy_source_path: None,
            destination_root: destination_root.into(),
        }
    }

    #[must_use]
    pub fn under_candidates(
        current_source_path: impl Into<PathBuf>,
        legacy_source_path: impl Into<PathBuf>,
        destination_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            current_source_path: current_source_path.into(),
            legacy_source_path: Some(legacy_source_path.into()),
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
        Ok(Self::under_candidates(current, legacy, native_paths.root()))
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
        let source = read_source(self.selected_source_path_post_approval()?)?;

        let Some(source) = source else {
            return match read_document_if_present(&destination)? {
                Some(mut document) => {
                    document.report.outcome = PythonImportOutcome::AlreadyImported;
                    Ok(document.report)
                }
                None => Ok(PythonImportReport::no_source()),
            };
        };

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
            source_format: IMPORT_SOURCE_FORMAT.to_owned(),
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

    /// Inspect only source metadata plus the credential-elided native snapshot.
    ///
    /// This method never creates the destination directory or lock and never
    /// opens either mixed Python configuration candidate.
    pub fn preview_state(&self) -> Result<PythonStatePreview, PortError> {
        validate_destination_root(&self.destination_root)?;
        let current = inspect_mixed_candidate(
            &self.current_source_path,
            PythonStateSourceKind::CurrentConfig,
        )?;
        let legacy = match &self.legacy_source_path {
            Some(path) => inspect_mixed_candidate(path, PythonStateSourceKind::LegacyConfig)?,
            None => absent_source_entry(
                &self.synthetic_legacy_locator(),
                PythonStateSourceKind::LegacyConfig.as_str(),
            )?,
        };
        let native = inspect_native_snapshot(&self.destination_path())?;
        let checked_source_set = source_set_fingerprint(current, legacy, native.entry.clone())?;
        let selected_source_kind = if checked_source_set.current_config.state == "metadata_present"
        {
            Some(PythonStateSourceKind::CurrentConfig)
        } else if checked_source_set.legacy_config.state == "metadata_present" {
            Some(PythonStateSourceKind::LegacyConfig)
        } else {
            None
        };
        let mixed_present = selected_source_kind.is_some();

        let Some((document, native_bytes_digest)) = native.document else {
            return if mixed_present {
                Ok(PythonStatePreview::ProtectedUninspectedMixedSource {
                    selected_locator_digest: selected_source_kind
                        .map(|kind| selected_locator_digest(kind, &checked_source_set)),
                    checked_source_set,
                    native_snapshot_digest: None,
                    reported_source_digest: None,
                    normalized_snapshot_digest: None,
                    reason: ProtectedHouseholdReason::UninspectedMixedSource,
                })
            } else {
                Ok(PythonStatePreview::NoSource { checked_source_set })
            };
        };

        let reported_source_digest =
            Sha256Digest(document.report.source_sha256.clone().ok_or_else(|| {
                PortError::new(
                    "python_snapshot_invalid",
                    "native import is missing source provenance",
                )
            })?);
        let normalized_snapshot_digest = normalized_state_digest(&document.state)?;
        let prior_keyring_not_read = document
            .report
            .dispositions
            .iter()
            .any(|item| item.action == PythonFieldAction::KeyringNotRead);
        if prior_keyring_not_read {
            return Ok(PythonStatePreview::ProtectedUninspectedMixedSource {
                selected_locator_digest: selected_source_kind
                    .map(|kind| selected_locator_digest(kind, &checked_source_set)),
                checked_source_set,
                native_snapshot_digest: Some(native_bytes_digest),
                reported_source_digest: Some(reported_source_digest),
                normalized_snapshot_digest: Some(normalized_snapshot_digest),
                reason: ProtectedHouseholdReason::PriorImporterSkippedKeyring,
            });
        }
        validate_safe_account_binding(&document.state)?;
        Ok(PythonStatePreview::SafeSnapshot {
            state: document.state,
            native_snapshot_digest: native_bytes_digest,
            normalized_state_digest: normalized_snapshot_digest,
            reported_source_digest,
            selected_source_kind,
            checked_source_set,
        })
    }

    /// Recheck the exact preview and, only then, read/import a selected mixed
    /// source. Callers invoke this only after the controlling-terminal `LOG`.
    pub fn verify_after_review(
        &self,
        reviewed: &PythonStatePreview,
    ) -> Result<VerifiedPythonState, PortError> {
        validate_destination_root(&self.destination_root)?;
        let current = self.preview_state()?;
        if &current != reviewed {
            return Err(PortError::new(
                "python_state_changed",
                "legacy household state changed after review",
            ));
        }
        match reviewed {
            PythonStatePreview::NoSource { .. } => Ok(VerifiedPythonState {
                state: None,
                pending_import_report: None,
                destination_root: self.destination_root.clone(),
            }),
            PythonStatePreview::SafeSnapshot {
                state,
                normalized_state_digest,
                reported_source_digest,
                selected_source_kind,
                checked_source_set,
                ..
            } => {
                if let Some(kind) = selected_source_kind {
                    create_private_dir(&self.destination_root)?;
                    let _lock =
                        FileLock::acquire(&self.destination_root.join(IMPORT_LOCK_NAME), false)?;
                    let expected = source_entry_for_kind(checked_source_set, *kind);
                    let bytes = read_source_bound(self.path_for_kind(*kind), *kind, expected)?;
                    verify_source_matches_snapshot(
                        &bytes,
                        state,
                        normalized_state_digest,
                        reported_source_digest,
                    )?;
                }
                Ok(VerifiedPythonState {
                    state: Some(state.clone()),
                    pending_import_report: None,
                    destination_root: self.destination_root.clone(),
                })
            }
            PythonStatePreview::ProtectedUninspectedMixedSource {
                checked_source_set,
                selected_locator_digest,
                native_snapshot_digest,
                reported_source_digest,
                normalized_snapshot_digest,
                reason,
                ..
            } => {
                let selected_kind = match selected_locator_digest {
                    Some(expected) => Some(
                        self.selected_kind_from_locator(Some(expected))
                            .ok_or_else(|| {
                                PortError::new(
                                    "python_state_changed",
                                    "selected legacy household locator changed after review",
                                )
                            })?,
                    ),
                    None => None,
                };
                if let Some(kind) = selected_kind {
                    validate_destination_root(&self.destination_root)?;
                    create_private_dir(&self.destination_root)?;
                    let _lock =
                        FileLock::acquire(&self.destination_root.join(IMPORT_LOCK_NAME), true)?;
                    let expected = source_entry_for_kind(checked_source_set, kind);
                    let bytes = read_source_bound(self.path_for_kind(kind), kind, expected)?;
                    let actual_source_digest = Sha256Digest(sha256(&bytes));
                    if let Some(expected) = reported_source_digest
                        && &actual_source_digest != expected
                    {
                        return Err(PortError::new(
                            "python_import_conflict",
                            "protected legacy state does not match its reviewed report",
                        ));
                    }
                    let (report, state) = build_import(&bytes, actual_source_digest.0.clone())?;
                    let state_digest = normalized_state_digest(&state)?;
                    if let Some(expected) = normalized_snapshot_digest
                        && &state_digest != expected
                    {
                        return Err(PortError::new(
                            "python_import_conflict",
                            "protected legacy state does not match its reviewed snapshot",
                        ));
                    }
                    if *reason == ProtectedHouseholdReason::PriorImporterSkippedKeyring
                        && !report
                            .dispositions
                            .iter()
                            .any(|item| item.action == PythonFieldAction::KeyringNotRead)
                    {
                        return Err(PortError::new(
                            "python_import_conflict",
                            "protected legacy state disposition changed after review",
                        ));
                    }
                    Ok(VerifiedPythonState {
                        state: Some(state),
                        pending_import_report: native_snapshot_digest.is_none().then_some(report),
                        destination_root: self.destination_root.clone(),
                    })
                } else {
                    create_private_dir(&self.destination_root)?;
                    let _lock =
                        FileLock::acquire(&self.destination_root.join(IMPORT_LOCK_NAME), false)?;
                    let (document, actual_native_digest) = read_native_document_bound(
                        &self.destination_path(),
                        &checked_source_set.native_snapshot,
                    )?;
                    if native_snapshot_digest.as_ref() != Some(&actual_native_digest)
                        || reported_source_digest.as_ref()
                            != document
                                .report
                                .source_sha256
                                .as_ref()
                                .map(|digest| Sha256Digest(digest.clone()))
                                .as_ref()
                        || normalized_snapshot_digest.as_ref()
                            != Some(&normalized_state_digest(&document.state)?)
                        || *reason != ProtectedHouseholdReason::PriorImporterSkippedKeyring
                        || !document
                            .report
                            .dispositions
                            .iter()
                            .any(|item| item.action == PythonFieldAction::KeyringNotRead)
                    {
                        return Err(PortError::new(
                            "python_import_conflict",
                            "protected native household snapshot changed after review",
                        ));
                    }
                    Ok(VerifiedPythonState {
                        state: Some(document.state),
                        pending_import_report: None,
                        destination_root: self.destination_root.clone(),
                    })
                }
            }
        }
    }

    fn path_for_kind(&self, kind: PythonStateSourceKind) -> &Path {
        match kind {
            PythonStateSourceKind::CurrentConfig => &self.current_source_path,
            PythonStateSourceKind::LegacyConfig => self
                .legacy_source_path
                .as_deref()
                .unwrap_or(&self.current_source_path),
        }
    }

    fn selected_source_path_post_approval(&self) -> Result<&Path, PortError> {
        let current = fs::symlink_metadata(&self.current_source_path);
        match current {
            Ok(_) => Ok(&self.current_source_path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(self
                .legacy_source_path
                .as_deref()
                .unwrap_or(&self.current_source_path)),
            Err(_) => Err(PortError::new(
                "python_import_read",
                "could not inspect the Python state source",
            )),
        }
    }

    fn selected_kind_from_locator(
        &self,
        expected: Option<&Sha256Digest>,
    ) -> Option<PythonStateSourceKind> {
        let expected = expected?;
        [
            PythonStateSourceKind::CurrentConfig,
            PythonStateSourceKind::LegacyConfig,
        ]
        .into_iter()
        .find(|kind| {
            let path = self.path_for_kind(*kind);
            locator_digest(path)
                .map(|digest| selected_locator_digest_for(*kind, &digest) == *expected)
                .unwrap_or(false)
        })
    }

    fn synthetic_legacy_locator(&self) -> PathBuf {
        self.current_source_path.with_extension("legacy-absent")
    }
}

#[derive(Clone, Eq, PartialEq)]
struct StrictSnapshotV1 {
    bytes_digest: CanonicalDigestV1,
    path_locator_digest: CanonicalDigestV1,
    reported_source_digest: String,
    normalized_state_digest: CanonicalDigestV1,
    document: ImportDocument,
}

fn legacy_root_ambiguous() -> PortError {
    PortError::new(
        "legacy_python_config_root_ambiguous",
        "legacy Python configuration root is ambiguous; relaunch with XDG_CONFIG_HOME unset or set to the one absolute root that created the legacy state",
    )
}

fn phase_b_error(message: &'static str) -> PortError {
    PortError::new("legacy_python_semantic_validation", message)
}

fn canonical_phase_a_error(_: heyfood_core::CanonicalJsonError) -> PortError {
    PortError::new(
        "legacy_python_source_syntax",
        "legacy Python state violates the bounded canonical JSON contract",
    )
}

fn canonical_phase_b_error(_: impl fmt::Debug) -> PortError {
    phase_b_error("legacy household state violates the canonical migration contract")
}

fn vault_readback_error(_: impl fmt::Debug) -> PortError {
    PortError::new(
        "legacy_python_vault_readback_mismatch",
        "authenticated household vault readback violates the exact reserved migration result",
    )
}

async fn migration_checkpoint(cancellation: &CancellationToken) -> Result<(), PortError> {
    if cancellation.is_cancelled() {
        return Err(PortError::new(
            "legacy_python_migration_cancelled",
            "legacy household migration was cancelled",
        ));
    }
    tokio::task::yield_now().await;
    if cancellation.is_cancelled() {
        return Err(PortError::new(
            "legacy_python_migration_cancelled",
            "legacy household migration was cancelled",
        ));
    }
    Ok(())
}

fn sibling_config_lock_path(config_path: &Path) -> Result<PathBuf, PortError> {
    if config_path.file_name().and_then(OsStr::to_str) != Some("config.json") {
        return Err(legacy_root_ambiguous());
    }
    Ok(config_path
        .parent()
        .ok_or_else(legacy_root_ambiguous)?
        .join("config.lock"))
}

fn acquire_source_file_lock_blocking(
    path: &Path,
    cancellation: &CancellationToken,
) -> Result<LegacySourceFileLock, PortError> {
    if cancellation.is_cancelled() {
        return Err(PortError::new(
            "legacy_python_migration_cancelled",
            "legacy household migration was cancelled while acquiring its source locks",
        ));
    }

    #[cfg(test)]
    let observer = take_legacy_source_lock_acquisition_observer(path);
    #[cfg(test)]
    if let Some(attempt) = observer
        .as_ref()
        .and_then(|observer| observer.attempt.as_ref())
    {
        let _ = attempt.send(());
    }

    let lock = match FileLock::acquire(path, true) {
        Ok(lock) => LegacySourceFileLock::new(lock),
        Err(_) if cancellation.is_cancelled() => {
            return Err(PortError::new(
                "legacy_python_migration_cancelled",
                "legacy household migration was cancelled while acquiring its source locks",
            ));
        }
        Err(error) => {
            return Err(PortError::new(
                "legacy_python_source_lock",
                format!(
                    "legacy household source lock could not be acquired: {}",
                    error.code
                ),
            ));
        }
    };
    #[cfg(test)]
    let lock = {
        let mut lock = lock;
        if let Some(observer) = observer {
            lock.drop_observer = Some(observer.drop_observer);
        }
        lock
    };
    if cancellation.is_cancelled() {
        drop(lock);
        return Err(PortError::new(
            "legacy_python_migration_cancelled",
            "legacy household migration was cancelled while acquiring its source locks",
        ));
    }
    Ok(lock)
}

fn legacy_source_lock_task_error(_: tokio::task::JoinError) -> PortError {
    PortError::new(
        "legacy_python_source_lock_task",
        "legacy household source lock task did not complete",
    )
}

fn sync_snapshot_parent(path: &Path) -> Result<(), PortError> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| {
                PortError::uncertain(
                    "legacy_python_snapshot_retirement_uncertain",
                    "legacy import snapshot directory durability could not be confirmed",
                )
            })
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(PortError::new(
            "household_secure_store_unavailable",
            "legacy import snapshot retirement is unavailable on this platform",
        ))
    }
}

fn resolve_strict_false(path: &Path) -> Result<PathBuf, PortError> {
    if !path.is_absolute() {
        return Err(legacy_root_ambiguous());
    }
    let mut existing = path;
    let mut suffix = Vec::new();
    loop {
        match fs::symlink_metadata(existing) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = existing.file_name().ok_or_else(legacy_root_ambiguous)?;
                suffix.push(name.to_os_string());
                existing = existing.parent().ok_or_else(legacy_root_ambiguous)?;
            }
            Err(_) => return Err(legacy_root_ambiguous()),
        }
    }
    let mut resolved = fs::canonicalize(existing).map_err(|_| legacy_root_ambiguous())?;
    for component in suffix.into_iter().rev() {
        resolved.push(component);
    }
    let resolved = lexical_normalize_absolute(&resolved)?;
    match fs::metadata(&resolved) {
        Ok(metadata) if !metadata.is_dir() => Err(legacy_root_ambiguous()),
        Ok(_) => Ok(resolved),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(resolved),
        Err(_) => Err(legacy_root_ambiguous()),
    }
}

fn lexical_normalize_absolute(path: &Path) -> Result<PathBuf, PortError> {
    use std::path::Component;

    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => output.push(prefix.as_os_str()),
            Component::RootDir => output.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                if !output.pop() {
                    return Err(legacy_root_ambiguous());
                }
            }
            Component::Normal(value) => output.push(value),
        }
    }
    if !output.is_absolute() {
        return Err(legacy_root_ambiguous());
    }
    Ok(output)
}

fn validate_account_slot_binding(
    account: &AccountId,
    account_slot: &HouseholdAccountSlotV1,
) -> Result<(), PortError> {
    let expected = domain_hash_v1(
        "heyfood.household.account-digest.v1",
        &[account.as_str().as_bytes()],
    )
    .map_err(canonical_phase_a_error)?;
    if expected.as_bytes() != &account_slot.account_digest() {
        return Err(PortError::new(
            "legacy_python_account_mismatch",
            "authenticated account does not match the native household account slot",
        ));
    }
    Ok(())
}

fn keyring_outcome_contract(
    outcome: &LegacyPythonKeyringProbeOutcomeV1,
) -> (
    &'static str,
    HouseholdBrokerOperationV1,
    Option<CanonicalDigestV1>,
) {
    match outcome {
        LegacyPythonKeyringProbeOutcomeV1::AuthoritativeMissing => (
            "authoritative_missing",
            HouseholdBrokerOperationV1::LegacyPythonHouseholdProbe,
            None,
        ),
        LegacyPythonKeyringProbeOutcomeV1::Present(bytes) => (
            "present",
            HouseholdBrokerOperationV1::LegacyPythonHouseholdLoad,
            Some(digest_bytes(bytes)),
        ),
        LegacyPythonKeyringProbeOutcomeV1::Unavailable => (
            "unavailable",
            HouseholdBrokerOperationV1::LegacyPythonHouseholdProbe,
            None,
        ),
    }
}

fn bind_keyring_probe(
    kind: LegacyPythonConfigKindV1,
    account_slot: &HouseholdAccountSlotV1,
    config_root: &LegacyPythonConfigRootV1,
    outcome: LegacyPythonKeyringProbeOutcomeV1,
) -> Result<LegacyPythonKeyringProbeV1, PortError> {
    let (outcome_label, operation, payload_digest) = keyring_outcome_contract(&outcome);
    let binding = LegacyPythonKeyringEvidenceBindingV1 {
        contract: D2_KEYRING_EVIDENCE_CONTRACT,
        config_kind: kind,
        operation: operation.action(),
        account_digest: encode_lower_hex(&account_slot.account_digest()),
        native_root_instance_digest: encode_lower_hex(&account_slot.native_root_instance_digest()),
        account_locator_digest: encode_lower_hex(&account_slot.account_locator_digest()),
        legacy_locator_digest: keyring_locator_digest(config_root.config_path(kind))?
            .to_lower_hex(),
        outcome: outcome_label,
        payload_digest: payload_digest.map(CanonicalDigestV1::to_lower_hex),
    };
    let evidence_digest = canonical_sha256_v1(&binding).map_err(canonical_phase_a_error)?;
    Ok(LegacyPythonKeyringProbeV1 {
        binding,
        evidence_digest,
        outcome,
    })
}

fn validate_keyring_probe_set(
    probes: &LegacyPythonKeyringProbeSetV1,
    account_slot: &HouseholdAccountSlotV1,
    config_root: &LegacyPythonConfigRootV1,
) -> Result<(), PortError> {
    validate_keyring_probe(
        &probes.current,
        LegacyPythonConfigKindV1::Current,
        account_slot,
        config_root,
    )?;
    validate_keyring_probe(
        &probes.legacy,
        LegacyPythonConfigKindV1::Legacy,
        account_slot,
        config_root,
    )
}

fn validate_keyring_probe(
    probe: &LegacyPythonKeyringProbeV1,
    expected_kind: LegacyPythonConfigKindV1,
    account_slot: &HouseholdAccountSlotV1,
    config_root: &LegacyPythonConfigRootV1,
) -> Result<(), PortError> {
    let expected = bind_keyring_probe(
        expected_kind,
        account_slot,
        config_root,
        probe.outcome.clone(),
    )?;
    if probe.binding != expected.binding
        || probe.evidence_digest != expected.evidence_digest
        || probe.binding.config_kind != expected_kind
    {
        return Err(PortError::new(
            "legacy_python_keyring_evidence_mismatch",
            "historical Python keyring evidence does not match its exact account, native root, locator, broker operation, outcome, or payload",
        ));
    }
    Ok(())
}

fn read_strict_config(
    path: &Path,
    kind: LegacyPythonConfigKindV1,
) -> Result<Option<StrictConfigDocumentV1>, PortError> {
    let Some(bytes) = read_source(path)? else {
        return Ok(None);
    };
    let object =
        parse_bounded_json_object_v1(&bytes, CompatibilityJsonLimitsV1::MIGRATION_CANDIDATE)
            .map_err(canonical_phase_a_error)?;
    Ok(Some(StrictConfigDocumentV1 {
        kind,
        bytes_digest: digest_bytes(&bytes),
        bytes,
        object,
    }))
}

fn parse_keyring_probe(
    probe: &LegacyPythonKeyringProbeV1,
    expected_kind: LegacyPythonConfigKindV1,
    account_slot: &HouseholdAccountSlotV1,
    config_root: &LegacyPythonConfigRootV1,
) -> Result<Option<StrictKeyringDocumentV1>, PortError> {
    validate_keyring_probe(probe, expected_kind, account_slot, config_root)?;
    match &probe.outcome {
        LegacyPythonKeyringProbeOutcomeV1::AuthoritativeMissing => Ok(None),
        LegacyPythonKeyringProbeOutcomeV1::Unavailable => Err(PortError::new(
            "legacy_python_source_probe_unavailable",
            "a historical Python keyring target could not be authoritatively inspected; repair secure-store access before migration",
        )),
        LegacyPythonKeyringProbeOutcomeV1::Present(bytes) => {
            let object =
                parse_bounded_json_object_v1(bytes, CompatibilityJsonLimitsV1::MIGRATION_CANDIDATE)
                    .map_err(canonical_phase_a_error)?;
            if object.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    D2_KEYRING_HOUSEHOLD_STATE
                        | D2_KEYRING_LOCAL_PROFILES
                        | D2_KEYRING_PROFILE_OUTBOX
                )
            }) {
                return Err(PortError::new(
                    "legacy_python_keyring_format",
                    "historical keyring household response contains an unsupported field",
                ));
            }
            let household = optional_object_clone(&object, D2_KEYRING_HOUSEHOLD_STATE)?;
            let local_profiles = optional_object_clone(&object, D2_KEYRING_LOCAL_PROFILES)?;
            let profile_outbox = optional_object_clone(&object, D2_KEYRING_PROFILE_OUTBOX)?;
            Ok(Some(StrictKeyringDocumentV1 {
                document_digest: digest_bytes(bytes),
                household,
                local_profiles,
                profile_outbox,
            }))
        }
    }
}

fn optional_object_clone(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<Map<String, Value>>, PortError> {
    object
        .get(key)
        .map(|value| {
            value.as_object().cloned().ok_or_else(|| {
                PortError::new(
                    "legacy_python_source_shape",
                    "legacy household source contains a non-object household document",
                )
            })
        })
        .transpose()
}

fn read_strict_snapshot(path: &Path) -> Result<Option<StrictSnapshotV1>, PortError> {
    if !path.is_absolute() || path.file_name().and_then(OsStr::to_str) != Some(IMPORT_FILE_NAME) {
        return Err(legacy_root_ambiguous());
    }
    let Some(bytes) = read_source(path)? else {
        return Ok(None);
    };
    parse_bounded_json_object_v1(&bytes, CompatibilityJsonLimitsV1::MIGRATION_CANDIDATE)
        .map_err(canonical_phase_a_error)?;
    let document = decode_import_document(&bytes)?;
    let reported_source_digest = document.report.source_sha256.clone().ok_or_else(|| {
        PortError::new(
            "legacy_python_snapshot_provenance",
            "native import snapshot lacks exact source provenance",
        )
    })?;
    if !valid_sha256(&reported_source_digest) {
        return Err(PortError::new(
            "legacy_python_snapshot_provenance",
            "native import snapshot has invalid source provenance",
        ));
    }
    Ok(Some(StrictSnapshotV1 {
        bytes_digest: digest_bytes(&bytes),
        path_locator_digest: path_locator_digest(path)?,
        reported_source_digest,
        normalized_state_digest: canonical_sha256_v1(&document.state)
            .map_err(canonical_phase_a_error)?,
        document,
    }))
}

fn validate_snapshot_binding(
    snapshot: Option<&StrictSnapshotV1>,
    selected: &StrictConfigDocumentV1,
    account: &AccountId,
    account_slot: &HouseholdAccountSlotV1,
    snapshot_path: &Path,
) -> Result<(), PortError> {
    let Some(snapshot) = snapshot else {
        return Ok(());
    };
    validate_account_slot_binding(account, account_slot)?;
    if snapshot.path_locator_digest != path_locator_digest(snapshot_path)?
        || snapshot.reported_source_digest != selected.bytes_digest.to_lower_hex()
        || snapshot.document.state.account_user_id.as_deref() != Some(account.as_str())
    {
        return Err(PortError::new(
            "legacy_python_snapshot_provenance",
            "native import snapshot does not match the authenticated account, native slot, path, or selected Python configuration",
        ));
    }
    let (rebuilt_report, rebuilt_state) =
        build_import(&selected.bytes, selected.bytes_digest.to_lower_hex())?;
    let rebuilt_normalized_state_digest =
        canonical_sha256_v1(&rebuilt_state).map_err(canonical_phase_a_error)?;
    if snapshot.document.report != rebuilt_report
        || snapshot.document.state != rebuilt_state
        || snapshot.normalized_state_digest != rebuilt_normalized_state_digest
    {
        return Err(PortError::new(
            "legacy_python_snapshot_provenance",
            "native import snapshot cannot be rebuilt exactly from its selected Python source",
        ));
    }
    Ok(())
}

fn validate_selected_account(
    selected: &StrictConfigDocumentV1,
    authenticated: &AccountId,
) -> Result<(), PortError> {
    let direct = selected.object.get("account_user_id");
    let session = selected
        .object
        .get("session")
        .and_then(Value::as_object)
        .and_then(|session| session.get("user_id"));
    let direct = direct
        .map(|value| {
            value.as_str().ok_or_else(|| {
                PortError::new(
                    "legacy_python_account_mismatch",
                    "legacy Python account binding is invalid",
                )
            })
        })
        .transpose()?;
    let session = session
        .map(|value| {
            value.as_str().ok_or_else(|| {
                PortError::new(
                    "legacy_python_account_mismatch",
                    "legacy Python session account binding is invalid",
                )
            })
        })
        .transpose()?;
    if direct.is_some_and(|value| AccountId::parse(value).is_err())
        || session.is_some_and(|value| AccountId::parse(value).is_err())
        || direct
            .zip(session)
            .is_some_and(|(left, right)| left != right)
    {
        return Err(PortError::new(
            "legacy_python_account_mismatch",
            "legacy Python account bindings are invalid or contradictory",
        ));
    }
    let selected_account = direct.or(session).ok_or_else(|| {
        PortError::new(
            "legacy_python_account_mismatch",
            "legacy Python source is not bound to an authenticated account",
        )
    })?;
    if selected_account != authenticated.as_str() {
        return Err(PortError::new(
            "legacy_python_account_mismatch",
            "legacy Python source is bound to a different account",
        ));
    }
    Ok(())
}

fn parse_credential_store(
    object: &Map<String, Value>,
) -> Result<LegacyCredentialStoreV1, PortError> {
    match object.get("credential_store") {
        None => Ok(LegacyCredentialStoreV1::File),
        Some(Value::String(value)) if value == "file" => Ok(LegacyCredentialStoreV1::File),
        Some(Value::String(value)) if value == "keyring" => Ok(LegacyCredentialStoreV1::Keyring),
        _ => Err(PortError::new(
            "legacy_python_credential_store",
            "legacy Python credential-store marker is invalid",
        )),
    }
}

fn contains_prohibited_credential_field(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            let normalized = key.to_ascii_lowercase();
            CREDENTIAL_FIELDS.contains(&normalized.as_str())
                || matches!(
                    normalized.as_str(),
                    "access_token"
                        | "refresh_token"
                        | "id_token"
                        | "password"
                        | "secret"
                        | "client_secret"
                        | "authorization"
                )
                || contains_prohibited_credential_field(value)
        }),
        Value::Array(values) => values.iter().any(contains_prohibited_credential_field),
        _ => false,
    }
}

struct LegacyCandidateConfigProjectionV1 {
    first_name: Option<CanonicalJsonValueV1>,
    compatibility_fields: BTreeMap<String, CanonicalJsonValueV1>,
    config_field_evidence: Vec<LegacyConfigFieldEvidenceV1>,
}

fn build_secret_free_candidate_config(
    object: &Map<String, Value>,
) -> Result<LegacyCandidateConfigProjectionV1, PortError> {
    const COMPATIBILITY_MIGRATED_FIELDS: &[&str] = &["last_restaurant_search", "location"];
    const COMPATIBILITY_RETIRED_FIELDS: &[&str] = &[
        "active_context",
        "api_url",
        "auth_url",
        "contexts",
        "device_id",
        "first_name_updated_at",
        "last_conversation",
        "last_recipe_search",
        "voice",
        "welcomed_at",
    ];

    let mut first_name = None;
    let mut compatibility_fields = BTreeMap::new();
    let mut evidence = Vec::with_capacity(object.len());
    for (field_name, value) in object {
        let role = match field_name.as_str() {
            "account_user_id" => LegacyConfigFieldRoleV1::AccountBinding,
            "first_name" => LegacyConfigFieldRoleV1::OwnerName,
            "household" => LegacyConfigFieldRoleV1::Household,
            "household_local_profiles" => LegacyConfigFieldRoleV1::LocalProfiles,
            "household_profile_outbox" => LegacyConfigFieldRoleV1::ProfileOutbox,
            "credential_store" => LegacyConfigFieldRoleV1::CredentialMarker,
            field if CREDENTIAL_FIELDS.contains(&field) => LegacyConfigFieldRoleV1::Credential,
            field if COMPATIBILITY_MIGRATED_FIELDS.contains(&field) => {
                LegacyConfigFieldRoleV1::CompatibilityMigrated
            }
            field if COMPATIBILITY_RETIRED_FIELDS.contains(&field) => {
                LegacyConfigFieldRoleV1::CompatibilityRetired
            }
            _ => LegacyConfigFieldRoleV1::Unknown,
        };
        let may_retain_value = !matches!(
            role,
            LegacyConfigFieldRoleV1::Credential
                | LegacyConfigFieldRoleV1::CredentialMarker
                | LegacyConfigFieldRoleV1::Unknown
                | LegacyConfigFieldRoleV1::Household
                | LegacyConfigFieldRoleV1::LocalProfiles
                | LegacyConfigFieldRoleV1::ProfileOutbox
        );
        let canonical = may_retain_value
            .then(|| {
                CanonicalJsonValueV1::from_value(value.clone(), MAX_MIGRATION_CANDIDATE_BYTES)
                    .map_err(canonical_phase_a_error)
            })
            .transpose()?;
        if role == LegacyConfigFieldRoleV1::OwnerName {
            first_name = canonical.clone();
        }
        if matches!(
            role,
            LegacyConfigFieldRoleV1::CompatibilityMigrated
                | LegacyConfigFieldRoleV1::CompatibilityRetired
        ) {
            compatibility_fields.insert(
                field_name.clone(),
                canonical
                    .clone()
                    .ok_or_else(|| phase_b_error("compatibility field retention failed"))?,
            );
        }
        evidence.push(LegacyConfigFieldEvidenceV1 {
            field_name: field_name.clone(),
            role,
            source_digest: if matches!(
                role,
                LegacyConfigFieldRoleV1::Credential
                    | LegacyConfigFieldRoleV1::CredentialMarker
                    | LegacyConfigFieldRoleV1::Unknown
            ) {
                None
            } else {
                Some(canonical_sha256_v1(value).map_err(canonical_phase_a_error)?)
            },
        });
    }
    evidence.sort_by(|left, right| left.field_name.cmp(&right.field_name));
    if evidence
        .windows(2)
        .any(|pair| pair[0].field_name == pair[1].field_name)
    {
        return Err(PortError::new(
            "legacy_python_source_syntax",
            "legacy Python state contains a duplicate top-level field",
        ));
    }
    Ok(LegacyCandidateConfigProjectionV1 {
        first_name,
        compatibility_fields,
        config_field_evidence: evidence,
    })
}

struct MergedHouseholdDocumentsV1 {
    household: Option<LegacyBoundDocumentV1>,
    local_profiles: Option<LegacyBoundDocumentV1>,
    profile_outbox: Option<LegacyBoundDocumentV1>,
}

fn merge_household_documents(
    selected: &StrictConfigDocumentV1,
    keyring: Option<&StrictKeyringDocumentV1>,
    credential_store: LegacyCredentialStoreV1,
) -> Result<MergedHouseholdDocumentsV1, PortError> {
    let file_household = optional_object_clone(&selected.object, "household")?;
    let file_profiles = optional_object_clone(&selected.object, "household_local_profiles")?;
    let file_outbox = optional_object_clone(&selected.object, "household_profile_outbox")?;
    match credential_store {
        LegacyCredentialStoreV1::File => Ok(MergedHouseholdDocumentsV1 {
            household: bind_legacy_document(file_household, LegacyDocumentSourceV1::File)?,
            local_profiles: bind_legacy_document(file_profiles, LegacyDocumentSourceV1::File)?,
            profile_outbox: bind_legacy_document(file_outbox, LegacyDocumentSourceV1::File)?,
        }),
        LegacyCredentialStoreV1::Keyring => {
            let keyring = keyring.ok_or_else(|| {
                PortError::new(
                    "legacy_python_keyring_missing",
                    "keyring-backed legacy Python state has no authoritative historical keyring entry",
                )
            })?;
            Ok(MergedHouseholdDocumentsV1 {
                household: bind_preferred_legacy_document(
                    keyring.household.clone(),
                    file_household,
                )?,
                local_profiles: bind_preferred_legacy_document(
                    keyring.local_profiles.clone(),
                    file_profiles,
                )?,
                profile_outbox: bind_preferred_legacy_document(
                    keyring.profile_outbox.clone(),
                    file_outbox,
                )?,
            })
        }
    }
}

fn bind_legacy_document(
    value: Option<Map<String, Value>>,
    source: LegacyDocumentSourceV1,
) -> Result<Option<LegacyBoundDocumentV1>, PortError> {
    value
        .map(|value| {
            let source_digest = canonical_sha256_v1(&Value::Object(value.clone()))
                .map_err(canonical_phase_a_error)?;
            Ok(LegacyBoundDocumentV1 {
                value: CredentialFreeLegacyObjectV1::new(value)?,
                source,
                source_digest,
            })
        })
        .transpose()
}

fn bind_preferred_legacy_document(
    keyring: Option<Map<String, Value>>,
    file: Option<Map<String, Value>>,
) -> Result<Option<LegacyBoundDocumentV1>, PortError> {
    match keyring {
        Some(value) => bind_legacy_document(Some(value), LegacyDocumentSourceV1::Keyring),
        None => bind_legacy_document(file, LegacyDocumentSourceV1::File),
    }
}

#[allow(clippy::too_many_arguments)]
fn build_present_manifest(
    selected: &StrictConfigDocumentV1,
    snapshot: Option<&StrictSnapshotV1>,
    credential_store: LegacyCredentialStoreV1,
    household: Option<&LegacyBoundDocumentV1>,
    local_profiles: Option<&LegacyBoundDocumentV1>,
    profile_outbox: Option<&LegacyBoundDocumentV1>,
    current: &Option<StrictConfigDocumentV1>,
    legacy: &Option<StrictConfigDocumentV1>,
    current_keyring: Option<&StrictKeyringDocumentV1>,
    legacy_keyring: Option<&StrictKeyringDocumentV1>,
    roots: &LegacyPythonConfigRootV1,
    account_slot: &HouseholdAccountSlotV1,
    snapshot_path: &Path,
) -> Result<LegacySourceBundleManifestV1, PortError> {
    let mut ignored_sources = Vec::new();
    for (kind, config) in [
        (LegacyPythonConfigKindV1::Current, current.as_ref()),
        (LegacyPythonConfigKindV1::Legacy, legacy.as_ref()),
    ] {
        if kind == selected.kind {
            continue;
        }
        ignored_sources.push(LegacyIgnoredSourceV1 {
            kind: kind.source_kind(),
            locator_digest: path_locator_digest(roots.config_path(kind))?.to_lower_hex(),
            state: if config.is_some() {
                "present"
            } else {
                "absent"
            },
            content_digest: config.map(|value| value.bytes_digest.to_lower_hex()),
        });
    }
    for (kind, document) in [
        (LegacyPythonConfigKindV1::Current, current_keyring),
        (LegacyPythonConfigKindV1::Legacy, legacy_keyring),
    ] {
        let selected_keyring_used =
            kind == selected.kind && credential_store == LegacyCredentialStoreV1::Keyring;
        if selected_keyring_used {
            continue;
        }
        ignored_sources.push(LegacyIgnoredSourceV1 {
            kind: kind.keyring_kind(),
            locator_digest: keyring_locator_digest(roots.config_path(kind))?.to_lower_hex(),
            state: if document.is_some() {
                "present"
            } else {
                "absent"
            },
            content_digest: document.map(|value| value.document_digest.to_lower_hex()),
        });
    }
    ignored_sources.sort_by(|left, right| left.kind.cmp(right.kind));
    Ok(LegacySourceBundleManifestV1 {
        contract: D2_SOURCE_MANIFEST_CONTRACT,
        account_digest: encode_lower_hex(&account_slot.account_digest()),
        native_root_instance_digest: encode_lower_hex(&account_slot.native_root_instance_digest()),
        account_locator_digest: encode_lower_hex(&account_slot.account_locator_digest()),
        selected_locator_kind: selected.kind.source_kind(),
        selected_locator_digest: path_locator_digest(roots.config_path(selected.kind))?
            .to_lower_hex(),
        config_file_digest: selected.bytes_digest.to_lower_hex(),
        matching_snapshot_digest: snapshot.map(|value| value.bytes_digest.to_lower_hex()),
        matching_snapshot_locator_digest: path_locator_digest(snapshot_path)?.to_lower_hex(),
        matching_snapshot_normalized_state_digest: snapshot
            .map(|value| value.normalized_state_digest.to_lower_hex()),
        credential_store,
        household_digest: household.map(|value| value.source_digest.to_lower_hex()),
        local_profiles_digest: local_profiles.map(|value| value.source_digest.to_lower_hex()),
        profile_outbox_digest: profile_outbox.map(|value| value.source_digest.to_lower_hex()),
        ignored_sources,
    })
}

fn no_source_result(
    account: &AccountId,
    account_slot: &HouseholdAccountSlotV1,
    roots: &LegacyPythonConfigRootV1,
    snapshot_path: &Path,
    keyring: &LegacyPythonKeyringProbeSetV1,
) -> Result<LegacyPythonPhaseAResultV1, PortError> {
    #[derive(Serialize)]
    struct SourceSet {
        contract: &'static str,
        account_digest: String,
        native_root_instance_digest: String,
        probes: Vec<LegacyNoSourceProbeV1>,
    }

    let mut probes = vec![
        LegacyNoSourceProbeV1 {
            kind: D2_CURRENT_CONFIG_KIND,
            locator_digest: path_locator_digest(
                roots.config_path(LegacyPythonConfigKindV1::Current),
            )?
            .to_lower_hex(),
            state: "absent",
            evidence_digest: None,
            content_digest: None,
        },
        LegacyNoSourceProbeV1 {
            kind: D2_LEGACY_CONFIG_KIND,
            locator_digest: path_locator_digest(
                roots.config_path(LegacyPythonConfigKindV1::Legacy),
            )?
            .to_lower_hex(),
            state: "absent",
            evidence_digest: None,
            content_digest: None,
        },
        no_source_keyring_probe(
            D2_CURRENT_KEYRING_KIND,
            &keyring.current,
            roots.config_path(LegacyPythonConfigKindV1::Current),
        )?,
        no_source_keyring_probe(
            D2_LEGACY_KEYRING_KIND,
            &keyring.legacy,
            roots.config_path(LegacyPythonConfigKindV1::Legacy),
        )?,
        LegacyNoSourceProbeV1 {
            kind: D2_SNAPSHOT_KIND,
            locator_digest: path_locator_digest(snapshot_path)?.to_lower_hex(),
            state: "absent",
            evidence_digest: None,
            content_digest: None,
        },
    ];
    probes.sort_by(|left, right| left.kind.cmp(right.kind));
    let source_set_fingerprint = canonical_sha256_v1(&SourceSet {
        contract: D2_SOURCE_SET_CONTRACT,
        account_digest: encode_lower_hex(&account_slot.account_digest()),
        native_root_instance_digest: encode_lower_hex(&account_slot.native_root_instance_digest()),
        probes,
    })
    .map_err(canonical_phase_a_error)?;
    Ok(LegacyPythonPhaseAResultV1::NoSource {
        account: account.clone(),
        source_set_fingerprint,
    })
}

fn no_source_keyring_probe(
    kind: &'static str,
    probe: &LegacyPythonKeyringProbeV1,
    config_path: &Path,
) -> Result<LegacyNoSourceProbeV1, PortError> {
    let (_, _, payload_digest) = keyring_outcome_contract(&probe.outcome);
    Ok(LegacyNoSourceProbeV1 {
        kind,
        locator_digest: keyring_locator_digest(config_path)?.to_lower_hex(),
        state: match probe.outcome {
            LegacyPythonKeyringProbeOutcomeV1::AuthoritativeMissing => "absent",
            LegacyPythonKeyringProbeOutcomeV1::Present(_) => "present_without_household_data",
            LegacyPythonKeyringProbeOutcomeV1::Unavailable => "unavailable",
        },
        evidence_digest: Some(probe.evidence_digest.to_lower_hex()),
        content_digest: payload_digest.map(CanonicalDigestV1::to_lower_hex),
    })
}

fn path_locator_digest(path: &Path) -> Result<CanonicalDigestV1, PortError> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        if !path.is_absolute() {
            return Err(legacy_root_ambiguous());
        }
        Ok(digest_bytes(path.as_os_str().as_bytes()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(PortError::new(
            "household_secure_store_unavailable",
            "legacy Python migration locator identity is unavailable on this platform",
        ))
    }
}

fn keyring_locator_digest(path: &Path) -> Result<CanonicalDigestV1, PortError> {
    #[cfg(unix)]
    #[derive(Serialize)]
    struct Locator<'a> {
        service: &'a str,
        username: &'a str,
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let locator = LegacyPythonKeyringLocatorV1::from_resolved_config_path_bytes(
            path.as_os_str().as_bytes(),
        )?;
        debug_assert_eq!(locator.service, LEGACY_PYTHON_KEYRING_SERVICE);
        canonical_sha256_v1(&Locator {
            service: locator.service,
            username: &locator.username,
        })
        .map_err(canonical_phase_a_error)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(PortError::new(
            "household_secure_store_unavailable",
            "legacy Python keyring locator identity is unavailable on this platform",
        ))
    }
}

fn digest_bytes(bytes: &[u8]) -> CanonicalDigestV1 {
    CanonicalDigestV1::from_bytes(Sha256::digest(bytes).into())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LegacyHouseholdFamilyV1 {
    Missing,
    PythonNormalizedV1,
    RustInstalledShowcasePartialV1,
    RustFixtureV4,
    RustUnversionedExplicitOwnerV0,
    RustUnversionedImplicitOwnerV0,
    PythonKeyringRawPartialV0,
}

#[derive(Clone)]
struct ParsedLegacyMemberV1 {
    id: String,
    name: String,
    relationship: Option<String>,
    archived: bool,
    profile_synced: Option<bool>,
    date_of_birth: Option<String>,
    created_at: Option<String>,
    is_owner: Option<bool>,
}

struct ClassifiedLegacyHouseholdV1 {
    family: LegacyHouseholdFamilyV1,
    members: Vec<ParsedLegacyMemberV1>,
    active_scope: Option<String>,
    updated_at: Option<String>,
    applied_mutation_ids: Vec<String>,
}

fn classify_household(
    candidate: &LegacyPythonPresentCandidateV1,
) -> Result<ClassifiedLegacyHouseholdV1, PortError> {
    let Some(household) = candidate
        .household
        .as_ref()
        .map(|document| document.value.as_map())
    else {
        if candidate.local_profiles.is_some() || candidate.profile_outbox.is_some() {
            return Err(phase_b_error(
                "legacy profiles or outbox cannot exist without a household",
            ));
        }
        return Ok(ClassifiedLegacyHouseholdV1 {
            family: LegacyHouseholdFamilyV1::Missing,
            members: Vec::new(),
            active_scope: None,
            updated_at: None,
            applied_mutation_ids: Vec::new(),
        });
    };
    let version = household.get("version");
    if let Some(version) = version {
        let version = version.as_u64().ok_or_else(|| {
            phase_b_error("legacy household version must be an exact supported integer")
        })?;
        return match version {
            1 if exact_object_keys(
                household,
                &[
                    "active_scope",
                    "applied_mutation_ids",
                    "members",
                    "owner_id",
                    "updated_at",
                    "version",
                ],
            ) =>
            {
                if household.get("owner_id").and_then(Value::as_str) != Some("_self") {
                    return Err(phase_b_error(
                        "normalized legacy household owner evidence is invalid",
                    ));
                }
                let members =
                    parse_member_array(household, LegacyHouseholdFamilyV1::PythonNormalizedV1)?;
                let applied_mutation_ids = parse_applied_mutation_ids(household)?;
                Ok(ClassifiedLegacyHouseholdV1 {
                    family: LegacyHouseholdFamilyV1::PythonNormalizedV1,
                    members,
                    active_scope: Some(required_string(household, "active_scope")?.to_owned()),
                    updated_at: Some(required_string(household, "updated_at")?.to_owned()),
                    applied_mutation_ids,
                })
            }
            1 if exact_object_keys(household, &["active_scope", "members", "version"]) => {
                Ok(ClassifiedLegacyHouseholdV1 {
                    family: LegacyHouseholdFamilyV1::RustInstalledShowcasePartialV1,
                    members: parse_member_array(
                        household,
                        LegacyHouseholdFamilyV1::RustInstalledShowcasePartialV1,
                    )?,
                    active_scope: Some(required_string(household, "active_scope")?.to_owned()),
                    updated_at: None,
                    applied_mutation_ids: Vec::new(),
                })
            }
            4 if exact_object_keys(household, &["active_scope", "members", "version"]) => {
                Ok(ClassifiedLegacyHouseholdV1 {
                    family: LegacyHouseholdFamilyV1::RustFixtureV4,
                    members: parse_member_array(household, LegacyHouseholdFamilyV1::RustFixtureV4)?,
                    active_scope: Some(required_string(household, "active_scope")?.to_owned()),
                    updated_at: None,
                    applied_mutation_ids: Vec::new(),
                })
            }
            _ => Err(phase_b_error(
                "legacy household version or exact family shape is unsupported",
            )),
        };
    }
    if !exact_optional_object_keys(household, &["members"], &["active_scope"]) {
        return Err(phase_b_error(
            "unversioned legacy household contains unsupported fields",
        ));
    }
    let raw_members = household
        .get("members")
        .and_then(Value::as_array)
        .ok_or_else(|| phase_b_error("legacy household members must be an array"))?;
    let has_explicit_owner = raw_members.iter().any(|member| {
        member
            .as_object()
            .and_then(|member| member.get("id"))
            .and_then(Value::as_str)
            == Some("_self")
    });
    let family = if candidate
        .household
        .as_ref()
        .is_some_and(|document| document.source == LegacyDocumentSourceV1::Keyring)
        && !has_explicit_owner
    {
        LegacyHouseholdFamilyV1::PythonKeyringRawPartialV0
    } else if has_explicit_owner {
        LegacyHouseholdFamilyV1::RustUnversionedExplicitOwnerV0
    } else {
        LegacyHouseholdFamilyV1::RustUnversionedImplicitOwnerV0
    };
    Ok(ClassifiedLegacyHouseholdV1 {
        family,
        members: parse_member_array(household, family)?,
        active_scope: optional_string(household, "active_scope")?.map(str::to_owned),
        updated_at: None,
        applied_mutation_ids: Vec::new(),
    })
}

fn parse_member_array(
    household: &Map<String, Value>,
    family: LegacyHouseholdFamilyV1,
) -> Result<Vec<ParsedLegacyMemberV1>, PortError> {
    let members = household
        .get("members")
        .and_then(Value::as_array)
        .ok_or_else(|| phase_b_error("legacy household members must be an array"))?;
    if members.len() > MAX_HOUSEHOLD_MEMBERS + 1 {
        return Err(phase_b_error("legacy household has too many members"));
    }
    members
        .iter()
        .map(|member| {
            let member = member
                .as_object()
                .ok_or_else(|| phase_b_error("legacy household member must be an object"))?;
            let valid_keys = match family {
                LegacyHouseholdFamilyV1::PythonNormalizedV1 => exact_optional_object_keys(
                    member,
                    &[
                        "archived",
                        "id",
                        "is_owner",
                        "name",
                        "profile_synced",
                        "relationship",
                    ],
                    &["created_at", "date_of_birth"],
                ),
                LegacyHouseholdFamilyV1::RustInstalledShowcasePartialV1
                | LegacyHouseholdFamilyV1::RustUnversionedExplicitOwnerV0 => {
                    exact_object_keys(member, &["archived", "id", "name", "relationship"])
                }
                LegacyHouseholdFamilyV1::RustFixtureV4 => {
                    exact_object_keys(member, &["id", "name"])
                }
                LegacyHouseholdFamilyV1::RustUnversionedImplicitOwnerV0 => {
                    exact_object_keys(member, &["id", "name", "relationship"])
                }
                LegacyHouseholdFamilyV1::PythonKeyringRawPartialV0 => {
                    exact_optional_object_keys(member, &["id", "name"], &["relationship"])
                }
                LegacyHouseholdFamilyV1::Missing => false,
            };
            if !valid_keys {
                return Err(phase_b_error(
                    "legacy household member does not match its exact family shape",
                ));
            }
            Ok(ParsedLegacyMemberV1 {
                id: required_string(member, "id")?.to_owned(),
                name: required_string(member, "name")?.to_owned(),
                relationship: optional_string(member, "relationship")?.map(str::to_owned),
                archived: match family {
                    LegacyHouseholdFamilyV1::PythonNormalizedV1
                    | LegacyHouseholdFamilyV1::RustInstalledShowcasePartialV1
                    | LegacyHouseholdFamilyV1::RustUnversionedExplicitOwnerV0 => {
                        required_bool(member, "archived")?
                    }
                    _ => false,
                },
                profile_synced: match family {
                    LegacyHouseholdFamilyV1::PythonNormalizedV1 => {
                        Some(required_bool(member, "profile_synced")?)
                    }
                    _ => None,
                },
                date_of_birth: optional_string(member, "date_of_birth")?.map(str::to_owned),
                created_at: optional_string(member, "created_at")?.map(str::to_owned),
                is_owner: match family {
                    LegacyHouseholdFamilyV1::PythonNormalizedV1 => {
                        Some(required_bool(member, "is_owner")?)
                    }
                    _ => None,
                },
            })
        })
        .collect()
}

fn parse_applied_mutation_ids(household: &Map<String, Value>) -> Result<Vec<String>, PortError> {
    let values = household
        .get("applied_mutation_ids")
        .and_then(Value::as_array)
        .ok_or_else(|| phase_b_error("legacy applied-mutation ledger must be an array"))?;
    if values.len() > MAX_LEGACY_APPLIED_MUTATION_IDS {
        return Err(phase_b_error(
            "legacy applied-mutation ledger exceeds its exact limit",
        ));
    }
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| phase_b_error("legacy applied-mutation identity is invalid"))
        })
        .collect()
}

fn exact_object_keys(object: &Map<String, Value>, required: &[&str]) -> bool {
    object.len() == required.len() && required.iter().all(|key| object.contains_key(*key))
}

fn exact_optional_object_keys(
    object: &Map<String, Value>,
    required: &[&str],
    optional: &[&str],
) -> bool {
    required.iter().all(|key| object.contains_key(*key))
        && object
            .keys()
            .all(|key| required.contains(&key.as_str()) || optional.contains(&key.as_str()))
}

fn required_string<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a str, PortError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| phase_b_error("legacy household string field is invalid"))
}

fn optional_string<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<Option<&'a str>, PortError> {
    object
        .get(key)
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| phase_b_error("legacy household optional string field is invalid"))
        })
        .transpose()
}

fn required_bool(object: &Map<String, Value>, key: &str) -> Result<bool, PortError> {
    object
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| phase_b_error("legacy household boolean field is invalid"))
}

fn build_no_source_phase_b(
    phase_a: &LegacyPythonPhaseAResultV1,
    context: &LegacyPythonPhaseBContextV1,
) -> Result<LegacyPythonPhaseBResultV1, PortError> {
    let source_identity = phase_a.source_identity();
    let state = HouseholdStateV1 {
        schema_version: HOUSEHOLD_STATE_SCHEMA_VERSION,
        account_binding: phase_a.account().clone(),
        revision: HouseholdRevision::new(1).map_err(canonical_phase_b_error)?,
        owner: HouseholdOwnerV1 {
            display_name: context.owner_display_name.clone(),
            relationship: RelationshipV1::Self_,
            profile_state: HouseholdProfileStateV1::Incomplete,
            created_at: context.migration_frozen_at.clone(),
            updated_at: context.migration_frozen_at.clone(),
        },
        active_scope: HouseholdScope::Subject(HouseholdSubjectId::self_()),
        members: Vec::new(),
        profiles: Vec::new(),
        outbox: Vec::new(),
        bounded_applied_commits: Vec::new(),
        imported_compatibility: ImportedCompatibilityStateV1 {
            fields: Vec::new(),
            legacy_python_applied_mutation_ids: Vec::new(),
            legacy_python_applied_mutation_ids_digest: None,
            legacy_remote_profile_references: Vec::new(),
            legacy_timestamp_provenance: vec![
                missing_time("owner.created_at", &context.migration_frozen_at),
                missing_time("owner.updated_at", &context.migration_frozen_at),
                missing_time("state.updated_at", &context.migration_frozen_at),
            ],
        },
        migration_dispositions: MigrationDispositionManifestV1 {
            dispositions: Vec::new(),
        },
        migration_provenance: migration_provenance(source_identity, None, context),
        updated_at: context.migration_frozen_at.clone(),
    };
    finish_phase_b(state, None)
}

fn build_present_phase_b(
    candidate: &LegacyPythonPresentCandidateV1,
    context: &LegacyPythonPhaseBContextV1,
) -> Result<LegacyPythonPhaseBResultV1, PortError> {
    let classified = classify_household(candidate)?;
    let evaluated_on = CanonicalDateV1::parse(
        context
            .migration_frozen_at
            .as_str()
            .get(..10)
            .ok_or_else(|| phase_b_error("migration frozen date is invalid"))?,
    )
    .map_err(canonical_phase_b_error)?;
    let mut timestamp_provenance = Vec::new();
    let OwnerAndMembersV1 {
        mut owner,
        mut members,
        synced_evidence,
    } = build_owner_and_members(
        candidate,
        &classified,
        context,
        &evaluated_on,
        &mut timestamp_provenance,
    )?;
    let member_ids = members
        .iter()
        .map(|member| member.member_id.as_str().to_owned())
        .collect::<BTreeSet<_>>();
    let mut profiles = build_local_profiles(candidate, &member_ids)?;
    let (outbox, contexts) = build_legacy_outbox(
        candidate,
        classified.family,
        context,
        &member_ids,
        &mut timestamp_provenance,
    )?;
    seed_profiles_from_outbox_contexts(&mut profiles, &contexts)?;

    let profile_status = derive_profile_statuses(&profiles, &contexts)?;
    owner.profile_state = profile_status
        .get("_self")
        .copied()
        .unwrap_or(HouseholdProfileStateV1::Incomplete);
    for member in &mut members {
        member.profile_state = profile_status
            .get(member.member_id.as_str())
            .copied()
            .unwrap_or(HouseholdProfileStateV1::Incomplete);
    }
    profiles.sort_by(|left, right| subject_order(&left.subject, &right.subject));
    members.sort_by(|left, right| {
        left.member_id
            .as_str()
            .as_bytes()
            .cmp(right.member_id.as_str().as_bytes())
    });

    let mut remote_references =
        build_remote_references(&synced_evidence, &profile_status, candidate.source_digest)?;
    remote_references.sort_by(|left, right| subject_order(&left.subject, &right.subject));
    timestamp_provenance.sort_by(|left, right| left.field_path.cmp(&right.field_path));
    if timestamp_provenance
        .windows(2)
        .any(|pair| pair[0].field_path == pair[1].field_path)
    {
        return Err(phase_b_error(
            "legacy timestamp provenance contains a duplicate field",
        ));
    }

    let household_updated_at = match classified.updated_at.as_deref() {
        Some(value) => {
            let normalized = normalize_legacy_timestamp_v1(value, &context.migration_frozen_at)
                .map_err(canonical_phase_b_error)?;
            timestamp_provenance.push(LegacyTimestampRecordV1 {
                field_path: "state.updated_at".to_owned(),
                disposition: LegacyTimestampDispositionV1::Normalized {
                    provenance: normalized.clone(),
                },
            });
            normalized.normalized
        }
        None => {
            timestamp_provenance.push(missing_time(
                "state.updated_at",
                &context.migration_frozen_at,
            ));
            context.migration_frozen_at.clone()
        }
    };
    timestamp_provenance.sort_by(|left, right| left.field_path.cmp(&right.field_path));
    let active_scope = parse_active_scope(classified.active_scope.as_deref(), &members)?;
    let compatibility_fields = build_compatibility_fields(candidate)?;
    let legacy_applied_digest = if classified.applied_mutation_ids.is_empty() {
        None
    } else {
        Some(
            canonical_sha256_v1(&classified.applied_mutation_ids)
                .map_err(canonical_phase_b_error)?,
        )
    };
    let mut state = HouseholdStateV1 {
        schema_version: HOUSEHOLD_STATE_SCHEMA_VERSION,
        account_binding: candidate.account.clone(),
        revision: HouseholdRevision::new(1).map_err(canonical_phase_b_error)?,
        owner,
        active_scope,
        members,
        profiles,
        outbox,
        bounded_applied_commits: Vec::new(),
        imported_compatibility: ImportedCompatibilityStateV1 {
            fields: compatibility_fields,
            legacy_python_applied_mutation_ids: classified.applied_mutation_ids,
            legacy_python_applied_mutation_ids_digest: legacy_applied_digest,
            legacy_remote_profile_references: remote_references,
            legacy_timestamp_provenance: timestamp_provenance,
        },
        migration_dispositions: MigrationDispositionManifestV1 {
            dispositions: Vec::new(),
        },
        migration_provenance: migration_provenance(
            LegacySourceIdentityV1::Present {
                source_kind: D2_SOURCE_BUNDLE_KIND.to_owned(),
                source_digest: candidate.source_digest,
            },
            candidate.snapshot_evidence.clone(),
            context,
        ),
        updated_at: household_updated_at,
    };
    state.migration_dispositions = MigrationDispositionManifestV1 {
        dispositions: build_migration_dispositions(candidate, &state)?,
    };
    finish_phase_b(state, candidate.snapshot_evidence.clone())
}

fn migration_provenance(
    source_identity: LegacySourceIdentityV1,
    legacy_python_snapshot: Option<LegacyPythonSnapshotProvenanceV1>,
    context: &LegacyPythonPhaseBContextV1,
) -> MigrationProvenanceV1 {
    MigrationProvenanceV1 {
        source_identity,
        legacy_python_snapshot,
        migration_id: context.migration_id,
        initialization_id: context.initialization_id,
        initial_commit_id: context.initial_commit_id,
        migration_frozen_at: context.migration_frozen_at.clone(),
    }
}

fn finish_phase_b(
    state: HouseholdStateV1,
    snapshot_evidence: Option<LegacyPythonSnapshotEvidenceV1>,
) -> Result<LegacyPythonPhaseBResultV1, PortError> {
    if state.migration_provenance.legacy_python_snapshot != snapshot_evidence {
        return Err(phase_b_error(
            "legacy snapshot provenance is not bound to the canonical migration state",
        ));
    }
    state.validate().map_err(canonical_phase_b_error)?;
    let canonical = state.canonical_bytes().map_err(canonical_phase_b_error)?;
    Ok(LegacyPythonPhaseBResultV1 {
        semantic_candidate_digest: digest_bytes(&canonical),
        state,
        snapshot_evidence,
    })
}

struct OwnerAndMembersV1 {
    owner: HouseholdOwnerV1,
    members: Vec<HouseholdMemberV1>,
    synced_evidence: BTreeMap<String, bool>,
}

fn build_owner_and_members(
    candidate: &LegacyPythonPresentCandidateV1,
    classified: &ClassifiedLegacyHouseholdV1,
    context: &LegacyPythonPhaseBContextV1,
    evaluated_on: &CanonicalDateV1,
    timestamps: &mut Vec<LegacyTimestampRecordV1>,
) -> Result<OwnerAndMembersV1, PortError> {
    let explicit_owner_required = matches!(
        classified.family,
        LegacyHouseholdFamilyV1::PythonNormalizedV1
            | LegacyHouseholdFamilyV1::RustInstalledShowcasePartialV1
            | LegacyHouseholdFamilyV1::RustUnversionedExplicitOwnerV0
    );
    let mut explicit_owner = None;
    let mut seen_ids = BTreeSet::new();
    let mut members = Vec::new();
    let mut synced_evidence = BTreeMap::new();
    for source in &classified.members {
        if !seen_ids.insert(source.id.clone()) {
            return Err(phase_b_error(
                "legacy household contains a duplicate identity",
            ));
        }
        if source.id == "_self" {
            if explicit_owner.is_some()
                || source.archived
                || source.relationship.as_deref() != Some("self")
                || source.is_owner == Some(false)
            {
                return Err(phase_b_error(
                    "legacy household contains conflicting owner evidence",
                ));
            }
            explicit_owner = Some(source);
            synced_evidence.insert("_self".to_owned(), source.profile_synced.unwrap_or(false));
            continue;
        }
        if source.is_owner == Some(true) || source.relationship.as_deref() == Some("self") {
            return Err(phase_b_error(
                "legacy household contains conflicting non-owner evidence",
            ));
        }
        let member_id =
            MemberId::parse_preserved(source.id.clone()).map_err(canonical_phase_b_error)?;
        let display_name =
            DisplayName::parse(source.name.clone()).map_err(canonical_phase_b_error)?;
        let (relationship, relationship_source) =
            parse_relationship(source.relationship.as_deref())?;
        let age_evidence = source
            .date_of_birth
            .as_ref()
            .map(|date| {
                DateOfBirthV1::parse_for_evaluation(date.clone(), evaluated_on)
                    .map(|date_of_birth| AgeEvidenceV1 {
                        date_of_birth: Some(date_of_birth),
                        age_band: None,
                        source: AgeEvidenceSourceV1::LegacyDateOfBirth,
                    })
                    .map_err(canonical_phase_b_error)
            })
            .transpose()?;
        let minor_status =
            derive_minor_status_v1(relationship, age_evidence.as_ref(), evaluated_on)
                .map_err(canonical_phase_b_error)?;
        let created_path = format!("members.{}.created_at", member_id.as_str());
        let created_at = normalize_or_backfill_time(
            source.created_at.as_deref(),
            &created_path,
            &context.migration_frozen_at,
            timestamps,
        )?;
        timestamps.push(missing_time(
            format!("members.{}.updated_at", member_id.as_str()),
            &context.migration_frozen_at,
        ));
        synced_evidence.insert(
            member_id.as_str().to_owned(),
            source.profile_synced.unwrap_or(false),
        );
        members.push(HouseholdMemberV1 {
            member_id,
            display_name,
            relationship,
            relationship_source,
            minor_status,
            age_evidence,
            minor_status_evaluated_on: evaluated_on.clone(),
            lifecycle: if source.archived {
                HouseholdLifecycleV1::Archived
            } else {
                HouseholdLifecycleV1::Active
            },
            profile_state: HouseholdProfileStateV1::Incomplete,
            created_at,
            updated_at: context.migration_frozen_at.clone(),
        });
    }
    if explicit_owner_required && explicit_owner.is_none() {
        return Err(phase_b_error(
            "legacy household family requires exactly one explicit owner",
        ));
    }
    if !explicit_owner_required && explicit_owner.is_some() {
        return Err(phase_b_error(
            "legacy household family forbids an explicit owner row",
        ));
    }
    let (display_name, created_at) = match explicit_owner {
        Some(owner) => (
            DisplayName::parse(owner.name.clone()).map_err(canonical_phase_b_error)?,
            normalize_or_backfill_time(
                owner.created_at.as_deref(),
                "owner.created_at",
                &context.migration_frozen_at,
                timestamps,
            )?,
        ),
        None if classified.family == LegacyHouseholdFamilyV1::Missing => {
            timestamps.push(missing_time(
                "owner.created_at",
                &context.migration_frozen_at,
            ));
            (
                context.owner_display_name.clone(),
                context.migration_frozen_at.clone(),
            )
        }
        None => {
            let first_name = candidate
                .first_name
                .as_ref()
                .and_then(|value| value.as_value().as_str())
                .ok_or_else(|| phase_b_error("legacy owner name is invalid"))?;
            timestamps.push(missing_time(
                "owner.created_at",
                &context.migration_frozen_at,
            ));
            (
                DisplayName::parse(first_name.to_owned()).map_err(canonical_phase_b_error)?,
                context.migration_frozen_at.clone(),
            )
        }
    };
    timestamps.push(missing_time(
        "owner.updated_at",
        &context.migration_frozen_at,
    ));
    Ok(OwnerAndMembersV1 {
        owner: HouseholdOwnerV1 {
            display_name,
            relationship: RelationshipV1::Self_,
            profile_state: HouseholdProfileStateV1::Incomplete,
            created_at,
            updated_at: context.migration_frozen_at.clone(),
        },
        members,
        synced_evidence,
    })
}

fn parse_relationship(
    value: Option<&str>,
) -> Result<(RelationshipV1, RelationshipSourceV1), PortError> {
    let Some(value) = value else {
        return Ok((RelationshipV1::Other, RelationshipSourceV1::LegacyMissing));
    };
    let relationship = match value {
        "self" => RelationshipV1::Self_,
        "spouse" => RelationshipV1::Spouse,
        "partner" => RelationshipV1::Partner,
        "parent" => RelationshipV1::Parent,
        "child" => RelationshipV1::Child,
        "sibling" => RelationshipV1::Sibling,
        "grandparent" => RelationshipV1::Grandparent,
        "friend" => RelationshipV1::Friend,
        "other" => RelationshipV1::Other,
        _ => {
            return Err(phase_b_error(
                "legacy household relationship value is unsupported",
            ));
        }
    };
    Ok((relationship, RelationshipSourceV1::LegacyDeclared))
}

fn normalize_or_backfill_time(
    value: Option<&str>,
    path: impl Into<String>,
    frozen_at: &CanonicalTimestampV1,
    timestamps: &mut Vec<LegacyTimestampRecordV1>,
) -> Result<CanonicalTimestampV1, PortError> {
    let path = path.into();
    match value {
        Some(value) => {
            let provenance =
                normalize_legacy_timestamp_v1(value, frozen_at).map_err(canonical_phase_b_error)?;
            let normalized = provenance.normalized.clone();
            timestamps.push(LegacyTimestampRecordV1 {
                field_path: path,
                disposition: LegacyTimestampDispositionV1::Normalized { provenance },
            });
            Ok(normalized)
        }
        None => {
            timestamps.push(missing_time(path, frozen_at));
            Ok(frozen_at.clone())
        }
    }
}

fn missing_time(
    path: impl Into<String>,
    frozen_at: &CanonicalTimestampV1,
) -> LegacyTimestampRecordV1 {
    LegacyTimestampRecordV1 {
        field_path: path.into(),
        disposition: LegacyTimestampDispositionV1::LegacyMissingTime {
            normalized: frozen_at.clone(),
        },
    }
}

type LegacyContextMap = BTreeMap<String, BTreeMap<CanonicalDigestV1, Map<String, Value>>>;

fn build_local_profiles(
    candidate: &LegacyPythonPresentCandidateV1,
    member_ids: &BTreeSet<String>,
) -> Result<Vec<HouseholdProfileRecordV1>, PortError> {
    let Some(profiles) = candidate
        .local_profiles
        .as_ref()
        .map(|document| document.value.as_map())
    else {
        return Ok(Vec::new());
    };
    if profiles.len() > MAX_HOUSEHOLD_PROFILES {
        return Err(phase_b_error(
            "legacy household profile collection exceeds its exact limit",
        ));
    }
    let mut records = Vec::with_capacity(profiles.len());
    for (source_key, value) in profiles {
        let object = value
            .as_object()
            .ok_or_else(|| phase_b_error("legacy household profile entry must be an object"))?;
        let subject = parse_source_subject(source_key, member_ids)?;
        let bytes = canonicalize_json_value_v1(&Value::Object(object.clone()))
            .map_err(canonical_phase_b_error)?;
        let document = HouseholdProfileDocumentV1::legacy_projection(&bytes)
            .map_err(canonical_phase_b_error)?;
        records.push(HouseholdProfileRecordV1 {
            subject,
            profile_revision: ProfileRevision::new(1).map_err(canonical_phase_b_error)?,
            document,
        });
    }
    records.sort_by(|left, right| subject_order(&left.subject, &right.subject));
    if records
        .windows(2)
        .any(|pair| pair[0].subject == pair[1].subject)
    {
        return Err(phase_b_error(
            "legacy household contains duplicate profile subjects",
        ));
    }
    Ok(records)
}

fn build_legacy_outbox(
    candidate: &LegacyPythonPresentCandidateV1,
    family: LegacyHouseholdFamilyV1,
    context: &LegacyPythonPhaseBContextV1,
    member_ids: &BTreeSet<String>,
    timestamps: &mut Vec<LegacyTimestampRecordV1>,
) -> Result<(Vec<HouseholdOutboxRecordV1>, LegacyContextMap), PortError> {
    let Some(source) = candidate
        .profile_outbox
        .as_ref()
        .map(|document| document.value.as_map())
    else {
        return Ok((Vec::new(), BTreeMap::new()));
    };
    if source.len() > MAX_HOUSEHOLD_OUTBOX_ENTRIES {
        return Err(phase_b_error(
            "legacy household outbox exceeds its exact limit",
        ));
    }
    let mut records = Vec::with_capacity(source.len());
    let mut contexts: LegacyContextMap = BTreeMap::new();
    let mut ids = BTreeSet::new();
    for (source_key, value) in source {
        let object = value
            .as_object()
            .ok_or_else(|| phase_b_error("legacy outbox entry must be an object"))?;
        let bytes = canonicalize_json_value_v1(&Value::Object(object.clone()))
            .map_err(canonical_phase_b_error)?;
        let (outbox_id, legacy) = classify_legacy_outbox_v1(
            candidate.source_digest,
            source_key,
            &bytes,
            &context.migration_frozen_at,
        )
        .map_err(canonical_phase_b_error)?;
        if !family_allows_outbox(family, legacy.source_kind) {
            return Err(phase_b_error(
                "legacy household and outbox families are incompatible",
            ));
        }
        validate_subject_exists(&legacy.target, member_ids)?;
        if !ids.insert(outbox_id.as_str().to_owned()) {
            return Err(phase_b_error(
                "legacy household outbox identities are not unique",
            ));
        }
        if legacy.source_kind == LegacyOutboxSourceKindV1::PythonSubjectKeyedV1 {
            let source_updated_at = object
                .get("updated_at")
                .and_then(Value::as_str)
                .ok_or_else(|| phase_b_error("legacy outbox timestamp is invalid"))?;
            let provenance =
                normalize_legacy_timestamp_v1(source_updated_at, &context.migration_frozen_at)
                    .map_err(canonical_phase_b_error)?;
            timestamps.push(LegacyTimestampRecordV1 {
                field_path: format!("outbox.{source_key}.updated_at"),
                disposition: LegacyTimestampDispositionV1::Normalized { provenance },
            });
        } else {
            timestamps.push(missing_time(
                format!("outbox.{source_key}.updated_at"),
                &context.migration_frozen_at,
            ));
        }
        if let Some(local_context) = legacy
            .payload
            .as_map()
            .get("local_context")
            .and_then(Value::as_object)
        {
            let bytes = canonicalize_json_value_v1(&Value::Object(local_context.clone()))
                .map_err(canonical_phase_b_error)?;
            let document = HouseholdProfileDocumentV1::legacy_projection(&bytes)
                .map_err(canonical_phase_b_error)?;
            if document
                .effective_profile()
                .map_err(canonical_phase_b_error)?
                .is_some()
            {
                let digest = canonical_sha256_v1(&Value::Object(local_context.clone()))
                    .map_err(canonical_phase_b_error)?;
                contexts
                    .entry(subject_storage_key(&legacy.target))
                    .or_default()
                    .entry(digest)
                    .or_insert_with(|| local_context.clone());
            }
        }
        let target = legacy.target.clone();
        records.push(HouseholdOutboxRecordV1 {
            outbox_id,
            outbox_revision: OutboxRevision::new(1).map_err(canonical_phase_b_error)?,
            entry: HouseholdProfileOutboxEntryV1::Legacy {
                version: 1,
                target,
                legacy,
            },
        });
    }
    records.sort_by(|left, right| {
        left.outbox_id
            .as_str()
            .as_bytes()
            .cmp(right.outbox_id.as_str().as_bytes())
    });
    if records.is_empty() {
        return Ok((records, contexts));
    }
    if matches!(
        family,
        LegacyHouseholdFamilyV1::Missing
            | LegacyHouseholdFamilyV1::RustInstalledShowcasePartialV1
            | LegacyHouseholdFamilyV1::RustUnversionedImplicitOwnerV0
    ) {
        return Err(phase_b_error(
            "legacy household family requires an absent or empty outbox",
        ));
    }
    Ok((records, contexts))
}

fn family_allows_outbox(
    family: LegacyHouseholdFamilyV1,
    source_kind: LegacyOutboxSourceKindV1,
) -> bool {
    matches!(
        (family, source_kind),
        (
            LegacyHouseholdFamilyV1::PythonNormalizedV1,
            LegacyOutboxSourceKindV1::PythonSubjectKeyedV1
        ) | (
            LegacyHouseholdFamilyV1::RustFixtureV4,
            LegacyOutboxSourceKindV1::RustMutationKeyedEmbeddedMemberV0
        ) | (
            LegacyHouseholdFamilyV1::RustUnversionedExplicitOwnerV0,
            LegacyOutboxSourceKindV1::RustSubjectKeyedLocalContextV0
        ) | (
            LegacyHouseholdFamilyV1::PythonKeyringRawPartialV0,
            LegacyOutboxSourceKindV1::PythonSubjectKeyedPatchV0
        )
    )
}

fn seed_profiles_from_outbox_contexts(
    profiles: &mut Vec<HouseholdProfileRecordV1>,
    contexts: &LegacyContextMap,
) -> Result<(), PortError> {
    for (key, candidates) in contexts {
        if candidates.len() != 1 {
            continue;
        }
        let existing = profiles
            .iter()
            .position(|profile| subject_storage_key(&profile.subject) == *key);
        let existing_usable = existing
            .and_then(|index| profiles.get(index))
            .map(|profile| profile.document.effective_profile())
            .transpose()
            .map_err(canonical_phase_b_error)?
            .flatten()
            .is_some();
        if existing_usable {
            continue;
        }
        let object = candidates
            .values()
            .next()
            .ok_or_else(|| phase_b_error("legacy outbox context is missing"))?;
        let bytes = canonicalize_json_value_v1(&Value::Object(object.clone()))
            .map_err(canonical_phase_b_error)?;
        let record = HouseholdProfileRecordV1 {
            subject: storage_key_subject(key)?,
            profile_revision: ProfileRevision::new(1).map_err(canonical_phase_b_error)?,
            document: HouseholdProfileDocumentV1::legacy_projection(&bytes)
                .map_err(canonical_phase_b_error)?,
        };
        if let Some(index) = existing {
            profiles[index] = record;
        } else {
            profiles.push(record);
        }
    }
    Ok(())
}

fn derive_profile_statuses(
    profiles: &[HouseholdProfileRecordV1],
    contexts: &LegacyContextMap,
) -> Result<BTreeMap<String, HouseholdProfileStateV1>, PortError> {
    let mut statuses = BTreeMap::new();
    statuses.insert("_self".to_owned(), HouseholdProfileStateV1::Incomplete);
    for profile in profiles {
        let key = subject_storage_key(&profile.subject);
        let usable = profile
            .document
            .effective_profile()
            .map_err(canonical_phase_b_error)?
            .is_some();
        statuses.insert(
            key,
            if usable {
                HouseholdProfileStateV1::LocalOnly
            } else {
                HouseholdProfileStateV1::Incomplete
            },
        );
    }
    for (key, values) in contexts {
        if values.len() > 1 {
            statuses.insert(key.clone(), HouseholdProfileStateV1::Conflicted);
        } else if values.len() == 1 {
            statuses.insert(key.clone(), HouseholdProfileStateV1::LocalOnly);
        }
    }
    Ok(statuses)
}

fn build_remote_references(
    evidence: &BTreeMap<String, bool>,
    statuses: &BTreeMap<String, HouseholdProfileStateV1>,
    source_digest: CanonicalDigestV1,
) -> Result<Vec<LegacyRemoteProfileReferenceV1>, PortError> {
    #[derive(Serialize)]
    struct Evidence<'a> {
        contract: &'static str,
        source_digest: String,
        subject: &'a HouseholdSubjectId,
        profile_synced: bool,
        materialized_profile: bool,
    }
    let mut output = Vec::new();
    for (key, synced) in evidence {
        if !synced {
            continue;
        }
        let subject = storage_key_subject(key)?;
        let materialized_profile =
            matches!(statuses.get(key), Some(HouseholdProfileStateV1::LocalOnly));
        let reference_digest = canonical_sha256_v1(&Evidence {
            contract: "heyfood.household.legacy-remote-profile-reference.v1",
            source_digest: source_digest.to_lower_hex(),
            subject: &subject,
            profile_synced: true,
            materialized_profile,
        })
        .map_err(canonical_phase_b_error)?;
        output.push(LegacyRemoteProfileReferenceV1 {
            subject,
            source_digest: reference_digest,
        });
    }
    Ok(output)
}

fn parse_active_scope(
    value: Option<&str>,
    members: &[HouseholdMemberV1],
) -> Result<HouseholdScope, PortError> {
    let Some(value) = value else {
        return Ok(HouseholdScope::Subject(HouseholdSubjectId::self_()));
    };
    match value {
        "_self" => Ok(HouseholdScope::Subject(HouseholdSubjectId::self_())),
        "__everyone__" => {
            if members
                .iter()
                .all(|member| member.lifecycle != HouseholdLifecycleV1::Active)
            {
                return Err(phase_b_error(
                    "everyone scope requires at least one active non-owner member",
                ));
            }
            Ok(HouseholdScope::Everyone)
        }
        value => {
            let member = members
                .iter()
                .find(|member| member.member_id.as_str() == value)
                .ok_or_else(|| phase_b_error("legacy active scope references an unknown member"))?;
            if member.lifecycle == HouseholdLifecycleV1::Archived {
                return Err(phase_b_error(
                    "legacy active scope references an archived member",
                ));
            }
            Ok(HouseholdScope::Subject(HouseholdSubjectId::member(
                member.member_id.clone(),
            )))
        }
    }
}

fn parse_source_subject(
    key: &str,
    member_ids: &BTreeSet<String>,
) -> Result<HouseholdSubjectId, PortError> {
    if key == "_self" {
        return Ok(HouseholdSubjectId::self_());
    }
    if !member_ids.contains(key) {
        return Err(phase_b_error(
            "legacy household reference targets an unknown member",
        ));
    }
    MemberId::parse_preserved(key.to_owned())
        .map(HouseholdSubjectId::member)
        .map_err(canonical_phase_b_error)
}

fn validate_subject_exists(
    subject: &HouseholdSubjectId,
    member_ids: &BTreeSet<String>,
) -> Result<(), PortError> {
    match subject {
        HouseholdSubjectId::Self_ => Ok(()),
        HouseholdSubjectId::Member(member) if member_ids.contains(member.as_str()) => Ok(()),
        HouseholdSubjectId::Member(_) => Err(phase_b_error(
            "legacy household outbox targets an unknown member",
        )),
    }
}

fn subject_storage_key(subject: &HouseholdSubjectId) -> String {
    match subject {
        HouseholdSubjectId::Self_ => "_self".to_owned(),
        HouseholdSubjectId::Member(member) => member.as_str().to_owned(),
    }
}

fn storage_key_subject(key: &str) -> Result<HouseholdSubjectId, PortError> {
    if key == "_self" {
        Ok(HouseholdSubjectId::self_())
    } else {
        MemberId::parse_preserved(key.to_owned())
            .map(HouseholdSubjectId::member)
            .map_err(canonical_phase_b_error)
    }
}

fn subject_order(left: &HouseholdSubjectId, right: &HouseholdSubjectId) -> std::cmp::Ordering {
    match (left, right) {
        (HouseholdSubjectId::Self_, HouseholdSubjectId::Self_) => std::cmp::Ordering::Equal,
        (HouseholdSubjectId::Self_, HouseholdSubjectId::Member(_)) => std::cmp::Ordering::Less,
        (HouseholdSubjectId::Member(_), HouseholdSubjectId::Self_) => std::cmp::Ordering::Greater,
        (HouseholdSubjectId::Member(left), HouseholdSubjectId::Member(right)) => {
            left.as_str().as_bytes().cmp(right.as_str().as_bytes())
        }
    }
}

fn build_compatibility_fields(
    candidate: &LegacyPythonPresentCandidateV1,
) -> Result<Vec<ImportedCompatibilityFieldV1>, PortError> {
    let mut fields = candidate
        .compatibility_fields
        .iter()
        .map(|(field_name, value)| ImportedCompatibilityFieldV1 {
            field_name: field_name.clone(),
            value: value.clone(),
            source_digest: value.canonical_sha256(),
        })
        .collect::<Vec<_>>();
    fields.sort_by(|left, right| left.field_name.cmp(&right.field_name));
    if fields.len() > MAX_IMPORTED_COMPATIBILITY_FIELDS
        || fields
            .windows(2)
            .any(|pair| pair[0].field_name == pair[1].field_name)
    {
        return Err(phase_b_error(
            "legacy compatibility field collection is invalid",
        ));
    }
    // These two fields are active restart inputs, not opaque archival blobs.
    // Validate their typed projection during conversion so a committed vault
    // can always recreate the exact local experience after restart.
    let temporary = ImportedCompatibilityStateV1 {
        fields: fields.clone(),
        legacy_python_applied_mutation_ids: Vec::new(),
        legacy_python_applied_mutation_ids_digest: None,
        legacy_remote_profile_references: Vec::new(),
        legacy_timestamp_provenance: Vec::new(),
    };
    let _ = parse_location_restart(&temporary)?;
    let _ = parse_restaurant_search_restart(&temporary)?;
    Ok(fields)
}

#[derive(Serialize)]
struct DestinationFragmentV1<'a, T: Serialize + ?Sized> {
    contract: &'static str,
    destination_schema: &'a str,
    field_name: &'a str,
    value: &'a T,
}

#[derive(Serialize)]
struct HouseholdDestinationProjectionV1<'a> {
    revision: HouseholdRevision,
    owner: &'a HouseholdOwnerV1,
    active_scope: &'a HouseholdScope,
    members: &'a [HouseholdMemberV1],
    legacy_python_applied_mutation_ids: &'a [String],
    legacy_python_applied_mutation_ids_digest: Option<CanonicalDigestV1>,
    updated_at: &'a CanonicalTimestampV1,
}

fn destination_fragment_digest<T: Serialize + ?Sized>(
    destination_schema: &str,
    field_name: &str,
    value: &T,
) -> Result<CanonicalDigestV1, PortError> {
    canonical_sha256_v1(&DestinationFragmentV1 {
        contract: D2_DESTINATION_FRAGMENT_CONTRACT,
        destination_schema,
        field_name,
        value,
    })
    .map_err(canonical_phase_b_error)
}

fn compatibility_destination_digest(
    state: &HouseholdStateV1,
    field_name: &str,
) -> Result<CanonicalDigestV1, PortError> {
    let field = state
        .imported_compatibility
        .fields
        .iter()
        .find(|field| field.field_name == field_name)
        .ok_or_else(|| phase_b_error("compatibility destination fragment is missing"))?;
    destination_fragment_digest("imported_compatibility_state_v1", field_name, field)
}

fn household_destination_digest(
    state: &HouseholdStateV1,
    field_name: &str,
) -> Result<CanonicalDigestV1, PortError> {
    destination_fragment_digest(
        "household_state_v1",
        field_name,
        &HouseholdDestinationProjectionV1 {
            revision: state.revision,
            owner: &state.owner,
            active_scope: &state.active_scope,
            members: &state.members,
            legacy_python_applied_mutation_ids: &state
                .imported_compatibility
                .legacy_python_applied_mutation_ids,
            legacy_python_applied_mutation_ids_digest: state
                .imported_compatibility
                .legacy_python_applied_mutation_ids_digest,
            updated_at: &state.updated_at,
        },
    )
}

fn destination_digest_for_schema(
    state: &HouseholdStateV1,
    field_name: &str,
    destination_schema: &str,
) -> Result<CanonicalDigestV1, PortError> {
    match destination_schema {
        "account_binding_v1" => {
            destination_fragment_digest(destination_schema, field_name, &state.account_binding)
        }
        "household_owner_v1" => {
            destination_fragment_digest(destination_schema, field_name, &state.owner)
        }
        "household_state_v1" => household_destination_digest(state, field_name),
        "household_profile_record_v1" => {
            destination_fragment_digest(destination_schema, field_name, &state.profiles)
        }
        "household_outbox_record_v1" => {
            destination_fragment_digest(destination_schema, field_name, &state.outbox)
        }
        "imported_compatibility_state_v1" => compatibility_destination_digest(state, field_name),
        _ => Err(phase_b_error(
            "migration disposition names an unsupported destination schema",
        )),
    }
}

fn push_disposition(
    output: &mut Vec<MigrationDispositionV1>,
    state: &HouseholdStateV1,
    field_name: impl Into<String>,
    disposition: MigrationDispositionKindV1,
    destination_schema: Option<&str>,
    source_digest: Option<CanonicalDigestV1>,
) -> Result<(), PortError> {
    let field_name = field_name.into();
    let destination_digest = destination_schema
        .map(|schema| destination_digest_for_schema(state, &field_name, schema))
        .transpose()?;
    output.push(MigrationDispositionV1 {
        field_name,
        disposition,
        destination_schema: destination_schema.map(str::to_owned),
        source_digest,
        destination_digest,
    });
    Ok(())
}

fn document_disposition(
    candidate: &LegacyPythonPresentCandidateV1,
    role: LegacyConfigFieldRoleV1,
) -> Option<&LegacyBoundDocumentV1> {
    match role {
        LegacyConfigFieldRoleV1::Household => candidate.household.as_ref(),
        LegacyConfigFieldRoleV1::LocalProfiles => candidate.local_profiles.as_ref(),
        LegacyConfigFieldRoleV1::ProfileOutbox => candidate.profile_outbox.as_ref(),
        _ => None,
    }
}

fn document_destination_schema(role: LegacyConfigFieldRoleV1) -> Option<&'static str> {
    match role {
        LegacyConfigFieldRoleV1::Household => Some("household_state_v1"),
        LegacyConfigFieldRoleV1::LocalProfiles => Some("household_profile_record_v1"),
        LegacyConfigFieldRoleV1::ProfileOutbox => Some("household_outbox_record_v1"),
        _ => None,
    }
}

fn first_name_drives_owner(candidate: &LegacyPythonPresentCandidateV1) -> Result<bool, PortError> {
    Ok(matches!(
        classify_household(candidate)?.family,
        LegacyHouseholdFamilyV1::RustUnversionedImplicitOwnerV0
            | LegacyHouseholdFamilyV1::PythonKeyringRawPartialV0
    ))
}

fn build_migration_dispositions(
    candidate: &LegacyPythonPresentCandidateV1,
    state: &HouseholdStateV1,
) -> Result<Vec<MigrationDispositionV1>, PortError> {
    let mut dispositions = Vec::new();
    let owner_from_first_name = first_name_drives_owner(candidate)?;
    for evidence in &candidate.config_field_evidence {
        match evidence.role {
            LegacyConfigFieldRoleV1::AccountBinding => push_disposition(
                &mut dispositions,
                state,
                evidence.field_name.clone(),
                MigrationDispositionKindV1::Migrated,
                Some("account_binding_v1"),
                evidence.source_digest,
            )?,
            LegacyConfigFieldRoleV1::OwnerName if owner_from_first_name => push_disposition(
                &mut dispositions,
                state,
                evidence.field_name.clone(),
                MigrationDispositionKindV1::Migrated,
                Some("household_owner_v1"),
                evidence.source_digest,
            )?,
            LegacyConfigFieldRoleV1::OwnerName => push_disposition(
                &mut dispositions,
                state,
                evidence.field_name.clone(),
                MigrationDispositionKindV1::Retired,
                None,
                evidence.source_digest,
            )?,
            LegacyConfigFieldRoleV1::CompatibilityMigrated => push_disposition(
                &mut dispositions,
                state,
                evidence.field_name.clone(),
                MigrationDispositionKindV1::Migrated,
                Some("imported_compatibility_state_v1"),
                evidence.source_digest,
            )?,
            LegacyConfigFieldRoleV1::CompatibilityRetired => push_disposition(
                &mut dispositions,
                state,
                evidence.field_name.clone(),
                MigrationDispositionKindV1::Retired,
                Some("imported_compatibility_state_v1"),
                evidence.source_digest,
            )?,
            role @ (LegacyConfigFieldRoleV1::Household
            | LegacyConfigFieldRoleV1::LocalProfiles
            | LegacyConfigFieldRoleV1::ProfileOutbox) => {
                let document = document_disposition(candidate, role);
                let selected_from_file = document
                    .is_some_and(|document| document.source == LegacyDocumentSourceV1::File);
                push_disposition(
                    &mut dispositions,
                    state,
                    evidence.field_name.clone(),
                    if selected_from_file {
                        MigrationDispositionKindV1::Migrated
                    } else {
                        MigrationDispositionKindV1::Retired
                    },
                    selected_from_file
                        .then(|| document_destination_schema(role))
                        .flatten(),
                    evidence.source_digest,
                )?;
            }
            LegacyConfigFieldRoleV1::CredentialMarker => push_disposition(
                &mut dispositions,
                state,
                evidence.field_name.clone(),
                MigrationDispositionKindV1::Retired,
                None,
                None,
            )?,
            LegacyConfigFieldRoleV1::Credential => push_disposition(
                &mut dispositions,
                state,
                evidence.field_name.clone(),
                MigrationDispositionKindV1::ReauthenticationRequired,
                None,
                None,
            )?,
            LegacyConfigFieldRoleV1::Unknown => push_disposition(
                &mut dispositions,
                state,
                evidence.field_name.clone(),
                MigrationDispositionKindV1::Retired,
                None,
                None,
            )?,
        }
    }
    for (field_name, document, schema) in [
        (
            "keyring.household.state",
            candidate.household.as_ref(),
            "household_state_v1",
        ),
        (
            "keyring.household.local_profiles",
            candidate.local_profiles.as_ref(),
            "household_profile_record_v1",
        ),
        (
            "keyring.household.profile_outbox",
            candidate.profile_outbox.as_ref(),
            "household_outbox_record_v1",
        ),
    ] {
        if let Some(document) =
            document.filter(|document| document.source == LegacyDocumentSourceV1::Keyring)
        {
            push_disposition(
                &mut dispositions,
                state,
                field_name,
                MigrationDispositionKindV1::Migrated,
                Some(schema),
                Some(document.source_digest),
            )?;
        }
    }
    dispositions.sort_by(|left, right| left.field_name.cmp(&right.field_name));
    if dispositions.len() > MAX_MIGRATION_DISPOSITIONS
        || dispositions
            .windows(2)
            .any(|pair| pair[0].field_name == pair[1].field_name)
    {
        return Err(phase_b_error(
            "legacy migration disposition collection is invalid",
        ));
    }
    Ok(dispositions)
}

fn verify_destination_disposition_digests(state: &HouseholdStateV1) -> Result<(), PortError> {
    for disposition in &state.migration_dispositions.dispositions {
        match (
            disposition.destination_schema.as_deref(),
            disposition.destination_digest,
        ) {
            (Some(schema), Some(actual)) => {
                let expected =
                    destination_digest_for_schema(state, &disposition.field_name, schema)?;
                if actual != expected || disposition.source_digest == Some(actual) {
                    return Err(PortError::new(
                        "legacy_python_vault_readback_mismatch",
                        "migration destination fragment digest is missing, copied from its source, or does not match authenticated vault state",
                    ));
                }
            }
            (None, None) => {}
            _ => {
                return Err(PortError::new(
                    "legacy_python_vault_readback_mismatch",
                    "migration disposition destination evidence is incomplete",
                ));
            }
        }
    }
    Ok(())
}

fn compatibility_value<'a>(
    compatibility: &'a ImportedCompatibilityStateV1,
    field_name: &str,
) -> Result<Option<&'a CanonicalJsonValueV1>, PortError> {
    let mut matches = compatibility
        .fields
        .iter()
        .filter(|field| field.field_name == field_name);
    let first = matches.next().map(|field| &field.value);
    if matches.next().is_some() {
        return Err(phase_b_error(
            "legacy compatibility restart field is duplicated",
        ));
    }
    Ok(first)
}

fn parse_location_restart(
    compatibility: &ImportedCompatibilityStateV1,
) -> Result<Option<LegacyLocationRestartV1>, PortError> {
    let Some(canonical) = compatibility_value(compatibility, "location")? else {
        return Ok(None);
    };
    let object = canonical
        .as_value()
        .as_object()
        .ok_or_else(|| phase_b_error("legacy location restart state must be an object"))?;
    let label = object
        .get("label")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.trim() == *value && value.len() <= 200)
        .ok_or_else(|| phase_b_error("legacy location restart label is invalid"))?;
    let latitude_value = object
        .get("latitude")
        .cloned()
        .ok_or_else(|| phase_b_error("legacy location restart latitude is missing"))?;
    let longitude_value = object
        .get("longitude")
        .cloned()
        .ok_or_else(|| phase_b_error("legacy location restart longitude is missing"))?;
    let latitude_number = latitude_value
        .as_f64()
        .filter(|value| value.is_finite() && (-90.0..=90.0).contains(value))
        .ok_or_else(|| phase_b_error("legacy location restart latitude is invalid"))?;
    let longitude_number = longitude_value
        .as_f64()
        .filter(|value| value.is_finite() && (-180.0..=180.0).contains(value))
        .ok_or_else(|| phase_b_error("legacy location restart longitude is invalid"))?;
    let _ = (latitude_number, longitude_number);
    Ok(Some(LegacyLocationRestartV1 {
        canonical: canonical.clone(),
        label: label.to_owned(),
        latitude: CanonicalJsonValueV1::from_value(latitude_value, MAX_MIGRATION_CANDIDATE_BYTES)
            .map_err(canonical_phase_b_error)?,
        longitude: CanonicalJsonValueV1::from_value(longitude_value, MAX_MIGRATION_CANDIDATE_BYTES)
            .map_err(canonical_phase_b_error)?,
    }))
}

fn parse_restaurant_search_restart(
    compatibility: &ImportedCompatibilityStateV1,
) -> Result<Option<LegacyRestaurantSearchRestartV1>, PortError> {
    let Some(canonical) = compatibility_value(compatibility, "last_restaurant_search")? else {
        return Ok(None);
    };
    let restaurants = canonical
        .as_value()
        .as_object()
        .and_then(|object| object.get("restaurants"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            phase_b_error("legacy restaurant-search restart state must contain a restaurant array")
        })?;
    if restaurants.len() > 1_000 {
        return Err(phase_b_error(
            "legacy restaurant-search restart state exceeds its exact limit",
        ));
    }
    let restaurant_names = restaurants
        .iter()
        .map(|restaurant| {
            restaurant
                .as_object()
                .and_then(|restaurant| restaurant.get("name"))
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty() && name.trim() == *name && name.len() <= 200)
                .map(str::to_owned)
                .ok_or_else(|| {
                    phase_b_error("legacy restaurant-search restart entry has an invalid name")
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(LegacyRestaurantSearchRestartV1 {
        canonical: canonical.clone(),
        restaurant_names,
    }))
}

fn build_restart_state(state: &HouseholdStateV1) -> Result<LegacyPythonRestartStateV1, PortError> {
    Ok(LegacyPythonRestartStateV1 {
        location: parse_location_restart(&state.imported_compatibility)?,
        last_restaurant_search: parse_restaurant_search_restart(&state.imported_compatibility)?,
    })
}

fn source_entry_for_kind(
    sources: &PythonSourceSetFingerprint,
    kind: PythonStateSourceKind,
) -> &CheckedSourceEntry {
    match kind {
        PythonStateSourceKind::CurrentConfig => &sources.current_config,
        PythonStateSourceKind::LegacyConfig => &sources.legacy_config,
    }
}

fn read_source_bound(
    path: &Path,
    kind: PythonStateSourceKind,
    expected: &CheckedSourceEntry,
) -> Result<Vec<u8>, PortError> {
    #[cfg(test)]
    MIXED_SOURCE_READ_PROBE.with(|probe| probe.set(probe.get() + 1));
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        PortError::new(
            "python_state_changed",
            "legacy household state disappeared after review",
        )
    })?;
    validate_candidate_parent(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAXIMUM_SOURCE_BYTES
    {
        return Err(PortError::new(
            "python_state_changed",
            "legacy household state changed after review",
        ));
    }
    let current = CheckedSourceEntry {
        kind: kind.as_str(),
        locator_digest: locator_digest(path)?,
        state: "metadata_present",
        file_type: Some("regular"),
        byte_len: Some(metadata.len()),
        modified_ns: metadata_modified_ns(&metadata),
        file_identity: checked_file_identity(path, &metadata, "python_state_changed")?,
        content_digest: None,
    };
    if &current != expected {
        return Err(PortError::new(
            "python_state_changed",
            "legacy household state changed after review",
        ));
    }
    read_bounded_regular(
        path,
        &metadata,
        expected.file_identity.as_ref(),
        "python_import_source_changed",
    )
}

fn read_native_document_bound(
    path: &Path,
    expected: &CheckedSourceEntry,
) -> Result<(ImportDocument, Sha256Digest), PortError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        PortError::new(
            "python_state_changed",
            "protected native household snapshot disappeared after review",
        )
    })?;
    validate_candidate_parent(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAXIMUM_SOURCE_BYTES
    {
        return Err(PortError::new(
            "python_state_changed",
            "protected native household snapshot changed after review",
        ));
    }
    let current = CheckedSourceEntry {
        kind: "native_snapshot",
        locator_digest: locator_digest(path)?,
        state: "metadata_present",
        file_type: Some("regular"),
        byte_len: Some(metadata.len()),
        modified_ns: metadata_modified_ns(&metadata),
        file_identity: checked_file_identity(path, &metadata, "python_state_changed")?,
        content_digest: None,
    };
    let mut expected_metadata = expected.clone();
    expected_metadata.content_digest = None;
    if current != expected_metadata {
        return Err(PortError::new(
            "python_state_changed",
            "protected native household snapshot changed after review",
        ));
    }
    let bytes = read_bounded_regular(
        path,
        &metadata,
        expected.file_identity.as_ref(),
        "python_state_changed",
    )?;
    let content_digest = Sha256Digest(sha256(&bytes));
    if expected.content_digest.as_ref() != Some(&content_digest) {
        return Err(PortError::new(
            "python_state_changed",
            "protected native household snapshot bytes changed after review",
        ));
    }
    Ok((decode_import_document(&bytes)?, content_digest))
}

struct NativeSnapshotInspection {
    entry: CheckedSourceEntry,
    document: Option<(ImportDocument, Sha256Digest)>,
}

fn inspect_mixed_candidate(
    path: &Path,
    kind: PythonStateSourceKind,
) -> Result<CheckedSourceEntry, PortError> {
    let locator_digest = locator_digest(path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CheckedSourceEntry {
                kind: kind.as_str(),
                locator_digest,
                state: "absent",
                file_type: None,
                byte_len: None,
                modified_ns: None,
                file_identity: None,
                content_digest: None,
            });
        }
        Err(_) => {
            return Err(PortError::new(
                "python_import_read",
                "could not inspect a Python state source candidate",
            ));
        }
    };
    validate_candidate_parent(path)?;
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
    Ok(CheckedSourceEntry {
        kind: kind.as_str(),
        locator_digest,
        state: "metadata_present",
        file_type: Some("regular"),
        byte_len: Some(metadata.len()),
        modified_ns: metadata_modified_ns(&metadata),
        file_identity: checked_file_identity(path, &metadata, "python_import_read")?,
        content_digest: None,
    })
}

fn absent_source_entry(path: &Path, kind: &'static str) -> Result<CheckedSourceEntry, PortError> {
    Ok(CheckedSourceEntry {
        kind,
        locator_digest: locator_digest(path)?,
        state: "absent",
        file_type: None,
        byte_len: None,
        modified_ns: None,
        file_identity: None,
        content_digest: None,
    })
}

fn inspect_native_snapshot(path: &Path) -> Result<NativeSnapshotInspection, PortError> {
    let locator_digest = locator_digest(path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(NativeSnapshotInspection {
                entry: CheckedSourceEntry {
                    kind: "native_snapshot",
                    locator_digest,
                    state: "absent",
                    file_type: None,
                    byte_len: None,
                    modified_ns: None,
                    file_identity: None,
                    content_digest: None,
                },
                document: None,
            });
        }
        Err(_) => {
            return Err(PortError::new(
                "python_snapshot_invalid",
                "could not inspect native import",
            ));
        }
    };
    validate_candidate_parent(path).map_err(|_| {
        PortError::new(
            "python_snapshot_invalid",
            "native import parent is not a direct directory",
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(PortError::new(
            "python_snapshot_invalid",
            "native import must be a regular non-symlink file",
        ));
    }
    if metadata.len() > MAXIMUM_SOURCE_BYTES {
        return Err(PortError::new(
            "python_snapshot_invalid",
            "native import exceeds its size limit",
        ));
    }
    let file_identity = checked_file_identity(path, &metadata, "python_snapshot_invalid")?;
    let bytes = read_bounded_regular(
        path,
        &metadata,
        file_identity.as_ref(),
        "python_snapshot_invalid",
    )?;
    let content_digest = Sha256Digest(sha256(&bytes));
    let document = decode_import_document(&bytes)?;
    Ok(NativeSnapshotInspection {
        entry: CheckedSourceEntry {
            kind: "native_snapshot",
            locator_digest,
            state: "metadata_present",
            file_type: Some("regular"),
            byte_len: Some(metadata.len()),
            modified_ns: metadata_modified_ns(&metadata),
            file_identity,
            content_digest: Some(content_digest.clone()),
        },
        document: Some((document, content_digest)),
    })
}

fn validate_candidate_parent(path: &Path) -> Result<(), PortError> {
    let Some(parent) = path.parent() else {
        return Err(PortError::new(
            "python_import_locator",
            "Python state source has no parent directory",
        ));
    };
    match fs::symlink_metadata(parent) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(PortError::new(
                "python_import_locator",
                "Python state source parent must be a direct directory",
            ))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(PortError::new(
            "python_import_locator",
            "could not inspect Python state source parent",
        )),
    }
}

fn read_bounded_regular(
    path: &Path,
    checked_metadata: &fs::Metadata,
    checked_file_identity: Option<&Sha256Digest>,
    error_code: &'static str,
) -> Result<Vec<u8>, PortError> {
    let file = File::open(path)
        .map_err(|_| PortError::new(error_code, "could not open bounded state file"))?;
    let opened_metadata = file
        .metadata()
        .map_err(|_| PortError::new(error_code, "could not inspect opened bounded state file"))?;
    #[cfg(windows)]
    let identity_matches = {
        let identity = heyfood_windows_file::file_id_128_identity(&file)
            .map_err(|_| PortError::new(error_code, "could not inspect opened file identity"))?;
        checked_file_identity
            == Some(&windows_file_identity_digest(
                identity.volume_serial_number,
                identity.file_id,
            ))
    };
    #[cfg(not(windows))]
    let identity_matches = {
        let _ = checked_file_identity;
        true
    };
    if !same_file_metadata(checked_metadata, &opened_metadata)
        || !identity_matches
        || !opened_metadata.is_file()
        || opened_metadata.len() > MAXIMUM_SOURCE_BYTES
    {
        return Err(PortError::new(
            error_code,
            "bounded state file changed while it was being opened",
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAXIMUM_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| PortError::new(error_code, "could not read bounded state file"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAXIMUM_SOURCE_BYTES {
        return Err(PortError::new(
            error_code,
            "bounded state file exceeds its size limit",
        ));
    }
    Ok(bytes)
}

#[cfg(unix)]
fn same_file_metadata(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && metadata_modified_ns(left) == metadata_modified_ns(right)
}

#[cfg(not(unix))]
fn same_file_metadata(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.len() == right.len() && metadata_modified_ns(left) == metadata_modified_ns(right)
}

fn metadata_modified_ns(metadata: &fs::Metadata) -> Option<String> {
    let modified = metadata.modified().ok()?;
    let value = match modified.duration_since(UNIX_EPOCH) {
        Ok(duration) => i128::try_from(duration.as_nanos()).ok()?,
        Err(error) => -i128::try_from(error.duration().as_nanos()).ok()?,
    };
    Some(value.to_string())
}

#[cfg(unix)]
fn checked_file_identity(
    _path: &Path,
    metadata: &fs::Metadata,
    _error_code: &'static str,
) -> Result<Option<Sha256Digest>, PortError> {
    use std::os::unix::fs::MetadataExt;
    Ok(Some(unix_file_identity_digest(
        metadata.dev(),
        metadata.ino(),
    )))
}

#[cfg(windows)]
fn checked_file_identity(
    path: &Path,
    _metadata: &fs::Metadata,
    error_code: &'static str,
) -> Result<Option<Sha256Digest>, PortError> {
    let identity = heyfood_windows_file::open_regular_file_id_128(path)
        .map_err(|_| PortError::new(error_code, "could not inspect exact file identity"))?;
    Ok(Some(windows_file_identity_digest(
        identity.volume_serial_number,
        identity.file_id,
    )))
}

#[cfg(not(any(unix, windows)))]
fn checked_file_identity(
    _path: &Path,
    _metadata: &fs::Metadata,
    _error_code: &'static str,
) -> Result<Option<Sha256Digest>, PortError> {
    Ok(None)
}

#[cfg(any(unix, test))]
fn unix_file_identity_digest(device: u64, inode: u64) -> Sha256Digest {
    let mut preimage = Vec::with_capacity(UNIX_FILE_ID_DOMAIN.len() + 17);
    preimage.extend_from_slice(UNIX_FILE_ID_DOMAIN);
    preimage.push(0);
    preimage.extend_from_slice(&device.to_be_bytes());
    preimage.extend_from_slice(&inode.to_be_bytes());
    Sha256Digest(sha256(&preimage))
}

#[cfg(any(windows, test))]
fn windows_file_identity_digest(volume_serial: u64, file_id: [u8; 16]) -> Sha256Digest {
    let mut preimage = Vec::with_capacity(WINDOWS_FILE_ID_DOMAIN.len() + 25);
    preimage.extend_from_slice(WINDOWS_FILE_ID_DOMAIN);
    preimage.push(0);
    preimage.extend_from_slice(&volume_serial.to_be_bytes());
    preimage.extend_from_slice(&file_id);
    Sha256Digest(sha256(&preimage))
}

fn locator_digest(path: &Path) -> Result<Sha256Digest, PortError> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|_| {
                PortError::new(
                    "python_import_locator",
                    "could not make Python state locator absolute",
                )
            })?
            .join(path)
    };
    let normalized = normalize_locator(&absolute);
    Ok(Sha256Digest(sha256(&normalized)))
}

#[cfg(unix)]
fn normalize_locator(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    lexical_normalize(path).as_os_str().as_bytes().to_vec()
}

#[cfg(windows)]
fn normalize_locator(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    lexical_normalize(path)
        .as_os_str()
        .encode_wide()
        .flat_map(u16::to_be_bytes)
        .collect()
}

#[cfg(not(any(unix, windows)))]
fn normalize_locator(path: &Path) -> Vec<u8> {
    lexical_normalize(path)
        .to_string_lossy()
        .as_bytes()
        .to_vec()
}

fn lexical_normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

fn source_set_fingerprint(
    current_config: CheckedSourceEntry,
    legacy_config: CheckedSourceEntry,
    native_snapshot: CheckedSourceEntry,
) -> Result<PythonSourceSetFingerprint, PortError> {
    let encoded = source_set_json_bytes(&current_config, &legacy_config, &native_snapshot)?;
    let mut preimage = Vec::with_capacity(SOURCE_SET_DOMAIN.len() + encoded.len() + 1);
    preimage.extend_from_slice(SOURCE_SET_DOMAIN);
    preimage.push(0);
    preimage.extend_from_slice(&encoded);
    Ok(PythonSourceSetFingerprint {
        digest: Sha256Digest(sha256(&preimage)),
        current_config,
        legacy_config,
        native_snapshot,
    })
}

fn source_set_json_bytes(
    current_config: &CheckedSourceEntry,
    legacy_config: &CheckedSourceEntry,
    native_snapshot: &CheckedSourceEntry,
) -> Result<Vec<u8>, PortError> {
    let entries = BTreeMap::from([
        ("current_config", source_entry_json(current_config, false)),
        ("legacy_config", source_entry_json(legacy_config, false)),
        ("native_snapshot", source_entry_json(native_snapshot, true)),
    ]);
    serde_json::to_vec(&entries).map_err(|_| {
        PortError::new(
            "python_preview_encode",
            "could not encode source-set fingerprint",
        )
    })
}

fn source_entry_json(entry: &CheckedSourceEntry, include_content: bool) -> Value {
    let mut object = BTreeMap::from([
        ("byte_len", entry.byte_len.map_or(Value::Null, Value::from)),
        (
            "file_identity",
            entry
                .file_identity
                .as_ref()
                .map_or(Value::Null, |value| Value::String(value.0.clone())),
        ),
        (
            "file_type",
            entry
                .file_type
                .map_or(Value::Null, |value| Value::String(value.to_owned())),
        ),
        ("kind", Value::String(entry.kind.to_owned())),
        (
            "locator_digest",
            Value::String(entry.locator_digest.0.clone()),
        ),
        (
            "modified_ns",
            entry
                .modified_ns
                .as_ref()
                .map_or(Value::Null, |value| Value::String(value.clone())),
        ),
        ("state", Value::String(entry.state.to_owned())),
    ]);
    if include_content {
        object.insert(
            "content_digest",
            entry
                .content_digest
                .as_ref()
                .map_or(Value::Null, |value| Value::String(value.0.clone())),
        );
    }
    serde_json::to_value(object).expect("source fingerprint values are serializable")
}

fn selected_locator_digest(
    kind: PythonStateSourceKind,
    sources: &PythonSourceSetFingerprint,
) -> Sha256Digest {
    let locator = match kind {
        PythonStateSourceKind::CurrentConfig => &sources.current_config.locator_digest,
        PythonStateSourceKind::LegacyConfig => &sources.legacy_config.locator_digest,
    };
    selected_locator_digest_for(kind, locator)
}

fn selected_locator_digest_for(
    kind: PythonStateSourceKind,
    locator: &Sha256Digest,
) -> Sha256Digest {
    let mut preimage = Vec::with_capacity(
        SELECTED_LOCATOR_DOMAIN.len() + kind.as_str().len() + locator.0.len() + 1,
    );
    preimage.extend_from_slice(SELECTED_LOCATOR_DOMAIN);
    preimage.push(0);
    preimage.extend_from_slice(kind.as_str().as_bytes());
    preimage.extend_from_slice(locator.0.as_bytes());
    Sha256Digest(sha256(&preimage))
}

fn normalized_state_digest(state: &ImportedPythonState) -> Result<Sha256Digest, PortError> {
    let encoded = serde_json::to_vec(state).map_err(|_| {
        PortError::new(
            "python_snapshot_invalid",
            "could not encode normalized imported state",
        )
    })?;
    let mut preimage = Vec::with_capacity(NORMALIZED_STATE_DOMAIN.len() + encoded.len() + 9);
    preimage.extend_from_slice(NORMALIZED_STATE_DOMAIN);
    preimage.push(0);
    preimage.extend_from_slice(
        &u64::try_from(encoded.len())
            .map_err(|_| {
                PortError::new(
                    "python_snapshot_invalid",
                    "normalized imported state is too large",
                )
            })?
            .to_be_bytes(),
    );
    preimage.extend_from_slice(&encoded);
    Ok(Sha256Digest(sha256(&preimage)))
}

fn validate_safe_account_binding(state: &ImportedPythonState) -> Result<(), PortError> {
    let valid = state.account_user_id.as_deref().is_some_and(|account| {
        !account.is_empty()
            && account.len() <= 255
            && account.trim() == account
            && !account.chars().any(char::is_control)
    });
    if !valid {
        return Err(PortError::new(
            "python_snapshot_invalid",
            "native import lacks a valid account binding",
        ));
    }
    Ok(())
}

fn verify_source_matches_snapshot(
    bytes: &[u8],
    expected_state: &ImportedPythonState,
    expected_normalized_digest: &Sha256Digest,
    expected_source_digest: &Sha256Digest,
) -> Result<(), PortError> {
    let actual_source_digest = Sha256Digest(sha256(bytes));
    if &actual_source_digest != expected_source_digest {
        return Err(PortError::new(
            "python_import_conflict",
            "Python state source no longer matches the reviewed report",
        ));
    }
    let (_, rebuilt_state) = build_import(bytes, actual_source_digest.0)?;
    let actual_normalized_digest = normalized_state_digest(&rebuilt_state)?;
    if &actual_normalized_digest != expected_normalized_digest || &rebuilt_state != expected_state {
        return Err(PortError::new(
            "python_import_conflict",
            "Python state source no longer matches the reviewed snapshot",
        ));
    }
    Ok(())
}

fn write_import_document(
    destination: &Path,
    report: PythonImportReport,
    state: ImportedPythonState,
) -> Result<(), PortError> {
    let document = ImportDocument {
        schema_version: IMPORT_SCHEMA_VERSION,
        source_format: IMPORT_SOURCE_FORMAT.to_owned(),
        report,
        state,
    };
    let mut encoded = serde_json::to_vec_pretty(&document)
        .map_err(|_| PortError::new("python_import_encode", "could not encode native import"))?;
    encoded.push(b'\n');
    AtomicFile::replace(destination, &encoded)
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
    if !same_file_metadata(&metadata, &opened_metadata) || !opened_metadata.is_file() {
        return Err(PortError::new(
            "python_import_source_changed",
            "the Python state source changed while it was being opened",
        ));
    }
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
    let document = decode_import_document(&bytes)?;
    Ok(Some(document))
}

fn decode_import_document(bytes: &[u8]) -> Result<ImportDocument, PortError> {
    let document: ImportDocument = serde_json::from_slice(bytes)
        .map_err(|_| PortError::new("python_snapshot_invalid", "native import is invalid JSON"))?;
    if document.schema_version != IMPORT_SCHEMA_VERSION
        || document.source_format != IMPORT_SOURCE_FORMAT
    {
        return Err(PortError::new(
            "python_snapshot_invalid",
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
            "python_snapshot_invalid",
            "native import contains invalid provenance or migration state",
        ));
    }
    Ok(document)
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
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod fingerprint_golden_tests {
    use super::*;

    fn lock_order_fixture(
        name: &str,
    ) -> (
        PathBuf,
        LegacyPythonHouseholdMigrationV1,
        crate::HouseholdVault,
    ) {
        let root = std::env::temp_dir().join(format!(
            "heyfood-python-lock-order-{name}-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let migration = LegacyPythonHouseholdMigrationV1::new(
            LegacyPythonConfigRootV1::from_absolute_root(root.join("legacy-config")).unwrap(),
            root.join("native/python-state-import.v1.json"),
        );
        let vault = crate::HouseholdVault::open(
            &root.join("native"),
            AccountId::parse("acct-lock-order").unwrap(),
        )
        .unwrap();
        (root, migration, vault)
    }

    #[derive(Clone, Copy)]
    enum ContendedSourceRank {
        Current,
        Legacy,
        Snapshot,
    }

    fn source_lock_paths(
        migration: &LegacyPythonHouseholdMigrationV1,
    ) -> [(PathBuf, &'static str); 3] {
        [
            (
                sibling_config_lock_path(migration.config_path(LegacyPythonConfigKindV1::Current))
                    .unwrap(),
                "current",
            ),
            (
                sibling_config_lock_path(migration.config_path(LegacyPythonConfigKindV1::Legacy))
                    .unwrap(),
                "legacy",
            ),
            (
                migration
                    .snapshot_path()
                    .parent()
                    .unwrap()
                    .join(IMPORT_LOCK_NAME),
                "snapshot",
            ),
        ]
    }

    fn source_rank_index(rank: ContendedSourceRank) -> usize {
        match rank {
            ContendedSourceRank::Current => 0,
            ContendedSourceRank::Legacy => 1,
            ContendedSourceRank::Snapshot => 2,
        }
    }

    fn register_source_contention_observers(
        paths: &[(PathBuf, &'static str)],
        target: usize,
        events: &std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>,
    ) -> std::sync::mpsc::Receiver<()> {
        let (attempt_tx, attempt_rx) = std::sync::mpsc::channel();
        for (index, (path, label)) in paths.iter().enumerate() {
            register_legacy_source_lock_acquisition_observer(
                path.clone(),
                label,
                std::sync::Arc::clone(events),
                (index == target).then(|| attempt_tx.clone()),
            );
        }
        attempt_rx
    }

    fn clear_source_contention_observers(paths: &[(PathBuf, &'static str)]) {
        for (path, _) in paths {
            unregister_legacy_source_lock_acquisition_observer(path);
        }
    }

    async fn wait_for_source_lock_attempt(attempt: std::sync::mpsc::Receiver<()>) {
        tokio::task::spawn_blocking(move || {
            attempt.recv_timeout(std::time::Duration::from_secs(2))
        })
        .await
        .unwrap()
        .expect("source lock worker must reach the contended rank");
    }

    async fn wait_for_lock_release_events(
        events: &std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>,
        count: usize,
    ) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if events
                    .lock()
                    .map(|events| events.len() == count)
                    .unwrap_or(false)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("source lock worker must release every retained authority");
    }

    async fn exercise_contended_source_token_cancellation(rank: ContendedSourceRank) {
        let target = source_rank_index(rank);
        let (root, migration, vault) = lock_order_fixture(&format!("source-cancel-{target}"));
        let paths = source_lock_paths(&migration);
        let blocker = FileLock::acquire(&paths[target].0, true).unwrap();
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let attempt = register_source_contention_observers(&paths, target, &events);
        let mut lifecycle = vault
            .acquire_lifecycle_lease(CancellationToken::new())
            .await
            .unwrap();
        lifecycle.observe_lock_release("lifecycle", std::sync::Arc::clone(&events));
        let cancellation = CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let task = tokio::spawn(async move {
            migration
                .acquire_source_lease(lifecycle, worker_cancellation)
                .await
        });

        wait_for_source_lock_attempt(attempt).await;
        cancellation.cancel();
        drop(blocker);
        assert_eq!(
            task.await.unwrap().unwrap_err().code,
            "legacy_python_migration_cancelled"
        );
        let expected = match rank {
            ContendedSourceRank::Current => vec!["current", "lifecycle"],
            ContendedSourceRank::Legacy => vec!["legacy", "current", "lifecycle"],
            ContendedSourceRank::Snapshot => {
                vec!["snapshot", "legacy", "current", "lifecycle"]
            }
        };
        assert_eq!(*events.lock().unwrap(), expected);
        clear_source_contention_observers(&paths);
        fs::remove_dir_all(root).unwrap();
    }

    async fn exercise_contended_source_outer_abort(rank: ContendedSourceRank) {
        let target = source_rank_index(rank);
        let (root, migration, vault) = lock_order_fixture(&format!("source-abort-{target}"));
        let paths = source_lock_paths(&migration);
        let blocker = FileLock::acquire(&paths[target].0, true).unwrap();
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let attempt = register_source_contention_observers(&paths, target, &events);
        let mut lifecycle = vault
            .acquire_lifecycle_lease(CancellationToken::new())
            .await
            .unwrap();
        lifecycle.observe_lock_release("lifecycle", std::sync::Arc::clone(&events));
        let task = tokio::spawn(async move {
            migration
                .acquire_source_lease(lifecycle, CancellationToken::new())
                .await
        });

        wait_for_source_lock_attempt(attempt).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        drop(blocker);
        wait_for_lock_release_events(&events, 4).await;
        assert_eq!(
            *events.lock().unwrap(),
            vec!["snapshot", "legacy", "current", "lifecycle"]
        );
        clear_source_contention_observers(&paths);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn contended_source_token_cancellation_is_drop_ordered_at_every_rank() {
        for rank in [
            ContendedSourceRank::Current,
            ContendedSourceRank::Legacy,
            ContendedSourceRank::Snapshot,
        ] {
            exercise_contended_source_token_cancellation(rank).await;
        }
    }

    #[tokio::test]
    async fn contended_source_outer_abort_is_drop_ordered_at_every_rank() {
        for rank in [
            ContendedSourceRank::Current,
            ContendedSourceRank::Legacy,
            ContendedSourceRank::Snapshot,
        ] {
            exercise_contended_source_outer_abort(rank).await;
        }
    }

    async fn exercise_contended_snapshot_acquisition(abort: bool) {
        let (root, migration, vault) = lock_order_fixture(if abort {
            "snapshot-acquire-abort"
        } else {
            "snapshot-acquire-cancel"
        });
        let paths = source_lock_paths(&migration);
        let snapshot_path = paths[2].0.clone();
        let blocker = FileLock::acquire(&snapshot_path, true).unwrap();
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let attempt = register_source_contention_observers(&paths[2..], 0, &events);
        let mut lifecycle = vault
            .acquire_lifecycle_lease(CancellationToken::new())
            .await
            .unwrap();
        lifecycle.observe_lock_release("lifecycle", std::sync::Arc::clone(&events));
        let cancellation = CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let task = tokio::spawn(async move {
            migration
                .acquire_snapshot_retirement_lease(lifecycle, worker_cancellation)
                .await
        });

        wait_for_source_lock_attempt(attempt).await;
        if abort {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            drop(blocker);
        } else {
            cancellation.cancel();
            drop(blocker);
            assert_eq!(
                task.await.unwrap().unwrap_err().code,
                "legacy_python_migration_cancelled"
            );
        }
        wait_for_lock_release_events(&events, 2).await;
        assert_eq!(*events.lock().unwrap(), vec!["snapshot", "lifecycle"]);
        clear_source_contention_observers(&paths);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn contended_snapshot_token_cancellation_is_drop_ordered() {
        exercise_contended_snapshot_acquisition(false).await;
    }

    #[tokio::test]
    async fn contended_snapshot_outer_abort_is_drop_ordered() {
        exercise_contended_snapshot_acquisition(true).await;
    }

    #[cfg(feature = "native-credentials")]
    async fn exercise_contended_credential_acquisition(target: usize, abort: bool) {
        let (root, migration, vault) = lock_order_fixture(&format!(
            "credential-acquire-{target}-{}",
            if abort { "abort" } else { "cancel" }
        ));
        let paths = source_lock_paths(&migration);
        let credential_paths = &paths[..2];
        let blocker = FileLock::acquire(&credential_paths[target].0, true).unwrap();
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let attempt = register_source_contention_observers(credential_paths, target, &events);
        let cancellation = CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let account_slot = vault.account_slot().clone();
        let task = tokio::spawn(async move {
            migration
                .acquire_credential_source_lease(account_slot, worker_cancellation)
                .await
        });

        wait_for_source_lock_attempt(attempt).await;
        if abort {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            drop(blocker);
        } else {
            cancellation.cancel();
            drop(blocker);
            assert_eq!(
                task.await.unwrap().unwrap_err().code,
                "legacy_python_migration_cancelled"
            );
        }
        let expected = if target == 0 && !abort {
            vec!["current"]
        } else {
            vec!["legacy", "current"]
        };
        wait_for_lock_release_events(&events, expected.len()).await;
        assert_eq!(*events.lock().unwrap(), expected);
        clear_source_contention_observers(credential_paths);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(feature = "native-credentials")]
    #[tokio::test]
    async fn contended_credential_token_cancellation_is_drop_ordered_at_every_rank() {
        for target in 0..2 {
            exercise_contended_credential_acquisition(target, false).await;
        }
    }

    #[cfg(feature = "native-credentials")]
    #[tokio::test]
    async fn contended_credential_outer_abort_is_drop_ordered_at_every_rank() {
        for target in 0..2 {
            exercise_contended_credential_acquisition(target, true).await;
        }
    }

    fn observe_source_transaction_release(
        transaction: &mut LegacyPythonSourceVaultLeaseTransactionV1,
    ) -> std::sync::Arc<std::sync::Mutex<Vec<&'static str>>> {
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        transaction
            .vault_lease_mut()
            .observe_lock_release_order(std::sync::Arc::clone(&events));
        let source = transaction.source_lease_mut();
        source
            ._current_config_lock
            .observe_drop("current", std::sync::Arc::clone(&events));
        source
            ._legacy_config_lock
            .observe_drop("legacy", std::sync::Arc::clone(&events));
        source
            ._snapshot_lock
            .observe_drop("snapshot", std::sync::Arc::clone(&events));
        events
    }

    fn observe_pending_source_release(
        source: &mut LegacyPythonSourceLeaseV1,
        lifecycle: &mut HouseholdLifecycleLease,
    ) -> std::sync::Arc<std::sync::Mutex<Vec<&'static str>>> {
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        source
            ._current_config_lock
            .observe_drop("current", std::sync::Arc::clone(&events));
        source
            ._legacy_config_lock
            .observe_drop("legacy", std::sync::Arc::clone(&events));
        source
            ._snapshot_lock
            .observe_drop("snapshot", std::sync::Arc::clone(&events));
        lifecycle.observe_lock_release("lifecycle", std::sync::Arc::clone(&events));
        events
    }

    async fn source_transaction_fixture(
        name: &str,
    ) -> (
        PathBuf,
        LegacyPythonHouseholdMigrationV1,
        LegacyPythonSourceVaultLeaseTransactionV1,
        std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>,
    ) {
        let (root, migration, vault) = lock_order_fixture(name);
        let lifecycle = vault
            .acquire_lifecycle_lease(CancellationToken::new())
            .await
            .unwrap();
        let mut source = migration
            .acquire_source_lease(lifecycle, CancellationToken::new())
            .await
            .unwrap();
        let lifecycle = migration.take_lifecycle_for_vault(&mut source).unwrap();
        let acquired = vault
            .acquire_vault_lease_after_narrower(
                source,
                lifecycle,
                crate::HouseholdVaultLeaseModeV1::CreateIfMissing,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let mut transaction = migration.bind_source_vault_transaction(acquired).unwrap();
        let events = observe_source_transaction_release(&mut transaction);
        (root, migration, transaction, events)
    }

    fn fail_with_question_mark(
        _transaction: LegacyPythonSourceVaultLeaseTransactionV1,
    ) -> Result<(), PortError> {
        Err(PortError::new(
            "lock_order_test",
            "exercise implicit transaction cleanup",
        ))?;
        Ok(())
    }

    #[tokio::test]
    async fn validated_source_transaction_release_is_exact_reverse_acquisition_order() {
        let (root, migration, transaction, events) = source_transaction_fixture("success").await;
        let debug = format!("{transaction:?}");
        assert!(debug.contains("vault_lease_retained: true"));
        assert!(debug.contains("source_lease_retained: true"));
        assert!(!debug.contains("acct-lock-order"));
        assert!(!debug.contains(root.to_string_lossy().as_ref()));

        let lifecycle = migration
            .release_source_vault_transaction(transaction, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            *events.lock().unwrap(),
            vec!["vault", "snapshot", "legacy", "current"]
        );
        drop(lifecycle);
        assert_eq!(
            *events.lock().unwrap(),
            vec!["vault", "snapshot", "legacy", "current", "lifecycle"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn question_mark_error_releases_composite_in_exact_reverse_order() {
        let (root, _migration, transaction, events) =
            source_transaction_fixture("question-mark").await;

        assert_eq!(
            fail_with_question_mark(transaction).unwrap_err().code,
            "lock_order_test"
        );
        assert_eq!(
            *events.lock().unwrap(),
            vec!["vault", "snapshot", "legacy", "current", "lifecycle"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn cancelled_validated_release_falls_back_to_exact_composite_drop_order() {
        let (root, migration, transaction, events) = source_transaction_fixture("cancelled").await;
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        assert_eq!(
            migration
                .release_source_vault_transaction(transaction, cancellation)
                .await
                .unwrap_err()
                .code,
            "household_operation_cancelled"
        );
        assert_eq!(
            *events.lock().unwrap(),
            vec!["vault", "snapshot", "legacy", "current", "lifecycle"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn cancelled_vault_acquisition_releases_source_before_lifecycle() {
        let (root, migration, vault) = lock_order_fixture("acquire-cancelled");
        let lifecycle = vault
            .acquire_lifecycle_lease(CancellationToken::new())
            .await
            .unwrap();
        let mut source = migration
            .acquire_source_lease(lifecycle, CancellationToken::new())
            .await
            .unwrap();
        let mut lifecycle = migration.take_lifecycle_for_vault(&mut source).unwrap();
        let events = observe_pending_source_release(&mut source, &mut lifecycle);
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        assert_eq!(
            vault
                .acquire_vault_lease_after_narrower(
                    source,
                    lifecycle,
                    crate::HouseholdVaultLeaseModeV1::CreateIfMissing,
                    cancellation,
                )
                .await
                .unwrap_err()
                .code,
            "household_operation_cancelled"
        );
        assert_eq!(
            *events.lock().unwrap(),
            vec!["snapshot", "legacy", "current", "lifecycle"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn failed_vault_acquisition_releases_source_before_lifecycle() {
        let (root, migration, vault) = lock_order_fixture("acquire-error");
        let lifecycle = vault
            .acquire_lifecycle_lease(CancellationToken::new())
            .await
            .unwrap();
        let mut source = migration
            .acquire_source_lease(lifecycle, CancellationToken::new())
            .await
            .unwrap();
        let mut lifecycle = migration.take_lifecycle_for_vault(&mut source).unwrap();
        let events = observe_pending_source_release(&mut source, &mut lifecycle);

        let error = vault
            .acquire_vault_lease_after_narrower(
                source,
                lifecycle,
                crate::HouseholdVaultLeaseModeV1::RequireExisting,
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "household_vault_path");
        assert_eq!(
            *events.lock().unwrap(),
            vec!["snapshot", "legacy", "current", "lifecycle"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn validated_snapshot_transaction_release_is_vault_snapshot_lifecycle() {
        let (root, migration, vault) = lock_order_fixture("snapshot-success");
        let lifecycle = vault
            .acquire_lifecycle_lease(CancellationToken::new())
            .await
            .unwrap();
        let mut snapshot = migration
            .acquire_snapshot_retirement_lease(lifecycle, CancellationToken::new())
            .await
            .unwrap();
        let lifecycle = migration
            .take_snapshot_lifecycle_for_vault(&mut snapshot)
            .unwrap();
        let acquired = vault
            .acquire_vault_lease_after_narrower(
                snapshot,
                lifecycle,
                crate::HouseholdVaultLeaseModeV1::CreateIfMissing,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let mut transaction = migration.bind_snapshot_vault_transaction(acquired).unwrap();
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        transaction
            .vault_lease_mut()
            .observe_lock_release_order(std::sync::Arc::clone(&events));
        transaction
            .source_lease_mut()
            ._snapshot_lock
            .observe_drop("snapshot", std::sync::Arc::clone(&events));

        let lifecycle = migration
            .release_snapshot_vault_transaction(transaction, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(*events.lock().unwrap(), vec!["vault", "snapshot"]);
        drop(lifecycle);
        assert_eq!(
            *events.lock().unwrap(),
            vec!["vault", "snapshot", "lifecycle"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn snapshot_and_credential_leases_release_their_narrow_locks_in_reverse_order() {
        let (root, migration, vault) = lock_order_fixture("narrow-leases");
        let lifecycle = vault
            .acquire_lifecycle_lease(CancellationToken::new())
            .await
            .unwrap();
        let mut snapshot = migration
            .acquire_snapshot_retirement_lease(lifecycle, CancellationToken::new())
            .await
            .unwrap();
        let snapshot_events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        snapshot
            ._snapshot_lock
            .observe_drop("snapshot", std::sync::Arc::clone(&snapshot_events));
        snapshot
            .lifecycle
            .as_mut()
            .unwrap()
            .observe_lock_release("lifecycle", std::sync::Arc::clone(&snapshot_events));
        drop(snapshot);
        assert_eq!(
            *snapshot_events.lock().unwrap(),
            vec!["snapshot", "lifecycle"]
        );

        #[cfg(feature = "native-credentials")]
        {
            let mut credential = migration
                .acquire_credential_source_lease(
                    vault.account_slot().clone(),
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            let credential_events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            credential
                ._current_config_lock
                .observe_drop("current", std::sync::Arc::clone(&credential_events));
            credential
                ._legacy_config_lock
                .observe_drop("legacy", std::sync::Arc::clone(&credential_events));
            drop(credential);
            assert_eq!(
                *credential_events.lock().unwrap(),
                vec!["legacy", "current"]
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn d2_phase_a_candidate_has_no_structural_slot_for_credential_values() {
        let object = parse_bounded_json_object_v1(
            br#"{
                "account_user_id":"acct-candidate",
                "first_name":"Owner",
                "credential_store":"file",
                "api_key":"api-key-canary",
                "oauth":{"access_token":"oauth-canary"},
                "session":{"refresh_token":"session-canary"},
                "location":{"label":"Home","latitude":35.0,"longitude":-120.0},
                "unknown_extension":{"secret":"unknown-canary"}
            }"#,
            CompatibilityJsonLimitsV1::MIGRATION_CANDIDATE,
        )
        .unwrap();
        let LegacyCandidateConfigProjectionV1 {
            first_name,
            compatibility_fields,
            config_field_evidence,
        } = build_secret_free_candidate_config(&object).unwrap();
        let candidate = LegacyPythonPresentCandidateV1 {
            account: AccountId::parse("acct-candidate").unwrap(),
            selected_kind: LegacyPythonConfigKindV1::Current,
            credential_store: LegacyCredentialStoreV1::File,
            source_digest: CanonicalDigestV1::from_bytes([0x11; 32]),
            snapshot_evidence: None,
            first_name,
            compatibility_fields,
            config_field_evidence,
            household: None,
            local_profiles: None,
            profile_outbox: None,
        };

        // Deliberately exhaustive: adding a value-retaining candidate field
        // forces this structural canary test to be reconsidered.
        let LegacyPythonPresentCandidateV1 {
            account: _,
            selected_kind: _,
            credential_store: _,
            source_digest: _,
            snapshot_evidence: _,
            first_name,
            compatibility_fields,
            config_field_evidence,
            household,
            local_profiles,
            profile_outbox,
        } = candidate;
        assert_eq!(
            first_name.unwrap().as_value(),
            &Value::String("Owner".to_owned())
        );
        assert_eq!(compatibility_fields.len(), 1);
        assert!(compatibility_fields.contains_key("location"));
        assert!(household.is_none());
        assert!(local_profiles.is_none());
        assert!(profile_outbox.is_none());
        for credential in ["api_key", "oauth", "session"] {
            let evidence = config_field_evidence
                .iter()
                .find(|entry| entry.field_name == credential)
                .unwrap();
            assert_eq!(evidence.role, LegacyConfigFieldRoleV1::Credential);
            assert!(evidence.source_digest.is_none());
        }
        let unknown = config_field_evidence
            .iter()
            .find(|entry| entry.field_name == "unknown_extension")
            .unwrap();
        assert_eq!(unknown.role, LegacyConfigFieldRoleV1::Unknown);
        assert!(unknown.source_digest.is_none());
        for value in compatibility_fields.values() {
            let retained = value.as_value().to_string();
            for canary in [
                "api-key-canary",
                "oauth-canary",
                "session-canary",
                "unknown-canary",
            ] {
                assert!(!retained.contains(canary));
            }
        }

        let nested_credential = serde_json::json!({
            "members": [{
                "id": "member-one",
                "name": "Member",
                "profile": {"access_token": "nested-canary"}
            }]
        })
        .as_object()
        .unwrap()
        .clone();
        assert_eq!(
            CredentialFreeLegacyObjectV1::new(nested_credential)
                .unwrap_err()
                .code,
            "legacy_python_credential_material"
        );
    }

    #[test]
    #[cfg(unix)]
    fn d2_swapped_or_tampered_bound_keyring_probes_are_rejected() {
        let root = std::env::temp_dir().join(format!(
            "heyfood-python-bound-probe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let account = AccountId::parse("acct-probe").unwrap();
        let vault = crate::HouseholdVault::open(&root.join("vault"), account).unwrap();
        let config_root =
            LegacyPythonConfigRootV1::from_absolute_root(root.join("legacy-config")).unwrap();
        let probes = LegacyPythonKeyringProbeSetV1::authoritative_missing(
            vault.account_slot(),
            &config_root,
        )
        .unwrap();
        let swapped = LegacyPythonKeyringProbeSetV1 {
            current: probes.legacy.clone(),
            legacy: probes.current.clone(),
        };
        assert_eq!(
            validate_keyring_probe_set(&swapped, vault.account_slot(), &config_root)
                .unwrap_err()
                .code,
            "legacy_python_keyring_evidence_mismatch"
        );

        let mut tampered = probes;
        tampered.current.evidence_digest = CanonicalDigestV1::from_bytes([0x99; 32]);
        assert_eq!(
            validate_keyring_probe_set(&tampered, vault.account_slot(), &config_root)
                .unwrap_err()
                .code,
            "legacy_python_keyring_evidence_mismatch"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preview_never_invokes_the_actual_mixed_source_reader() {
        MIXED_SOURCE_READ_PROBE.with(|probe| probe.set(0));
        let root = std::env::temp_dir().join(format!(
            "heyfood-python-reader-probe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("config.json");
        fs::write(&source, b"{mixed-source-reader-probe").unwrap();
        let importer = PythonStateImporter::under(&source, root.join("native"));

        let preview = importer.preview_state().unwrap();
        MIXED_SOURCE_READ_PROBE.with(|probe| assert_eq!(probe.get(), 0));
        assert_eq!(
            importer.verify_after_review(&preview).unwrap_err().code,
            "python_import_format"
        );
        MIXED_SOURCE_READ_PROBE.with(|probe| assert_eq!(probe.get(), 1));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unix_and_windows_file_identity_preimages_have_exact_golden_digests() {
        assert_eq!(
            unix_file_identity_digest(0x0102_0304_0506_0708, 0x1112_1314_1516_1718).as_str(),
            "c2623c6f5678e33ce36053460208966634dfbea69af0af01fdf1534b9dffdcfd"
        );
        assert_eq!(
            windows_file_identity_digest(
                0x0102_0304_0506_0708,
                [
                    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c,
                    0x0d, 0x0e, 0x0f,
                ],
            )
            .as_str(),
            "257d6ea932be9d42121bfbe342df728abdabb5f80c4b2638c5edaec79bdb970e"
        );
    }

    #[test]
    fn source_set_v2_exact_vector_covers_absent_present_timestamp_and_platform_forms() {
        let current = CheckedSourceEntry {
            kind: "current_config",
            locator_digest: Sha256Digest("a".repeat(64)),
            state: "metadata_present",
            file_type: Some("regular"),
            byte_len: Some(7),
            modified_ns: Some("-5".to_owned()),
            file_identity: Some(Sha256Digest(
                "c2623c6f5678e33ce36053460208966634dfbea69af0af01fdf1534b9dffdcfd".to_owned(),
            )),
            content_digest: None,
        };
        let legacy = CheckedSourceEntry {
            kind: "legacy_config",
            locator_digest: Sha256Digest("b".repeat(64)),
            state: "metadata_present",
            file_type: Some("regular"),
            byte_len: Some(9),
            modified_ns: None,
            file_identity: Some(Sha256Digest(
                "257d6ea932be9d42121bfbe342df728abdabb5f80c4b2638c5edaec79bdb970e".to_owned(),
            )),
            content_digest: None,
        };
        let native = CheckedSourceEntry {
            kind: "native_snapshot",
            locator_digest: Sha256Digest("c".repeat(64)),
            state: "absent",
            file_type: None,
            byte_len: None,
            modified_ns: None,
            file_identity: None,
            content_digest: None,
        };
        let expected_json = concat!(
            "{\"current_config\":{\"byte_len\":7,\"file_identity\":\"",
            "c2623c6f5678e33ce36053460208966634dfbea69af0af01fdf1534b9dffdcfd",
            "\",\"file_type\":\"regular\",\"kind\":\"current_config\",\"locator_digest\":\"",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "\",\"modified_ns\":\"-5\",\"state\":\"metadata_present\"},",
            "\"legacy_config\":{\"byte_len\":9,\"file_identity\":\"",
            "257d6ea932be9d42121bfbe342df728abdabb5f80c4b2638c5edaec79bdb970e",
            "\",\"file_type\":\"regular\",\"kind\":\"legacy_config\",\"locator_digest\":\"",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "\",\"modified_ns\":null,\"state\":\"metadata_present\"},",
            "\"native_snapshot\":{\"byte_len\":null,\"content_digest\":null,",
            "\"file_identity\":null,\"file_type\":null,\"kind\":\"native_snapshot\",",
            "\"locator_digest\":\"",
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "\",\"modified_ns\":null,\"state\":\"absent\"}}"
        );
        assert_eq!(
            source_set_json_bytes(&current, &legacy, &native).unwrap(),
            expected_json.as_bytes()
        );
        assert_eq!(
            source_set_fingerprint(current, legacy, native)
                .unwrap()
                .digest()
                .as_str(),
            "8925fd250b0d474ad3bc06c40ec18067f2d1dd9e4566731292edf5e2b5951440"
        );
    }
}
