//! Thin native composition seams for the Phase 0 qualification build.

#![forbid(unsafe_code)]

pub mod agent_discovery;
pub mod native_household_composition;

use std::fmt::Write as _;
use std::{
    collections::BTreeSet,
    fmt, io,
    marker::PhantomData,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use heyfood_agent_runtime::{HttpService, MAX_JSON_RESPONSE_BYTES, OwnerSyncTransportResultV1};
use heyfood_application::{
    AudioCapturePort, AuthoritativeConsentStateV1, AuthorizedHostedContextV1, BoxFuture,
    CapabilitySnapshot, ConfirmGroceryMutation, CreateMemberWithDeclaredProfileV1, CreateMenuWatch,
    CreateMenuWatchRequest, DeployedGroceryMutationRequest, DiscoverCapabilities, EnsureSession,
    EnsureSessionError, EnsureSessionOutcome, ExportGroceryList, GroceryExport, HouseholdLoad,
    HouseholdSession, ListMenuWatches, NativeMemberAgeEvidenceV1, OptionalCapabilityStatus,
    OwnerSyncTransitionEventV1, PortError, PrepareGroceryMutation, ProfileReadinessStatus,
    ReadActiveGroceryDisplay, ReadGroceryExclusions, ReadStatus, RefreshPolicy, RemoveMenuWatch,
    RunTurnOutcome, SaveMemberDeclaredProfileV1, SaveOwnerProfileAndSyncIntentV1,
    SelectedHouseholdTargetV1, ServicePort, TransitionOwnerSyncIntentV1, TurnContext, TurnFailure,
    TurnFailureKind, TurnRequest, UNRENDERABLE_AGENT_RESULT_MESSAGE, VoiceReadinessStatus,
    execute_one_shot_turn, owner_profile_action_eligibility_v1,
};
use heyfood_cli::{
    AskArgs, Command, GroceryCommand, HealthCommand, ItemArgs, LogArgs, MealType, MenuWatchCommand,
    OutputMode, render_agent_result_with_private_authorities, render_grocery_exclusions,
    render_grocery_list, render_grocery_mutation_result, render_grocery_proposal,
    render_health_context, render_item_result, render_json, render_menu_watch,
    render_menu_watch_list,
};
use heyfood_core::{
    AddItemsRequestWire, AgentConfirmationCommandWire, AgentEvent, CanonicalJsonObjectV1,
    CanonicalTimestampV1, CommitId, CompatibilityJsonLimitsV1, DisplayName,
    ExclusionMutationRequestWire, GroceryConfirmationToken, GroceryDecisionWire, GroceryEntityId,
    GroceryItemInputWire, GroceryListVersion, GroceryMutationConfirmRequestWire,
    HouseholdDeclaredProfileV1, HouseholdLifecycleV1, HouseholdProfileDocumentV1,
    HouseholdProfileOutboxEntryV1, HouseholdProfileRecordV1, HouseholdProfileStateV1,
    HouseholdRevision, HouseholdScope, HouseholdStateV1, HouseholdSubjectId, ImportedPythonState,
    LastDefiniteOwnerSyncErrorV1, MAX_OWNER_SYNC_REQUEST_BODY_BYTES, MenuWatchId,
    OnboardingProfileInput, OperationId, OwnerSyncIntentPhaseV1, OwnerSyncIntentV1,
    ProfileRevision, RelationshipV1, RemoteProfileBaseV1, RemoteProfileExistenceV1,
    RemoveItemsRequestWire, RestaurantId, SessionCredentials, SessionSnapshot,
    TranscriptionPurpose, UpdateItemStateRequestWire, WatchCadenceWire, WatchHour, WatchWeekday,
    canonical_sha256_v1, parse_bounded_typed_json_v1, terminal_safe_text,
};
use heyfood_platform::{
    NativeSignalSource, ProtectedHouseholdReason, PythonStatePreview, SensitiveExportWriter,
    SignalEvent, VerifiedPythonState,
};
use heyfood_tui::{
    BoundedHouseholdMemberDraftV1, Effect, ExitReason, HouseholdAccountBindingDigestV1,
    HouseholdAgeEvidenceInputV1, HouseholdContextApplyFailureV1, HouseholdManagementFailureV1,
    HouseholdManagementLoadPurposeV1, HouseholdMemberPresentationV1, HouseholdModeGenerationV1,
    HouseholdMutationFailureV1, HouseholdMutationKindV1, HouseholdOperationBindingV1,
    HouseholdOperationIdV1, HouseholdPresentationModeV1, HouseholdReducerCorrelationV1,
    NativeOwnerProfileSaveStatusV1, OwnerProfileActionLoadPurposeV1, OwnerProfileRetryActionV1,
    OwnerProfileRetryEligibilityV1, OwnerProfileRetryUnavailableReasonV1, OwnerSyncIntentHandleV1,
    PanelRequest, PresentedHouseholdContextV1, ProfileActionsLoadedV1, ProfileConsentFailureV1,
    ProfileConsentFinishedV1, ProfilePresentationModeV1, ProfileRetrySyncFinishedV1, RuntimeEvent,
    TuiError, VoiceAvailability,
};
use serde_json::{Map, Value, json};
use tokio::{
    runtime::Runtime,
    sync::{Mutex, mpsc},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

pub const QUALIFIED_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
pub const MAX_CONFIRMATION_STDIN_BYTES: usize = 1024 * 1024;
const HOUSEHOLD_LOG_HUMAN_ERROR_MESSAGE: &str =
    "hey.food could not complete this Household request. Review current state before trying again.";
const AGENT_HUMAN_ERROR_MESSAGE: &str = "hey.food could not complete this request. Try again.";
const GROCERY_EXPORT_REQUIRES_PROTECTED_FILE_MESSAGE: &str = "Grocery exports can contain private Household data. Use `--out FILE` to write an owner-only export.";

#[derive(Clone, Eq, PartialEq)]
pub struct OneShotError {
    pub code: &'static str,
    pub message: String,
    pub outcome_uncertain: bool,
}

impl fmt::Debug for OneShotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OneShotError")
            .field("code", &self.code)
            .field("message", &"[REDACTED]")
            .field("outcome_uncertain", &self.outcome_uncertain)
            .finish()
    }
}

impl fmt::Display for OneShotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for OneShotError {}

impl From<heyfood_application::PortError> for OneShotError {
    fn from(value: heyfood_application::PortError) -> Self {
        Self {
            code: value.code,
            message: value.message,
            outcome_uncertain: value.outcome_uncertain,
        }
    }
}

impl From<EnsureSessionError> for OneShotError {
    fn from(value: EnsureSessionError) -> Self {
        let (code, outcome_uncertain) = match &value {
            EnsureSessionError::ReconciliationRequired => ("session_reconciliation_required", true),
            EnsureSessionError::Service(error) => (error.code, error.outcome_uncertain),
            EnsureSessionError::ServiceReconciliationRequired(_) => {
                ("session_refresh_outcome_uncertain", true)
            }
            EnsureSessionError::CredentialReconciliationRequired(_) => {
                ("session_refresh_persistence_uncertain", true)
            }
            EnsureSessionError::ReconciliationMarkerWrite { .. } => {
                ("session_reconciliation_marker_write", true)
            }
        };
        Self {
            code,
            message: terminal_safe_text(&value.to_string()),
            outcome_uncertain,
        }
    }
}

const OWNER_SYNC_SUCCESS_RESPONSE_LIMITS: CompatibilityJsonLimitsV1 = CompatibilityJsonLimitsV1 {
    maximum_bytes: MAX_JSON_RESPONSE_BYTES,
    maximum_depth: 2,
    maximum_object_keys: 3,
    maximum_array_entries: 0,
    maximum_nodes: 4,
};

#[derive(Clone, Eq, PartialEq)]
pub enum OwnerSyncDispatchClassificationV1 {
    CancelledBeforeSend,
    DefiniteSuccess {
        remote_version: u64,
        updated_at: CanonicalTimestampV1,
    },
    DefiniteFailure {
        error: LastDefiniteOwnerSyncErrorV1,
    },
    VersionConflict,
    OutcomeUncertain,
}

impl fmt::Debug for OwnerSyncDispatchClassificationV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CancelledBeforeSend => formatter.write_str("CancelledBeforeSend"),
            Self::DefiniteSuccess {
                remote_version,
                updated_at,
            } => formatter
                .debug_struct("DefiniteSuccess")
                .field("remote_version", remote_version)
                .field("updated_at", updated_at)
                .finish(),
            Self::DefiniteFailure { error } => formatter
                .debug_struct("DefiniteFailure")
                .field("error", error)
                .finish(),
            Self::VersionConflict => formatter.write_str("VersionConflict"),
            Self::OutcomeUncertain => formatter.write_str("OutcomeUncertain"),
        }
    }
}

/// Total D2 classification of the purpose-specific transport evidence. Error
/// response bodies never influence a transition; only an exact bounded 2xx
/// success schema may remove the durable owner-sync intent.
#[must_use]
pub fn classify_owner_sync_transport_v1(
    result: OwnerSyncTransportResultV1,
) -> OwnerSyncDispatchClassificationV1 {
    let (status, body) = match result {
        OwnerSyncTransportResultV1::CancelledBeforeSend => {
            return OwnerSyncDispatchClassificationV1::CancelledBeforeSend;
        }
        OwnerSyncTransportResultV1::OutcomeUncertain { .. } => {
            return OwnerSyncDispatchClassificationV1::OutcomeUncertain;
        }
        OwnerSyncTransportResultV1::Response { status, body } => (status, body),
    };
    match status {
        200..=299 => classify_owner_sync_success_body(&body)
            .unwrap_or(OwnerSyncDispatchClassificationV1::OutcomeUncertain),
        400 | 422 => OwnerSyncDispatchClassificationV1::DefiniteFailure {
            error: LastDefiniteOwnerSyncErrorV1::Validation,
        },
        401 => OwnerSyncDispatchClassificationV1::DefiniteFailure {
            error: LastDefiniteOwnerSyncErrorV1::Unauthorized,
        },
        403 => OwnerSyncDispatchClassificationV1::DefiniteFailure {
            error: LastDefiniteOwnerSyncErrorV1::Forbidden,
        },
        404 => OwnerSyncDispatchClassificationV1::DefiniteFailure {
            error: LastDefiniteOwnerSyncErrorV1::NotFound,
        },
        409 => OwnerSyncDispatchClassificationV1::VersionConflict,
        _ => OwnerSyncDispatchClassificationV1::OutcomeUncertain,
    }
}

fn classify_owner_sync_success_body(body: &[u8]) -> Option<OwnerSyncDispatchClassificationV1> {
    let value = parse_bounded_typed_json_v1(body, OWNER_SYNC_SUCCESS_RESPONSE_LIMITS).ok()?;
    let object = value.as_object()?;
    if object.len() != 3 || object.get("member_id").and_then(Value::as_str) != Some("_self") {
        return None;
    }
    let remote_version = object.get("version").and_then(Value::as_u64)?;
    if remote_version == 0 {
        return None;
    }
    let updated_at =
        CanonicalTimestampV1::parse(object.get("updated_at").and_then(Value::as_str)?.to_owned())
            .ok()?;
    Some(OwnerSyncDispatchClassificationV1::DefiniteSuccess {
        remote_version,
        updated_at,
    })
}

#[derive(Clone)]
struct LoadedOwnerSyncIntentV1 {
    handle: OwnerSyncIntentHandleV1,
    intent: OwnerSyncIntentV1,
    effective_profile: Value,
    household_updated_at: CanonicalTimestampV1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeOwnerSyncOutcomeV1 {
    Synced,
    Pending,
    LocalOnlyNoConsent,
    Interrupted,
    ConsentVersionChangedRequiresNewSave,
    ConsentRevokedRegrantRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RemoteOwnerProfileV1 {
    Absent,
    Present {
        version: u64,
        profile_digest: heyfood_core::CanonicalDigestV1,
    },
}

fn native_owner_error(code: &'static str, message: &'static str) -> PortError {
    PortError::new(code, message)
}

fn canonical_timestamp_now_v1() -> Result<CanonicalTimestampV1, PortError> {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| {
        native_owner_error(
            "owner_profile_clock",
            "the system clock is before the Unix epoch",
        )
    })?;
    let seconds = i64::try_from(elapsed.as_secs()).map_err(|_| {
        native_owner_error(
            "owner_profile_clock",
            "the system clock is outside the supported range",
        )
    })?;
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_date_from_unix_days(days)?;
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    CanonicalTimestampV1::parse(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{:03}Z",
        elapsed.subsec_millis()
    ))
    .map_err(|_| {
        native_owner_error(
            "owner_profile_clock",
            "the system clock could not be canonicalized",
        )
    })
}

fn civil_date_from_unix_days(days: i64) -> Result<(i64, i64, i64), PortError> {
    // Howard Hinnant's civil-from-days algorithm. The input is bounded by
    // SystemTime above; this dependency-free conversion keeps commit
    // timestamps canonical without exposing another clock across the driver.
    let shifted = days.checked_add(719_468).ok_or_else(|| {
        native_owner_error(
            "owner_profile_clock",
            "the system clock is outside the supported range",
        )
    })?;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    if !(0..=9_999).contains(&year) {
        return Err(native_owner_error(
            "owner_profile_clock",
            "the system clock is outside the canonical timestamp range",
        ));
    }
    Ok((year, month, day))
}

fn transition_timestamp_v1(
    previous: &LoadedOwnerSyncIntentV1,
) -> Result<CanonicalTimestampV1, PortError> {
    Ok(std::cmp::max(
        canonical_timestamp_now_v1()?,
        previous.household_updated_at.clone(),
    ))
}

async fn ensure_native_owner_credentials_v1(
    ensure_session: &EnsureSession,
    session: &Mutex<SessionSnapshot>,
    cancellation: CancellationToken,
) -> Result<SessionCredentials, ()> {
    let snapshot = session.lock().await.clone();
    match ensure_session.execute(snapshot, cancellation).await {
        Ok(EnsureSessionOutcome::Current(credentials)) => Ok(credentials),
        Ok(EnsureSessionOutcome::Refreshed(credentials)) => {
            let mut current = session.lock().await;
            current.credentials = credentials.clone();
            current.reconciliation_required = false;
            Ok(credentials)
        }
        Ok(EnsureSessionOutcome::CancelledBeforeDispatch) | Err(_) => Err(()),
    }
}

fn declared_owner_profile_v1(
    input: &OnboardingProfileInput,
    profile_revision: ProfileRevision,
) -> Result<HouseholdProfileRecordV1, PortError> {
    input.profile_data().map_err(|_| {
        native_owner_error(
            "owner_profile_invalid",
            "the reviewed owner profile is invalid",
        )
    })?;
    let document = HouseholdProfileDocumentV1::native(HouseholdDeclaredProfileV1 {
        diet_style_ids: input.diet_style_ids.clone(),
        custom_diet_styles: input.custom_diet_styles.clone(),
        allergy_ids: input.allergy_ids.clone(),
        custom_restrictions: input.custom_restrictions.clone(),
        health_condition_ids: input.health_condition_ids.clone(),
        custom_health_conditions: input.custom_health_conditions.clone(),
        avoid_ingredients: input.avoid_ingredients.clone(),
        activity_level: input.activity_level.clone(),
        cuisine_preferences: input.cuisine_preferences.clone(),
        custom_cuisines: input.custom_cuisines.clone(),
        severity_level: input.severity_level,
        notes: input.notes.clone(),
    })
    .map_err(|_| {
        native_owner_error(
            "owner_profile_invalid",
            "the reviewed owner profile is invalid",
        )
    })?;
    Ok(HouseholdProfileRecordV1 {
        subject: HouseholdSubjectId::self_(),
        profile_revision,
        document,
    })
}

async fn save_native_owner_profile_v1(
    household: &HouseholdSession,
    input: &OnboardingProfileInput,
    cancellation: CancellationToken,
) -> Result<String, PortError> {
    // Validation and the complete local profile+intent commit precede every
    // credential refresh, consent read, profile read, and profile upload.
    input.profile_data().map_err(|_| {
        native_owner_error(
            "owner_profile_invalid",
            "the reviewed owner profile is invalid",
        )
    })?;
    let load = household.load_required(cancellation.clone()).await?;
    let current_profile = load
        .state
        .profiles
        .iter()
        .find(|profile| profile.subject == HouseholdSubjectId::self_());
    let expected_profile_revision = current_profile.map(|profile| profile.profile_revision);
    let profile_revision = expected_profile_revision
        .map_or_else(|| ProfileRevision::new(1), ProfileRevision::checked_next)
        .map_err(|_| {
            native_owner_error(
                "owner_profile_revision",
                "the owner profile revision cannot advance",
            )
        })?;
    let owner_profile = declared_owner_profile_v1(input, profile_revision)?;
    let effective_profile = owner_profile
        .document
        .effective_profile()
        .map_err(|_| {
            native_owner_error(
                "owner_profile_invalid",
                "the reviewed owner profile is invalid",
            )
        })?
        .ok_or_else(|| {
            native_owner_error(
                "owner_profile_invalid",
                "the reviewed owner profile is incomplete",
            )
        })?;
    let local_profile_digest = canonical_sha256_v1(&effective_profile).map_err(|_| {
        native_owner_error(
            "owner_profile_canonical",
            "the reviewed owner profile cannot be canonicalized",
        )
    })?;
    let resulting_household_revision = load.state.revision.checked_next().map_err(|_| {
        native_owner_error(
            "owner_profile_revision",
            "the household revision cannot advance",
        )
    })?;
    let replaced_intent = load
        .state
        .outbox
        .iter()
        .find_map(|record| match &record.entry {
            HouseholdProfileOutboxEntryV1::OwnerSync { .. } => Some(OwnerSyncIntentHandleV1 {
                outbox_id: record.outbox_id.clone(),
                expected_household_revision: load.state.revision,
                expected_profile_revision: current_profile?.profile_revision,
                expected_outbox_revision: record.outbox_revision,
            }),
            HouseholdProfileOutboxEntryV1::Legacy { .. } => None,
        });
    let frozen_commit_timestamp =
        std::cmp::max(canonical_timestamp_now_v1()?, load.state.updated_at.clone());
    let intent_id = CommitId::new().as_uuid();
    let owner_sync_intent = OwnerSyncIntentV1 {
        schema_version: 1,
        intent_id,
        intent_revision: 1,
        phase: OwnerSyncIntentPhaseV1::NeedsConsentCheck,
        subject: HouseholdSubjectId::self_(),
        local_household_revision: resulting_household_revision.get(),
        local_profile_revision: profile_revision.get(),
        local_profile_digest,
        remote_request_id: intent_id,
        consent_version: None,
        remote_base: None,
        expected_remote_version: None,
        request_method: None,
        request_path: None,
        request_body: None,
        request_body_digest: None,
        attempt_count: 0,
        last_definite_error: None,
        created_at: frozen_commit_timestamp.clone(),
        updated_at: frozen_commit_timestamp.clone(),
    };
    let saved = household
        .save_owner_profile_and_sync_intent(
            SaveOwnerProfileAndSyncIntentV1 {
                expected_household_revision: load.state.revision,
                expected_profile_revision,
                replaced_intent,
                owner_profile,
                owner_sync_intent,
                commit_id: CommitId::new(),
                frozen_commit_timestamp,
            },
            cancellation,
        )
        .await?;
    Ok(saved.handle.outbox_id.as_str().to_owned())
}

async fn load_owner_sync_intent_v1(
    household: &HouseholdSession,
    outbox_id: &str,
    cancellation: CancellationToken,
) -> Result<LoadedOwnerSyncIntentV1, PortError> {
    let load = household.load_required(cancellation).await?;
    let record = load
        .state
        .outbox
        .iter()
        .find(|record| record.outbox_id.as_str() == outbox_id)
        .ok_or_else(|| {
            native_owner_error(
                "owner_sync_intent_missing",
                "the exact owner sync intent is unavailable",
            )
        })?;
    let HouseholdProfileOutboxEntryV1::OwnerSync {
        version,
        target,
        intent,
    } = &record.entry
    else {
        return Err(native_owner_error(
            "owner_sync_intent_invalid",
            "the exact owner sync record is invalid",
        ));
    };
    if *version != 1
        || target != &HouseholdSubjectId::self_()
        || record.outbox_revision.get() != intent.intent_revision
    {
        return Err(native_owner_error(
            "owner_sync_intent_invalid",
            "the exact owner sync record is invalid",
        ));
    }
    let profile = load
        .state
        .profiles
        .iter()
        .find(|profile| profile.subject == HouseholdSubjectId::self_())
        .ok_or_else(|| {
            native_owner_error(
                "owner_sync_profile_missing",
                "the exact owner profile is unavailable",
            )
        })?;
    if profile.profile_revision.get() != intent.local_profile_revision {
        return Err(native_owner_error(
            "owner_sync_profile_changed",
            "the exact owner profile revision changed",
        ));
    }
    let effective_profile = profile
        .document
        .effective_profile()
        .map_err(|_| {
            native_owner_error(
                "owner_sync_profile_invalid",
                "the exact owner profile is invalid",
            )
        })?
        .ok_or_else(|| {
            native_owner_error(
                "owner_sync_profile_invalid",
                "the exact owner profile is incomplete",
            )
        })?;
    if canonical_sha256_v1(&effective_profile).map_err(|_| {
        native_owner_error(
            "owner_sync_profile_canonical",
            "the exact owner profile cannot be canonicalized",
        )
    })? != intent.local_profile_digest
    {
        return Err(native_owner_error(
            "owner_sync_profile_changed",
            "the exact owner profile digest changed",
        ));
    }
    Ok(LoadedOwnerSyncIntentV1 {
        handle: OwnerSyncIntentHandleV1 {
            outbox_id: record.outbox_id.clone(),
            expected_household_revision: load.state.revision,
            expected_profile_revision: profile.profile_revision,
            expected_outbox_revision: record.outbox_revision,
        },
        intent: intent.clone(),
        effective_profile,
        household_updated_at: load.state.updated_at,
    })
}

async fn load_exact_owner_sync_intent_v1(
    household: &HouseholdSession,
    expected: &OwnerSyncIntentHandleV1,
    cancellation: CancellationToken,
) -> Result<LoadedOwnerSyncIntentV1, PortError> {
    let loaded =
        load_owner_sync_intent_v1(household, expected.outbox_id.as_str(), cancellation).await?;
    if loaded.handle != *expected {
        return Err(native_owner_error(
            "owner_sync_revision_conflict",
            "the owner sync retry authority is stale",
        ));
    }
    Ok(loaded)
}

async fn retain_owner_sync_transition_v1(
    household: &HouseholdSession,
    loaded: LoadedOwnerSyncIntentV1,
    event: OwnerSyncTransitionEventV1,
    resulting_profile_state: HouseholdProfileStateV1,
    update: impl FnOnce(&mut OwnerSyncIntentV1),
) -> Result<LoadedOwnerSyncIntentV1, PortError> {
    let mut replacement = loaded.intent.clone();
    replacement.intent_revision = replacement.intent_revision.checked_add(1).ok_or_else(|| {
        native_owner_error(
            "owner_sync_revision_overflow",
            "the owner sync intent revision cannot advance",
        )
    })?;
    let frozen_commit_timestamp = transition_timestamp_v1(&loaded)?;
    replacement.updated_at = frozen_commit_timestamp.clone();
    update(&mut replacement);
    let outbox_id = loaded.handle.outbox_id.as_str().to_owned();
    household
        .transition_owner_sync_intent(
            TransitionOwnerSyncIntentV1 {
                handle: loaded.handle,
                event,
                replacement: Some(replacement),
                resulting_profile_state,
                commit_id: CommitId::new(),
                frozen_commit_timestamp,
            },
            // Once network evidence has been observed, action cancellation
            // cannot suppress its durable classification.
            CancellationToken::new(),
        )
        .await?;
    load_owner_sync_intent_v1(household, &outbox_id, CancellationToken::new()).await
}

async fn complete_owner_sync_transition_v1(
    household: &HouseholdSession,
    loaded: LoadedOwnerSyncIntentV1,
    event: OwnerSyncTransitionEventV1,
) -> Result<(), PortError> {
    let frozen_commit_timestamp = transition_timestamp_v1(&loaded)?;
    household
        .transition_owner_sync_intent(
            TransitionOwnerSyncIntentV1 {
                handle: loaded.handle,
                event,
                replacement: None,
                resulting_profile_state: HouseholdProfileStateV1::Synced,
                commit_id: CommitId::new(),
                frozen_commit_timestamp,
            },
            CancellationToken::new(),
        )
        .await?;
    Ok(())
}

fn parse_remote_owner_profile_v1(value: &Value) -> Result<RemoteOwnerProfileV1, PortError> {
    let object = value.as_object().ok_or_else(|| {
        native_owner_error(
            "owner_sync_remote_profile_invalid",
            "the authoritative owner profile response is malformed",
        )
    })?;
    if object.get("schema_version").and_then(Value::as_u64) != Some(1)
        || object.get("member_id").and_then(Value::as_str) != Some("_self")
        || !object.keys().all(|key| {
            matches!(
                key.as_str(),
                "schema_version" | "member_id" | "version" | "updated_at" | "profile_data"
            )
        })
    {
        return Err(native_owner_error(
            "owner_sync_remote_profile_invalid",
            "the authoritative owner profile response is malformed",
        ));
    }
    if let Some(updated_at) = object.get("updated_at")
        && updated_at
            .as_str()
            .and_then(|value| CanonicalTimestampV1::parse(value.to_owned()).ok())
            .is_none()
    {
        return Err(native_owner_error(
            "owner_sync_remote_profile_invalid",
            "the authoritative owner profile response is malformed",
        ));
    }
    let version = object
        .get("version")
        .and_then(Value::as_u64)
        .filter(|version| *version != 0)
        .ok_or_else(|| {
            native_owner_error(
                "owner_sync_remote_profile_invalid",
                "the authoritative owner profile response is malformed",
            )
        })?;
    let profile_data = object
        .get("profile_data")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            native_owner_error(
                "owner_sync_remote_profile_invalid",
                "the authoritative owner profile response is malformed",
            )
        })?;
    let profile_digest =
        canonical_sha256_v1(&Value::Object(profile_data.clone())).map_err(|_| {
            native_owner_error(
                "owner_sync_remote_profile_invalid",
                "the authoritative owner profile response is malformed",
            )
        })?;
    Ok(RemoteOwnerProfileV1::Present {
        version,
        profile_digest,
    })
}

async fn read_remote_owner_profile_v1(
    service: &HttpService,
    credentials: &SessionCredentials,
    cancellation: CancellationToken,
) -> Result<RemoteOwnerProfileV1, PortError> {
    match service
        .download_profile(credentials, "_self", OperationId::new(), cancellation)
        .await
    {
        Ok(value) => parse_remote_owner_profile_v1(&value),
        Err(error) if error.code == "resource_not_found" => Ok(RemoteOwnerProfileV1::Absent),
        Err(error) => Err(error),
    }
}

fn remote_base_v1(remote: RemoteOwnerProfileV1) -> RemoteProfileBaseV1 {
    match remote {
        RemoteOwnerProfileV1::Absent => RemoteProfileBaseV1 {
            existence: RemoteProfileExistenceV1::Absent,
            version: None,
            profile_digest: None,
        },
        RemoteOwnerProfileV1::Present {
            version,
            profile_digest,
        } => RemoteProfileBaseV1 {
            existence: RemoteProfileExistenceV1::Present,
            version: Some(version),
            profile_digest: Some(profile_digest),
        },
    }
}

fn frozen_owner_request_v1(
    loaded: &LoadedOwnerSyncIntentV1,
    remote_base: &RemoteProfileBaseV1,
) -> Result<CanonicalJsonObjectV1, PortError> {
    let mut request = Map::from_iter([
        ("member_id".to_owned(), Value::String("_self".to_owned())),
        ("profile_data".to_owned(), loaded.effective_profile.clone()),
    ]);
    if let Some(version) = remote_base.version {
        request.insert("expected_version".to_owned(), Value::from(version));
    }
    CanonicalJsonObjectV1::from_map(request, MAX_OWNER_SYNC_REQUEST_BODY_BYTES).map_err(|_| {
        native_owner_error(
            "owner_sync_request_invalid",
            "the frozen owner sync request cannot be canonicalized",
        )
    })
}

async fn reconcile_owner_sync_v1(
    household: &HouseholdSession,
    service: &HttpService,
    credentials: &SessionCredentials,
    outbox_id: &str,
    cancellation: &CancellationToken,
) -> Result<NativeOwnerSyncOutcomeV1, PortError> {
    let loaded = load_owner_sync_intent_v1(household, outbox_id, CancellationToken::new()).await?;
    if !matches!(
        loaded.intent.phase,
        OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown
            | OwnerSyncIntentPhaseV1::OutcomeUncertain
    ) {
        return Err(native_owner_error(
            "owner_sync_retry_action_mismatch",
            "the owner sync intent no longer requires reconciliation",
        ));
    }
    if cancellation.is_cancelled() {
        return Ok(NativeOwnerSyncOutcomeV1::Interrupted);
    }
    let consent = match service
        .profile_consent_authority_v1(credentials, OperationId::new(), cancellation.child_token())
        .await
    {
        Ok(consent) => consent,
        Err(_) => {
            retain_owner_sync_transition_v1(
                household,
                loaded,
                OwnerSyncTransitionEventV1::ReconciliationReadUnavailable,
                HouseholdProfileStateV1::PendingSync,
                |intent| {
                    intent.phase = OwnerSyncIntentPhaseV1::OutcomeUncertain;
                },
            )
            .await?;
            return Ok(NativeOwnerSyncOutcomeV1::Interrupted);
        }
    };
    let loaded = load_owner_sync_intent_v1(household, outbox_id, CancellationToken::new()).await?;
    if cancellation.is_cancelled() {
        return Ok(NativeOwnerSyncOutcomeV1::Interrupted);
    }
    let remote = match read_remote_owner_profile_v1(
        service,
        credentials,
        cancellation.child_token(),
    )
    .await
    {
        Ok(remote) => remote,
        Err(_) => {
            retain_owner_sync_transition_v1(
                household,
                loaded,
                OwnerSyncTransitionEventV1::ReconciliationReadUnavailable,
                HouseholdProfileStateV1::PendingSync,
                |intent| {
                    intent.phase = OwnerSyncIntentPhaseV1::OutcomeUncertain;
                },
            )
            .await?;
            return Ok(NativeOwnerSyncOutcomeV1::Interrupted);
        }
    };
    let intended_digest = matches!(
        remote,
        RemoteOwnerProfileV1::Present { profile_digest, .. }
            if profile_digest == loaded.intent.local_profile_digest
    );
    let exact_old_base = match (&loaded.intent.remote_base, remote) {
        (
            Some(RemoteProfileBaseV1 {
                existence: RemoteProfileExistenceV1::Absent,
                ..
            }),
            RemoteOwnerProfileV1::Absent,
        ) => true,
        (
            Some(RemoteProfileBaseV1 {
                existence: RemoteProfileExistenceV1::Present,
                version: Some(old_version),
                profile_digest: Some(old_digest),
            }),
            RemoteOwnerProfileV1::Present {
                version,
                profile_digest,
            },
        ) => *old_version == version && *old_digest == profile_digest,
        _ => false,
    };

    match (intended_digest, exact_old_base, consent) {
        (true, _, AuthoritativeConsentStateV1::Active(_)) => {
            complete_owner_sync_transition_v1(
                household,
                loaded,
                OwnerSyncTransitionEventV1::ReconciliationProvedApplied,
            )
            .await?;
            Ok(NativeOwnerSyncOutcomeV1::Synced)
        }
        (true, _, AuthoritativeConsentStateV1::Absent) => {
            retain_owner_sync_transition_v1(
                household,
                loaded,
                OwnerSyncTransitionEventV1::ConsentRevoked,
                HouseholdProfileStateV1::LocalOnly,
                |intent| {
                    intent.phase = OwnerSyncIntentPhaseV1::DefiniteFailure;
                    intent.last_definite_error =
                        Some(LastDefiniteOwnerSyncErrorV1::ConsentRevokedRegrantRequired);
                },
            )
            .await?;
            Ok(NativeOwnerSyncOutcomeV1::ConsentRevokedRegrantRequired)
        }
        (false, true, AuthoritativeConsentStateV1::Active(active_version))
            if Some(active_version) == loaded.intent.consent_version =>
        {
            retain_owner_sync_transition_v1(
                household,
                loaded,
                OwnerSyncTransitionEventV1::ReconciliationFoundOldBase,
                HouseholdProfileStateV1::PendingSync,
                |intent| {
                    intent.phase = OwnerSyncIntentPhaseV1::ReadyToDispatch;
                    intent.last_definite_error = None;
                },
            )
            .await?;
            Ok(NativeOwnerSyncOutcomeV1::Pending)
        }
        (false, true, AuthoritativeConsentStateV1::Active(_)) => {
            retain_owner_sync_transition_v1(
                household,
                loaded,
                OwnerSyncTransitionEventV1::ConsentVersionChangedAfterFreeze,
                HouseholdProfileStateV1::PendingSync,
                |intent| {
                    intent.phase = OwnerSyncIntentPhaseV1::DefiniteFailure;
                    intent.last_definite_error =
                        Some(LastDefiniteOwnerSyncErrorV1::ConsentVersionChangedRequiresNewSave);
                },
            )
            .await?;
            Ok(NativeOwnerSyncOutcomeV1::ConsentVersionChangedRequiresNewSave)
        }
        (false, true, AuthoritativeConsentStateV1::Absent) => {
            retain_owner_sync_transition_v1(
                household,
                loaded,
                OwnerSyncTransitionEventV1::ConsentRevoked,
                HouseholdProfileStateV1::LocalOnly,
                |intent| {
                    intent.phase = OwnerSyncIntentPhaseV1::DefiniteFailure;
                    intent.last_definite_error =
                        Some(LastDefiniteOwnerSyncErrorV1::ConsentRevokedRegrantRequired);
                },
            )
            .await?;
            Ok(NativeOwnerSyncOutcomeV1::ConsentRevokedRegrantRequired)
        }
        (_, _, AuthoritativeConsentStateV1::Malformed) => {
            retain_owner_sync_transition_v1(
                household,
                loaded,
                OwnerSyncTransitionEventV1::ReconciliationReadUnavailable,
                HouseholdProfileStateV1::PendingSync,
                |intent| {
                    intent.phase = OwnerSyncIntentPhaseV1::OutcomeUncertain;
                },
            )
            .await?;
            Ok(NativeOwnerSyncOutcomeV1::Interrupted)
        }
        _ => {
            retain_owner_sync_transition_v1(
                household,
                loaded,
                OwnerSyncTransitionEventV1::ReconciliationConflicted,
                HouseholdProfileStateV1::Conflicted,
                |intent| {
                    intent.phase = OwnerSyncIntentPhaseV1::Conflicted;
                    intent.last_definite_error =
                        Some(LastDefiniteOwnerSyncErrorV1::VersionConflict);
                },
            )
            .await?;
            Ok(NativeOwnerSyncOutcomeV1::Interrupted)
        }
    }
}

async fn continue_owner_sync_v1(
    household: &HouseholdSession,
    service: &HttpService,
    credentials: &SessionCredentials,
    outbox_id: &str,
    reconcile_uncertain: bool,
    cancellation: &CancellationToken,
) -> Result<NativeOwnerSyncOutcomeV1, PortError> {
    loop {
        let loaded =
            load_owner_sync_intent_v1(household, outbox_id, CancellationToken::new()).await?;
        match loaded.intent.phase {
            OwnerSyncIntentPhaseV1::NeedsConsentCheck => {
                if cancellation.is_cancelled() {
                    return Ok(NativeOwnerSyncOutcomeV1::Interrupted);
                }
                let consent = match service
                    .profile_consent_authority_v1(
                        credentials,
                        OperationId::new(),
                        cancellation.child_token(),
                    )
                    .await
                {
                    Ok(consent) => consent,
                    Err(_) => return Ok(NativeOwnerSyncOutcomeV1::Interrupted),
                };
                if cancellation.is_cancelled() {
                    return Ok(NativeOwnerSyncOutcomeV1::Interrupted);
                }
                match consent {
                    AuthoritativeConsentStateV1::Active(version) => {
                        retain_owner_sync_transition_v1(
                            household,
                            loaded,
                            OwnerSyncTransitionEventV1::ActiveConsentObserved,
                            HouseholdProfileStateV1::PendingSync,
                            |intent| {
                                intent.phase = OwnerSyncIntentPhaseV1::NeedsRemoteBase;
                                intent.consent_version = Some(version);
                            },
                        )
                        .await?;
                    }
                    AuthoritativeConsentStateV1::Absent => {
                        retain_owner_sync_transition_v1(
                            household,
                            loaded,
                            OwnerSyncTransitionEventV1::AuthoritativeConsentAbsent,
                            HouseholdProfileStateV1::LocalOnly,
                            |intent| {
                                intent.phase = OwnerSyncIntentPhaseV1::LocalOnlyNoConsent;
                                intent.consent_version = None;
                                intent.last_definite_error =
                                    Some(LastDefiniteOwnerSyncErrorV1::ConsentAbsent);
                            },
                        )
                        .await?;
                        return Ok(NativeOwnerSyncOutcomeV1::LocalOnlyNoConsent);
                    }
                    AuthoritativeConsentStateV1::Malformed => {
                        return Ok(NativeOwnerSyncOutcomeV1::Interrupted);
                    }
                }
            }
            OwnerSyncIntentPhaseV1::NeedsRemoteBase => {
                if cancellation.is_cancelled() {
                    return Ok(NativeOwnerSyncOutcomeV1::Interrupted);
                }
                let consent = match service
                    .profile_consent_authority_v1(
                        credentials,
                        OperationId::new(),
                        cancellation.child_token(),
                    )
                    .await
                {
                    Ok(consent) => consent,
                    Err(_) => return Ok(NativeOwnerSyncOutcomeV1::Interrupted),
                };
                if cancellation.is_cancelled() {
                    return Ok(NativeOwnerSyncOutcomeV1::Interrupted);
                }
                match consent {
                    AuthoritativeConsentStateV1::Active(version)
                        if Some(version) == loaded.intent.consent_version => {}
                    AuthoritativeConsentStateV1::Active(version) => {
                        retain_owner_sync_transition_v1(
                            household,
                            loaded,
                            OwnerSyncTransitionEventV1::ConsentVersionUpdatedBeforeBase,
                            HouseholdProfileStateV1::PendingSync,
                            |intent| {
                                intent.consent_version = Some(version);
                            },
                        )
                        .await?;
                    }
                    AuthoritativeConsentStateV1::Absent => {
                        retain_owner_sync_transition_v1(
                            household,
                            loaded,
                            OwnerSyncTransitionEventV1::AuthoritativeConsentAbsent,
                            HouseholdProfileStateV1::LocalOnly,
                            |intent| {
                                intent.phase = OwnerSyncIntentPhaseV1::LocalOnlyNoConsent;
                                intent.consent_version = None;
                                intent.last_definite_error =
                                    Some(LastDefiniteOwnerSyncErrorV1::ConsentAbsent);
                            },
                        )
                        .await?;
                        return Ok(NativeOwnerSyncOutcomeV1::LocalOnlyNoConsent);
                    }
                    AuthoritativeConsentStateV1::Malformed => {
                        return Ok(NativeOwnerSyncOutcomeV1::Interrupted);
                    }
                }

                // A fresh repository load separates the consent read from the
                // profile-base read and proves the exact intent is still live.
                let loaded =
                    load_owner_sync_intent_v1(household, outbox_id, CancellationToken::new())
                        .await?;
                if cancellation.is_cancelled() {
                    return Ok(NativeOwnerSyncOutcomeV1::Interrupted);
                }
                let remote = match read_remote_owner_profile_v1(
                    service,
                    credentials,
                    cancellation.child_token(),
                )
                .await
                {
                    Ok(remote) => remote,
                    Err(_) => return Ok(NativeOwnerSyncOutcomeV1::Interrupted),
                };
                if cancellation.is_cancelled() {
                    return Ok(NativeOwnerSyncOutcomeV1::Interrupted);
                }
                let remote_base = remote_base_v1(remote);
                let request_body = frozen_owner_request_v1(&loaded, &remote_base)?;
                let request_body_digest = request_body.canonical_sha256();
                let expected_remote_version = remote_base.version;
                retain_owner_sync_transition_v1(
                    household,
                    loaded,
                    OwnerSyncTransitionEventV1::RemoteBaseFrozen,
                    HouseholdProfileStateV1::PendingSync,
                    move |intent| {
                        intent.phase = OwnerSyncIntentPhaseV1::ReadyToDispatch;
                        intent.remote_base = Some(remote_base);
                        intent.expected_remote_version = expected_remote_version;
                        intent.request_method = Some("PUT".to_owned());
                        intent.request_path = Some("/v1/profile/sync".to_owned());
                        intent.request_body = Some(request_body);
                        intent.request_body_digest = Some(request_body_digest);
                    },
                )
                .await?;
            }
            OwnerSyncIntentPhaseV1::ReadyToDispatch => {
                if cancellation.is_cancelled() {
                    return Ok(NativeOwnerSyncOutcomeV1::Interrupted);
                }
                let consent = match service
                    .profile_consent_authority_v1(
                        credentials,
                        OperationId::new(),
                        cancellation.child_token(),
                    )
                    .await
                {
                    Ok(consent) => consent,
                    Err(_) => return Ok(NativeOwnerSyncOutcomeV1::Interrupted),
                };
                if cancellation.is_cancelled() {
                    return Ok(NativeOwnerSyncOutcomeV1::Interrupted);
                }
                match consent {
                    AuthoritativeConsentStateV1::Active(version)
                        if Some(version) == loaded.intent.consent_version =>
                    {
                        let next_attempt =
                            loaded.intent.attempt_count.checked_add(1).ok_or_else(|| {
                                native_owner_error(
                                    "owner_sync_attempt_count_overflow",
                                    "the owner sync attempt count cannot advance",
                                )
                            })?;
                        retain_owner_sync_transition_v1(
                            household,
                            loaded,
                            OwnerSyncTransitionEventV1::DispatchStarted,
                            HouseholdProfileStateV1::PendingSync,
                            move |intent| {
                                intent.phase = OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown;
                                intent.attempt_count = next_attempt;
                                intent.last_definite_error = None;
                            },
                        )
                        .await?;
                    }
                    AuthoritativeConsentStateV1::Active(_) => {
                        retain_owner_sync_transition_v1(
                            household,
                            loaded,
                            OwnerSyncTransitionEventV1::ConsentVersionChangedAfterFreeze,
                            HouseholdProfileStateV1::PendingSync,
                            |intent| {
                                intent.phase = OwnerSyncIntentPhaseV1::DefiniteFailure;
                                intent.last_definite_error = Some(
                                    LastDefiniteOwnerSyncErrorV1::ConsentVersionChangedRequiresNewSave,
                                );
                            },
                        )
                        .await?;
                        return Ok(NativeOwnerSyncOutcomeV1::ConsentVersionChangedRequiresNewSave);
                    }
                    AuthoritativeConsentStateV1::Absent => {
                        retain_owner_sync_transition_v1(
                            household,
                            loaded,
                            OwnerSyncTransitionEventV1::ConsentRevoked,
                            HouseholdProfileStateV1::LocalOnly,
                            |intent| {
                                intent.phase = OwnerSyncIntentPhaseV1::DefiniteFailure;
                                intent.last_definite_error = Some(
                                    LastDefiniteOwnerSyncErrorV1::ConsentRevokedRegrantRequired,
                                );
                            },
                        )
                        .await?;
                        return Ok(NativeOwnerSyncOutcomeV1::ConsentRevokedRegrantRequired);
                    }
                    AuthoritativeConsentStateV1::Malformed => {
                        return Ok(NativeOwnerSyncOutcomeV1::Interrupted);
                    }
                }

                let dispatching =
                    load_owner_sync_intent_v1(household, outbox_id, CancellationToken::new())
                        .await?;
                if dispatching.intent.phase != OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown {
                    return Err(native_owner_error(
                        "owner_sync_dispatch_state",
                        "the durable owner sync dispatch authority changed",
                    ));
                }
                let request_body = dispatching
                    .intent
                    .request_body
                    .as_ref()
                    .ok_or_else(|| {
                        native_owner_error(
                            "owner_sync_dispatch_state",
                            "the frozen owner sync request is unavailable",
                        )
                    })?
                    .canonical_bytes()
                    .map_err(|_| {
                        native_owner_error(
                            "owner_sync_dispatch_state",
                            "the frozen owner sync request is invalid",
                        )
                    })?;
                if canonical_sha256_v1(&Value::Object(
                    dispatching
                        .intent
                        .request_body
                        .as_ref()
                        .expect("checked above")
                        .as_map()
                        .clone(),
                ))
                .ok()
                    != dispatching.intent.request_body_digest
                {
                    return Err(native_owner_error(
                        "owner_sync_dispatch_state",
                        "the frozen owner sync request digest changed",
                    ));
                }
                let transport = service
                    .send_owner_profile_sync_v1(
                        credentials,
                        &request_body,
                        OperationId::from_uuid(dispatching.intent.remote_request_id),
                        cancellation.child_token(),
                    )
                    .await?;
                match classify_owner_sync_transport_v1(transport) {
                    OwnerSyncDispatchClassificationV1::CancelledBeforeSend => {
                        retain_owner_sync_transition_v1(
                            household,
                            dispatching,
                            OwnerSyncTransitionEventV1::PredispatchCancelled,
                            HouseholdProfileStateV1::PendingSync,
                            |intent| {
                                intent.phase = OwnerSyncIntentPhaseV1::ReadyToDispatch;
                                intent.last_definite_error =
                                    Some(LastDefiniteOwnerSyncErrorV1::PredispatchCancelled);
                            },
                        )
                        .await?;
                        return Ok(NativeOwnerSyncOutcomeV1::Interrupted);
                    }
                    OwnerSyncDispatchClassificationV1::DefiniteSuccess { .. } => {
                        complete_owner_sync_transition_v1(
                            household,
                            dispatching,
                            OwnerSyncTransitionEventV1::DefiniteRemoteSuccess,
                        )
                        .await?;
                        return Ok(NativeOwnerSyncOutcomeV1::Synced);
                    }
                    OwnerSyncDispatchClassificationV1::DefiniteFailure { error } => {
                        retain_owner_sync_transition_v1(
                            household,
                            dispatching,
                            OwnerSyncTransitionEventV1::DefiniteRemoteFailure,
                            HouseholdProfileStateV1::PendingSync,
                            |intent| {
                                intent.phase = OwnerSyncIntentPhaseV1::DefiniteFailure;
                                intent.last_definite_error = Some(error);
                            },
                        )
                        .await?;
                        return Ok(NativeOwnerSyncOutcomeV1::Pending);
                    }
                    OwnerSyncDispatchClassificationV1::VersionConflict => {
                        retain_owner_sync_transition_v1(
                            household,
                            dispatching,
                            OwnerSyncTransitionEventV1::VersionConflictObserved,
                            HouseholdProfileStateV1::PendingSync,
                            |intent| {
                                intent.phase = OwnerSyncIntentPhaseV1::OutcomeUncertain;
                                intent.last_definite_error =
                                    Some(LastDefiniteOwnerSyncErrorV1::VersionConflict);
                            },
                        )
                        .await?;
                        return Ok(NativeOwnerSyncOutcomeV1::Interrupted);
                    }
                    OwnerSyncDispatchClassificationV1::OutcomeUncertain => {
                        retain_owner_sync_transition_v1(
                            household,
                            dispatching,
                            OwnerSyncTransitionEventV1::DispatchOutcomeUncertain,
                            HouseholdProfileStateV1::PendingSync,
                            |intent| {
                                intent.phase = OwnerSyncIntentPhaseV1::OutcomeUncertain;
                                intent.last_definite_error = None;
                            },
                        )
                        .await?;
                        return Ok(NativeOwnerSyncOutcomeV1::Interrupted);
                    }
                }
            }
            OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown
            | OwnerSyncIntentPhaseV1::OutcomeUncertain
                if reconcile_uncertain =>
            {
                let outcome = reconcile_owner_sync_v1(
                    household,
                    service,
                    credentials,
                    outbox_id,
                    cancellation,
                )
                .await?;
                if outcome != NativeOwnerSyncOutcomeV1::Pending {
                    return Ok(outcome);
                }
            }
            OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown
            | OwnerSyncIntentPhaseV1::OutcomeUncertain => {
                return Ok(NativeOwnerSyncOutcomeV1::Interrupted);
            }
            OwnerSyncIntentPhaseV1::LocalOnlyNoConsent => {
                return Ok(NativeOwnerSyncOutcomeV1::LocalOnlyNoConsent);
            }
            OwnerSyncIntentPhaseV1::DefiniteFailure => {
                return Ok(match loaded.intent.last_definite_error {
                    Some(LastDefiniteOwnerSyncErrorV1::ConsentVersionChangedRequiresNewSave) => {
                        NativeOwnerSyncOutcomeV1::ConsentVersionChangedRequiresNewSave
                    }
                    Some(LastDefiniteOwnerSyncErrorV1::ConsentRevokedRegrantRequired) => {
                        NativeOwnerSyncOutcomeV1::ConsentRevokedRegrantRequired
                    }
                    _ => NativeOwnerSyncOutcomeV1::Pending,
                });
            }
            OwnerSyncIntentPhaseV1::Conflicted => {
                return Ok(NativeOwnerSyncOutcomeV1::Interrupted);
            }
        }
    }
}

async fn run_native_owner_onboarding_v1(
    profile: OnboardingProfileInput,
    household: HouseholdSession,
    service: Arc<HttpService>,
    ensure_session: Arc<EnsureSession>,
    session: Arc<Mutex<SessionSnapshot>>,
    authorization_scope: &str,
    cancellation: CancellationToken,
) -> Result<NativeOwnerProfileSaveStatusV1, OnboardingOperationError> {
    let outbox_id = save_native_owner_profile_v1(&household, &profile, cancellation.child_token())
        .await
        .map_err(|error| {
            if error.code.contains("cancelled") {
                OnboardingOperationError::Cancelled(RunTurnOutcome::CancelledBeforeServerAcceptance)
            } else {
                OnboardingOperationError::Failed(format!(
                    "{}: {}",
                    terminal_safe_text(error.code),
                    terminal_safe_text(&error.message)
                ))
            }
        })?;
    if cancellation.is_cancelled()
        || !["profile:read", "profile:write"]
            .iter()
            .all(|required| authorization_has_scope(authorization_scope, required))
    {
        return Ok(NativeOwnerProfileSaveStatusV1::SyncPending);
    }
    let Ok(credentials) =
        ensure_native_owner_credentials_v1(&ensure_session, &session, cancellation.child_token())
            .await
    else {
        return Ok(NativeOwnerProfileSaveStatusV1::SyncPending);
    };
    if credentials.account_id != *household.account() {
        return Ok(NativeOwnerProfileSaveStatusV1::SyncPending);
    }
    let outcome = continue_owner_sync_v1(
        &household,
        &service,
        &credentials,
        &outbox_id,
        false,
        &cancellation,
    )
    .await
    .unwrap_or(NativeOwnerSyncOutcomeV1::Interrupted);
    Ok(if outcome == NativeOwnerSyncOutcomeV1::LocalOnlyNoConsent {
        NativeOwnerProfileSaveStatusV1::SavedWithAbsentConsent
    } else {
        NativeOwnerProfileSaveStatusV1::SyncPending
    })
}

fn unavailable_native_owner_actions_v1(
    reason: OwnerProfileRetryUnavailableReasonV1,
) -> heyfood_application::OwnerProfileActionEligibilityV1 {
    heyfood_application::OwnerProfileActionEligibilityV1 {
        active_consent_version: None,
        retry: OwnerProfileRetryEligibilityV1::Unavailable { reason },
        intent: None,
    }
}

async fn load_native_owner_actions_v1(
    household: &HouseholdSession,
    service: &HttpService,
    ensure_session: &EnsureSession,
    session: &Mutex<SessionSnapshot>,
    authorization_scope: &str,
    cancellation: CancellationToken,
) -> heyfood_application::OwnerProfileActionEligibilityV1 {
    let Ok(load) = household.load_required(cancellation.child_token()).await else {
        return unavailable_native_owner_actions_v1(
            OwnerProfileRetryUnavailableReasonV1::ModeOrAccountIneligible,
        );
    };
    if !authorization_has_scope(authorization_scope, "profile:read") {
        return unavailable_native_owner_actions_v1(
            OwnerProfileRetryUnavailableReasonV1::ModeOrAccountIneligible,
        );
    }
    let Ok(credentials) =
        ensure_native_owner_credentials_v1(ensure_session, session, cancellation.child_token())
            .await
    else {
        return unavailable_native_owner_actions_v1(
            OwnerProfileRetryUnavailableReasonV1::ModeOrAccountIneligible,
        );
    };
    if credentials.account_id != *household.account() {
        return unavailable_native_owner_actions_v1(
            OwnerProfileRetryUnavailableReasonV1::ModeOrAccountIneligible,
        );
    }
    let consent = service
        .profile_consent_authority_v1(&credentials, OperationId::new(), cancellation.child_token())
        .await
        .unwrap_or(AuthoritativeConsentStateV1::Malformed);
    owner_profile_action_eligibility_v1(&load.state, consent)
}

async fn grant_native_owner_consent_v1(
    household: &HouseholdSession,
    service: &HttpService,
    ensure_session: &EnsureSession,
    session: &Mutex<SessionSnapshot>,
    authorization_scope: &str,
    cancellation: CancellationToken,
) -> Result<ProfileConsentFinishedV1, ProfileConsentFailureV1> {
    if !authorization_has_scope(authorization_scope, "profile:write") {
        return Err(ProfileConsentFailureV1::Unavailable);
    }
    let credentials =
        ensure_native_owner_credentials_v1(ensure_session, session, cancellation.child_token())
            .await
            .map_err(|_| {
                if cancellation.is_cancelled() {
                    ProfileConsentFailureV1::Cancelled
                } else {
                    ProfileConsentFailureV1::Unavailable
                }
            })?;
    if credentials.account_id != *household.account() {
        return Err(ProfileConsentFailureV1::Unavailable);
    }
    let consent_version = service
        .grant_owner_profile_consent_v1(
            &credentials,
            OperationId::new(),
            cancellation.child_token(),
        )
        .await
        .map_err(|error| {
            if error.outcome_uncertain {
                ProfileConsentFailureV1::Uncertain
            } else if error.code.contains("cancelled") {
                ProfileConsentFailureV1::Cancelled
            } else if error.code == "profile_consent_contract" {
                ProfileConsentFailureV1::MalformedResponse
            } else {
                ProfileConsentFailureV1::Unavailable
            }
        })?;
    // Consent success is presentation-only here. It neither changes the
    // outbox nor uploads a profile; a later explicit retry must authorize the
    // local-only -> needs-consent-check CAS.
    let retry_offered = household
        .load_required(CancellationToken::new())
        .await
        .ok()
        .map(|load| {
            owner_profile_action_eligibility_v1(
                &load.state,
                AuthoritativeConsentStateV1::Active(consent_version),
            )
            .retry
                == OwnerProfileRetryEligibilityV1::StartLocalOnlyAfterConsent
        })
        .unwrap_or(false);
    Ok(ProfileConsentFinishedV1 {
        consent_version,
        retry_offered,
    })
}

fn retry_action_matches_phase_v1(
    action: OwnerProfileRetryActionV1,
    phase: OwnerSyncIntentPhaseV1,
) -> bool {
    matches!(
        (action, phase),
        (
            OwnerProfileRetryActionV1::StartLocalOnlyAfterConsent,
            OwnerSyncIntentPhaseV1::LocalOnlyNoConsent
        ) | (
            OwnerProfileRetryActionV1::ResumeNeedsConsentCheck,
            OwnerSyncIntentPhaseV1::NeedsConsentCheck
        ) | (
            OwnerProfileRetryActionV1::ResumeNeedsRemoteBase,
            OwnerSyncIntentPhaseV1::NeedsRemoteBase
        ) | (
            OwnerProfileRetryActionV1::ResumeReadyToDispatch,
            OwnerSyncIntentPhaseV1::ReadyToDispatch
        ) | (
            OwnerProfileRetryActionV1::ReconcileDispatchingOutcomeUnknown,
            OwnerSyncIntentPhaseV1::DispatchingOutcomeUnknown
        ) | (
            OwnerProfileRetryActionV1::ReconcileOutcomeUncertain,
            OwnerSyncIntentPhaseV1::OutcomeUncertain
        )
    )
}

struct NativeOwnerRetryRequestV1 {
    action: OwnerProfileRetryActionV1,
    expected: OwnerSyncIntentHandleV1,
}

async fn retry_native_owner_sync_v1(
    household: &HouseholdSession,
    service: &HttpService,
    ensure_session: &EnsureSession,
    session: &Mutex<SessionSnapshot>,
    authorization_scope: &str,
    request: NativeOwnerRetryRequestV1,
    cancellation: CancellationToken,
) -> ProfileRetrySyncFinishedV1 {
    let NativeOwnerRetryRequestV1 { action, expected } = request;
    let loaded =
        match load_exact_owner_sync_intent_v1(household, &expected, CancellationToken::new()).await
        {
            Ok(loaded) => loaded,
            Err(_) => {
                return ProfileRetrySyncFinishedV1::Unavailable {
                    reason: OwnerProfileRetryUnavailableReasonV1::StaleRevision,
                };
            }
        };
    if !retry_action_matches_phase_v1(action, loaded.intent.phase)
        || !["profile:read", "profile:write"]
            .iter()
            .all(|required| authorization_has_scope(authorization_scope, required))
    {
        return ProfileRetrySyncFinishedV1::Unavailable {
            reason: OwnerProfileRetryUnavailableReasonV1::ModeOrAccountIneligible,
        };
    }
    let credentials = match ensure_native_owner_credentials_v1(
        ensure_session,
        session,
        cancellation.child_token(),
    )
    .await
    {
        Ok(credentials) => credentials,
        Err(()) => return ProfileRetrySyncFinishedV1::Interrupted,
    };
    if credentials.account_id != *household.account() {
        return ProfileRetrySyncFinishedV1::Unavailable {
            reason: OwnerProfileRetryUnavailableReasonV1::ModeOrAccountIneligible,
        };
    }
    if action == OwnerProfileRetryActionV1::StartLocalOnlyAfterConsent {
        if cancellation.is_cancelled() {
            return ProfileRetrySyncFinishedV1::Interrupted;
        }
        let consent = match service
            .profile_consent_authority_v1(
                &credentials,
                OperationId::new(),
                cancellation.child_token(),
            )
            .await
        {
            Ok(AuthoritativeConsentStateV1::Active(version)) => version,
            Ok(AuthoritativeConsentStateV1::Absent) => {
                return ProfileRetrySyncFinishedV1::Unavailable {
                    reason: OwnerProfileRetryUnavailableReasonV1::ConsentRequired,
                };
            }
            Ok(AuthoritativeConsentStateV1::Malformed) | Err(_) => {
                return ProfileRetrySyncFinishedV1::Interrupted;
            }
        };
        if cancellation.is_cancelled() {
            return ProfileRetrySyncFinishedV1::Interrupted;
        }
        let loaded =
            match load_exact_owner_sync_intent_v1(household, &expected, CancellationToken::new())
                .await
            {
                Ok(loaded) if loaded.intent.phase == OwnerSyncIntentPhaseV1::LocalOnlyNoConsent => {
                    loaded
                }
                _ => {
                    return ProfileRetrySyncFinishedV1::Unavailable {
                        reason: OwnerProfileRetryUnavailableReasonV1::StaleRevision,
                    };
                }
            };
        if retain_owner_sync_transition_v1(
            household,
            loaded,
            OwnerSyncTransitionEventV1::ExplicitRetryAfterConsent,
            HouseholdProfileStateV1::PendingSync,
            |intent| {
                intent.phase = OwnerSyncIntentPhaseV1::NeedsConsentCheck;
                intent.consent_version = None;
                intent.last_definite_error = None;
            },
        )
        .await
        .is_err()
        {
            return ProfileRetrySyncFinishedV1::Interrupted;
        }
        let _ = consent;
    }
    let reconcile_uncertain = matches!(
        action,
        OwnerProfileRetryActionV1::ReconcileDispatchingOutcomeUnknown
            | OwnerProfileRetryActionV1::ReconcileOutcomeUncertain
    );
    match continue_owner_sync_v1(
        household,
        service,
        &credentials,
        expected.outbox_id.as_str(),
        reconcile_uncertain,
        &cancellation,
    )
    .await
    {
        Ok(NativeOwnerSyncOutcomeV1::ConsentVersionChangedRequiresNewSave) => {
            ProfileRetrySyncFinishedV1::ConsentVersionChangedRequiresNewSave
        }
        Ok(NativeOwnerSyncOutcomeV1::ConsentRevokedRegrantRequired) => {
            ProfileRetrySyncFinishedV1::ConsentRevokedRegrantRequired
        }
        Ok(NativeOwnerSyncOutcomeV1::Interrupted) | Err(_) => {
            ProfileRetrySyncFinishedV1::Interrupted
        }
        Ok(
            NativeOwnerSyncOutcomeV1::Synced
            | NativeOwnerSyncOutcomeV1::Pending
            | NativeOwnerSyncOutcomeV1::LocalOnlyNoConsent,
        ) => ProfileRetrySyncFinishedV1::SyncPending,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum LogTargetMode {
    SelfSubject,
    Member,
    Everyone,
}

struct FrozenMember {
    raw: Map<String, Value>,
    id: String,
    name: String,
    archived: bool,
}

struct FrozenHouseholdSnapshot {
    members: Vec<FrozenMember>,
    active_scope: String,
    local_profiles: Option<Map<String, Value>>,
    profile_outbox: Option<Map<String, Value>>,
}

impl FrozenHouseholdSnapshot {
    fn self_only() -> Self {
        let raw = json!({
            "id": "_self",
            "name": "Me",
            "relationship": "self",
            "is_owner": true,
            "archived": false
        })
        .as_object()
        .expect("self fixture is an object")
        .clone();
        Self {
            members: vec![FrozenMember {
                raw,
                id: "_self".to_owned(),
                name: "Me".to_owned(),
                archived: false,
            }],
            active_scope: "_self".to_owned(),
            local_profiles: None,
            profile_outbox: None,
        }
    }

    fn active_members(&self) -> impl Iterator<Item = &FrozenMember> {
        self.members.iter().filter(|member| !member.archived)
    }

    fn member(&self, id: &str) -> Option<&FrozenMember> {
        self.members.iter().find(|member| member.id == id)
    }

    fn active_member(&self, id: &str) -> Option<&FrozenMember> {
        self.active_members().find(|member| member.id == id)
    }
}

struct CanonicalTargetDisplay {
    escaped_label: String,
    stable_id_token: String,
}

struct ResolvedLogTarget {
    raw_id: String,
    raw_label: String,
    display: CanonicalTargetDisplay,
    mode: LogTargetMode,
}

pub struct ReviewReady;

pub struct DispatchReady {
    request: TurnRequest,
}

/// A consuming log command whose reviewed target cannot be resolved again.
pub struct PreparedLogCommand<State> {
    meal: String,
    meal_type: Option<MealType>,
    prompt: String,
    target: ResolvedLogTarget,
    household_snapshot: FrozenHouseholdSnapshot,
    legacy_source_binding: PythonStatePreview,
    state: State,
    _not_clone: PhantomData<fn() -> State>,
}

/// Exact native household meal intent retained under a repository read lease
/// from local target resolution through human review.
pub struct PreparedNativeLogCommand {
    meal: String,
    meal_type: Option<MealType>,
    prompt: String,
    target: ResolvedLogTarget,
    authorized_context: AuthorizedHostedContextV1,
}

impl fmt::Debug for PreparedNativeLogCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedNativeLogCommand")
            .field("meal_present", &!self.meal.is_empty())
            .field("meal_type_present", &self.meal_type.is_some())
            .field("prompt_present", &!self.prompt.is_empty())
            .field("target_mode", &self.target.mode)
            .field(
                "household_revision",
                &self.authorized_context.snapshot().household_revision,
            )
            .finish_non_exhaustive()
    }
}

impl PreparedNativeLogCommand {
    #[must_use]
    pub fn review_document(&self) -> String {
        // The native request projection deliberately names the owner `Me` on
        // the wire. Review that exact attribution rather than substituting the
        // private account display name.
        log_review_document(&self.meal, self.meal_type, &self.target, "Me")
    }
}

/// Native meal request frozen after account binding and local profile
/// projection. The retained authorized context is deliberately carried until
/// the first hosted dispatch completes.
pub struct QualifiedNativeLogCommand {
    request: TurnRequest,
    authorized_context: AuthorizedHostedContextV1,
}

impl fmt::Debug for QualifiedNativeLogCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QualifiedNativeLogCommand")
            .field("prompt_present", &!self.request.prompt.is_empty())
            .field(
                "household_revision",
                &self.authorized_context.snapshot().household_revision,
            )
            .finish_non_exhaustive()
    }
}

impl<State> fmt::Debug for PreparedLogCommand<State> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedLogCommand")
            .field("meal_present", &!self.meal.is_empty())
            .field("meal_type_present", &self.meal_type.is_some())
            .field("prompt_present", &!self.prompt.is_empty())
            .field("target_mode", &self.target.mode)
            .field("member_count", &self.household_snapshot.members.len())
            .field(
                "active_member_count",
                &self.household_snapshot.active_members().count(),
            )
            .field(
                "source_set_digest",
                self.legacy_source_binding.checked_source_set().digest(),
            )
            .finish()
    }
}

impl PreparedLogCommand<ReviewReady> {
    #[must_use]
    pub fn source_preview(&self) -> &PythonStatePreview {
        &self.legacy_source_binding
    }

    #[must_use]
    pub fn review_document(&self) -> String {
        log_review_document(
            &self.meal,
            self.meal_type,
            &self.target,
            self.household_snapshot
                .active_member("_self")
                .map(|owner| owner.name.as_str())
                .unwrap_or("Me"),
        )
    }

    fn bind_account(
        mut self,
        credentials: &SessionCredentials,
        verified: VerifiedPythonState,
    ) -> Result<Self, OneShotError> {
        let verified_state = verified.state();
        match &self.legacy_source_binding {
            PythonStatePreview::NoSource { .. } => {
                if verified_state.is_some() {
                    return Err(OneShotError::new(
                        "python_state_changed",
                        "legacy household state appeared after review",
                    ));
                }
            }
            PythonStatePreview::SafeSnapshot { state, .. } => {
                let verified_state = verified_state.ok_or_else(|| {
                    OneShotError::new(
                        "python_state_changed",
                        "reviewed household snapshot is no longer available",
                    )
                })?;
                if verified_state != state {
                    return Err(OneShotError::new(
                        "python_state_changed",
                        "reviewed household snapshot changed before dispatch",
                    ));
                }
                require_account_binding(verified_state, credentials)?;
            }
            PythonStatePreview::ProtectedUninspectedMixedSource { .. } => {
                let verified_state = verified_state.ok_or_else(|| {
                    OneShotError::new(
                        "python_state_changed",
                        "protected household state is no longer available",
                    )
                })?;
                require_account_binding(verified_state, credentials)?;
                if verified_state.account_scoped.contains_key("household") {
                    let _ = strict_frozen_household(verified_state)?;
                }
                self.household_snapshot.local_profiles = verified_state
                    .account_scoped
                    .get("household_local_profiles")
                    .and_then(Value::as_object)
                    .cloned();
                self.household_snapshot.profile_outbox = verified_state
                    .account_scoped
                    .get("household_profile_outbox")
                    .and_then(Value::as_object)
                    .cloned();
            }
        }
        verified.commit_validated()?;
        Ok(self)
    }
}

fn log_review_document(
    meal: &str,
    meal_type: Option<MealType>,
    target: &ResolvedLogTarget,
    owner_label: &str,
) -> String {
    let attribution = if target.mode == LogTargetMode::Everyone {
        format!(
            "\nMeal write: one meal for owner {}",
            ascii_json_string(owner_label)
        )
    } else {
        String::new()
    };
    format!(
        "Mutation: log meal memory\nMeal: {}\nMeal type: {}\nHousehold target: {}{}",
        terminal_safe_text(meal),
        meal_type.map(MealType::as_str).unwrap_or("unspecified"),
        target.display.escaped_label,
        attribution,
    )
}

impl PreparedLogCommand<DispatchReady> {
    fn into_request(self) -> TurnRequest {
        self.state.request
    }
}

fn require_account_binding(
    state: &ImportedPythonState,
    credentials: &SessionCredentials,
) -> Result<(), OneShotError> {
    if state.account_user_id.as_deref() != Some(credentials.account_id.as_str()) {
        return Err(OneShotError::new(
            "python_state_account_mismatch",
            "imported Python state does not belong to the authenticated account",
        ));
    }
    Ok(())
}

/// Validate meal input and resolve the immutable household target before any
/// credential or network access.
pub fn prepare_log_command(
    arguments: LogArgs,
    stdin: &[u8],
    preview: PythonStatePreview,
) -> Result<PreparedLogCommand<ReviewReady>, OneShotError> {
    let (meal, prompt) = prepare_log_input(&arguments, stdin)?;

    let (household_snapshot, target) = match &preview {
        PythonStatePreview::SafeSnapshot { state, .. } => {
            let snapshot = strict_frozen_household(state)?;
            let target =
                resolve_log_target(&snapshot, arguments.checking_for.as_deref(), false, false)?;
            (snapshot, target)
        }
        PythonStatePreview::NoSource { .. } => {
            let snapshot = FrozenHouseholdSnapshot::self_only();
            let target =
                resolve_log_target(&snapshot, arguments.checking_for.as_deref(), true, false)?;
            (snapshot, target)
        }
        PythonStatePreview::ProtectedUninspectedMixedSource { reason, .. } => {
            let snapshot = FrozenHouseholdSnapshot::self_only();
            let target =
                resolve_log_target(&snapshot, arguments.checking_for.as_deref(), false, true)
                    .map_err(|mut error| {
                        if error.code == "household_state_protected" {
                            error.message = match reason {
                                ProtectedHouseholdReason::UninspectedMixedSource => {
                                    "Protected legacy Household data must be unlocked after login; use `--for self` only when that is the intended target."
                                }
                                ProtectedHouseholdReason::PriorImporterSkippedKeyring => {
                                    "Legacy keyring-backed Household data requires authenticated migration; use `--for self` only when that is the intended target."
                                }
                            }
                            .to_owned();
                        }
                        error
                    })?;
            (snapshot, target)
        }
    };
    Ok(PreparedLogCommand {
        meal,
        meal_type: arguments.meal_type,
        prompt,
        target,
        household_snapshot,
        legacy_source_binding: preview,
        state: ReviewReady,
        _not_clone: PhantomData,
    })
}

fn prepare_log_input(arguments: &LogArgs, stdin: &[u8]) -> Result<(String, String), OneShotError> {
    let meal = if arguments.meal.is_empty() {
        if stdin.is_empty() || stdin.len() > MAX_CONFIRMATION_STDIN_BYTES {
            return Err(OneShotError::new(
                "invalid_meal",
                "meal text or at most 1 MiB of UTF-8 stdin is required",
            ));
        }
        std::str::from_utf8(stdin)
            .map_err(|_| OneShotError::new("invalid_meal", "meal stdin is not UTF-8"))?
            .trim_end_matches(['\r', '\n'])
            .to_owned()
    } else {
        arguments.meal_text()
    };
    let meal = required_text(meal, 500, "meal")?;
    let mut prompt = format!("Log this meal: {meal}");
    if let Some(meal_type) = arguments.meal_type {
        prompt.push_str(". Meal type: ");
        prompt.push_str(meal_type.as_str());
        prompt.push('.');
    }
    let prompt = required_text(prompt, 500, "query")?;
    Ok((meal, prompt))
}

/// Bind the reviewed command to the authenticated account, perform read-only
/// enrichment for the frozen roster, and freeze the exact final request.
pub async fn prepare_qualified_log(
    service: &HttpService,
    credentials: &SessionCredentials,
    prepared: PreparedLogCommand<ReviewReady>,
    verified: VerifiedPythonState,
    cancellation: CancellationToken,
) -> Result<PreparedLogCommand<DispatchReady>, OneShotError> {
    let prepared = prepared.bind_account(credentials, verified)?;
    let context = build_household_turn_context_for_resolved_target(
        service,
        credentials,
        &prepared.household_snapshot,
        &prepared.target,
        cancellation,
    )
    .await?;
    validate_frozen_target_context(&context, &prepared.household_snapshot, &prepared.target)?;
    let request = TurnRequest {
        prompt: prepared.prompt.clone(),
        conversation_id: None,
        context,
        refresh: RefreshPolicy::Never,
    };
    Ok(PreparedLogCommand {
        meal: prepared.meal,
        meal_type: prepared.meal_type,
        prompt: prepared.prompt,
        target: prepared.target,
        household_snapshot: prepared.household_snapshot,
        legacy_source_binding: prepared.legacy_source_binding,
        state: DispatchReady { request },
        _not_clone: PhantomData,
    })
}

/// Consume and dispatch one already-qualified immutable log command.
pub async fn execute_qualified_prepared_log(
    service: &HttpService,
    credentials: SessionCredentials,
    output_mode: OutputMode,
    prepared: PreparedLogCommand<DispatchReady>,
    cancellation: CancellationToken,
) -> Result<String, OneShotError> {
    let private_household_ids = prepared
        .household_snapshot
        .members
        .iter()
        .map(|member| member.id.clone())
        .collect::<Vec<_>>();
    let result = execute_one_shot_turn(
        service,
        prepared.into_request(),
        credentials,
        OperationId::new(),
        cancellation,
    )
    .await
    .map_err(|error| {
        sanitize_household_log_error(error.into(), output_mode, &private_household_ids)
    })?;
    let private_household_ids = private_household_ids
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    Ok(render_household_log_result(
        &result,
        output_mode,
        &private_household_ids,
    ))
}

/// Resolve one native Household meal target, acquire its exact revision under
/// a read lease, and freeze the human review document. This function has no
/// service/provider parameter and performs no hosted work.
pub async fn prepare_native_log_command(
    arguments: LogArgs,
    stdin: &[u8],
    household: &HouseholdSession,
    cancellation: CancellationToken,
) -> Result<PreparedNativeLogCommand, OneShotError> {
    let (meal, prompt) = prepare_log_input(&arguments, stdin)?;
    if cancellation.is_cancelled() {
        return Err(OneShotError::new(
            "household_hosted_context_cancelled",
            "native Household target qualification was cancelled",
        ));
    }
    let load = household
        .load_required(cancellation.child_token())
        .await
        .map_err(OneShotError::from)?;
    let (scope, target) =
        resolve_native_log_target(&load.state, arguments.checking_for.as_deref())?;
    let authorized_context = household
        .acquire_authorized_hosted_context_for_scope(
            load.state.revision,
            scope,
            cancellation.child_token(),
        )
        .await
        .map_err(OneShotError::from)?;
    let context = native_household_turn_context(&authorized_context).map_err(OneShotError::from)?;
    validate_native_log_target_context(&context, &authorized_context, &target)?;
    if cancellation.is_cancelled() {
        return Err(OneShotError::new(
            "household_hosted_context_cancelled",
            "native Household target qualification was cancelled",
        ));
    }
    Ok(PreparedNativeLogCommand {
        meal,
        meal_type: arguments.meal_type,
        prompt,
        target,
        authorized_context,
    })
}

/// Bind the reviewed native command to the post-review authenticated account
/// and freeze the exact request without re-resolving its target.
pub fn prepare_qualified_native_log(
    credentials: &SessionCredentials,
    prepared: PreparedNativeLogCommand,
) -> Result<QualifiedNativeLogCommand, OneShotError> {
    if credentials.account_id.as_str()
        != prepared
            .authorized_context
            .load()
            .state
            .account_binding
            .as_str()
    {
        return Err(OneShotError::new(
            "household_account_mismatch",
            "reviewed native Household target belongs to another account",
        ));
    }
    let context =
        native_household_turn_context(&prepared.authorized_context).map_err(OneShotError::from)?;
    validate_native_log_target_context(&context, &prepared.authorized_context, &prepared.target)?;
    Ok(QualifiedNativeLogCommand {
        request: TurnRequest {
            prompt: prepared.prompt,
            conversation_id: None,
            context,
            refresh: RefreshPolicy::Never,
        },
        authorized_context: prepared.authorized_context,
    })
}

/// Dispatch one already-reviewed native meal intent while retaining its exact
/// Household revision through completion of the first hosted operation.
pub async fn execute_qualified_native_log(
    service: &HttpService,
    credentials: SessionCredentials,
    output_mode: OutputMode,
    prepared: QualifiedNativeLogCommand,
    cancellation: CancellationToken,
) -> Result<String, OneShotError> {
    let QualifiedNativeLogCommand {
        request,
        authorized_context,
    } = prepared;
    let private_household_ids = std::iter::once("_self".to_owned())
        .chain(
            authorized_context
                .load()
                .state
                .members
                .iter()
                .map(|member| member.member_id.as_str().to_owned()),
        )
        .collect::<Vec<_>>();
    let result = execute_one_shot_turn(
        service,
        request,
        credentials,
        OperationId::new(),
        cancellation,
    )
    .await
    .map_err(|error| {
        sanitize_household_log_error(error.into(), output_mode, &private_household_ids)
    })?;
    let private_household_ids = private_household_ids
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let rendered = render_household_log_result(&result, output_mode, &private_household_ids);
    drop(authorized_context);
    Ok(rendered)
}

fn sanitize_household_log_error(
    mut error: OneShotError,
    output_mode: OutputMode,
    _retained_private_household_ids: &[String],
) -> OneShotError {
    if output_mode != OutputMode::Json {
        error.message = HOUSEHOLD_LOG_HUMAN_ERROR_MESSAGE.to_owned();
    }
    error
}

fn render_household_log_result(
    result: &heyfood_application::OneShotTurnResult,
    output_mode: OutputMode,
    private_household_ids: &[&str],
) -> String {
    if output_mode != OutputMode::Json && result.partial_text_promoted {
        return format!("{UNRENDERABLE_AGENT_RESULT_MESSAGE}\n");
    }
    let retained_choice_values = result
        .streamed_choice_value_authorities
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    render_agent_result_with_private_authorities(
        &result.document,
        output_mode,
        private_household_ids,
        &retained_choice_values,
    )
}

/// Phase 2 executor over explicit, already-validated native state. The public
/// binary constructs this for the native command families it advertises.
pub struct OneShotExecutor<'a> {
    service: &'a HttpService,
    credentials: &'a SessionCredentials,
    output_mode: OutputMode,
    imported_state: Option<&'a ImportedPythonState>,
}

/// Refresh and durably reconcile the session before entering any authenticated
/// one-shot command. A refresh cancellation observed before dispatch never
/// reaches the command; accepted rotations are committed by `EnsureSession`
/// before this function constructs the executor.
pub async fn execute_qualified_one_shot(
    service: &HttpService,
    ensure_session: &EnsureSession,
    snapshot: heyfood_core::SessionSnapshot,
    output_mode: OutputMode,
    command: Command,
    stdin: &[u8],
    cancellation: CancellationToken,
) -> Result<String, OneShotError> {
    execute_qualified_one_shot_with_state(
        service,
        ensure_session,
        snapshot,
        output_mode,
        command,
        stdin,
        cancellation,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn execute_qualified_one_shot_with_state(
    service: &HttpService,
    ensure_session: &EnsureSession,
    snapshot: heyfood_core::SessionSnapshot,
    output_mode: OutputMode,
    command: Command,
    stdin: &[u8],
    cancellation: CancellationToken,
    imported_state: Option<&ImportedPythonState>,
) -> Result<String, OneShotError> {
    if matches!(command, Command::Log(_)) {
        return Err(OneShotError::new(
            "prepared_log_required",
            "meal logging requires an immutable reviewed command",
        ));
    }
    let credentials = match ensure_session
        .execute(snapshot, cancellation.child_token())
        .await
        .map_err(OneShotError::from)?
    {
        EnsureSessionOutcome::Current(credentials)
        | EnsureSessionOutcome::Refreshed(credentials) => credentials,
        EnsureSessionOutcome::CancelledBeforeDispatch => {
            return Err(OneShotError::new(
                "session_cancelled_before_dispatch",
                "session refresh was cancelled before dispatch",
            ));
        }
    };
    OneShotExecutor::new(service, &credentials, output_mode)
        .with_imported_state(imported_state)
        .execute(command, stdin, cancellation)
        .await
}

impl<'a> OneShotExecutor<'a> {
    #[must_use]
    pub const fn new(
        service: &'a HttpService,
        credentials: &'a SessionCredentials,
        output_mode: OutputMode,
    ) -> Self {
        Self {
            service,
            credentials,
            output_mode,
            imported_state: None,
        }
    }

    #[must_use]
    pub const fn with_imported_state(
        mut self,
        imported_state: Option<&'a ImportedPythonState>,
    ) -> Self {
        self.imported_state = imported_state;
        self
    }

    pub async fn execute(
        &self,
        command: Command,
        stdin: &[u8],
        cancellation: CancellationToken,
    ) -> Result<String, OneShotError> {
        match command {
            Command::Ask(arguments) => self.execute_agent(arguments, stdin, cancellation).await,
            Command::Reply(arguments) => {
                if arguments.conversation_id.is_none() {
                    return Err(OneShotError::new(
                        "conversation_required",
                        "native reply requires --conversation-id until conversation persistence is implemented",
                    ));
                }
                self.execute_agent(arguments, stdin, cancellation).await
            }
            Command::Log(_) => Err(OneShotError::new(
                "prepared_log_required",
                "meal logging requires an immutable reviewed command",
            )),
            Command::Item(arguments) => self.execute_item(arguments, cancellation).await,
            Command::Grocery { command } => {
                self.execute_grocery(command.unwrap_or(GroceryCommand::List), stdin, cancellation)
                    .await
            }
            Command::Health { command } => self.execute_health(command, cancellation).await,
            Command::Watch { command } => {
                self.execute_menu_watch(command.unwrap_or(MenuWatchCommand::List), cancellation)
                    .await
            }
            Command::Completion { shell } => {
                String::from_utf8(heyfood_cli::generate_completion(shell)).map_err(|_| {
                    OneShotError::new("completion_encoding", "completion output is invalid UTF-8")
                })
            }
            _ => Err(OneShotError::new(
                "phase2_parity_pending",
                "this command is present for topology parity but its Phase 2 use case is not yet qualified",
            )),
        }
    }

    async fn execute_agent(
        &self,
        arguments: AskArgs,
        stdin: &[u8],
        cancellation: CancellationToken,
    ) -> Result<String, OneShotError> {
        let prompt = if arguments.text.is_empty() {
            if stdin.is_empty() || stdin.len() > MAX_CONFIRMATION_STDIN_BYTES {
                return Err(OneShotError::new(
                    "invalid_prompt",
                    "prompt text or at most 1 MiB of UTF-8 stdin is required",
                ));
            }
            std::str::from_utf8(stdin)
                .map_err(|_| OneShotError::new("invalid_prompt", "prompt stdin is not UTF-8"))?
                .trim_end_matches(['\r', '\n'])
                .to_owned()
        } else {
            arguments.prompt()
        };
        let prompt = required_text(prompt, 500, "prompt")?;
        self.execute_prompt(
            prompt,
            arguments.conversation_id,
            TurnContext {
                latitude: arguments.latitude,
                longitude: arguments.longitude,
                ..TurnContext::default()
            },
            cancellation,
        )
        .await
    }

    async fn execute_item(
        &self,
        arguments: ItemArgs,
        cancellation: CancellationToken,
    ) -> Result<String, OneShotError> {
        let item_name = required_text(arguments.item_name(), 200, "item name")?;
        let mut restaurant = arguments
            .restaurant
            .map(|value| optional_text(Some(value), 200, "restaurant name"))
            .transpose()?
            .flatten();
        if let Some(selector) = arguments.at.as_deref()
            && selector.trim().bytes().all(|byte| byte.is_ascii_digit())
            && !selector.trim().is_empty()
        {
            restaurant = Some(self.restaurant_from_selector(selector)?);
        }
        let document = self
            .service
            .explain_item(
                &item_name,
                restaurant.as_deref(),
                OperationId::new(),
                cancellation,
            )
            .await?;
        Ok(render_item_result(&document, self.output_mode))
    }

    fn restaurant_from_selector(&self, selector: &str) -> Result<String, OneShotError> {
        let normalized = selector.trim();
        let index = normalized
            .parse::<usize>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| {
                OneShotError::new(
                    "restaurant_selector",
                    "restaurant selection is out of range",
                )
            })?;
        let state = self.bound_imported_state()?;
        let restaurants = state
            .account_scoped
            .get("last_restaurant_search")
            .and_then(|value| value.get("restaurants"))
            .and_then(Value::as_array)
            .ok_or_else(|| {
                OneShotError::new(
                    "restaurant_search_missing",
                    "no previous restaurant search was imported; run search before using --at",
                )
            })?;
        let restaurant = restaurants
            .get(index - 1)
            .and_then(Value::as_object)
            .ok_or_else(|| {
                OneShotError::new(
                    "restaurant_selector",
                    format!("restaurant selection {index} is out of range for the last search"),
                )
            })?;
        let name = restaurant
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                OneShotError::new(
                    "restaurant_selector",
                    "the selected restaurant does not contain a name",
                )
            })?;
        required_text(name.to_owned(), 200, "restaurant name")
    }

    fn bound_imported_state(&self) -> Result<&ImportedPythonState, OneShotError> {
        let state = self.imported_state.ok_or_else(|| {
            OneShotError::new(
                "python_state_required",
                "this selector requires account-bound state imported from the Python client",
            )
        })?;
        if state.account_user_id.as_deref() != Some(self.credentials.account_id.as_str()) {
            return Err(OneShotError::new(
                "python_state_account_mismatch",
                "imported Python state does not belong to the authenticated account",
            ));
        }
        Ok(state)
    }

    async fn execute_prompt(
        &self,
        prompt: String,
        conversation_id: Option<String>,
        context: TurnContext,
        cancellation: CancellationToken,
    ) -> Result<String, OneShotError> {
        let result = execute_one_shot_turn(
            self.service,
            TurnRequest {
                prompt,
                conversation_id,
                context,
                refresh: RefreshPolicy::Never,
            },
            self.credentials.clone(),
            OperationId::new(),
            cancellation,
        )
        .await
        .map_err(|error| {
            let mut error = OneShotError::from(error);
            if self.output_mode != OutputMode::Json && error.code == "agent_error" {
                error.message = AGENT_HUMAN_ERROR_MESSAGE.to_owned();
            }
            error
        })?;
        let retained_choice_values = result
            .streamed_choice_value_authorities
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        Ok(render_agent_result_with_private_authorities(
            &result.document,
            self.output_mode,
            &[],
            &retained_choice_values,
        ))
    }

    async fn execute_grocery(
        &self,
        command: GroceryCommand,
        stdin: &[u8],
        cancellation: CancellationToken,
    ) -> Result<String, OneShotError> {
        let capabilities = DiscoverCapabilities::new(self.service)
            .execute(cancellation.child_token())
            .await?;
        match command {
            GroceryCommand::List => {
                let list = ReadActiveGroceryDisplay::new(self.service)
                    .execute(
                        capabilities,
                        self.credentials.clone(),
                        OperationId::new(),
                        cancellation,
                    )
                    .await?;
                Ok(render_grocery_list(&list, self.output_mode))
            }
            GroceryCommand::Add(arguments) => {
                if arguments.items.len() > 25 {
                    return Err(OneShotError::new(
                        "grocery_item_count",
                        "a Grocery add request may contain at most 25 items",
                    ));
                }
                let request = AddItemsRequestWire {
                    list_id: parse_list_id(&arguments.list.list_id)?,
                    expected_version: parse_list_version(arguments.list.version)?,
                    items: arguments
                        .items
                        .into_iter()
                        .map(|name| {
                            let name = bounded_text(name, 255, "grocery item name")?;
                            Ok(GroceryItemInputWire {
                                name,
                                quantity: None,
                                unit: None,
                                package_quantity: None,
                                note: None,
                                intended_for: arguments.intended_for.clone(),
                                source_type: "manual".into(),
                                source_ref: None,
                                source_detail: None,
                            })
                        })
                        .collect::<Result<_, OneShotError>>()?,
                };
                let proposal = PrepareGroceryMutation::new(self.service)
                    .execute(
                        capabilities,
                        self.credentials.clone(),
                        OperationId::new(),
                        DeployedGroceryMutationRequest::Add(request),
                        cancellation,
                    )
                    .await?;
                Ok(render_grocery_proposal(&proposal, self.output_mode))
            }
            GroceryCommand::Remove(arguments) => {
                let (list_id, version, item_ids) = self
                    .resolve_references(
                        &capabilities,
                        &arguments.list.list_id,
                        arguments.list.version,
                        &arguments.items,
                        cancellation.child_token(),
                    )
                    .await?;
                let proposal = PrepareGroceryMutation::new(self.service)
                    .execute(
                        capabilities,
                        self.credentials.clone(),
                        OperationId::new(),
                        DeployedGroceryMutationRequest::Remove(RemoveItemsRequestWire {
                            list_id,
                            expected_version: version,
                            item_ids,
                        }),
                        cancellation,
                    )
                    .await?;
                Ok(render_grocery_proposal(&proposal, self.output_mode))
            }
            GroceryCommand::State(arguments) => {
                let (list_id, version, item_ids) = self
                    .resolve_references(
                        &capabilities,
                        &arguments.list.list_id,
                        arguments.list.version,
                        std::slice::from_ref(&arguments.item),
                        cancellation.child_token(),
                    )
                    .await?;
                let proposal = PrepareGroceryMutation::new(self.service)
                    .execute(
                        capabilities,
                        self.credentials.clone(),
                        OperationId::new(),
                        DeployedGroceryMutationRequest::UpdateState(UpdateItemStateRequestWire {
                            list_id,
                            expected_version: version,
                            item_id: item_ids.into_iter().next().ok_or_else(|| {
                                OneShotError::new("grocery_item_reference", "item is required")
                            })?,
                            state: arguments.state.into(),
                        }),
                        cancellation,
                    )
                    .await?;
                Ok(render_grocery_proposal(&proposal, self.output_mode))
            }
            GroceryCommand::Exclusions => {
                let exclusions = ReadGroceryExclusions::new(self.service)
                    .execute(
                        capabilities,
                        self.credentials.clone(),
                        OperationId::new(),
                        cancellation,
                    )
                    .await?;
                Ok(render_grocery_exclusions(&exclusions, self.output_mode))
            }
            GroceryCommand::Never(arguments) => {
                let request = ExclusionMutationRequestWire {
                    name: bounded_text(arguments.item, 255, "grocery exclusion")?,
                    list_id: parse_list_id(&arguments.list.list_id)?,
                    expected_version: parse_list_version(arguments.list.version)?,
                };
                let request = if arguments.remove {
                    DeployedGroceryMutationRequest::RemoveExclusion(request)
                } else {
                    DeployedGroceryMutationRequest::AddExclusion(request)
                };
                let proposal = PrepareGroceryMutation::new(self.service)
                    .execute(
                        capabilities,
                        self.credentials.clone(),
                        OperationId::new(),
                        request,
                        cancellation,
                    )
                    .await?;
                Ok(render_grocery_proposal(&proposal, self.output_mode))
            }
            GroceryCommand::Export(arguments) => {
                if arguments.out.is_none() && self.output_mode != OutputMode::Json {
                    return Err(OneShotError::new(
                        "grocery_export_requires_out",
                        GROCERY_EXPORT_REQUIRES_PROTECTED_FILE_MESSAGE,
                    ));
                }
                let export = ExportGroceryList::new(self.service)
                    .execute(
                        capabilities,
                        self.credentials.clone(),
                        OperationId::new(),
                        parse_list_id(&arguments.list_id)?,
                        arguments.format.as_wire_value().to_owned(),
                        cancellation,
                    )
                    .await?;
                if let Some(path) = arguments.out.as_deref() {
                    let bytes = grocery_export_bytes(&export)?;
                    SensitiveExportWriter::write(path, &bytes, arguments.overwrite)?;
                    if self.output_mode == OutputMode::Json {
                        return render_json(&json!({
                            "written": true,
                            "format": arguments.format.as_wire_value(),
                            "bytes": bytes.len()
                        }))
                        .map_err(|_| {
                            OneShotError::new("output_json", "could not encode export receipt")
                        });
                    }
                    return Ok(format!(
                        "Grocery export written to {}.\n",
                        terminal_safe_text(&path.display().to_string())
                    ));
                }
                render_grocery_export_stdout(export, self.output_mode)
            }
            GroceryCommand::Confirm(arguments) => {
                if !arguments.proposal_stdin {
                    return Err(OneShotError::new(
                        "confirmation_input",
                        "confirmation proposals must be read from stdin",
                    ));
                }
                if stdin.is_empty() || stdin.len() > MAX_CONFIRMATION_STDIN_BYTES {
                    return Err(OneShotError::new(
                        "confirmation_input",
                        "confirmation proposal stdin must contain at most 1 MiB",
                    ));
                }
                let proposal: heyfood_core::GroceryMutationProposalWire =
                    serde_json::from_slice(stdin).map_err(|_| {
                        OneShotError::new(
                            "confirmation_input",
                            "confirmation proposal stdin is invalid JSON",
                        )
                    })?;
                let result = ConfirmGroceryMutation::new(self.service)
                    .execute(
                        capabilities,
                        self.credentials.clone(),
                        OperationId::new(),
                        GroceryMutationConfirmRequestWire {
                            confirmation_token: GroceryConfirmationToken::parse(
                                proposal
                                    .confirmation_token
                                    .expose_at_transport_boundary()
                                    .to_owned(),
                            )
                            .map_err(|message| OneShotError::new("confirmation_input", message))?,
                            decision: GroceryDecisionWire::from(arguments.decision),
                        },
                        cancellation,
                    )
                    .await?;
                Ok(render_grocery_mutation_result(&result, self.output_mode))
            }
        }
    }

    async fn execute_health(
        &self,
        command: HealthCommand,
        cancellation: CancellationToken,
    ) -> Result<String, OneShotError> {
        match command {
            HealthCommand::Status => {
                let integrations = self
                    .service
                    .health_integrations(self.credentials, OperationId::new(), cancellation)
                    .await?;
                if self.output_mode == OutputMode::Json {
                    return render_json(&integrations).map_err(|_| {
                        OneShotError::new("output_json", "could not encode integration status")
                    });
                }
                let mut output = String::new();
                if integrations.integrations.is_empty() {
                    output.push_str("No health integrations connected.\n");
                }
                for integration in integrations.integrations {
                    let provider = serde_json::to_value(integration.provider)
                        .ok()
                        .and_then(|value| value.as_str().map(str::to_owned))
                        .unwrap_or_else(|| "provider".into());
                    let status = serde_json::to_value(integration.status)
                        .ok()
                        .and_then(|value| value.as_str().map(str::to_owned))
                        .unwrap_or_else(|| "unknown".into());
                    output.push_str(&format!("{provider}: {status}\n"));
                }
                Ok(output)
            }
            HealthCommand::Show => {
                let context = self
                    .service
                    .health_context(self.credentials, OperationId::new(), cancellation)
                    .await?;
                Ok(render_health_context(&context, self.output_mode))
            }
            HealthCommand::Connect(arguments) => {
                ensure_oura(arguments.provider)?;
                let authorization = self
                    .service
                    .health_authorize_oura(self.credentials, OperationId::new(), cancellation)
                    .await?;
                if self.output_mode == OutputMode::Json {
                    render_json(&authorization).map_err(|_| {
                        OneShotError::new("output_json", "could not encode authorization")
                    })
                } else {
                    Ok(format!(
                        "Open this authorization URL in your browser:\n{}\n",
                        terminal_safe_text(&authorization.auth_url)
                    ))
                }
            }
            HealthCommand::Sync(arguments) => {
                ensure_oura(arguments.provider)?;
                let result = self
                    .service
                    .health_sync_oura(self.credentials, OperationId::new(), cancellation)
                    .await?;
                render_json(&result)
                    .map_err(|_| OneShotError::new("output_json", "could not encode sync result"))
            }
            HealthCommand::Disconnect(arguments) => {
                ensure_oura(arguments.provider.provider)?;
                if !arguments.yes {
                    return Err(OneShotError::new(
                        "confirmation_required",
                        "health disconnect requires --yes",
                    ));
                }
                let result = self
                    .service
                    .health_disconnect_oura(self.credentials, OperationId::new(), cancellation)
                    .await?;
                render_json(&result).map_err(|_| {
                    OneShotError::new("output_json", "could not encode disconnect result")
                })
            }
        }
    }

    async fn execute_menu_watch(
        &self,
        command: MenuWatchCommand,
        cancellation: CancellationToken,
    ) -> Result<String, OneShotError> {
        match command {
            MenuWatchCommand::List => {
                let watches = ListMenuWatches::new(self.service)
                    .execute(self.credentials.clone(), OperationId::new(), cancellation)
                    .await?;
                Ok(render_menu_watch_list(&watches, self.output_mode))
            }
            MenuWatchCommand::Add(arguments) => {
                let restaurant_id = RestaurantId::parse(&arguments.restaurant_id)
                    .map_err(|message| OneShotError::new("restaurant_id", message))?;
                let menu_url = optional_text(arguments.menu_url, 2_048, "menu URL")?;
                let timezone = optional_text(arguments.tz, 64, "IANA timezone")?;
                let weekday = WatchWeekday::new(arguments.weekday.as_contract_value())
                    .map_err(|message| OneShotError::new("menu_watch_weekday", message))?;
                let hour = WatchHour::new(arguments.hour)
                    .map_err(|message| OneShotError::new("menu_watch_hour", message))?;
                let watch = CreateMenuWatch::new(self.service)
                    .execute(
                        self.credentials.clone(),
                        OperationId::new(),
                        CreateMenuWatchRequest {
                            restaurant_id,
                            cadence: WatchCadenceWire { weekday, hour },
                            notify: arguments.notify,
                            menu_url,
                            confirm_menu_url: arguments.confirm_menu_url,
                            tz: timezone,
                        },
                        cancellation,
                    )
                    .await?;
                Ok(render_menu_watch(&watch, self.output_mode))
            }
            MenuWatchCommand::Remove(arguments) => {
                let watch_id = MenuWatchId::parse(&arguments.watch_id)
                    .map_err(|message| OneShotError::new("menu_watch_id", message))?;
                RemoveMenuWatch::new(self.service)
                    .execute(
                        self.credentials.clone(),
                        OperationId::new(),
                        watch_id,
                        cancellation,
                    )
                    .await?;
                if self.output_mode == OutputMode::Json {
                    render_json(&json!({
                        "deleted": true,
                        "watch_id": watch_id.as_uuid().hyphenated().to_string()
                    }))
                    .map_err(|_| {
                        OneShotError::new("output_json", "could not encode Menu Watch result")
                    })
                } else {
                    Ok(format!(
                        "Removed Menu Watch {}.\n",
                        watch_id.as_uuid().hyphenated()
                    ))
                }
            }
        }
    }

    async fn resolve_references(
        &self,
        capabilities: &CapabilitySnapshot,
        requested_list_id: &str,
        requested_version: u64,
        references: &[String],
        cancellation: CancellationToken,
    ) -> Result<(GroceryEntityId, GroceryListVersion, Vec<String>), OneShotError> {
        let list_id = parse_list_id(requested_list_id)?;
        let version = parse_list_version(requested_version)?;
        let list = ReadActiveGroceryDisplay::new(self.service)
            .execute(
                capabilities.clone(),
                self.credentials.clone(),
                OperationId::new(),
                cancellation,
            )
            .await?;
        if list.id != list_id.as_uuid().hyphenated().to_string() || list.version != version.get() {
            return Err(OneShotError::new(
                "version_conflict",
                "the active Grocery list identity or version changed; fetch it again",
            ));
        }
        let item_ids = references
            .iter()
            .map(|reference| {
                if let Some(index) = reference.strip_prefix('#') {
                    let index = index.parse::<usize>().map_err(|_| {
                        OneShotError::new(
                            "grocery_item_reference",
                            "Grocery item index must be written as #N",
                        )
                    })?;
                    if index == 0 {
                        return Err(OneShotError::new(
                            "grocery_item_reference",
                            "Grocery item indexes are one-based",
                        ));
                    }
                    list.items
                        .get(index - 1)
                        .map(|item| item.id.clone())
                        .ok_or_else(|| {
                            OneShotError::new(
                                "grocery_item_reference",
                                "Grocery item index is outside the current list",
                            )
                        })
                } else {
                    bounded_text(reference.clone(), 255, "grocery item ID")
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((list_id, version, item_ids))
    }
}

fn grocery_export_bytes(export: &GroceryExport) -> Result<Vec<u8>, OneShotError> {
    match export {
        GroceryExport::Json(list) => {
            let mut bytes = serde_json::to_vec(list)
                .map_err(|_| OneShotError::new("output_json", "could not encode Grocery export"))?;
            bytes.push(b'\n');
            Ok(bytes)
        }
        GroceryExport::Markdown(text) | GroceryExport::Text(text) => Ok(text.as_bytes().to_vec()),
    }
}

fn render_grocery_export_stdout(
    export: GroceryExport,
    output_mode: OutputMode,
) -> Result<String, OneShotError> {
    if output_mode != OutputMode::Json {
        return Err(OneShotError::new(
            "grocery_export_requires_out",
            GROCERY_EXPORT_REQUIRES_PROTECTED_FILE_MESSAGE,
        ));
    }
    match export {
        GroceryExport::Json(list) => render_json(&list)
            .map_err(|_| OneShotError::new("output_json", "could not encode Grocery export")),
        GroceryExport::Markdown(content) => render_json(&json!({
            "format": "markdown",
            "content": content
        }))
        .map_err(|_| OneShotError::new("output_json", "could not encode Grocery export")),
        GroceryExport::Text(content) => render_json(&json!({
            "format": "text",
            "content": content
        }))
        .map_err(|_| OneShotError::new("output_json", "could not encode Grocery export")),
    }
}

impl OneShotError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            outcome_uncertain: false,
        }
    }
}

fn parse_list_id(value: &str) -> Result<GroceryEntityId, OneShotError> {
    GroceryEntityId::parse(value).map_err(|message| OneShotError::new("grocery_list_id", message))
}

fn parse_list_version(value: u64) -> Result<GroceryListVersion, OneShotError> {
    GroceryListVersion::new(value)
        .map_err(|message| OneShotError::new("grocery_list_version", message))
}

fn bounded_text(
    value: String,
    maximum: usize,
    label: &'static str,
) -> Result<String, OneShotError> {
    if value.trim() != value || value.is_empty() || value.len() > maximum {
        return Err(OneShotError::new(
            "invalid_argument",
            format!("{label} is invalid"),
        ));
    }
    let value = terminal_safe_text(&value);
    if value.is_empty() {
        return Err(OneShotError::new(
            "invalid_argument",
            format!("{label} is invalid"),
        ));
    }
    Ok(value)
}

fn required_text(
    value: String,
    maximum_characters: usize,
    label: &'static str,
) -> Result<String, OneShotError> {
    heyfood_core::required_text(&value, maximum_characters).map_err(|_| {
        OneShotError::new(
            "invalid_argument",
            format!("{label} must contain 1 to {maximum_characters} characters"),
        )
    })
}

fn optional_text(
    value: Option<String>,
    maximum_characters: usize,
    label: &'static str,
) -> Result<Option<String>, OneShotError> {
    heyfood_core::optional_text(value.as_deref(), maximum_characters).map_err(|_| {
        OneShotError::new(
            "invalid_argument",
            format!("{label} must contain at most {maximum_characters} characters"),
        )
    })
}

fn strict_frozen_household(
    state: &ImportedPythonState,
) -> Result<FrozenHouseholdSnapshot, OneShotError> {
    let household = state
        .account_scoped
        .get("household")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            OneShotError::new(
                "household_state_invalid",
                "saved Household identity is incomplete; repair it before logging",
            )
        })?;
    let rows = household
        .get("members")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            OneShotError::new(
                "household_state_invalid",
                "saved Household roster is invalid; repair it before logging",
            )
        })?;
    let active_scope = household
        .get("active_scope")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            OneShotError::new(
                "household_active_scope_invalid",
                "saved Household scope is invalid; choose it again before logging",
            )
        })?;
    if active_scope.is_empty() {
        return Err(OneShotError::new(
            "household_active_scope_invalid",
            "saved Household scope is invalid; choose it again before logging",
        ));
    }

    let mut members = Vec::with_capacity(rows.len());
    let mut identifiers = BTreeSet::new();
    let mut displays = BTreeSet::new();
    let mut tokens = BTreeSet::new();
    let mut self_seen = false;
    for row in rows {
        let mut raw = row.as_object().cloned().ok_or_else(|| {
            OneShotError::new(
                "household_state_invalid",
                "saved Household roster contains an invalid row",
            )
        })?;
        let id = raw
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                OneShotError::new(
                    "household_state_invalid",
                    "saved Household roster contains an invalid member identity",
                )
            })?
            .to_owned();
        validate_stored_member_id(&id)?;
        let name = raw
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                OneShotError::new(
                    "household_state_invalid",
                    "saved Household roster contains an invalid display name",
                )
            })?
            .to_owned();
        validate_stored_member_name(&name)?;
        let archived = match raw.get("archived") {
            None => false,
            Some(Value::Bool(value)) => *value,
            Some(_) => {
                return Err(OneShotError::new(
                    "household_state_invalid",
                    "saved Household roster contains an invalid archive marker",
                ));
            }
        };
        if !identifiers.insert(id.clone()) {
            return Err(OneShotError::new(
                "household_state_invalid",
                "saved Household roster contains duplicate member identities",
            ));
        }
        if id == "_self" {
            if self_seen || archived {
                return Err(OneShotError::new(
                    "household_state_invalid",
                    "saved Household owner identity is invalid",
                ));
            }
            self_seen = true;
        }
        let mode = if id == "_self" {
            LogTargetMode::SelfSubject
        } else {
            LogTargetMode::Member
        };
        let display = canonical_display(mode, &name, &id);
        if !tokens.insert(display.stable_id_token.clone())
            || !displays.insert((
                mode,
                display.escaped_label.clone(),
                display.stable_id_token.clone(),
            ))
        {
            return Err(OneShotError::new(
                "household_identity_display_collision",
                "saved Household identities cannot be rendered uniquely",
            ));
        }
        raw.insert("id".into(), Value::String(id.clone()));
        raw.insert("name".into(), Value::String(name.clone()));
        raw.insert("archived".into(), Value::Bool(archived));
        raw.insert("is_owner".into(), Value::Bool(id == "_self"));
        if id == "_self" {
            raw.insert("relationship".into(), Value::String("self".into()));
        }
        members.push(FrozenMember {
            raw,
            id,
            name,
            archived,
        });
    }

    let active_non_self = members
        .iter()
        .filter(|member| !member.archived && member.id != "_self")
        .count();
    match active_scope {
        "_self" => {}
        "__everyone__" if active_non_self > 0 => {}
        "__everyone__" => {
            return Err(OneShotError::new(
                "household_active_scope_invalid",
                "saved Everyone scope has no active Household member",
            ));
        }
        scope => match members.iter().find(|member| member.id == scope) {
            Some(member) if !member.archived && member.id != "_self" => {}
            _ => {
                return Err(OneShotError::new(
                    "household_active_scope_invalid",
                    "saved Household scope is missing, unknown, or archived",
                ));
            }
        },
    }

    Ok(FrozenHouseholdSnapshot {
        members,
        active_scope: active_scope.to_owned(),
        local_profiles: state
            .account_scoped
            .get("household_local_profiles")
            .and_then(Value::as_object)
            .cloned(),
        profile_outbox: state
            .account_scoped
            .get("household_profile_outbox")
            .and_then(Value::as_object)
            .cloned(),
    })
}

fn validate_stored_member_id(value: &str) -> Result<(), OneShotError> {
    let is_self = value == "_self";
    if !is_self {
        let folded = value.to_ascii_lowercase();
        let reserved = matches!(
            folded.as_str(),
            "me" | "myself"
                | "self"
                | "all"
                | "everyone"
                | "household"
                | "family"
                | "_self"
                | "__everyone__"
        );
        if value.is_empty()
            || value.len() > 128
            || value.trim() != value
            || value == "."
            || value == ".."
            || reserved
            || value.contains(['/', '\\'])
            || value.chars().any(forbidden_terminal_scalar)
        {
            return Err(OneShotError::new(
                "household_state_invalid",
                "saved Household roster contains an invalid member identity",
            ));
        }
    }
    Ok(())
}

fn validate_stored_member_name(value: &str) -> Result<(), OneShotError> {
    let scalar_count = value.chars().count();
    if value.is_empty()
        || value.len() > 320
        || scalar_count > 80
        || value.trim() != value
        || value.chars().any(forbidden_terminal_scalar)
    {
        return Err(OneShotError::new(
            "household_state_invalid",
            "saved Household roster contains an invalid display name",
        ));
    }
    Ok(())
}

fn forbidden_terminal_scalar(value: char) -> bool {
    value.is_control()
        || matches!(
            value,
            '\u{001b}'
                | '\u{009b}'
                | '\u{061c}'
                | '\u{200e}'..='\u{200f}'
                | '\u{2028}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
                | '\u{feff}'
        )
}

fn resolve_log_target(
    household: &FrozenHouseholdSnapshot,
    selector: Option<&str>,
    no_source: bool,
    protected: bool,
) -> Result<ResolvedLogTarget, OneShotError> {
    if protected {
        let Some(selector) = selector else {
            return Err(OneShotError::new(
                "household_state_protected",
                "protected Household state cannot resolve an omitted target",
            ));
        };
        let selector = validate_selector(selector)?;
        if is_self_alias(selector) {
            return Ok(resolved_log_target(
                LogTargetMode::SelfSubject,
                "_self",
                "Me",
            ));
        }
        return Err(OneShotError::new(
            "household_state_protected",
            "protected Household state cannot resolve this target",
        ));
    }

    if no_source {
        if selector.is_none() {
            return Ok(resolved_log_target(
                LogTargetMode::SelfSubject,
                "_self",
                "Me",
            ));
        }
        let selector = validate_selector(selector.expect("checked above"))?;
        if is_self_alias(selector) {
            return Ok(resolved_log_target(
                LogTargetMode::SelfSubject,
                "_self",
                "Me",
            ));
        }
        return Err(OneShotError::new(
            "household_state_unavailable",
            "Household state is unavailable; only self can be selected",
        ));
    }

    if selector.is_none() {
        return resolve_exact_frozen_scope(household, &household.active_scope);
    }
    let selector = validate_selector(selector.expect("checked above"))?;
    if is_self_alias(selector) {
        return Ok(resolved_log_target(
            LogTargetMode::SelfSubject,
            "_self",
            "Me",
        ));
    }
    if is_everyone_alias(selector) {
        return resolve_everyone_log_target(household);
    }
    if let Some(member) = household.member(selector) {
        if member.archived {
            return Err(OneShotError::new(
                "household_target_archived",
                "the selected Household member is archived",
            ));
        }
        return Ok(resolved_log_target(
            LogTargetMode::Member,
            &member.id,
            &member.name,
        ));
    }
    let folded = selector.to_lowercase();
    let matches = household
        .active_members()
        .filter(|member| member.id != "_self" && member.name.to_lowercase() == folded)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [member] => Ok(resolved_log_target(
            LogTargetMode::Member,
            &member.id,
            &member.name,
        )),
        [] => Err(OneShotError::new(
            "household_target_unknown",
            "the Household target is unknown",
        )),
        _ => Err(ambiguous_log_target_error()),
    }
}

fn ambiguous_log_target_error() -> OneShotError {
    OneShotError::new(
        "household_target_ambiguous",
        "more than one active Household member has that name; give members unique names in Household management, then retry",
    )
}

fn resolve_native_log_target(
    state: &HouseholdStateV1,
    selector: Option<&str>,
) -> Result<(HouseholdScope, ResolvedLogTarget), OneShotError> {
    state.validate().map_err(|_| {
        OneShotError::new(
            "household_state_invalid",
            "native Household state is invalid; repair it before logging",
        )
    })?;
    let scope = match selector {
        None => state.active_scope.clone(),
        Some(selector) => {
            let selector = validate_selector(selector)?;
            if is_self_alias(selector) {
                HouseholdScope::Subject(HouseholdSubjectId::self_())
            } else if is_everyone_alias(selector) {
                HouseholdScope::Everyone
            } else if let Some(member) = state
                .members
                .iter()
                .find(|member| member.member_id.as_str() == selector)
            {
                if member.lifecycle != HouseholdLifecycleV1::Active {
                    return Err(OneShotError::new(
                        "household_target_archived",
                        "the selected Household member is archived",
                    ));
                }
                HouseholdScope::Subject(HouseholdSubjectId::member(member.member_id.clone()))
            } else {
                let folded = selector.to_lowercase();
                let matches = state
                    .members
                    .iter()
                    .filter(|member| member.lifecycle == HouseholdLifecycleV1::Active)
                    .filter(|member| member.display_name.as_str().to_lowercase() == folded)
                    .collect::<Vec<_>>();
                match matches.as_slice() {
                    [member] => HouseholdScope::Subject(HouseholdSubjectId::member(
                        member.member_id.clone(),
                    )),
                    [] => {
                        return Err(OneShotError::new(
                            "household_target_unknown",
                            "the Household target is unknown",
                        ));
                    }
                    _ => {
                        return Err(ambiguous_log_target_error());
                    }
                }
            }
        }
    };
    let target = native_log_target_for_scope(state, &scope)?;
    Ok((scope, target))
}

fn native_log_target_for_scope(
    state: &HouseholdStateV1,
    scope: &HouseholdScope,
) -> Result<ResolvedLogTarget, OneShotError> {
    match scope {
        HouseholdScope::Subject(HouseholdSubjectId::Self_) => Ok(resolved_log_target(
            LogTargetMode::SelfSubject,
            "_self",
            "Me",
        )),
        HouseholdScope::Subject(HouseholdSubjectId::Member(member_id)) => {
            let member = state
                .members
                .iter()
                .find(|member| &member.member_id == member_id)
                .ok_or_else(|| {
                    OneShotError::new(
                        "household_target_unknown",
                        "the selected Household member is unknown",
                    )
                })?;
            if member.lifecycle != HouseholdLifecycleV1::Active {
                return Err(OneShotError::new(
                    "household_target_archived",
                    "the selected Household member is archived",
                ));
            }
            Ok(resolved_log_target(
                LogTargetMode::Member,
                member.member_id.as_str(),
                member.display_name.as_str(),
            ))
        }
        HouseholdScope::Everyone => {
            if !state
                .members
                .iter()
                .any(|member| member.lifecycle == HouseholdLifecycleV1::Active)
            {
                return Err(OneShotError::new(
                    "household_target_unknown",
                    "Everyone requires at least one active non-self Household member",
                ));
            }
            Ok(resolved_log_target(
                LogTargetMode::Everyone,
                "__everyone__",
                "Everyone",
            ))
        }
    }
}

fn validate_native_log_target_context(
    context: &TurnContext,
    authorized: &AuthorizedHostedContextV1,
    target: &ResolvedLogTarget,
) -> Result<(), OneShotError> {
    if context.household_scope.is_some() {
        return Err(OneShotError::new(
            "prepared_log_context_invalid",
            "prepared native log unexpectedly contained a server-resolved Household scope",
        ));
    }
    let scope_matches = match (&authorized.snapshot().scope, target.mode) {
        (HouseholdScope::Subject(HouseholdSubjectId::Self_), LogTargetMode::SelfSubject) => {
            target.raw_id == "_self"
        }
        (HouseholdScope::Subject(HouseholdSubjectId::Member(member_id)), LogTargetMode::Member) => {
            member_id.as_str() == target.raw_id
        }
        (HouseholdScope::Everyone, LogTargetMode::Everyone) => true,
        _ => false,
    };
    if !scope_matches {
        return Err(OneShotError::new(
            "prepared_log_context_invalid",
            "prepared native log target does not match its retained Household scope",
        ));
    }
    let meal = context
        .meal
        .as_ref()
        .and_then(Value::as_object)
        .ok_or_else(|| {
            OneShotError::new(
                "prepared_log_context_invalid",
                "prepared native log has no frozen meal target context",
            )
        })?;
    let (expected_id, expected_label) = match target.mode {
        LogTargetMode::SelfSubject | LogTargetMode::Everyone => ("_self", "Me"),
        LogTargetMode::Member => (target.raw_id.as_str(), target.raw_label.as_str()),
    };
    if meal.get("active_member_id").and_then(Value::as_str) != Some(expected_id)
        || meal.get("active_member_name").and_then(Value::as_str) != Some(expected_label)
        || meal.get("is_cook_mode").and_then(Value::as_bool) != Some(false)
    {
        return Err(OneShotError::new(
            "prepared_log_context_invalid",
            if target.mode == LogTargetMode::Everyone {
                "prepared Everyone target did not preserve one owner-attributed meal"
            } else {
                "prepared native log target did not preserve its reviewed identity"
            },
        ));
    }
    if target.mode == LogTargetMode::Everyone {
        let members = context
            .dietary
            .as_ref()
            .and_then(|value| value.get("members"))
            .and_then(Value::as_array)
            .ok_or_else(|| {
                OneShotError::new(
                    "prepared_log_context_invalid",
                    "prepared Everyone log has no exact Household profile projection",
                )
            })?;
        if members.len() != authorized.snapshot().subjects.len()
            || !members
                .iter()
                .any(|member| member.get("member_id").and_then(Value::as_str) == Some("_self"))
        {
            return Err(OneShotError::new(
                "prepared_log_context_invalid",
                "prepared Everyone log did not preserve every retained Household subject",
            ));
        }
    }
    Ok(())
}

fn resolve_exact_frozen_scope(
    household: &FrozenHouseholdSnapshot,
    scope: &str,
) -> Result<ResolvedLogTarget, OneShotError> {
    match scope {
        "_self" => Ok(resolved_log_target(
            LogTargetMode::SelfSubject,
            "_self",
            "Me",
        )),
        "__everyone__" => resolve_everyone_log_target(household),
        identifier => {
            let member = household.active_member(identifier).ok_or_else(|| {
                OneShotError::new(
                    "household_active_scope_invalid",
                    "saved Household scope is missing, unknown, or archived",
                )
            })?;
            Ok(resolved_log_target(
                LogTargetMode::Member,
                &member.id,
                &member.name,
            ))
        }
    }
}

fn resolve_everyone_log_target(
    household: &FrozenHouseholdSnapshot,
) -> Result<ResolvedLogTarget, OneShotError> {
    if household.active_member("_self").is_none() {
        return Err(OneShotError::new(
            "household_owner_missing",
            "Everyone meal attribution requires an active Household owner",
        ));
    }
    if !household
        .active_members()
        .any(|member| member.id != "_self")
    {
        return Err(OneShotError::new(
            "household_target_unknown",
            "Everyone requires at least one active non-self Household member",
        ));
    }
    Ok(resolved_log_target(
        LogTargetMode::Everyone,
        "__everyone__",
        "Everyone",
    ))
}

fn validate_selector(value: &str) -> Result<&str, OneShotError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 320 || value.chars().count() > 128 {
        return Err(OneShotError::new(
            "household_target_invalid",
            "Household selector must contain 1 to 128 characters",
        ));
    }
    Ok(value)
}

fn is_self_alias(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "_self" | "self" | "me" | "myself"
    )
}

fn is_everyone_alias(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "__everyone__" | "all" | "everyone" | "household" | "family"
    )
}

fn resolved_log_target(mode: LogTargetMode, raw_id: &str, raw_label: &str) -> ResolvedLogTarget {
    ResolvedLogTarget {
        raw_id: raw_id.to_owned(),
        raw_label: raw_label.to_owned(),
        display: canonical_display(mode, raw_label, raw_id),
        mode,
    }
}

fn canonical_display(mode: LogTargetMode, raw_label: &str, raw_id: &str) -> CanonicalTargetDisplay {
    let stable_id_token = match mode {
        LogTargetMode::SelfSubject => "scope=_self".to_owned(),
        LogTargetMode::Everyone => "scope=__everyone__".to_owned(),
        LogTargetMode::Member => {
            let mut token = String::with_capacity(20 + raw_id.len() * 2);
            token.push_str("member-id-utf8-hex=");
            for byte in raw_id.as_bytes() {
                write!(&mut token, "{byte:02x}").expect("writing to String cannot fail");
            }
            token
        }
    };
    CanonicalTargetDisplay {
        escaped_label: ascii_json_string(raw_label),
        stable_id_token,
    }
}

fn ascii_json_string(value: &str) -> String {
    let mut rendered = String::with_capacity(value.len() + 2);
    rendered.push('"');
    for scalar in value.chars() {
        match scalar {
            '"' => rendered.push_str("\\\""),
            '\\' => rendered.push_str("\\\\"),
            '\u{20}'..='\u{7e}' => rendered.push(scalar),
            scalar if u32::from(scalar) <= 0xffff => {
                write!(&mut rendered, "\\u{:04X}", u32::from(scalar))
                    .expect("writing to String cannot fail");
            }
            scalar => {
                let value = u32::from(scalar) - 0x1_0000;
                let high = 0xd800 + (value >> 10);
                let low = 0xdc00 + (value & 0x3ff);
                write!(&mut rendered, "\\u{high:04X}\\u{low:04X}")
                    .expect("writing to String cannot fail");
            }
        }
    }
    rendered.push('"');
    rendered
}

fn strict_profile_consent(value: &Value) -> Result<bool, OneShotError> {
    value
        .as_object()
        .and_then(|object| object.get("has_consent"))
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            OneShotError::new(
                "profile_consent_contract_invalid",
                "profile consent response is malformed",
            )
        })
}

async fn build_household_turn_context_for_resolved_target(
    service: &HttpService,
    credentials: &SessionCredentials,
    household: &FrozenHouseholdSnapshot,
    target: &ResolvedLogTarget,
    cancellation: CancellationToken,
) -> Result<TurnContext, OneShotError> {
    let consent = service
        .profile_consent_status(credentials, OperationId::new(), cancellation.child_token())
        .await?;
    let has_consent = strict_profile_consent(&consent)?;
    let active = household.active_members().collect::<Vec<_>>();
    let owner = household.active_member("_self");
    let local_profiles = household.local_profiles.as_ref();
    let profile_outbox = household.profile_outbox.as_ref();

    let dietary = if target.mode == LogTargetMode::Everyone {
        let mut members = Vec::with_capacity(active.len());
        for member in &active {
            let profile = profile_for_household_member(
                service,
                credentials,
                &member.raw,
                local_profiles,
                profile_outbox,
                has_consent,
                cancellation.child_token(),
            )
            .await?;
            let mut context =
                member_dietary_context(&member.raw, &profile, owner.map(|value| &value.raw))?;
            context.insert("member_id".into(), Value::String(member.id.clone()));
            context.insert("label".into(), Value::String(member.name.clone()));
            members.push(Value::Object(context));
        }
        json!({"mode": "household", "members": members})
    } else {
        if let Some(member) = household.active_member(&target.raw_id) {
            let profile = profile_for_household_member(
                service,
                credentials,
                &member.raw,
                local_profiles,
                profile_outbox,
                has_consent,
                cancellation.child_token(),
            )
            .await?;
            Value::Object(member_dietary_context(
                &member.raw,
                &profile,
                owner.map(|value| &value.raw),
            )?)
        } else if target.mode == LogTargetMode::SelfSubject && target.raw_id == "_self" {
            let profile = profile_for_household_subject(
                service,
                credentials,
                "_self",
                Some("self"),
                local_profiles,
                profile_outbox,
                has_consent,
                cancellation.child_token(),
            )
            .await?;
            Value::Object(dietary_context_for_identity(
                "_self", "Me", "self", None, &profile, None,
            ))
        } else {
            return Err(OneShotError::new(
                "household_state_changed",
                "frozen target is no longer present",
            ));
        }
    };
    let device = has_consent.then(|| {
        json!({
            "household": {
                "owner_id": "_self",
                "members": active.iter().map(|member| {
                    json!({
                        "id": member.id,
                        "name": member.name,
                        "relationship": member.raw.get("relationship").and_then(Value::as_str).unwrap_or("other"),
                        "is_owner": member.id == "_self"
                    })
                }).collect::<Vec<_>>()
            }
        })
    });
    let meal = if target.mode == LogTargetMode::Everyone {
        let owner = owner.ok_or_else(|| {
            OneShotError::new(
                "household_owner_missing",
                "Everyone meal attribution requires an active Household owner",
            )
        })?;
        json!({
            "active_member_id": "_self",
            "active_member_name": owner.name,
            "is_cook_mode": false
        })
    } else {
        json!({
            "active_member_id": target.raw_id,
            "active_member_name": target.raw_label,
            "is_cook_mode": false
        })
    };
    Ok(TurnContext {
        dietary: Some(dietary),
        device,
        meal: Some(meal),
        ..TurnContext::default()
    })
}

fn validate_frozen_target_context(
    context: &TurnContext,
    household: &FrozenHouseholdSnapshot,
    target: &ResolvedLogTarget,
) -> Result<(), OneShotError> {
    let meal = context
        .meal
        .as_ref()
        .and_then(Value::as_object)
        .ok_or_else(|| {
            OneShotError::new(
                "prepared_log_context_invalid",
                "prepared log has no frozen meal target context",
            )
        })?;
    match target.mode {
        LogTargetMode::Everyone => {
            let owner_label = household
                .active_member("_self")
                .map(|owner| owner.name.as_str());
            if meal.get("active_member_id").and_then(Value::as_str) != Some("_self")
                || meal.get("active_member_name").and_then(Value::as_str) != owner_label
                || meal.get("is_cook_mode").and_then(Value::as_bool) != Some(false)
            {
                return Err(OneShotError::new(
                    "prepared_log_context_invalid",
                    "prepared Everyone target did not preserve one owner-attributed meal",
                ));
            }
        }
        LogTargetMode::SelfSubject | LogTargetMode::Member => {
            if meal.get("active_member_id").and_then(Value::as_str) != Some(target.raw_id.as_str())
                || meal.get("active_member_name").and_then(Value::as_str)
                    != Some(target.raw_label.as_str())
                || meal.get("is_cook_mode").and_then(Value::as_bool) != Some(false)
            {
                return Err(OneShotError::new(
                    "prepared_log_context_invalid",
                    "prepared log target did not preserve its reviewed identity",
                ));
            }
        }
    }
    let allowed = household
        .active_members()
        .map(|member| member.id.as_str())
        .collect::<BTreeSet<_>>();
    if let Some(members) = context
        .dietary
        .as_ref()
        .and_then(|value| value.get("members"))
        .and_then(Value::as_array)
    {
        for member in members {
            if !member
                .get("member_id")
                .and_then(Value::as_str)
                .is_some_and(|id| allowed.contains(id))
            {
                return Err(OneShotError::new(
                    "prepared_log_context_invalid",
                    "prepared dietary context contains an unfrozen identity",
                ));
            }
        }
    }
    if let Some(members) = context
        .device
        .as_ref()
        .and_then(|value| value.get("household"))
        .and_then(|value| value.get("members"))
        .and_then(Value::as_array)
    {
        for member in members {
            if !member
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| allowed.contains(id))
            {
                return Err(OneShotError::new(
                    "prepared_log_context_invalid",
                    "prepared device context contains an unfrozen identity",
                ));
            }
        }
    }
    Ok(())
}

fn normalized_household(state: &ImportedPythonState) -> Result<Map<String, Value>, OneShotError> {
    let owner_name = state
        .account_scoped
        .get("first_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Me");
    let mut household = state
        .account_scoped
        .get("household")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(Map::new);
    let raw_members = household
        .remove("members")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let mut members = Vec::new();
    let mut identifiers = std::collections::BTreeSet::new();
    for raw in raw_members {
        let Some(mut member) = raw.as_object().cloned() else {
            continue;
        };
        let Some(id) = member
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != "__everyone__")
            .map(str::to_owned)
        else {
            continue;
        };
        if !identifiers.insert(id.clone()) {
            continue;
        }
        let name = member
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(if id == "_self" { owner_name } else { &id })
            .to_owned();
        let relationship = member
            .get("relationship")
            .and_then(Value::as_str)
            .unwrap_or(if id == "_self" { "self" } else { "other" })
            .to_owned();
        member.insert("id".into(), Value::String(id.clone()));
        member.insert("name".into(), Value::String(name));
        member.insert(
            "relationship".into(),
            Value::String(if id == "_self" {
                "self".to_owned()
            } else {
                relationship
            }),
        );
        member.insert("is_owner".into(), Value::Bool(id == "_self"));
        members.push(Value::Object(member));
    }
    if !identifiers.contains("_self") {
        members.insert(
            0,
            json!({
                "id": "_self",
                "name": owner_name,
                "relationship": "self",
                "is_owner": true,
                "archived": false
            }),
        );
    }
    let active_ids = members
        .iter()
        .filter_map(Value::as_object)
        .filter(|member| {
            !member
                .get("archived")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .filter_map(|member| member.get("id").and_then(Value::as_str))
        .collect::<std::collections::BTreeSet<_>>();
    let active_scope = household
        .get("active_scope")
        .and_then(Value::as_str)
        .filter(|scope| {
            active_ids.contains(*scope) || (*scope == "__everyone__" && active_ids.len() >= 2)
        })
        .unwrap_or("_self")
        .to_owned();
    household.insert("owner_id".into(), Value::String("_self".into()));
    household.insert("active_scope".into(), Value::String(active_scope));
    household.insert("members".into(), Value::Array(members));
    Ok(household)
}

fn active_household_members(household: &Map<String, Value>) -> Vec<&Map<String, Value>> {
    household
        .get("members")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter(|member| {
            !member
                .get("archived")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .collect()
}

fn member_by_id<'a>(
    members: &'a [&Map<String, Value>],
    identifier: &str,
) -> Option<&'a Map<String, Value>> {
    members
        .iter()
        .copied()
        .find(|member| member.get("id").and_then(Value::as_str) == Some(identifier))
}

fn member_id(member: &Map<String, Value>) -> Result<&str, OneShotError> {
    member
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| OneShotError::new("household_state", "household member ID is missing"))
}

fn member_name(member: &Map<String, Value>) -> Result<&str, OneShotError> {
    member
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| OneShotError::new("household_state", "household member name is missing"))
}

fn resolve_household_scope(
    household: &Map<String, Value>,
    selector: Option<&str>,
) -> Result<String, OneShotError> {
    let selector = selector
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| household.get("active_scope").and_then(Value::as_str))
        .unwrap_or("_self");
    let members = active_household_members(household);
    let folded = selector.to_lowercase();
    if matches!(folded.as_str(), "me" | "myself" | "self" | "_self") {
        return Ok("_self".into());
    }
    if matches!(
        folded.as_str(),
        "all" | "everyone" | "household" | "family" | "__everyone__"
    ) {
        if members.len() < 2 {
            return Err(OneShotError::new(
                "household_scope",
                "add or import another household member before selecting everyone",
            ));
        }
        return Ok("__everyone__".into());
    }
    if member_by_id(&members, selector).is_some() {
        return Ok(selector.to_owned());
    }
    let matches = members
        .iter()
        .filter(|member| {
            member
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.to_lowercase() == folded)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [member] => Ok(member_id(member)?.to_owned()),
        [] => Err(OneShotError::new(
            "household_scope",
            format!("unknown household scope '{selector}'"),
        )),
        _ => Err(OneShotError::new(
            "household_scope",
            format!("more than one household member is named '{selector}'; use a member ID"),
        )),
    }
}

fn resolve_household_scope_with_label(
    state: &ImportedPythonState,
    selector: &str,
) -> Result<(String, String), String> {
    let household = normalized_household(state).map_err(|error| error.message)?;
    let identifier =
        resolve_household_scope(&household, Some(selector)).map_err(|error| error.message)?;
    if identifier == "__everyone__" {
        return Ok((identifier, "Everyone".into()));
    }
    let members = active_household_members(&household);
    let label = member_by_id(&members, &identifier)
        .and_then(|member| member.get("name"))
        .and_then(Value::as_str)
        .ok_or_else(|| "Selected household member is unavailable.".to_owned())?
        .to_owned();
    Ok((identifier, label))
}

async fn build_household_turn_context(
    service: &HttpService,
    credentials: &SessionCredentials,
    state: &ImportedPythonState,
    selector: Option<&str>,
    cancellation: CancellationToken,
) -> Result<TurnContext, OneShotError> {
    if state.account_user_id.as_deref() != Some(credentials.account_id.as_str()) {
        return Err(OneShotError::new(
            "python_state_account_mismatch",
            "imported Python state does not belong to the authenticated account",
        ));
    }
    let household = normalized_household(state)?;
    let selected = resolve_household_scope(&household, selector)?;
    let consent = service
        .profile_consent_status(credentials, OperationId::new(), cancellation.child_token())
        .await?;
    let has_consent = strict_profile_consent(&consent)?;
    let active = active_household_members(&household);
    let owner = member_by_id(&active, "_self")
        .or_else(|| active.first().copied())
        .ok_or_else(|| OneShotError::new("household_state", "household has no active owner"))?;
    let local_profiles = state
        .account_scoped
        .get("household_local_profiles")
        .and_then(Value::as_object);
    let profile_outbox = state
        .account_scoped
        .get("household_profile_outbox")
        .and_then(Value::as_object);

    let dietary = if selected == "__everyone__" {
        let mut members = Vec::with_capacity(active.len());
        for member in &active {
            let profile = profile_for_household_member(
                service,
                credentials,
                member,
                local_profiles,
                profile_outbox,
                has_consent,
                cancellation.child_token(),
            )
            .await?;
            let mut context = member_dietary_context(member, &profile, Some(owner))?;
            context.insert(
                "member_id".into(),
                Value::String(member_id(member)?.to_owned()),
            );
            context.insert(
                "label".into(),
                Value::String(member_name(member)?.to_owned()),
            );
            members.push(Value::Object(context));
        }
        json!({"mode": "household", "members": members})
    } else {
        let member = member_by_id(&active, &selected).ok_or_else(|| {
            OneShotError::new(
                "household_scope",
                "selected household member is unavailable",
            )
        })?;
        let profile = profile_for_household_member(
            service,
            credentials,
            member,
            local_profiles,
            profile_outbox,
            has_consent,
            cancellation.child_token(),
        )
        .await?;
        Value::Object(member_dietary_context(member, &profile, Some(owner))?)
    };
    let selected_member = member_by_id(&active, &selected);
    let scope_label = if selected == "__everyone__" {
        "Everyone".to_owned()
    } else {
        member_name(selected_member.ok_or_else(|| {
            OneShotError::new(
                "household_scope",
                "selected household member is unavailable",
            )
        })?)?
        .to_owned()
    };
    let device = has_consent.then(|| {
        json!({
            "household": {
                "owner_id": "_self",
                "members": active.iter().filter_map(|member| {
                    Some(json!({
                        "id": member.get("id")?.as_str()?,
                        "name": member.get("name")?.as_str()?,
                        "relationship": member.get("relationship").and_then(Value::as_str).unwrap_or("other"),
                        "is_owner": member.get("id").and_then(Value::as_str) == Some("_self")
                    }))
                }).collect::<Vec<_>>()
            }
        })
    });
    let meal = if selected == "__everyone__" {
        json!({
            "active_member_id": "_self",
            "active_member_name": member_name(owner)?,
            "is_cook_mode": false
        })
    } else {
        json!({
            "active_member_id": selected,
            "active_member_name": scope_label,
            "is_cook_mode": false
        })
    };
    Ok(TurnContext {
        dietary: Some(dietary),
        device,
        meal: Some(meal),
        ..TurnContext::default()
    })
}

#[allow(clippy::too_many_arguments)]
async fn profile_for_household_member(
    service: &HttpService,
    credentials: &SessionCredentials,
    member: &Map<String, Value>,
    local_profiles: Option<&Map<String, Value>>,
    profile_outbox: Option<&Map<String, Value>>,
    has_consent: bool,
    cancellation: CancellationToken,
) -> Result<Value, OneShotError> {
    let id = member_id(member)?;
    profile_for_household_subject(
        service,
        credentials,
        id,
        member.get("relationship").and_then(Value::as_str),
        local_profiles,
        profile_outbox,
        has_consent,
        cancellation,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn profile_for_household_subject(
    service: &HttpService,
    credentials: &SessionCredentials,
    id: &str,
    relationship: Option<&str>,
    local_profiles: Option<&Map<String, Value>>,
    profile_outbox: Option<&Map<String, Value>>,
    has_consent: bool,
    cancellation: CancellationToken,
) -> Result<Value, OneShotError> {
    if relationship == Some("child") {
        return match local_profiles.and_then(|profiles| profiles.get(id)) {
            Some(profile) if profile.is_object() => Ok(profile.clone()),
            Some(_) => Err(OneShotError::new(
                "household_profile_contract_invalid",
                "saved Household child profile is malformed",
            )),
            None => Ok(json!({})),
        };
    }
    if let Some(pending) = profile_outbox.and_then(|outbox| outbox.get(id)) {
        return pending
            .get("local_context")
            .filter(|value| value.is_object())
            .cloned()
            .ok_or_else(|| {
                OneShotError::new(
                    "household_profile_contract_invalid",
                    "saved Household profile outbox entry is malformed",
                )
            });
    }
    if !has_consent {
        return Ok(json!({}));
    }
    let downloaded = service
        .download_profile(credentials, id, OperationId::new(), cancellation)
        .await?;
    downloaded
        .get("profile_data")
        .filter(|value| value.is_object())
        .cloned()
        .ok_or_else(|| {
            OneShotError::new(
                "household_profile_contract_invalid",
                "remote Household profile response is malformed",
            )
        })
}

fn member_dietary_context(
    member: &Map<String, Value>,
    profile: &Value,
    owner: Option<&Map<String, Value>>,
) -> Result<Map<String, Value>, OneShotError> {
    let id = member_id(member)?;
    let name = member_name(member)?;
    let relationship = member
        .get("relationship")
        .and_then(Value::as_str)
        .unwrap_or("other");
    let birth_date = member.get("date_of_birth").and_then(Value::as_str);
    let owner_name = if id == "_self" {
        None
    } else {
        owner.map(member_name).transpose()?
    };
    Ok(dietary_context_for_identity(
        id,
        name,
        relationship,
        birth_date,
        profile,
        owner_name,
    ))
}

fn dietary_context_for_identity(
    id: &str,
    name: &str,
    relationship: &str,
    birth_date: Option<&str>,
    profile: &Value,
    owner_name: Option<&str>,
) -> Map<String, Value> {
    // Keep the complete canonical source projection in the outbound document.
    // The deployed DietaryContext currently ignores the provenance fields it
    // does not understand, but retaining them here prevents the native owner
    // path from becoming a lossy serialization boundary when that contract is
    // expanded.
    const CANONICAL_PROFILE_KEYS: &[&str] = &[
        "preferences",
        "preference_strictness",
        "restrictions",
        "restriction_handling",
        "avoid_ingredients",
        "medical_constraints",
        "severity_level",
        "notes",
        "activity_level",
        "cuisine_preferences",
        "health_condition_ids",
        "custom_health_conditions",
        "custom_diet_styles",
        "custom_restrictions",
        "custom_cuisines",
        "diet_style_ids",
        "allergy_ids",
        "additional_restriction_ids",
        "additional_medical_constraints",
        "condition_severity_levels",
        "medical_condition_id",
        "selection_provenance_version",
    ];
    let mut context = Map::new();
    if let Some(profile) = profile.as_object() {
        for key in CANONICAL_PROFILE_KEYS {
            if let Some(value) = profile.get(*key).filter(|value| !value.is_null()) {
                context.insert((*key).to_owned(), value.clone());
            }
        }

        // These projections are deliberately limited to fields accepted by
        // the deployed backend. Unknown source fields remain above for a
        // future lossless contract, while safety-significant custom values
        // still reach today's prompt/evaluation paths.
        project_string_arrays_if_representable(
            &mut context,
            profile,
            "avoid_ingredients",
            &["custom_restrictions"],
            20,
        );
        project_additional_restrictions(&mut context, profile);
        project_medical_context(&mut context, profile);
        project_string_arrays_if_representable(
            &mut context,
            profile,
            "cuisine_preferences",
            &["custom_cuisines"],
            20,
        );
        project_conservative_severity(&mut context, profile);

        if let Some(primary) = profile_primary_medical_condition(profile) {
            context.insert(
                "medical_condition".into(),
                Value::String(primary.to_owned()),
            );
        }
    }
    context.insert("name".into(), Value::String(name.to_owned()));
    context.insert(
        "relationship".into(),
        Value::String(relationship.to_owned()),
    );
    if id != "_self"
        && let Some(owner_name) = owner_name
    {
        context.insert("owner_name".into(), Value::String(owner_name.to_owned()));
    }
    if let Some(birth_date) = birth_date {
        context.insert("date_of_birth".into(), Value::String(birth_date.to_owned()));
    }
    context
}

fn project_string_arrays_if_representable(
    context: &mut Map<String, Value>,
    profile: &Map<String, Value>,
    target: &str,
    sources: &[&str],
    maximum: usize,
) {
    let (values, projected) = projected_string_array(profile, target, sources, None);
    let target_was_present = profile.get(target).is_some_and(|value| !value.is_null());
    // Do not partially project a safety field. The native owner path rejects
    // an unrepresentable union before refresh; compatibility call sites keep
    // their exact original normalized and source arrays instead of truncating.
    if values.len() <= maximum && (target_was_present || projected) {
        context.insert(target.to_owned(), Value::Array(values));
    }
}

fn projected_string_array(
    profile: &Map<String, Value>,
    target: &str,
    sources: &[&str],
    excluded: Option<&str>,
) -> (Vec<Value>, bool) {
    let mut values = profile
        .get(target)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut projected = false;
    for source in sources {
        projected |= append_unique_profile_strings(&mut values, profile, source, excluded);
    }
    (values, projected)
}

fn append_unique_profile_strings(
    values: &mut Vec<Value>,
    profile: &Map<String, Value>,
    source: &str,
    excluded: Option<&str>,
) -> bool {
    let mut projected = false;
    let Some(source_values) = profile.get(source).and_then(Value::as_array) else {
        return projected;
    };
    for value in source_values.iter().filter_map(Value::as_str) {
        if Some(value) != excluded
            && !values
                .iter()
                .any(|candidate| candidate.as_str() == Some(value))
        {
            values.push(Value::String(value.to_owned()));
            projected = true;
        }
    }
    projected
}

fn project_additional_restrictions(context: &mut Map<String, Value>, profile: &Map<String, Value>) {
    const ACCEPTED_RESTRICTIONS: &[&str] = &[
        "glutenFree",
        "dairyFree",
        "nutFree",
        "peanutFree",
        "treeNutFree",
        "shellfishFree",
        "fishFree",
        "soyFree",
        "eggFree",
        "sesameFree",
        "lactoseIntolerant",
        "halal",
        "kosher",
    ];
    let mut restrictions = profile
        .get("restrictions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut handling = profile
        .get("restriction_handling")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut projected = false;
    if let Some(additional) = profile
        .get("additional_restriction_ids")
        .and_then(Value::as_array)
    {
        for value in additional.iter().filter_map(Value::as_str) {
            if !ACCEPTED_RESTRICTIONS.contains(&value)
                || restrictions
                    .iter()
                    .any(|candidate| candidate.as_str() == Some(value))
            {
                continue;
            }
            restrictions.push(Value::String(value.to_owned()));
            handling.entry(value.to_owned()).or_insert_with(|| {
                Value::String(default_hosted_restriction_handling(value).to_owned())
            });
            projected = true;
        }
    }
    if profile
        .get("restrictions")
        .is_some_and(|value| !value.is_null())
        || projected
    {
        context.insert("restrictions".into(), Value::Array(restrictions));
    }
    if profile
        .get("restriction_handling")
        .is_some_and(|value| !value.is_null())
        || projected
    {
        context.insert("restriction_handling".into(), Value::Object(handling));
    }
}

fn default_hosted_restriction_handling(restriction: &str) -> &'static str {
    match restriction {
        "nutFree" | "peanutFree" | "treeNutFree" | "shellfishFree" | "fishFree" | "eggFree"
        | "sesameFree" => "strictAvoid",
        "lactoseIntolerant" => "doseDependent",
        "halal" | "kosher" => "verificationRequired",
        _ => "ingredientsOnly",
    }
}

fn project_medical_context(context: &mut Map<String, Value>, profile: &Map<String, Value>) {
    let (medical, projected) = projected_medical_constraints(profile);
    let target_was_present = profile
        .get("medical_constraints")
        .is_some_and(|value| !value.is_null());
    if medical.len() <= 20 && (target_was_present || projected) {
        context.insert("medical_constraints".into(), Value::Array(medical));
    }
}

fn projected_medical_constraints(profile: &Map<String, Value>) -> (Vec<Value>, bool) {
    let mut medical = profile
        .get("medical_constraints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let primary = profile_primary_medical_condition(profile);
    // Only secondary condition identifiers belong in medical_constraints;
    // the primary identifier is represented by medical_condition below.
    let mut projected =
        append_unique_profile_strings(&mut medical, profile, "health_condition_ids", primary);
    projected |= append_unique_profile_strings(
        &mut medical,
        profile,
        "additional_medical_constraints",
        None,
    );
    projected |=
        append_unique_profile_strings(&mut medical, profile, "custom_health_conditions", None);
    (medical, projected)
}

fn profile_primary_medical_condition(profile: &Map<String, Value>) -> Option<&str> {
    profile
        .get("medical_condition_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.chars().count() <= 100)
        .or_else(|| {
            profile
                .get("health_condition_ids")
                .and_then(Value::as_array)
                .and_then(|values| values.iter().filter_map(Value::as_str).next())
                .filter(|value| !value.is_empty() && value.chars().count() <= 100)
        })
        .or_else(|| {
            profile
                .get("custom_health_conditions")
                .and_then(Value::as_array)
                .and_then(|values| values.iter().filter_map(Value::as_str).next())
                .filter(|value| !value.is_empty() && value.chars().count() <= 100)
        })
}

fn validate_native_hosted_profile(profile: &Value) -> Result<(), PortError> {
    let profile = profile.as_object().ok_or_else(|| {
        PortError::new(
            "household_hosted_context_invalid",
            "an authorized native household profile is malformed",
        )
    })?;

    let avoid_count =
        projected_string_array(profile, "avoid_ingredients", &["custom_restrictions"], None)
            .0
            .len();
    if avoid_count > 20 {
        return Err(hosted_context_unrepresentable(
            "a household avoid/restriction profile exceeds the deployed context bound",
        ));
    }

    let cuisine_count =
        projected_string_array(profile, "cuisine_preferences", &["custom_cuisines"], None)
            .0
            .len();
    if cuisine_count > 20 {
        return Err(hosted_context_unrepresentable(
            "a household cuisine profile exceeds the deployed context bound",
        ));
    }

    let (medical, _) = projected_medical_constraints(profile);
    if medical.len() > 20 {
        return Err(hosted_context_unrepresentable(
            "a household medical profile exceeds the deployed context bound",
        ));
    }
    Ok(())
}

fn hosted_context_unrepresentable(message: &'static str) -> PortError {
    PortError::new("household_hosted_context_unrepresentable", message)
}

fn project_conservative_severity(context: &mut Map<String, Value>, profile: &Map<String, Value>) {
    let scalar = profile
        .get("severity_level")
        .and_then(Value::as_u64)
        .filter(|value| (1..=5).contains(value));
    let per_condition = profile
        .get("condition_severity_levels")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(Map::values)
        .filter_map(Value::as_u64)
        .filter(|value| (1..=5).contains(value))
        .max();
    if let Some(severity) = scalar.into_iter().chain(per_condition).max() {
        context.insert("severity_level".into(), Value::from(severity));
    } else {
        // DietaryContext.severity_level is non-nullable. Omitting an absent
        // canonical scalar lets the deployed backend apply its safe default;
        // serializing JSON null would turn an otherwise valid turn into 422.
        context.remove("severity_level");
    }
}

fn ensure_oura(provider: heyfood_cli::HealthProviderArgument) -> Result<(), OneShotError> {
    if !matches!(provider, heyfood_cli::HealthProviderArgument::Oura) {
        return Err(OneShotError::new(
            "health_provider",
            "only provider-neutral Oura management is implemented",
        ));
    }
    Ok(())
}

static NEXT_HOUSEHOLD_MODE_GENERATION_V1: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Eq, PartialEq)]
struct NativeHouseholdDriverBindingV1 {
    session_mode_generation: HouseholdModeGenerationV1,
    account_binding_digest: HouseholdAccountBindingDigestV1,
    mode: HouseholdPresentationModeV1,
}

#[derive(Clone, Eq, PartialEq)]
struct NativeHouseholdCommittedEvidenceV1 {
    binding: HouseholdOperationBindingV1,
    resulting_household_revision: HouseholdRevision,
    affected_subject: Option<HouseholdSubjectId>,
    active_scope: HouseholdScope,
    bounded_active_label: String,
}

impl NativeHouseholdCommittedEvidenceV1 {
    fn matches_apply(
        &self,
        binding: &HouseholdOperationBindingV1,
        resulting_household_revision: HouseholdRevision,
        affected_subject: &Option<HouseholdSubjectId>,
        active_scope: &HouseholdScope,
        bounded_active_label: &str,
    ) -> bool {
        self.binding == *binding
            && self.resulting_household_revision == resulting_household_revision
            && self.affected_subject == *affected_subject
            && self.active_scope == *active_scope
            && self.bounded_active_label == bounded_active_label
    }
}

struct OwnedInteractiveTurn {
    operation_id: u64,
    household_binding: Option<HouseholdOperationBindingV1>,
    /// Set with `Release` immediately before this turn sends its terminal
    /// event. The driver reads it with `Acquire` so an event-triggered
    /// follow-up does not have to wait for the sender task's scheduler tail.
    followup_ready: Arc<AtomicBool>,
    cancellation: CancellationToken,
    stop: Option<CancellationToken>,
    task: JoinHandle<()>,
}

impl OwnedInteractiveTurn {
    fn blocks_interactive_followup(&self) -> bool {
        !self.followup_ready.load(AtomicOrdering::Acquire)
    }
}

struct OwnedSignalForwarder {
    cancellation: CancellationToken,
    task: JoinHandle<io::Result<()>>,
}

#[derive(Clone, Eq, PartialEq)]
struct NativeHouseholdContextBindingV1 {
    household_revision: HouseholdRevision,
    active_scope: HouseholdScope,
}

impl NativeHouseholdContextBindingV1 {
    fn from_load(load: &HouseholdLoad) -> Self {
        Self {
            household_revision: load.state.revision,
            active_scope: load.state.active_scope.clone(),
        }
    }

    fn from_authorized(context: &AuthorizedHostedContextV1) -> Self {
        Self {
            household_revision: context.snapshot().household_revision,
            active_scope: context.snapshot().scope.clone(),
        }
    }

    fn matches_presented(&self, presented: &PresentedHouseholdContextV1) -> bool {
        self.household_revision == presented.household_revision()
            && self.active_scope == *presented.active_scope()
    }
}

#[derive(Default)]
struct InteractiveContinuity {
    conversation_id: Option<String>,
    household_scope: Option<String>,
    native_context: Option<NativeHouseholdContextBindingV1>,
}

impl InteractiveContinuity {
    fn clear_conversation(&mut self) {
        self.conversation_id = None;
        self.native_context = None;
    }

    fn conversation_for_native_context(
        &mut self,
        authorized: &NativeHouseholdContextBindingV1,
        presented: Option<&PresentedHouseholdContextV1>,
    ) -> Result<Option<String>, RunTurnOutcome> {
        let chrome_is_stale =
            presented.is_none_or(|presented| !authorized.matches_presented(presented));
        let conversation_is_stale =
            self.conversation_id.is_some() && self.native_context.as_ref() != Some(authorized);
        if chrome_is_stale || conversation_is_stale {
            self.clear_conversation();
            return Err(RunTurnOutcome::StaleGeneration);
        }
        self.native_context = Some(authorized.clone());
        Ok(self.conversation_id.clone())
    }
}

/// Fresh native authorization and session composition for one interactive
/// operation. Long-running terminal sessions must not retain the channel
/// access token captured when the TUI first opened.
pub struct InteractiveSessionPreparation {
    service: Arc<dyn ServicePort>,
    http_service: Option<Arc<HttpService>>,
    ensure_session: Arc<EnsureSession>,
    snapshot: SessionSnapshot,
    authorization_scope: Arc<str>,
}

impl InteractiveSessionPreparation {
    #[must_use]
    pub fn new(
        service: Arc<HttpService>,
        ensure_session: Arc<EnsureSession>,
        snapshot: SessionSnapshot,
        authorization_scope: impl Into<Arc<str>>,
    ) -> Self {
        let conversational_service: Arc<dyn ServicePort> = service.clone();
        Self {
            service: conversational_service,
            http_service: Some(service),
            ensure_session,
            snapshot,
            authorization_scope: authorization_scope.into(),
        }
    }

    #[cfg(test)]
    fn from_service(
        service: Arc<dyn ServicePort>,
        ensure_session: Arc<EnsureSession>,
        snapshot: SessionSnapshot,
        authorization_scope: impl Into<Arc<str>>,
    ) -> Self {
        Self {
            service,
            http_service: None,
            ensure_session,
            snapshot,
            authorization_scope: authorization_scope.into(),
        }
    }
}

/// Re-load and reconcile native account authority before an authenticated TUI
/// operation. Implementations own channel-refresh persistence and must return
/// only after the complete account-bound bundle is durable.
pub trait InteractiveSessionProvider: Send + Sync {
    fn prepare(
        &self,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<InteractiveSessionPreparation, OneShotError>>;
}

struct PreparedInteractiveOperation {
    service: Arc<dyn ServicePort>,
    http_service: Option<Arc<HttpService>>,
    ensure_session: Arc<EnsureSession>,
    authorization_scope: Arc<str>,
}

struct PreparedHostedInteractiveOperation {
    operation: PreparedInteractiveOperation,
    hosted_context: Option<AuthorizedHostedContextV1>,
}

/// Production driver for the retained terminal surface.
///
/// The terminal loop stays synchronous and owns stdout. Every authenticated
/// refresh and SSE operation runs on this driver's private Tokio runtime and
/// communicates with the reducer through the bounded runtime-event channel.
/// Conversation continuity is process-memory-only, matching the TUI privacy
/// contract.
pub struct InteractiveTurnDriver {
    runtime: Runtime,
    service: Arc<dyn ServicePort>,
    interactive_service: Option<Arc<HttpService>>,
    audio_capture: Option<Arc<dyn AudioCapturePort>>,
    authorization_scope: Arc<str>,
    local_state: Option<Arc<ImportedPythonState>>,
    household_session: Option<HouseholdSession>,
    profile_presentation_mode: ProfilePresentationModeV1,
    startup_notice: Option<String>,
    startup_onboarding: bool,
    session_provider: Option<Arc<dyn InteractiveSessionProvider>>,
    ensure_session: Arc<EnsureSession>,
    session: Arc<Mutex<SessionSnapshot>>,
    continuity: Arc<Mutex<InteractiveContinuity>>,
    turns: Vec<OwnedInteractiveTurn>,
    signals: Option<OwnedSignalForwarder>,
    household_driver_binding: Option<NativeHouseholdDriverBindingV1>,
    household_runtime_events: Option<mpsc::Sender<RuntimeEvent>>,
    household_committed_evidence: Arc<StdMutex<Option<NativeHouseholdCommittedEvidenceV1>>>,
}

fn allocate_household_mode_generation_v1() -> io::Result<HouseholdModeGenerationV1> {
    let value = NEXT_HOUSEHOLD_MODE_GENERATION_V1
        .fetch_update(AtomicOrdering::SeqCst, AtomicOrdering::SeqCst, |current| {
            current.checked_add(1)
        })
        .map_err(|_| io::Error::other("household mode generation authority is exhausted"))?;
    HouseholdModeGenerationV1::new(value)
        .map_err(|_| io::Error::other("household mode generation authority is exhausted"))
}

fn household_account_binding_digest_v1(
    household: &HouseholdSession,
) -> io::Result<HouseholdAccountBindingDigestV1> {
    let digest = canonical_sha256_v1(&json!({
        "account_id": household.account().as_str(),
        "contract": "heyfood.tui.household-account-binding.v1"
    }))
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "household account binding failed",
        )
    })?;
    Ok(HouseholdAccountBindingDigestV1::from_bytes(
        *digest.as_bytes(),
    ))
}

const fn household_presentation_mode_v1(
    mode: ProfilePresentationModeV1,
) -> Option<HouseholdPresentationModeV1> {
    match mode {
        ProfilePresentationModeV1::LegacyCompatibility => None,
        ProfilePresentationModeV1::NativeEnabled => {
            Some(HouseholdPresentationModeV1::NativeEnabled)
        }
        ProfilePresentationModeV1::NativeRollbackReadOnly => {
            Some(HouseholdPresentationModeV1::NativeRollbackReadOnly)
        }
    }
}

impl InteractiveTurnDriver {
    pub fn new(
        service: Arc<dyn ServicePort>,
        ensure_session: Arc<EnsureSession>,
        session: SessionSnapshot,
    ) -> io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("heyfood-turn")
            .build()?;
        Ok(Self {
            runtime,
            service,
            interactive_service: None,
            audio_capture: None,
            authorization_scope: Arc::from(""),
            local_state: None,
            household_session: None,
            profile_presentation_mode: ProfilePresentationModeV1::LegacyCompatibility,
            startup_notice: None,
            startup_onboarding: false,
            session_provider: None,
            ensure_session,
            session: Arc::new(Mutex::new(session)),
            continuity: Arc::new(Mutex::new(InteractiveContinuity::default())),
            turns: Vec::new(),
            signals: None,
            household_driver_binding: None,
            household_runtime_events: None,
            household_committed_evidence: Arc::new(StdMutex::new(None)),
        })
    }

    pub fn new_http(
        service: Arc<HttpService>,
        ensure_session: Arc<EnsureSession>,
        session: SessionSnapshot,
        authorization_scope: impl Into<Arc<str>>,
    ) -> io::Result<Self> {
        let conversational_service: Arc<dyn ServicePort> = service.clone();
        let mut driver = Self::new(conversational_service, ensure_session, session)?;
        driver.interactive_service = Some(service);
        driver.authorization_scope = authorization_scope.into();
        Ok(driver)
    }

    #[must_use]
    pub fn with_local_state(mut self, state: Option<ImportedPythonState>) -> Self {
        self.local_state = state.map(Arc::new);
        self
    }

    #[must_use]
    pub fn with_household_session(mut self, session: Option<HouseholdSession>) -> Self {
        self.household_session = session;
        self
    }

    #[must_use]
    pub fn with_profile_presentation_mode(mut self, mode: ProfilePresentationModeV1) -> Self {
        self.profile_presentation_mode = mode;
        self
    }

    #[must_use]
    pub fn with_startup_notice(mut self, notice: Option<String>) -> Self {
        self.startup_notice = notice;
        self
    }

    #[must_use]
    pub fn with_startup_onboarding(mut self, enabled: bool) -> Self {
        self.startup_onboarding = enabled;
        self
    }

    #[must_use]
    pub fn with_session_provider(mut self, provider: Arc<dyn InteractiveSessionProvider>) -> Self {
        self.session_provider = Some(provider);
        self
    }

    #[must_use]
    pub fn with_audio_capture(mut self, audio_capture: Arc<dyn AudioCapturePort>) -> Self {
        self.audio_capture = Some(audio_capture);
        self
    }

    fn native_voice_available(&self) -> bool {
        self.audio_capture
            .as_ref()
            .is_some_and(|capture| capture.available())
    }

    fn reap_finished(&mut self) {
        self.turns.retain(|turn| !turn.task.is_finished());
    }

    fn has_blocking_interactive_work(&self) -> bool {
        self.turns
            .iter()
            .any(OwnedInteractiveTurn::blocks_interactive_followup)
    }

    fn start_conversational_input(
        &mut self,
        operation_id: u64,
        prompt: String,
        confirmation: Option<AgentConfirmationCommandWire>,
        presented_household_context: Option<PresentedHouseholdContextV1>,
        runtime_events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        self.reap_finished();
        if self.has_blocking_interactive_work() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "a conversational turn is already active",
            ));
        }

        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let followup_ready = Arc::new(AtomicBool::new(false));
        let task_followup_ready = followup_ready.clone();
        let fallback_service = self.service.clone();
        let fallback_http_service = self.interactive_service.clone();
        let fallback_ensure_session = self.ensure_session.clone();
        let fallback_authorization_scope = self.authorization_scope.clone();
        let session_provider = self.session_provider.clone();
        let session = self.session.clone();
        let continuity = self.continuity.clone();
        let local_state = self.local_state.clone();
        let household_session = self.household_session.clone();
        let task = self.runtime.spawn(async move {
            let initial_snapshot = session.lock().await.clone();
            let preflight = preflight_native_hosted_scope(
                &initial_snapshot,
                household_session.as_ref(),
                &task_cancellation,
            )
            .await;
            let outcome = match preflight {
                Ok(NativeHostedScopePreflightV1::Allowed) => None,
                Ok(NativeHostedScopePreflightV1::Cancelled) => {
                    Some(Ok(RunTurnOutcome::CancelledBeforeServerAcceptance))
                }
                Err(error) => Some(Err(TurnFailure::from_port_error(&error))),
            };
            if let Some(outcome) = outcome {
                let terminal_event = match outcome {
                    Ok(outcome) => RuntimeEvent::TurnFinished {
                        operation_id,
                        outcome,
                    },
                    Err(failure) => RuntimeEvent::TurnFailed {
                        operation_id,
                        failure,
                    },
                };
                task_followup_ready.store(true, AtomicOrdering::Release);
                let _ = runtime_events.send(terminal_event).await;
                return;
            }
            let prepared = prepare_hosted_interactive_operation(
                session_provider,
                fallback_service,
                fallback_http_service,
                fallback_ensure_session,
                fallback_authorization_scope,
                session.clone(),
                household_session.as_ref(),
                task_cancellation.child_token(),
            )
            .await;
            let outcome = match prepared {
                Ok(prepared) => {
                    let PreparedHostedInteractiveOperation {
                        operation: prepared,
                        hosted_context,
                    } = prepared;
                    run_interactive_turn(
                        operation_id,
                        prompt,
                        confirmation,
                        prepared.service,
                        prepared.ensure_session,
                        session,
                        continuity,
                        prepared.http_service,
                        local_state,
                        hosted_context,
                        presented_household_context,
                        task_cancellation,
                        runtime_events.clone(),
                    )
                    .await
                }
                Err(InteractivePreparationError::CancelledBeforeDispatch) => {
                    Ok(RunTurnOutcome::CancelledBeforeServerAcceptance)
                }
                Err(InteractivePreparationError::Failed(failure)) => Err(failure),
            };
            let terminal_event = match outcome {
                Ok(outcome) => RuntimeEvent::TurnFinished {
                    operation_id,
                    outcome,
                },
                Err(failure) => RuntimeEvent::TurnFailed {
                    operation_id,
                    failure,
                },
            };
            task_followup_ready.store(true, AtomicOrdering::Release);
            let _ = runtime_events.send(terminal_event).await;
        });
        self.turns.push(OwnedInteractiveTurn {
            operation_id,
            household_binding: None,
            followup_ready,
            cancellation,
            stop: None,
            task,
        });
        Ok(())
    }
}

fn household_operation_matches_driver_v1(
    driver: NativeHouseholdDriverBindingV1,
    binding: &HouseholdOperationBindingV1,
) -> bool {
    driver.mode == HouseholdPresentationModeV1::NativeEnabled
        && driver.session_mode_generation == binding.session_mode_generation()
        && driver.account_binding_digest == binding.account_binding_digest()
}

fn household_load_matches_driver_v1(
    driver: NativeHouseholdDriverBindingV1,
    session_mode_generation: HouseholdModeGenerationV1,
    account_binding_digest: HouseholdAccountBindingDigestV1,
) -> bool {
    driver.session_mode_generation == session_mode_generation
        && driver.account_binding_digest == account_binding_digest
}

async fn load_bound_native_household_v1(
    household: &HouseholdSession,
    session: &Arc<Mutex<SessionSnapshot>>,
    cancellation: CancellationToken,
) -> Result<HouseholdLoad, HouseholdManagementFailureV1> {
    if cancellation.is_cancelled() {
        return Err(HouseholdManagementFailureV1::Unavailable);
    }
    let snapshot = session.lock().await.clone();
    if &snapshot.credentials.account_id != household.account() {
        return Err(HouseholdManagementFailureV1::AccountChanged);
    }
    household
        .load_required(cancellation)
        .await
        .map_err(|error| {
            if error.code == "household_account_mismatch" {
                HouseholdManagementFailureV1::AccountChanged
            } else {
                HouseholdManagementFailureV1::Unavailable
            }
        })
}

fn household_management_presentation_v1(
    load: &HouseholdLoad,
) -> Result<Vec<HouseholdMemberPresentationV1>, HouseholdManagementFailureV1> {
    let owner_profile_revision = load
        .state
        .profiles
        .iter()
        .find(|profile| profile.subject == HouseholdSubjectId::self_())
        .map(|profile| profile.profile_revision);
    let mut members = Vec::with_capacity(load.state.members.len().saturating_add(1));
    members.push(
        HouseholdMemberPresentationV1::new(
            HouseholdSubjectId::self_(),
            "Me",
            RelationshipV1::Self_,
            HouseholdLifecycleV1::Active,
            load.state.owner.profile_state,
            owner_profile_revision,
        )
        .map_err(|_| HouseholdManagementFailureV1::MalformedPresentation)?,
    );
    for member in &load.state.members {
        let subject = HouseholdSubjectId::member(member.member_id.clone());
        let profile_revision = load
            .state
            .profiles
            .iter()
            .find(|profile| profile.subject == subject)
            .map(|profile| profile.profile_revision);
        members.push(
            HouseholdMemberPresentationV1::new(
                subject,
                member.display_name.as_str(),
                member.relationship,
                member.lifecycle,
                member.profile_state,
                profile_revision,
            )
            .map_err(|_| HouseholdManagementFailureV1::MalformedPresentation)?,
        );
    }
    Ok(members)
}

fn bounded_active_household_label_v1(
    load: &HouseholdLoad,
) -> Result<String, HouseholdManagementFailureV1> {
    match &load.state.active_scope {
        HouseholdScope::Subject(HouseholdSubjectId::Self_) => Ok("Me".to_owned()),
        HouseholdScope::Subject(HouseholdSubjectId::Member(member_id)) => load
            .state
            .members
            .iter()
            .find(|member| &member.member_id == member_id)
            .map(|member| member.display_name.as_str().to_owned())
            .ok_or(HouseholdManagementFailureV1::StateChanged),
        HouseholdScope::Everyone => Ok("Everyone".to_owned()),
    }
}

fn household_mutation_failure_v1(error: &PortError) -> HouseholdMutationFailureV1 {
    if error.outcome_uncertain || error.code == "household_mutation_outcome_uncertain" {
        return HouseholdMutationFailureV1::OutcomeUncertain;
    }
    match error.code {
        "household_member_create_cancelled"
        | "household_member_profile_cancelled"
        | "household_scope_selection_cancelled"
        | "household_operation_cancelled"
        | "household_commit_cancelled" => HouseholdMutationFailureV1::BeforeCommitCancelled,
        "household_revision_conflict" | "household_revision_stale" => {
            HouseholdMutationFailureV1::StaleRevision
        }
        "household_member_conflict_resolution_required" | "profile_conflicted" => {
            HouseholdMutationFailureV1::ConflictResolutionRequired
        }
        "household_member_relationship_invalid"
        | "household_member_capacity"
        | "household_member_conflict"
        | "household_member_unknown"
        | "household_member_archived"
        | "household_member_profile_ineligible"
        | "household_member_profile_revision_conflict"
        | "household_profile_capacity"
        | "household_subject_unknown"
        | "household_subject_archived"
        | "profile_incomplete"
        | "household_everyone_requires_two_eligible_subjects"
        | "household_state_invalid" => HouseholdMutationFailureV1::Ineligible,
        _ => HouseholdMutationFailureV1::Unavailable,
    }
}

fn affected_household_subject_v1(scope: &HouseholdScope) -> Option<HouseholdSubjectId> {
    match scope {
        HouseholdScope::Subject(subject) => Some(subject.clone()),
        HouseholdScope::Everyone => None,
    }
}

fn selected_household_target_matches_v1(
    target: &SelectedHouseholdTargetV1,
    scope: &HouseholdScope,
    active_label: &str,
) -> bool {
    match (target, scope) {
        (SelectedHouseholdTargetV1::Me, HouseholdScope::Subject(HouseholdSubjectId::Self_)) => {
            active_label == "Me"
        }
        (
            SelectedHouseholdTargetV1::Member {
                member_id,
                display_label,
            },
            HouseholdScope::Subject(HouseholdSubjectId::Member(selected)),
        ) => member_id == selected && display_label.as_str() == active_label,
        (SelectedHouseholdTargetV1::Everyone, HouseholdScope::Everyone) => {
            active_label == "Everyone"
        }
        _ => false,
    }
}

impl QualifiedTurnDriver for InteractiveTurnDriver {
    fn start_session(&mut self, runtime_events: mpsc::Sender<RuntimeEvent>) -> io::Result<()> {
        if self.signals.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "interactive signal forwarding is already active",
            ));
        }
        let presentation_mode = household_presentation_mode_v1(self.profile_presentation_mode);
        let native_mode = presentation_mode.is_some();
        if native_mode != self.household_session.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "interactive household mode and live repository session disagree",
            ));
        }
        self.household_driver_binding = match (self.household_session.as_ref(), presentation_mode) {
            (Some(household), Some(mode)) => {
                let snapshot = self
                    .runtime
                    .block_on(async { self.session.lock().await.clone() });
                if &snapshot.credentials.account_id != household.account() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "interactive credential and household accounts disagree",
                    ));
                }
                Some(NativeHouseholdDriverBindingV1 {
                    session_mode_generation: allocate_household_mode_generation_v1()?,
                    account_binding_digest: household_account_binding_digest_v1(household)?,
                    mode,
                })
            }
            (None, None) => None,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "interactive household mode binding is unavailable",
                ));
            }
        };
        self.household_committed_evidence
            .lock()
            .map_err(|_| io::Error::other("household commit evidence is unavailable"))?
            .take();
        self.household_runtime_events = self
            .household_driver_binding
            .is_some()
            .then(|| runtime_events.clone());
        let mut source = self
            .runtime
            .block_on(async { NativeSignalSource::install() })
            .map_err(|error| io::Error::other(error.to_string()))?;
        if let Some(message) = self.startup_notice.take() {
            runtime_events
                .try_send(RuntimeEvent::Notice { message })
                .map_err(io::Error::other)?;
        }
        runtime_events
            .try_send(RuntimeEvent::ProfilePresentationMode(
                self.profile_presentation_mode,
            ))
            .map_err(io::Error::other)?;
        if let Some(binding) = self.household_driver_binding {
            runtime_events
                .try_send(RuntimeEvent::HouseholdGenerationReadyV1 {
                    session_mode_generation: binding.session_mode_generation,
                    mode: binding.mode,
                    account_binding_digest: binding.account_binding_digest,
                })
                .map_err(io::Error::other)?;
        }
        if self.startup_onboarding {
            let event = match self.profile_presentation_mode {
                ProfilePresentationModeV1::NativeEnabled => {
                    RuntimeEvent::BeginNativeOwnerOnboarding {
                        message: "Let's build your dietary profile. Nothing is sent until you review and save it.".into(),
                    }
                }
                ProfilePresentationModeV1::LegacyCompatibility => RuntimeEvent::BeginOnboarding {
                    message: "Let's build your dietary profile. Nothing is sent until you review and save it.".into(),
                },
                ProfilePresentationModeV1::NativeRollbackReadOnly => RuntimeEvent::Notice {
                    message:
                        "Dietary onboarding is unavailable in native rollback read-only mode."
                            .into(),
                },
            };
            runtime_events.try_send(event).map_err(io::Error::other)?;
            self.startup_onboarding = false;
        }
        let voice_availability = interactive_voice_availability(
            self.native_voice_available(),
            &self.authorization_scope,
        );
        runtime_events
            .try_send(RuntimeEvent::VoiceAvailability(voice_availability))
            .map_err(io::Error::other)?;
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let task = self.runtime.spawn(async move {
            let signal = tokio::select! {
                signal = source.next() => signal,
                () = task_cancellation.cancelled() => None,
            };
            if let Some(signal) = signal {
                let reason = match signal {
                    SignalEvent::Interrupt => ExitReason::Interrupt,
                    SignalEvent::Terminate | SignalEvent::ConsoleClose => ExitReason::Terminate,
                    SignalEvent::Hangup => ExitReason::Hangup,
                };
                let _ = runtime_events
                    .send(RuntimeEvent::ExternalSignal(reason))
                    .await;
            }
            source
                .shutdown(Duration::from_secs(1))
                .await
                .map_err(|error| io::Error::other(error.to_string()))
        });
        self.signals = Some(OwnedSignalForwarder { cancellation, task });
        Ok(())
    }

    fn start_turn(
        &mut self,
        operation_id: u64,
        prompt: String,
        presented_household_context: Option<PresentedHouseholdContextV1>,
        runtime_events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        self.start_conversational_input(
            operation_id,
            prompt,
            None,
            presented_household_context,
            runtime_events,
        )
    }

    fn start_confirmation(
        &mut self,
        operation_id: u64,
        command: AgentConfirmationCommandWire,
        presented_household_context: Option<PresentedHouseholdContextV1>,
        runtime_events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        self.start_conversational_input(
            operation_id,
            String::new(),
            Some(command),
            presented_household_context,
            runtime_events,
        )
    }

    fn start_household_management_load(
        &mut self,
        operation_id: HouseholdOperationIdV1,
        session_mode_generation: HouseholdModeGenerationV1,
        expected_account_binding_digest: HouseholdAccountBindingDigestV1,
        reducer_correlation: HouseholdReducerCorrelationV1,
        purpose: HouseholdManagementLoadPurposeV1,
        runtime_events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        self.reap_finished();
        if self
            .turns
            .iter()
            .any(OwnedInteractiveTurn::blocks_interactive_followup)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "interactive work is already active",
            ));
        }
        let Some(driver_binding) = self.household_driver_binding else {
            runtime_events
                .try_send(RuntimeEvent::HouseholdManagementLoadFailedV1 {
                    operation_id,
                    session_mode_generation,
                    reducer_correlation,
                    purpose,
                    account_binding_digest: expected_account_binding_digest,
                    observed_household_revision: None,
                    reason: HouseholdManagementFailureV1::Unavailable,
                })
                .map_err(io::Error::other)?;
            return Ok(());
        };
        if !household_load_matches_driver_v1(
            driver_binding,
            session_mode_generation,
            expected_account_binding_digest,
        ) || (driver_binding.mode == HouseholdPresentationModeV1::NativeRollbackReadOnly
            && !matches!(
                purpose,
                HouseholdManagementLoadPurposeV1::Bootstrap
                    | HouseholdManagementLoadPurposeV1::Panel
            ))
        {
            runtime_events
                .try_send(RuntimeEvent::HouseholdManagementLoadFailedV1 {
                    operation_id,
                    session_mode_generation,
                    reducer_correlation,
                    purpose,
                    account_binding_digest: expected_account_binding_digest,
                    observed_household_revision: None,
                    reason: HouseholdManagementFailureV1::ModeChanged,
                })
                .map_err(io::Error::other)?;
            return Ok(());
        }
        let Some(household) = self.household_session.clone() else {
            runtime_events
                .try_send(RuntimeEvent::HouseholdManagementLoadFailedV1 {
                    operation_id,
                    session_mode_generation,
                    reducer_correlation,
                    purpose,
                    account_binding_digest: expected_account_binding_digest,
                    observed_household_revision: None,
                    reason: HouseholdManagementFailureV1::Unavailable,
                })
                .map_err(io::Error::other)?;
            return Ok(());
        };
        let session = self.session.clone();
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let completed_household_operation = Arc::new(AtomicBool::new(false));
        let task_completed_household_operation = completed_household_operation.clone();
        let task = self.runtime.spawn(async move {
            let event =
                match load_bound_native_household_v1(&household, &session, task_cancellation).await
                {
                    Ok(load) => match household_management_presentation_v1(&load) {
                        Ok(members) => RuntimeEvent::HouseholdManagementLoadedV1 {
                            operation_id,
                            session_mode_generation,
                            reducer_correlation,
                            purpose,
                            account_binding_digest: expected_account_binding_digest,
                            household_revision: load.state.revision,
                            active_scope: load.state.active_scope,
                            members,
                        },
                        Err(reason) => RuntimeEvent::HouseholdManagementLoadFailedV1 {
                            operation_id,
                            session_mode_generation,
                            reducer_correlation,
                            purpose,
                            account_binding_digest: expected_account_binding_digest,
                            observed_household_revision: Some(load.state.revision),
                            reason,
                        },
                    },
                    Err(reason) => RuntimeEvent::HouseholdManagementLoadFailedV1 {
                        operation_id,
                        session_mode_generation,
                        reducer_correlation,
                        purpose,
                        account_binding_digest: expected_account_binding_digest,
                        observed_household_revision: None,
                        reason,
                    },
                };
            task_completed_household_operation.store(true, AtomicOrdering::Release);
            let _ = runtime_events.send(event).await;
        });
        self.turns.push(OwnedInteractiveTurn {
            operation_id: operation_id.get(),
            household_binding: None,
            followup_ready: completed_household_operation,
            cancellation,
            stop: None,
            task,
        });
        Ok(())
    }

    fn start_household_member_create(
        &mut self,
        binding: HouseholdOperationBindingV1,
        draft: BoundedHouseholdMemberDraftV1,
        profile: OnboardingProfileInput,
        runtime_events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        self.reap_finished();
        if self
            .turns
            .iter()
            .any(OwnedInteractiveTurn::blocks_interactive_followup)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "interactive work is already active",
            ));
        }
        self.household_committed_evidence
            .lock()
            .map_err(|_| io::Error::other("household commit evidence is unavailable"))?
            .take();
        let kind = HouseholdMutationKindV1::CreateMember;
        let Some(driver_binding) = self.household_driver_binding else {
            runtime_events
                .try_send(RuntimeEvent::HouseholdMutationFailedV1 {
                    binding,
                    kind,
                    affected_subject: None,
                    observed_household_revision: None,
                    reason: HouseholdMutationFailureV1::Unavailable,
                })
                .map_err(io::Error::other)?;
            return Ok(());
        };
        if !household_operation_matches_driver_v1(driver_binding, &binding) {
            runtime_events
                .try_send(RuntimeEvent::HouseholdMutationFailedV1 {
                    binding,
                    kind,
                    affected_subject: None,
                    observed_household_revision: None,
                    reason: HouseholdMutationFailureV1::Unavailable,
                })
                .map_err(io::Error::other)?;
            return Ok(());
        }
        let Some(household) = self.household_session.clone() else {
            runtime_events
                .try_send(RuntimeEvent::HouseholdMutationFailedV1 {
                    binding,
                    kind,
                    affected_subject: None,
                    observed_household_revision: None,
                    reason: HouseholdMutationFailureV1::Unavailable,
                })
                .map_err(io::Error::other)?;
            return Ok(());
        };
        let operation_id = binding.operation_id().get();
        let owned_binding = binding.clone();
        let session = self.session.clone();
        let committed_evidence = self.household_committed_evidence.clone();
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let completed_household_operation = Arc::new(AtomicBool::new(false));
        let task_completed_household_operation = completed_household_operation.clone();
        let task = self.runtime.spawn(async move {
            let mut observed_revision = None;
            let outcome = async {
                if task_cancellation.is_cancelled() {
                    return Err(HouseholdMutationFailureV1::BeforeCommitCancelled);
                }
                let before = load_bound_native_household_v1(
                    &household,
                    &session,
                    task_cancellation.child_token(),
                )
                .await
                .map_err(|_| HouseholdMutationFailureV1::Unavailable)?;
                observed_revision = Some(before.state.revision);
                if before.state.revision != binding.expected_household_revision() {
                    return Err(HouseholdMutationFailureV1::StaleRevision);
                }
                let display_name = DisplayName::parse(draft.display_name().to_owned())
                    .map_err(|_| HouseholdMutationFailureV1::Ineligible)?;
                let age_evidence = match draft.age_evidence() {
                    HouseholdAgeEvidenceInputV1::Under13 => NativeMemberAgeEvidenceV1::Under13,
                    HouseholdAgeEvidenceInputV1::Age13To17 => NativeMemberAgeEvidenceV1::Age13_17,
                    HouseholdAgeEvidenceInputV1::Age18Plus => NativeMemberAgeEvidenceV1::Age18Plus,
                    HouseholdAgeEvidenceInputV1::Unknown => NativeMemberAgeEvidenceV1::Unknown,
                };
                let created = household
                    .create_member_with_declared_profile(
                        CreateMemberWithDeclaredProfileV1 {
                            expected_household_revision: binding.expected_household_revision(),
                            display_name,
                            relationship: draft.relationship(),
                            age_evidence,
                            declared_profile: profile,
                        },
                        task_cancellation,
                    )
                    .await
                    .map_err(|error| household_mutation_failure_v1(&error))?;
                let readback =
                    load_bound_native_household_v1(&household, &session, CancellationToken::new())
                        .await
                        .map_err(|_| HouseholdMutationFailureV1::OutcomeUncertain)?;
                let subject = HouseholdSubjectId::member(created.member_id.clone());
                let active_label = bounded_active_household_label_v1(&readback)
                    .map_err(|_| HouseholdMutationFailureV1::OutcomeUncertain)?;
                if readback.state.revision != created.resulting_household_revision
                    || binding.expected_household_revision().checked_next().ok()
                        != Some(created.resulting_household_revision)
                    || readback.state.active_scope != created.active_scope
                    || created.active_scope != HouseholdScope::Subject(subject.clone())
                    || created.display_label.as_str() != active_label
                    || !readback.state.members.iter().any(|member| {
                        member.member_id == created.member_id
                            && member.profile_state == HouseholdProfileStateV1::LocalOnly
                    })
                    || !readback
                        .state
                        .profiles
                        .iter()
                        .any(|candidate| candidate.subject == subject)
                {
                    return Err(HouseholdMutationFailureV1::OutcomeUncertain);
                }
                *committed_evidence
                    .lock()
                    .map_err(|_| HouseholdMutationFailureV1::OutcomeUncertain)? =
                    Some(NativeHouseholdCommittedEvidenceV1 {
                        binding: binding.clone(),
                        resulting_household_revision: created.resulting_household_revision,
                        affected_subject: Some(subject.clone()),
                        active_scope: created.active_scope.clone(),
                        bounded_active_label: active_label.clone(),
                    });
                Ok(RuntimeEvent::HouseholdMutationCommittedV1 {
                    binding: binding.clone(),
                    kind,
                    resulting_household_revision: created.resulting_household_revision,
                    affected_subject: Some(subject),
                    active_scope: created.active_scope,
                    bounded_active_label: active_label,
                })
            }
            .await;
            let event = outcome.unwrap_or_else(|reason| RuntimeEvent::HouseholdMutationFailedV1 {
                binding,
                kind,
                affected_subject: None,
                observed_household_revision: observed_revision,
                reason,
            });
            task_completed_household_operation.store(true, AtomicOrdering::Release);
            let _ = runtime_events.send(event).await;
        });
        self.turns.push(OwnedInteractiveTurn {
            operation_id,
            household_binding: Some(owned_binding),
            followup_ready: completed_household_operation,
            cancellation,
            stop: None,
            task,
        });
        Ok(())
    }

    fn start_household_member_profile_save(
        &mut self,
        binding: HouseholdOperationBindingV1,
        subject: HouseholdSubjectId,
        expected_profile_revision: Option<ProfileRevision>,
        profile: OnboardingProfileInput,
        runtime_events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        self.reap_finished();
        if self
            .turns
            .iter()
            .any(OwnedInteractiveTurn::blocks_interactive_followup)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "interactive work is already active",
            ));
        }
        self.household_committed_evidence
            .lock()
            .map_err(|_| io::Error::other("household commit evidence is unavailable"))?
            .take();
        let kind = HouseholdMutationKindV1::SaveMemberProfile;
        let affected_subject = Some(subject.clone());
        let Some(member_id) = subject.as_member().cloned() else {
            runtime_events
                .try_send(RuntimeEvent::HouseholdMutationFailedV1 {
                    binding,
                    kind,
                    affected_subject,
                    observed_household_revision: None,
                    reason: HouseholdMutationFailureV1::Ineligible,
                })
                .map_err(io::Error::other)?;
            return Ok(());
        };
        let Some(driver_binding) = self.household_driver_binding else {
            runtime_events
                .try_send(RuntimeEvent::HouseholdMutationFailedV1 {
                    binding,
                    kind,
                    affected_subject,
                    observed_household_revision: None,
                    reason: HouseholdMutationFailureV1::Unavailable,
                })
                .map_err(io::Error::other)?;
            return Ok(());
        };
        if !household_operation_matches_driver_v1(driver_binding, &binding) {
            runtime_events
                .try_send(RuntimeEvent::HouseholdMutationFailedV1 {
                    binding,
                    kind,
                    affected_subject,
                    observed_household_revision: None,
                    reason: HouseholdMutationFailureV1::Unavailable,
                })
                .map_err(io::Error::other)?;
            return Ok(());
        }
        let Some(household) = self.household_session.clone() else {
            runtime_events
                .try_send(RuntimeEvent::HouseholdMutationFailedV1 {
                    binding,
                    kind,
                    affected_subject,
                    observed_household_revision: None,
                    reason: HouseholdMutationFailureV1::Unavailable,
                })
                .map_err(io::Error::other)?;
            return Ok(());
        };
        let operation_id = binding.operation_id().get();
        let owned_binding = binding.clone();
        let session = self.session.clone();
        let committed_evidence = self.household_committed_evidence.clone();
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let completed_household_operation = Arc::new(AtomicBool::new(false));
        let task_completed_household_operation = completed_household_operation.clone();
        let task = self.runtime.spawn(async move {
            let mut observed_revision = None;
            let outcome = async {
                if task_cancellation.is_cancelled() {
                    return Err(HouseholdMutationFailureV1::BeforeCommitCancelled);
                }
                let before = load_bound_native_household_v1(
                    &household,
                    &session,
                    task_cancellation.child_token(),
                )
                .await
                .map_err(|_| HouseholdMutationFailureV1::Unavailable)?;
                observed_revision = Some(before.state.revision);
                if before.state.revision != binding.expected_household_revision() {
                    return Err(HouseholdMutationFailureV1::StaleRevision);
                }
                let saved = household
                    .save_member_declared_profile(
                        SaveMemberDeclaredProfileV1 {
                            expected_household_revision: binding.expected_household_revision(),
                            member_id: member_id.clone(),
                            expected_profile_revision,
                            declared_profile: profile,
                        },
                        task_cancellation,
                    )
                    .await
                    .map_err(|error| household_mutation_failure_v1(&error))?;
                let readback =
                    load_bound_native_household_v1(&household, &session, CancellationToken::new())
                        .await
                        .map_err(|_| HouseholdMutationFailureV1::OutcomeUncertain)?;
                let active_label = bounded_active_household_label_v1(&readback)
                    .map_err(|_| HouseholdMutationFailureV1::OutcomeUncertain)?;
                if saved.member_id != member_id
                    || readback.state.revision != saved.resulting_household_revision
                    || binding.expected_household_revision().checked_next().ok()
                        != Some(saved.resulting_household_revision)
                    || readback.state.active_scope != saved.active_scope
                    || !readback.state.members.iter().any(|member| {
                        member.member_id == member_id
                            && member.profile_state == HouseholdProfileStateV1::LocalOnly
                    })
                    || !readback.state.profiles.iter().any(|candidate| {
                        candidate.subject == subject
                            && candidate.profile_revision == saved.profile_revision
                    })
                {
                    return Err(HouseholdMutationFailureV1::OutcomeUncertain);
                }
                *committed_evidence
                    .lock()
                    .map_err(|_| HouseholdMutationFailureV1::OutcomeUncertain)? =
                    Some(NativeHouseholdCommittedEvidenceV1 {
                        binding: binding.clone(),
                        resulting_household_revision: saved.resulting_household_revision,
                        affected_subject: Some(subject.clone()),
                        active_scope: saved.active_scope.clone(),
                        bounded_active_label: active_label.clone(),
                    });
                Ok(RuntimeEvent::HouseholdMutationCommittedV1 {
                    binding: binding.clone(),
                    kind,
                    resulting_household_revision: saved.resulting_household_revision,
                    affected_subject: Some(subject.clone()),
                    active_scope: saved.active_scope,
                    bounded_active_label: active_label,
                })
            }
            .await;
            let event = outcome.unwrap_or_else(|reason| RuntimeEvent::HouseholdMutationFailedV1 {
                binding,
                kind,
                affected_subject: Some(subject),
                observed_household_revision: observed_revision,
                reason,
            });
            task_completed_household_operation.store(true, AtomicOrdering::Release);
            let _ = runtime_events.send(event).await;
        });
        self.turns.push(OwnedInteractiveTurn {
            operation_id,
            household_binding: Some(owned_binding),
            followup_ready: completed_household_operation,
            cancellation,
            stop: None,
            task,
        });
        Ok(())
    }

    fn start_native_household_scope_selection(
        &mut self,
        binding: HouseholdOperationBindingV1,
        scope: HouseholdScope,
        runtime_events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        self.reap_finished();
        if self
            .turns
            .iter()
            .any(OwnedInteractiveTurn::blocks_interactive_followup)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "interactive work is already active",
            ));
        }
        self.household_committed_evidence
            .lock()
            .map_err(|_| io::Error::other("household commit evidence is unavailable"))?
            .take();
        let kind = HouseholdMutationKindV1::SelectScope;
        let affected_subject = affected_household_subject_v1(&scope);
        let Some(driver_binding) = self.household_driver_binding else {
            runtime_events
                .try_send(RuntimeEvent::HouseholdMutationFailedV1 {
                    binding,
                    kind,
                    affected_subject,
                    observed_household_revision: None,
                    reason: HouseholdMutationFailureV1::Unavailable,
                })
                .map_err(io::Error::other)?;
            return Ok(());
        };
        if !household_operation_matches_driver_v1(driver_binding, &binding) {
            runtime_events
                .try_send(RuntimeEvent::HouseholdMutationFailedV1 {
                    binding,
                    kind,
                    affected_subject,
                    observed_household_revision: None,
                    reason: HouseholdMutationFailureV1::Unavailable,
                })
                .map_err(io::Error::other)?;
            return Ok(());
        }
        let Some(household) = self.household_session.clone() else {
            runtime_events
                .try_send(RuntimeEvent::HouseholdMutationFailedV1 {
                    binding,
                    kind,
                    affected_subject,
                    observed_household_revision: None,
                    reason: HouseholdMutationFailureV1::Unavailable,
                })
                .map_err(io::Error::other)?;
            return Ok(());
        };
        let operation_id = binding.operation_id().get();
        let owned_binding = binding.clone();
        let session = self.session.clone();
        let committed_evidence = self.household_committed_evidence.clone();
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let completed_household_operation = Arc::new(AtomicBool::new(false));
        let task_completed_household_operation = completed_household_operation.clone();
        let task = self.runtime.spawn(async move {
            let mut observed_revision = None;
            let outcome = async {
                if task_cancellation.is_cancelled() {
                    return Err(HouseholdMutationFailureV1::BeforeCommitCancelled);
                }
                let before = load_bound_native_household_v1(
                    &household,
                    &session,
                    task_cancellation.child_token(),
                )
                .await
                .map_err(|_| HouseholdMutationFailureV1::Unavailable)?;
                observed_revision = Some(before.state.revision);
                if before.state.revision != binding.expected_household_revision() {
                    return Err(HouseholdMutationFailureV1::StaleRevision);
                }
                let selected = household
                    .select_scope(
                        binding.expected_household_revision(),
                        scope.clone(),
                        task_cancellation,
                    )
                    .await
                    .map_err(|error| household_mutation_failure_v1(&error))?;
                let readback =
                    load_bound_native_household_v1(&household, &session, CancellationToken::new())
                        .await
                        .map_err(|_| HouseholdMutationFailureV1::OutcomeUncertain)?;
                let active_label = bounded_active_household_label_v1(&readback)
                    .map_err(|_| HouseholdMutationFailureV1::OutcomeUncertain)?;
                if readback.state.revision != selected.resulting_household_revision
                    || binding.expected_household_revision().checked_next().ok()
                        != Some(selected.resulting_household_revision)
                    || readback.state.active_scope != selected.active_scope
                    || selected.active_scope != scope
                    || !selected_household_target_matches_v1(
                        &selected.target,
                        &scope,
                        &active_label,
                    )
                {
                    return Err(HouseholdMutationFailureV1::OutcomeUncertain);
                }
                *committed_evidence
                    .lock()
                    .map_err(|_| HouseholdMutationFailureV1::OutcomeUncertain)? =
                    Some(NativeHouseholdCommittedEvidenceV1 {
                        binding: binding.clone(),
                        resulting_household_revision: selected.resulting_household_revision,
                        affected_subject: affected_household_subject_v1(&scope),
                        active_scope: selected.active_scope.clone(),
                        bounded_active_label: active_label.clone(),
                    });
                Ok(RuntimeEvent::HouseholdMutationCommittedV1 {
                    binding: binding.clone(),
                    kind,
                    resulting_household_revision: selected.resulting_household_revision,
                    affected_subject: affected_household_subject_v1(&scope),
                    active_scope: selected.active_scope,
                    bounded_active_label: active_label,
                })
            }
            .await;
            let event = outcome.unwrap_or_else(|reason| RuntimeEvent::HouseholdMutationFailedV1 {
                binding,
                kind,
                affected_subject: affected_household_subject_v1(&scope),
                observed_household_revision: observed_revision,
                reason,
            });
            task_completed_household_operation.store(true, AtomicOrdering::Release);
            let _ = runtime_events.send(event).await;
        });
        self.turns.push(OwnedInteractiveTurn {
            operation_id,
            household_binding: Some(owned_binding),
            followup_ready: completed_household_operation,
            cancellation,
            stop: None,
            task,
        });
        Ok(())
    }

    fn start_household_context_apply(
        &mut self,
        binding: HouseholdOperationBindingV1,
        resulting_household_revision: HouseholdRevision,
        affected_subject: Option<HouseholdSubjectId>,
        active_scope: HouseholdScope,
        bounded_active_label: String,
        runtime_events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        self.reap_finished();
        if self.turns.iter().any(|turn| {
            turn.blocks_interactive_followup()
                && turn
                    .household_binding
                    .as_ref()
                    .is_none_or(|candidate| candidate != &binding)
        }) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "interactive work is already active",
            ));
        }
        let Some(driver_binding) = self.household_driver_binding else {
            runtime_events
                .try_send(RuntimeEvent::HouseholdContextApplyFailedV1 {
                    binding,
                    resulting_household_revision,
                    reason: HouseholdContextApplyFailureV1::Unavailable,
                })
                .map_err(io::Error::other)?;
            return Ok(());
        };
        if !household_operation_matches_driver_v1(driver_binding, &binding) {
            runtime_events
                .try_send(RuntimeEvent::HouseholdContextApplyFailedV1 {
                    binding,
                    resulting_household_revision,
                    reason: HouseholdContextApplyFailureV1::ModeChanged,
                })
                .map_err(io::Error::other)?;
            return Ok(());
        }
        let has_exact_committed_evidence = self
            .household_committed_evidence
            .lock()
            .map_err(|_| io::Error::other("household commit evidence is unavailable"))?
            .as_ref()
            .is_some_and(|evidence| {
                evidence.matches_apply(
                    &binding,
                    resulting_household_revision,
                    &affected_subject,
                    &active_scope,
                    &bounded_active_label,
                )
            });
        if !has_exact_committed_evidence {
            runtime_events
                .try_send(RuntimeEvent::HouseholdContextApplyFailedV1 {
                    binding,
                    resulting_household_revision,
                    reason: HouseholdContextApplyFailureV1::StateChanged,
                })
                .map_err(io::Error::other)?;
            return Ok(());
        }
        let Some(household) = self.household_session.clone() else {
            runtime_events
                .try_send(RuntimeEvent::HouseholdContextApplyFailedV1 {
                    binding,
                    resulting_household_revision,
                    reason: HouseholdContextApplyFailureV1::Unavailable,
                })
                .map_err(io::Error::other)?;
            return Ok(());
        };
        let operation_id = binding.operation_id().get();
        let owned_binding = binding.clone();
        let session = self.session.clone();
        let continuity = self.continuity.clone();
        let committed_evidence = self.household_committed_evidence.clone();
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let completed_household_operation = Arc::new(AtomicBool::new(false));
        let task_completed_household_operation = completed_household_operation.clone();
        let task = self.runtime.spawn(async move {
            let event =
                match load_bound_native_household_v1(&household, &session, task_cancellation).await
                {
                    Err(HouseholdManagementFailureV1::AccountChanged) => {
                        RuntimeEvent::HouseholdContextApplyFailedV1 {
                            binding,
                            resulting_household_revision,
                            reason: HouseholdContextApplyFailureV1::AccountChanged,
                        }
                    }
                    Err(_) => RuntimeEvent::HouseholdContextApplyFailedV1 {
                        binding,
                        resulting_household_revision,
                        reason: HouseholdContextApplyFailureV1::Unavailable,
                    },
                    Ok(load) => {
                        let computed_label = bounded_active_household_label_v1(&load);
                        let affected_is_current = affected_subject.as_ref().is_none_or(|subject| {
                            matches!(subject, HouseholdSubjectId::Self_)
                                || subject.as_member().is_some_and(|member_id| {
                                    load.state.members.iter().any(|member| {
                                        &member.member_id == member_id
                                            && member.lifecycle == HouseholdLifecycleV1::Active
                                    })
                                })
                        });
                        if load.state.revision != resulting_household_revision
                            || load.state.active_scope != active_scope
                            || computed_label.as_deref() != Ok(bounded_active_label.as_str())
                            || !affected_is_current
                        {
                            RuntimeEvent::HouseholdContextApplyFailedV1 {
                                binding,
                                resulting_household_revision,
                                reason: HouseholdContextApplyFailureV1::StateChanged,
                            }
                        } else {
                            let evidence_consumed = committed_evidence
                                .lock()
                                .ok()
                                .and_then(|mut evidence| {
                                    evidence
                                        .as_ref()
                                        .is_some_and(|candidate| {
                                            candidate.matches_apply(
                                                &binding,
                                                resulting_household_revision,
                                                &affected_subject,
                                                &active_scope,
                                                &bounded_active_label,
                                            )
                                        })
                                        .then(|| evidence.take())
                                })
                                .flatten()
                                .is_some();
                            if evidence_consumed {
                                let mut continuity = continuity.lock().await;
                                continuity.clear_conversation();
                                continuity.household_scope = None;
                                continuity.native_context =
                                    Some(NativeHouseholdContextBindingV1::from_load(&load));
                                RuntimeEvent::HouseholdContextAppliedV1 {
                                    binding,
                                    resulting_household_revision,
                                    active_scope: load.state.active_scope,
                                    bounded_active_label: computed_label
                                        .expect("checked active household label"),
                                }
                            } else {
                                RuntimeEvent::HouseholdContextApplyFailedV1 {
                                    binding,
                                    resulting_household_revision,
                                    reason: HouseholdContextApplyFailureV1::StateChanged,
                                }
                            }
                        }
                    }
                };
            task_completed_household_operation.store(true, AtomicOrdering::Release);
            let _ = runtime_events.send(event).await;
        });
        self.turns.push(OwnedInteractiveTurn {
            operation_id,
            household_binding: Some(owned_binding),
            followup_ready: completed_household_operation,
            cancellation,
            stop: None,
            task,
        });
        Ok(())
    }

    fn cancel_household_operation(
        &mut self,
        binding: &HouseholdOperationBindingV1,
    ) -> io::Result<()> {
        for turn in &self.turns {
            if turn.household_binding.as_ref() == Some(binding) {
                turn.cancellation.cancel();
            }
        }
        Ok(())
    }

    fn start_household_scope(
        &mut self,
        operation_id: u64,
        selector: String,
        runtime_events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        self.reap_finished();
        if self.has_blocking_interactive_work() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "interactive work is already active",
            ));
        }
        let local_state = self.local_state.clone();
        let continuity = self.continuity.clone();
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let followup_ready = Arc::new(AtomicBool::new(false));
        let task_followup_ready = followup_ready.clone();
        let task = self.runtime.spawn(async move {
            let result = local_state
                .as_deref()
                .ok_or_else(|| {
                    "No household context is saved yet. Complete dietary onboarding first."
                        .to_owned()
                })
                .and_then(|state| resolve_household_scope_with_label(state, &selector));
            let event = match result {
                Ok((identifier, label)) => {
                    if task_cancellation.is_cancelled() {
                        RuntimeEvent::HouseholdScopeFailed {
                            operation_id,
                            message: "Household target change was cancelled.".into(),
                        }
                    } else {
                        let mut continuity = continuity.lock().await;
                        continuity.household_scope = Some(identifier);
                        continuity.conversation_id = None;
                        RuntimeEvent::HouseholdScopeReady {
                            operation_id,
                            label,
                        }
                    }
                }
                Err(message) => RuntimeEvent::HouseholdScopeFailed {
                    operation_id,
                    message,
                },
            };
            task_followup_ready.store(true, AtomicOrdering::Release);
            let _ = runtime_events.send(event).await;
        });
        self.turns.push(OwnedInteractiveTurn {
            operation_id,
            household_binding: None,
            followup_ready,
            cancellation,
            stop: None,
            task,
        });
        Ok(())
    }

    fn start_panel(
        &mut self,
        operation_id: u64,
        panel: PanelRequest,
        runtime_events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        self.reap_finished();
        if self.has_blocking_interactive_work() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "interactive work is already active",
            ));
        }
        if self.interactive_service.is_none() && self.session_provider.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "interactive panels require the authenticated HTTP adapter",
            ));
        }
        let fallback_http_service = self.interactive_service.clone();
        let fallback_service = self.service.clone();
        let fallback_ensure_session = self.ensure_session.clone();
        let fallback_authorization_scope = self.authorization_scope.clone();
        let session_provider = self.session_provider.clone();
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let followup_ready = Arc::new(AtomicBool::new(false));
        let task_followup_ready = followup_ready.clone();
        let session = self.session.clone();
        let native_voice_available = self.native_voice_available();
        runtime_events
            .try_send(RuntimeEvent::VoiceAvailability(
                interactive_voice_availability(
                    native_voice_available,
                    &fallback_authorization_scope,
                ),
            ))
            .map_err(io::Error::other)?;
        let environment = InteractivePanelEnvironment {
            local_state: self.local_state.clone(),
            native_voice_available,
        };
        let task = self.runtime.spawn(async move {
            let prepared = prepare_interactive_operation(
                session_provider,
                fallback_service,
                fallback_http_service,
                fallback_ensure_session,
                fallback_authorization_scope,
                session.clone(),
                task_cancellation.child_token(),
            )
            .await;
            let result = match prepared {
                Ok(prepared) => match prepared.http_service {
                    Some(service) => {
                        run_interactive_panel(
                            panel,
                            service,
                            prepared.ensure_session,
                            session,
                            &prepared.authorization_scope,
                            environment,
                            task_cancellation.clone(),
                        )
                        .await
                    }
                    None => {
                        Err("Interactive panels require the authenticated HTTP adapter.".to_owned())
                    }
                },
                Err(InteractivePreparationError::CancelledBeforeDispatch) => {
                    Err("Operation cancelled.".into())
                }
                Err(InteractivePreparationError::Failed(failure)) => {
                    Err(interactive_preparation_failure_message(failure))
                }
            };
            let event = match result {
                Ok(body) => RuntimeEvent::PanelReady {
                    operation_id,
                    panel,
                    body,
                },
                Err(_) if task_cancellation.is_cancelled() => RuntimeEvent::TurnFinished {
                    operation_id,
                    outcome: RunTurnOutcome::CancelledBeforeServerAcceptance,
                },
                Err(message) => RuntimeEvent::PanelFailed {
                    operation_id,
                    panel,
                    message,
                },
            };
            task_followup_ready.store(true, AtomicOrdering::Release);
            let _ = runtime_events.send(event).await;
        });
        self.turns.push(OwnedInteractiveTurn {
            operation_id,
            household_binding: None,
            followup_ready,
            cancellation,
            stop: None,
            task,
        });
        Ok(())
    }

    fn start_owner_profile_actions(
        &mut self,
        operation_id: u64,
        _purpose: OwnerProfileActionLoadPurposeV1,
        runtime_events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        self.reap_finished();
        if self.has_blocking_interactive_work() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "interactive work is already active",
            ));
        }
        let native_household = match self.profile_presentation_mode {
            ProfilePresentationModeV1::NativeEnabled => {
                let Some(household) = self.household_session.clone() else {
                    runtime_events
                        .try_send(RuntimeEvent::ProfileActionsLoaded {
                            operation_id,
                            loaded: ProfileActionsLoadedV1::NativeActions(
                                unavailable_native_owner_actions_v1(
                                    OwnerProfileRetryUnavailableReasonV1::ModeOrAccountIneligible,
                                ),
                            ),
                        })
                        .map_err(io::Error::other)?;
                    return Ok(());
                };
                Some(household)
            }
            ProfilePresentationModeV1::NativeRollbackReadOnly => {
                runtime_events
                    .try_send(RuntimeEvent::ProfileActionsLoaded {
                        operation_id,
                        loaded: ProfileActionsLoadedV1::NativeActions(
                            unavailable_native_owner_actions_v1(
                                OwnerProfileRetryUnavailableReasonV1::ModeOrAccountIneligible,
                            ),
                        ),
                    })
                    .map_err(io::Error::other)?;
                return Ok(());
            }
            ProfilePresentationModeV1::LegacyCompatibility => None,
        };
        if self.interactive_service.is_none() && self.session_provider.is_none() {
            if native_household.is_some() {
                runtime_events
                    .try_send(RuntimeEvent::ProfileActionsLoaded {
                        operation_id,
                        loaded: ProfileActionsLoadedV1::NativeActions(
                            unavailable_native_owner_actions_v1(
                                OwnerProfileRetryUnavailableReasonV1::ModeOrAccountIneligible,
                            ),
                        ),
                    })
                    .map_err(io::Error::other)?;
                return Ok(());
            }
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "interactive Profile actions require the authenticated HTTP adapter",
            ));
        }
        let fallback_http_service = self.interactive_service.clone();
        let fallback_service = self.service.clone();
        let fallback_ensure_session = self.ensure_session.clone();
        let fallback_authorization_scope = self.authorization_scope.clone();
        let session_provider = self.session_provider.clone();
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let followup_ready = Arc::new(AtomicBool::new(false));
        let task_followup_ready = followup_ready.clone();
        let session = self.session.clone();
        let environment = InteractivePanelEnvironment {
            local_state: self.local_state.clone(),
            native_voice_available: self.native_voice_available(),
        };
        let task = self.runtime.spawn(async move {
            let prepared = prepare_interactive_operation(
                session_provider,
                fallback_service,
                fallback_http_service,
                fallback_ensure_session,
                fallback_authorization_scope,
                session.clone(),
                task_cancellation.child_token(),
            )
            .await;
            let loaded = if let Some(household) = native_household {
                let actions = match prepared {
                    Ok(prepared) => match prepared.http_service {
                        Some(service) => {
                            load_native_owner_actions_v1(
                                &household,
                                &service,
                                &prepared.ensure_session,
                                &session,
                                &prepared.authorization_scope,
                                task_cancellation,
                            )
                            .await
                        }
                        None => unavailable_native_owner_actions_v1(
                            OwnerProfileRetryUnavailableReasonV1::ModeOrAccountIneligible,
                        ),
                    },
                    Err(_) => unavailable_native_owner_actions_v1(
                        OwnerProfileRetryUnavailableReasonV1::ModeOrAccountIneligible,
                    ),
                };
                ProfileActionsLoadedV1::NativeActions(actions)
            } else {
                let body = match prepared {
                    Ok(prepared) => match prepared.http_service {
                        Some(service) => run_interactive_panel(
                            PanelRequest::Profile,
                            service,
                            prepared.ensure_session,
                            session,
                            &prepared.authorization_scope,
                            environment,
                            task_cancellation.clone(),
                        )
                        .await
                        .unwrap_or_else(|message| {
                            if task_cancellation.is_cancelled() {
                                "Dietary profile request was cancelled.".into()
                            } else {
                                message
                            }
                        }),
                        None => {
                            "Interactive Profile actions require the authenticated HTTP adapter."
                                .into()
                        }
                    },
                    Err(InteractivePreparationError::CancelledBeforeDispatch) => {
                        "Dietary profile request was cancelled.".into()
                    }
                    Err(InteractivePreparationError::Failed(failure)) => {
                        interactive_preparation_failure_message(failure)
                    }
                };
                ProfileActionsLoadedV1::LegacyPanel { body }
            };
            task_followup_ready.store(true, AtomicOrdering::Release);
            let _ = runtime_events
                .send(RuntimeEvent::ProfileActionsLoaded {
                    operation_id,
                    loaded,
                })
                .await;
        });
        self.turns.push(OwnedInteractiveTurn {
            operation_id,
            household_binding: None,
            followup_ready,
            cancellation,
            stop: None,
            task,
        });
        Ok(())
    }

    fn start_owner_profile_consent(
        &mut self,
        operation_id: u64,
        runtime_events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        if matches!(
            self.profile_presentation_mode,
            ProfilePresentationModeV1::NativeEnabled
        ) {
            self.reap_finished();
            if self.has_blocking_interactive_work() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "interactive work is already active",
                ));
            }
            let Some(household) = self.household_session.clone() else {
                runtime_events
                    .try_send(RuntimeEvent::ProfileConsentFinished {
                        operation_id,
                        result: Err(ProfileConsentFailureV1::Unavailable),
                    })
                    .map_err(io::Error::other)?;
                return Ok(());
            };
            if self.interactive_service.is_none() && self.session_provider.is_none() {
                runtime_events
                    .try_send(RuntimeEvent::ProfileConsentFinished {
                        operation_id,
                        result: Err(ProfileConsentFailureV1::Unavailable),
                    })
                    .map_err(io::Error::other)?;
                return Ok(());
            }
            let cancellation = CancellationToken::new();
            let task_cancellation = cancellation.clone();
            let followup_ready = Arc::new(AtomicBool::new(false));
            let task_followup_ready = followup_ready.clone();
            let fallback_service = self.service.clone();
            let fallback_http_service = self.interactive_service.clone();
            let fallback_ensure_session = self.ensure_session.clone();
            let fallback_authorization_scope = self.authorization_scope.clone();
            let session_provider = self.session_provider.clone();
            let session = self.session.clone();
            let task = self.runtime.spawn(async move {
                let prepared = prepare_interactive_operation(
                    session_provider,
                    fallback_service,
                    fallback_http_service,
                    fallback_ensure_session,
                    fallback_authorization_scope,
                    session.clone(),
                    task_cancellation.child_token(),
                )
                .await;
                let result = match prepared {
                    Ok(prepared) => match prepared.http_service {
                        Some(service) => {
                            grant_native_owner_consent_v1(
                                &household,
                                &service,
                                &prepared.ensure_session,
                                &session,
                                &prepared.authorization_scope,
                                task_cancellation,
                            )
                            .await
                        }
                        None => Err(ProfileConsentFailureV1::Unavailable),
                    },
                    Err(InteractivePreparationError::CancelledBeforeDispatch) => {
                        Err(ProfileConsentFailureV1::Cancelled)
                    }
                    Err(InteractivePreparationError::Failed(_)) => {
                        Err(ProfileConsentFailureV1::Unavailable)
                    }
                };
                task_followup_ready.store(true, AtomicOrdering::Release);
                let _ = runtime_events
                    .send(RuntimeEvent::ProfileConsentFinished {
                        operation_id,
                        result,
                    })
                    .await;
            });
            self.turns.push(OwnedInteractiveTurn {
                operation_id,
                household_binding: None,
                followup_ready,
                cancellation,
                stop: None,
                task,
            });
            return Ok(());
        }
        runtime_events
            .try_send(RuntimeEvent::ProfileConsentFinished {
                operation_id,
                result: Err(ProfileConsentFailureV1::Unavailable),
            })
            .map_err(io::Error::other)
    }

    fn start_owner_profile_retry(
        &mut self,
        operation_id: u64,
        action: OwnerProfileRetryActionV1,
        intent: OwnerSyncIntentHandleV1,
        runtime_events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        if matches!(
            self.profile_presentation_mode,
            ProfilePresentationModeV1::NativeEnabled
        ) {
            self.reap_finished();
            if self.has_blocking_interactive_work() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "interactive work is already active",
                ));
            }
            let Some(household) = self.household_session.clone() else {
                runtime_events
                    .try_send(RuntimeEvent::ProfileRetrySyncFinished {
                        operation_id,
                        outcome: ProfileRetrySyncFinishedV1::Unavailable {
                            reason: OwnerProfileRetryUnavailableReasonV1::ModeOrAccountIneligible,
                        },
                    })
                    .map_err(io::Error::other)?;
                return Ok(());
            };
            if self.interactive_service.is_none() && self.session_provider.is_none() {
                runtime_events
                    .try_send(RuntimeEvent::ProfileRetrySyncFinished {
                        operation_id,
                        outcome: ProfileRetrySyncFinishedV1::Unavailable {
                            reason: OwnerProfileRetryUnavailableReasonV1::ModeOrAccountIneligible,
                        },
                    })
                    .map_err(io::Error::other)?;
                return Ok(());
            }
            let cancellation = CancellationToken::new();
            let task_cancellation = cancellation.clone();
            let followup_ready = Arc::new(AtomicBool::new(false));
            let task_followup_ready = followup_ready.clone();
            let fallback_service = self.service.clone();
            let fallback_http_service = self.interactive_service.clone();
            let fallback_ensure_session = self.ensure_session.clone();
            let fallback_authorization_scope = self.authorization_scope.clone();
            let session_provider = self.session_provider.clone();
            let session = self.session.clone();
            let task = self.runtime.spawn(async move {
                let prepared = prepare_interactive_operation(
                    session_provider,
                    fallback_service,
                    fallback_http_service,
                    fallback_ensure_session,
                    fallback_authorization_scope,
                    session.clone(),
                    task_cancellation.child_token(),
                )
                .await;
                let outcome = match prepared {
                    Ok(prepared) => match prepared.http_service {
                        Some(service) => {
                            retry_native_owner_sync_v1(
                                &household,
                                &service,
                                &prepared.ensure_session,
                                &session,
                                &prepared.authorization_scope,
                                NativeOwnerRetryRequestV1 {
                                    action,
                                    expected: intent,
                                },
                                task_cancellation,
                            )
                            .await
                        }
                        None => ProfileRetrySyncFinishedV1::Unavailable {
                            reason: OwnerProfileRetryUnavailableReasonV1::ModeOrAccountIneligible,
                        },
                    },
                    Err(InteractivePreparationError::CancelledBeforeDispatch) => {
                        ProfileRetrySyncFinishedV1::Interrupted
                    }
                    Err(InteractivePreparationError::Failed(_)) => {
                        ProfileRetrySyncFinishedV1::Unavailable {
                            reason: OwnerProfileRetryUnavailableReasonV1::ModeOrAccountIneligible,
                        }
                    }
                };
                task_followup_ready.store(true, AtomicOrdering::Release);
                let _ = runtime_events
                    .send(RuntimeEvent::ProfileRetrySyncFinished {
                        operation_id,
                        outcome,
                    })
                    .await;
            });
            self.turns.push(OwnedInteractiveTurn {
                operation_id,
                household_binding: None,
                followup_ready,
                cancellation,
                stop: None,
                task,
            });
            return Ok(());
        }
        runtime_events
            .try_send(RuntimeEvent::ProfileRetrySyncFinished {
                operation_id,
                outcome: ProfileRetrySyncFinishedV1::Unavailable {
                    reason: OwnerProfileRetryUnavailableReasonV1::ModeOrAccountIneligible,
                },
            })
            .map_err(io::Error::other)
    }

    fn start_onboarding(
        &mut self,
        operation_id: u64,
        profile: OnboardingProfileInput,
        runtime_events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        self.reap_finished();
        if self.has_blocking_interactive_work() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "interactive work is already active",
            ));
        }
        let native_household = match self.profile_presentation_mode {
            ProfilePresentationModeV1::NativeEnabled => {
                let Some(household) = self.household_session.clone() else {
                    runtime_events
                        .try_send(RuntimeEvent::OnboardingFailed {
                            operation_id,
                            message:
                                "Native owner onboarding is unavailable until the local household session is ready."
                                    .into(),
                        })
                        .map_err(io::Error::other)?;
                    return Ok(());
                };
                Some(household)
            }
            ProfilePresentationModeV1::NativeRollbackReadOnly => {
                runtime_events
                    .try_send(RuntimeEvent::OnboardingFailed {
                        operation_id,
                        message: "Native household state is read-only in rollback mode; profile changes are unavailable."
                            .into(),
                    })
                    .map_err(io::Error::other)?;
                return Ok(());
            }
            ProfilePresentationModeV1::LegacyCompatibility => None,
        };
        if self.interactive_service.is_none() && self.session_provider.is_none() {
            if native_household.is_some() {
                runtime_events
                    .try_send(RuntimeEvent::OnboardingFailed {
                        operation_id,
                        message: "Native owner onboarding requires the authenticated HTTP adapter."
                            .into(),
                    })
                    .map_err(io::Error::other)?;
                return Ok(());
            }
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "dietary onboarding requires the authenticated HTTP adapter",
            ));
        }
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let followup_ready = Arc::new(AtomicBool::new(false));
        let task_followup_ready = followup_ready.clone();
        let fallback_service = self.service.clone();
        let fallback_http_service = self.interactive_service.clone();
        let fallback_ensure_session = self.ensure_session.clone();
        let fallback_authorization_scope = self.authorization_scope.clone();
        let session_provider = self.session_provider.clone();
        let session = self.session.clone();
        let task = self.runtime.spawn(async move {
            let prepared = prepare_interactive_operation(
                session_provider,
                fallback_service,
                fallback_http_service,
                fallback_ensure_session,
                fallback_authorization_scope,
                session.clone(),
                task_cancellation.child_token(),
            )
            .await;
            let event = if let Some(household) = native_household {
                let result = match prepared {
                    Ok(prepared) => match prepared.http_service {
                        Some(service) => {
                            run_native_owner_onboarding_v1(
                                profile,
                                household,
                                service,
                                prepared.ensure_session,
                                session,
                                &prepared.authorization_scope,
                                task_cancellation,
                            )
                            .await
                        }
                        None => Err(OnboardingOperationError::Failed(
                            "Native owner onboarding requires the authenticated HTTP adapter."
                                .into(),
                        )),
                    },
                    Err(InteractivePreparationError::CancelledBeforeDispatch) => {
                        Err(OnboardingOperationError::Cancelled(
                            RunTurnOutcome::CancelledBeforeServerAcceptance,
                        ))
                    }
                    Err(InteractivePreparationError::Failed(failure)) => {
                        Err(OnboardingOperationError::Failed(
                            interactive_preparation_failure_message(failure),
                        ))
                    }
                };
                match result {
                    Ok(status) => RuntimeEvent::NativeOwnerOnboardingSaved {
                        operation_id,
                        status,
                    },
                    Err(OnboardingOperationError::Failed(message)) => {
                        RuntimeEvent::OnboardingFailed {
                            operation_id,
                            message,
                        }
                    }
                    Err(OnboardingOperationError::Cancelled(outcome)) => {
                        RuntimeEvent::OnboardingCancelled {
                            operation_id,
                            outcome,
                        }
                    }
                }
            } else {
                let result = match prepared {
                    Ok(prepared) => match prepared.http_service {
                        Some(service) => {
                            run_interactive_onboarding(
                                profile,
                                service,
                                prepared.ensure_session,
                                session,
                                &prepared.authorization_scope,
                                task_cancellation,
                            )
                            .await
                        }
                        None => Err(OnboardingOperationError::Failed(
                            "Dietary onboarding requires the authenticated HTTP adapter.".into(),
                        )),
                    },
                    Err(InteractivePreparationError::CancelledBeforeDispatch) => {
                        Err(OnboardingOperationError::Cancelled(
                            RunTurnOutcome::CancelledBeforeServerAcceptance,
                        ))
                    }
                    Err(InteractivePreparationError::Failed(failure)) => {
                        Err(OnboardingOperationError::Failed(
                            interactive_preparation_failure_message(failure),
                        ))
                    }
                };
                match result {
                    Ok(()) => RuntimeEvent::OnboardingSaved { operation_id },
                    Err(OnboardingOperationError::Failed(message)) => {
                        RuntimeEvent::OnboardingFailed {
                            operation_id,
                            message,
                        }
                    }
                    Err(OnboardingOperationError::Cancelled(outcome)) => {
                        RuntimeEvent::OnboardingCancelled {
                            operation_id,
                            outcome,
                        }
                    }
                }
            };
            task_followup_ready.store(true, AtomicOrdering::Release);
            let _ = runtime_events.send(event).await;
        });
        self.turns.push(OwnedInteractiveTurn {
            operation_id,
            household_binding: None,
            followup_ready,
            cancellation,
            stop: None,
            task,
        });
        Ok(())
    }

    fn start_voice(
        &mut self,
        operation_id: u64,
        runtime_events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        self.reap_finished();
        if self.has_blocking_interactive_work() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "interactive work is already active",
            ));
        }
        let Some(audio_capture) = self.audio_capture.clone() else {
            runtime_events
                .try_send(RuntimeEvent::VoiceFailed {
                    operation_id,
                    message: "Native microphone capture is unavailable in this artifact. Nothing was recorded or submitted.".into(),
                })
                .map_err(io::Error::other)?;
            return Ok(());
        };
        if !audio_capture.available() {
            runtime_events
                .try_send(RuntimeEvent::VoiceFailed {
                    operation_id,
                    message: "No compatible microphone input device is currently available. Nothing was recorded or submitted.".into(),
                })
                .map_err(io::Error::other)?;
            return Ok(());
        }
        if self.interactive_service.is_none() && self.session_provider.is_none() {
            runtime_events
                .try_send(RuntimeEvent::VoiceFailed {
                    operation_id,
                    message: "Voice transcription requires the authenticated HTTP adapter. Nothing was recorded or submitted.".into(),
                })
                .map_err(io::Error::other)?;
            return Ok(());
        }
        let stop = CancellationToken::new();
        let cancellation = CancellationToken::new();
        let task_stop = stop.clone();
        let task_cancellation = cancellation.clone();
        let followup_ready = Arc::new(AtomicBool::new(false));
        let task_followup_ready = followup_ready.clone();
        let fallback_service = self.service.clone();
        let fallback_http_service = self.interactive_service.clone();
        let fallback_ensure_session = self.ensure_session.clone();
        let fallback_authorization_scope = self.authorization_scope.clone();
        let session_provider = self.session_provider.clone();
        let session = self.session.clone();
        let household_session = self.household_session.clone();
        let task = self.runtime.spawn(async move {
            let initial_snapshot = session.lock().await.clone();
            let preflight_event = match preflight_native_hosted_scope(
                &initial_snapshot,
                household_session.as_ref(),
                &task_cancellation,
            )
            .await
            {
                Ok(NativeHostedScopePreflightV1::Allowed) => None,
                Ok(NativeHostedScopePreflightV1::Cancelled) => {
                    Some(RuntimeEvent::VoiceCancelled { operation_id })
                }
                Err(error) => Some(RuntimeEvent::VoiceFailed {
                    operation_id,
                    message: format!("{}: {}", error.code, error.message),
                }),
            };
            if let Some(event) = preflight_event {
                task_followup_ready.store(true, AtomicOrdering::Release);
                let _ = runtime_events.send(event).await;
                return;
            }
            let prepared = prepare_hosted_interactive_operation(
                session_provider,
                fallback_service,
                fallback_http_service,
                fallback_ensure_session,
                fallback_authorization_scope,
                session.clone(),
                household_session.as_ref(),
                task_cancellation.child_token(),
            )
            .await;
            let event = match prepared {
                Ok(prepared) => {
                    let PreparedHostedInteractiveOperation {
                        operation: prepared,
                        hosted_context,
                    } = prepared;
                    let availability =
                        interactive_voice_availability(true, &prepared.authorization_scope);
                    let _ = runtime_events
                        .send(RuntimeEvent::VoiceAvailability(availability))
                        .await;
                    if availability == VoiceAvailability::AuthorizationRequired {
                        RuntimeEvent::VoiceFailed {
                            operation_id,
                            message: "Additional authorization (audio:transcribe) is required. Exit the TUI and run `heyfood login`; no microphone was opened.".into(),
                        }
                    } else {
                        match prepared.http_service {
                            Some(service) => {
                                run_interactive_voice(
                                    operation_id,
                                    audio_capture,
                                    service,
                                    task_stop,
                                    task_cancellation,
                                    runtime_events.clone(),
                                    hosted_context,
                                )
                                .await
                            }
                            None => RuntimeEvent::VoiceFailed {
                                operation_id,
                                message: "Voice transcription requires the authenticated HTTP adapter. Nothing was recorded or submitted.".into(),
                            },
                        }
                    }
                }
                Err(InteractivePreparationError::CancelledBeforeDispatch) => {
                    RuntimeEvent::VoiceCancelled { operation_id }
                }
                Err(InteractivePreparationError::Failed(failure)) => {
                    RuntimeEvent::VoiceFailed {
                        operation_id,
                        message: interactive_preparation_failure_message(failure),
                    }
                }
            };
            task_followup_ready.store(true, AtomicOrdering::Release);
            let _ = runtime_events.send(event).await;
        });
        self.turns.push(OwnedInteractiveTurn {
            operation_id,
            household_binding: None,
            followup_ready,
            cancellation,
            stop: Some(stop),
            task,
        });
        Ok(())
    }

    fn stop_voice(&mut self, operation_id: u64) -> io::Result<()> {
        let turn = self
            .turns
            .iter()
            .find(|turn| turn.operation_id == operation_id)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "active voice input is missing")
            })?;
        let stop = turn.stop.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the active operation is not a voice recording",
            )
        })?;
        stop.cancel();
        Ok(())
    }

    fn cancel_voice(&mut self, operation_id: u64) -> io::Result<()> {
        self.cancel_turn(operation_id)
    }

    fn cancel_turn(&mut self, operation_id: u64) -> io::Result<()> {
        // Profile action loading and its retry reuse one operation ID. Search
        // newest-first so cancellation targets the retry after it is pushed.
        // A ready sender tail can still be visible while its terminal event is
        // backpressured; cancelling that token is a harmless successful no-op.
        let turn = self
            .turns
            .iter()
            .rev()
            .find(|turn| turn.operation_id == operation_id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "active turn is missing"))?;
        turn.cancellation.cancel();
        Ok(())
    }

    fn reset_conversation(&mut self) -> io::Result<()> {
        self.runtime.block_on(async {
            self.continuity.lock().await.clear_conversation();
        });
        Ok(())
    }

    fn shutdown_and_join(&mut self, timeout: Duration) -> io::Result<()> {
        self.household_committed_evidence
            .lock()
            .map_err(|_| io::Error::other("household commit evidence is unavailable"))?
            .take();
        if let Some(binding) = self.household_driver_binding.take()
            && let Some(events) = self.household_runtime_events.take()
        {
            let _ = events.try_send(RuntimeEvent::HouseholdGenerationInvalidatedV1 {
                session_mode_generation: binding.session_mode_generation,
            });
        }
        for turn in &self.turns {
            turn.cancellation.cancel();
        }
        let turns = std::mem::take(&mut self.turns);
        if let Some(signals) = &self.signals {
            signals.cancellation.cancel();
        }
        let signals = self.signals.take();
        self.runtime.block_on(async move {
            tokio::time::timeout(timeout, async move {
                for turn in turns {
                    turn.task.await.map_err(|error| {
                        io::Error::other(format!("turn supervisor task failed: {error}"))
                    })?;
                }
                if let Some(signals) = signals {
                    signals.task.await.map_err(|error| {
                        io::Error::other(format!("signal supervisor task failed: {error}"))
                    })??;
                }
                Ok(())
            })
            .await
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "turn supervisor exceeded its shutdown deadline",
                )
            })?
        })
    }
}

async fn run_interactive_voice(
    operation_id: u64,
    audio_capture: Arc<dyn AudioCapturePort>,
    service: Arc<HttpService>,
    stop: CancellationToken,
    cancellation: CancellationToken,
    runtime_events: mpsc::Sender<RuntimeEvent>,
    hosted_context: Option<AuthorizedHostedContextV1>,
) -> RuntimeEvent {
    let capture = audio_capture.capture(stop, cancellation.child_token());
    tokio::pin!(capture);
    let started = tokio::time::Instant::now();
    let mut elapsed =
        tokio::time::interval_at(started + Duration::from_secs(1), Duration::from_secs(1));
    elapsed.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let capture = loop {
        tokio::select! {
            result = &mut capture => break result,
            _ = elapsed.tick() => {
                let elapsed_event = RuntimeEvent::VoiceRecordingElapsed {
                        operation_id,
                        seconds: started.elapsed().as_secs(),
                    };
                match runtime_events.try_send(elapsed_event) {
                    Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        cancellation.cancel();
                        return RuntimeEvent::VoiceCancelled { operation_id };
                    }
                }
            }
        }
    };
    let capture = match capture {
        Ok(capture) => capture,
        Err(_) if cancellation.is_cancelled() => {
            return RuntimeEvent::VoiceCancelled { operation_id };
        }
        Err(error) if error.code == "voice_capture_cancelled" => {
            return RuntimeEvent::VoiceCancelled { operation_id };
        }
        Err(error) => {
            return RuntimeEvent::VoiceFailed {
                operation_id,
                message: terminal_safe_text(&error.message),
            };
        }
    };
    if cancellation.is_cancelled() {
        return RuntimeEvent::VoiceCancelled { operation_id };
    }
    if capture.truncated || capture.overflowed {
        return RuntimeEvent::VoiceFailed {
            operation_id,
            message: "The recording exceeded a native capture bound or lost audio samples, so it was discarded without transcription or submission.".into(),
        };
    }
    if capture.duration_millis == 0
        || !heyfood_core::transcription_sample_rate_supported(capture.sample_rate_hz)
    {
        return RuntimeEvent::VoiceFailed {
            operation_id,
            message: "The recording did not satisfy the transcription contract and was discarded."
                .into(),
        };
    }
    let transcription = service
        .transcribe_audio(
            &capture.wav_bytes,
            TranscriptionPurpose::Ask,
            None,
            OperationId::new(),
            cancellation.child_token(),
        )
        .await;
    // The exact native generation stays locked until capture serialization
    // and the transcription response complete.
    drop(hosted_context);
    match transcription {
        Ok(_) if cancellation.is_cancelled() => RuntimeEvent::VoiceCancelled { operation_id },
        Ok(transcription) => RuntimeEvent::VoiceTranscriptReady {
            operation_id,
            transcript: transcription.transcript().to_owned(),
        },
        Err(_) if cancellation.is_cancelled() => RuntimeEvent::VoiceCancelled { operation_id },
        Err(error)
            if matches!(
                error.code,
                "request_cancelled_before_dispatch"
                    | "request_cancelled_after_dispatch"
                    | "response_cancelled"
            ) =>
        {
            RuntimeEvent::VoiceCancelled { operation_id }
        }
        Err(error) => RuntimeEvent::VoiceFailed {
            operation_id,
            message: terminal_safe_text(&error.message),
        },
    }
}

enum InteractivePreparationError {
    CancelledBeforeDispatch,
    Failed(TurnFailure),
}

fn interactive_preparation_error_from_port(error: PortError) -> InteractivePreparationError {
    if error.code == "household_hosted_context_cancelled"
        || error.code == "household_load_cancelled"
    {
        InteractivePreparationError::CancelledBeforeDispatch
    } else {
        InteractivePreparationError::Failed(TurnFailure::from_port_error(&error))
    }
}

fn interactive_preparation_failure_message(failure: TurnFailure) -> String {
    match failure.kind {
        TurnFailureKind::AuthenticationRequired => {
            "Your hello.food sign-in expired. Exit heyfood, run `heyfood login`, then reopen the TUI. No operation was sent.".into()
        }
        TurnFailureKind::AuthenticationChanged => {
            "The connected hello.food account changed. Exit and reopen heyfood before continuing. No operation was sent.".into()
        }
        TurnFailureKind::DispatchOutcomeUnknown => {
            "Account authorization could not be reconciled safely. Exit heyfood and run `heyfood login` before trying again.".into()
        }
        TurnFailureKind::Inactivity
        | TurnFailureKind::StreamInterrupted
        | TurnFailureKind::Unavailable
        | TurnFailureKind::Internal => {
            "hey.food could not prepare this operation. Check your connection, then try again.".into()
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn prepare_hosted_interactive_operation(
    provider: Option<Arc<dyn InteractiveSessionProvider>>,
    fallback_service: Arc<dyn ServicePort>,
    fallback_http_service: Option<Arc<HttpService>>,
    fallback_ensure_session: Arc<EnsureSession>,
    fallback_authorization_scope: Arc<str>,
    session: Arc<Mutex<SessionSnapshot>>,
    household_session: Option<&HouseholdSession>,
    cancellation: CancellationToken,
) -> Result<PreparedHostedInteractiveOperation, InteractivePreparationError> {
    if cancellation.is_cancelled() {
        return Err(InteractivePreparationError::CancelledBeforeDispatch);
    }
    let hosted_context = if let Some(household) = household_session {
        let snapshot = session.lock().await.clone();
        if &snapshot.credentials.account_id != household.account() {
            return Err(InteractivePreparationError::Failed(
                TurnFailure::from_port_error(&PortError::new(
                    "household_account_mismatch",
                    "Native household context is bound to another account.",
                )),
            ));
        }
        Some({
            let authorized = household
                .acquire_authorized_hosted_context(cancellation.child_token())
                .await
                .map_err(interactive_preparation_error_from_port)?;
            // Validate that the exact canonical profile fits the deployed
            // request schema before provider/session refresh can perform
            // any network operation. A safety projection must never be
            // silently truncated to make it fit a transport bound.
            native_household_turn_context(&authorized).map_err(|error| {
                InteractivePreparationError::Failed(TurnFailure::from_port_error(&error))
            })?;
            authorized
        })
    } else {
        None
    };
    // `hosted_context` retains the native lifecycle/vault lock while the
    // provider performs channel/session credential preparation.
    let operation = prepare_interactive_operation(
        provider,
        fallback_service,
        fallback_http_service,
        fallback_ensure_session,
        fallback_authorization_scope,
        session,
        cancellation,
    )
    .await?;
    Ok(PreparedHostedInteractiveOperation {
        operation,
        hosted_context,
    })
}

#[allow(clippy::too_many_arguments)]
async fn prepare_interactive_operation(
    provider: Option<Arc<dyn InteractiveSessionProvider>>,
    fallback_service: Arc<dyn ServicePort>,
    fallback_http_service: Option<Arc<HttpService>>,
    fallback_ensure_session: Arc<EnsureSession>,
    fallback_authorization_scope: Arc<str>,
    session: Arc<Mutex<SessionSnapshot>>,
    cancellation: CancellationToken,
) -> Result<PreparedInteractiveOperation, InteractivePreparationError> {
    let Some(provider) = provider else {
        return Ok(PreparedInteractiveOperation {
            service: fallback_service,
            http_service: fallback_http_service,
            ensure_session: fallback_ensure_session,
            authorization_scope: fallback_authorization_scope,
        });
    };
    if cancellation.is_cancelled() {
        return Err(InteractivePreparationError::CancelledBeforeDispatch);
    }
    let prepared = provider
        .prepare(cancellation.child_token())
        .await
        .map_err(|error| {
            if error.code == "channel_refresh_cancelled_before_dispatch" {
                InteractivePreparationError::CancelledBeforeDispatch
            } else {
                InteractivePreparationError::Failed(turn_failure_from_one_shot_error(&error))
            }
        })?;
    {
        let mut current = session.lock().await;
        if current.credentials.account_id != prepared.snapshot.credentials.account_id {
            return Err(InteractivePreparationError::Failed(
                TurnFailure::from_port_error(&PortError::new(
                    "interactive_account_changed",
                    "the connected account changed while the TUI was open",
                )),
            ));
        }
        *current = prepared.snapshot;
    }
    Ok(PreparedInteractiveOperation {
        service: prepared.service,
        http_service: prepared.http_service,
        ensure_session: prepared.ensure_session,
        authorization_scope: prepared.authorization_scope,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeHostedScopePreflightV1 {
    Allowed,
    Cancelled,
}

async fn preflight_native_hosted_scope(
    snapshot: &SessionSnapshot,
    household_session: Option<&HouseholdSession>,
    cancellation: &CancellationToken,
) -> Result<NativeHostedScopePreflightV1, PortError> {
    let Some(household) = household_session else {
        return Ok(NativeHostedScopePreflightV1::Allowed);
    };
    if cancellation.is_cancelled() {
        return Ok(NativeHostedScopePreflightV1::Cancelled);
    }
    if &snapshot.credentials.account_id != household.account() {
        return Err(PortError::new(
            "household_account_mismatch",
            "Native household context is bound to another account.",
        ));
    }
    match household.load_required(cancellation.child_token()).await {
        Ok(_) => {}
        Err(error) if cancellation.is_cancelled() || error.code == "household_load_cancelled" => {
            return Ok(NativeHostedScopePreflightV1::Cancelled);
        }
        Err(_) => {
            return Err(PortError::new(
                "household_context_unavailable",
                "Native household context could not be loaded.",
            ));
        }
    }
    Ok(NativeHostedScopePreflightV1::Allowed)
}

fn native_household_turn_context(
    authorized: &AuthorizedHostedContextV1,
) -> Result<TurnContext, PortError> {
    let snapshot = authorized.snapshot();
    let state = &authorized.load().state;
    if snapshot.household_revision != state.revision {
        return Err(PortError::new(
            "household_hosted_context_invalid",
            "the authorized native context is detached from its retained Household generation",
        ));
    }
    // `_self` is intentionally presented as "Me" on the deployed wire. The
    // retained owner record remains the authority for state validation, while
    // this stable label preserves single-member compatibility and avoids
    // exposing a private account name as protocol identity.
    let owner_label = "Me";
    let household_wide = matches!(&snapshot.scope, HouseholdScope::Everyone);
    let mut seen = BTreeSet::new();
    let mut members = Vec::with_capacity(snapshot.subjects.len());
    for subject in &snapshot.subjects {
        validate_native_hosted_profile(&subject.effective_profile)?;
        let (identifier, label, relationship, birth_month) = match &subject.subject {
            HouseholdSubjectId::Self_ => ("_self", owner_label, "self", None),
            HouseholdSubjectId::Member(member_id) => {
                let member = state
                    .members
                    .iter()
                    .find(|member| {
                        &member.member_id == member_id
                            && member.lifecycle == HouseholdLifecycleV1::Active
                    })
                    .ok_or_else(|| {
                        PortError::new(
                            "household_hosted_context_invalid",
                            "the authorized native context contains an unknown or archived member",
                        )
                    })?;
                (
                    member.member_id.as_str(),
                    member.display_name.as_str(),
                    relationship_wire_name(member.relationship),
                    member
                        .age_evidence
                        .as_ref()
                        .and_then(|evidence| evidence.date_of_birth.as_ref())
                        .map(|date| &date.as_str()[..7]),
                )
            }
        };
        if !seen.insert(identifier.to_owned()) {
            return Err(PortError::new(
                "household_hosted_context_invalid",
                "the authorized native context contains duplicate Household subjects",
            ));
        }
        let mut context = dietary_context_for_identity(
            identifier,
            label,
            relationship,
            birth_month,
            &subject.effective_profile,
            (identifier != "_self").then_some(owner_label),
        );
        if household_wide {
            context.insert("member_id".into(), Value::String(identifier.to_owned()));
            context.insert("label".into(), Value::String(label.to_owned()));
        }
        members.push(Value::Object(context));
    }

    let (dietary, meal) = match &snapshot.scope {
        HouseholdScope::Subject(subject) => {
            if snapshot.subjects.len() != 1 || &snapshot.subjects[0].subject != subject {
                return Err(PortError::new(
                    "household_hosted_context_invalid",
                    "the authorized native subject context is not singular and exact",
                ));
            }
            let only = members.into_iter().next().ok_or_else(|| {
                PortError::new(
                    "household_hosted_context_invalid",
                    "the authorized native subject context is empty",
                )
            })?;
            let (identifier, label) = match subject {
                HouseholdSubjectId::Self_ => ("_self", owner_label),
                HouseholdSubjectId::Member(member_id) => {
                    let member = state
                        .members
                        .iter()
                        .find(|member| {
                            &member.member_id == member_id
                                && member.lifecycle == HouseholdLifecycleV1::Active
                        })
                        .ok_or_else(|| {
                            PortError::new(
                                "household_hosted_context_invalid",
                                "the selected native Household member is unavailable",
                            )
                        })?;
                    (member.member_id.as_str(), member.display_name.as_str())
                }
            };
            (
                only,
                json!({
                    "active_member_id": identifier,
                    "active_member_name": label,
                    "is_cook_mode": false
                }),
            )
        }
        HouseholdScope::Everyone => {
            if members.len() < 2 {
                return Err(PortError::new(
                    "household_hosted_context_invalid",
                    "the authorized Everyone context requires two eligible subjects",
                ));
            }
            (
                json!({"mode": "household", "members": members}),
                json!({
                    "active_member_id": "_self",
                    "active_member_name": owner_label,
                    "is_cook_mode": false
                }),
            )
        }
    };
    Ok(TurnContext {
        dietary: Some(dietary),
        meal: Some(meal),
        // Native household profiles are local-first. Sending top-level
        // household_scope would cause the deployed server to discard this
        // exact frozen dietary context and resolve server-shared profiles
        // instead. The explicit persisted /for selection remains represented
        // by the exact dietary projection plus active_member_id.
        household_scope: None,
        ..TurnContext::default()
    })
}

fn relationship_wire_name(relationship: RelationshipV1) -> &'static str {
    match relationship {
        RelationshipV1::Self_ => "self",
        RelationshipV1::Spouse => "spouse",
        RelationshipV1::Partner => "partner",
        RelationshipV1::Parent => "parent",
        RelationshipV1::Child => "child",
        RelationshipV1::Sibling => "sibling",
        RelationshipV1::Grandparent => "grandparent",
        RelationshipV1::Friend => "friend",
        RelationshipV1::Other => "other",
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_interactive_turn(
    operation_id: u64,
    prompt: String,
    confirmation: Option<AgentConfirmationCommandWire>,
    service: Arc<dyn ServicePort>,
    ensure_session: Arc<EnsureSession>,
    session: Arc<Mutex<SessionSnapshot>>,
    continuity: Arc<Mutex<InteractiveContinuity>>,
    context_service: Option<Arc<HttpService>>,
    local_state: Option<Arc<ImportedPythonState>>,
    hosted_context: Option<AuthorizedHostedContextV1>,
    presented_household_context: Option<PresentedHouseholdContextV1>,
    cancellation: CancellationToken,
    runtime_events: mpsc::Sender<RuntimeEvent>,
) -> Result<RunTurnOutcome, TurnFailure> {
    let native_context = hosted_context
        .as_ref()
        .map(NativeHouseholdContextBindingV1::from_authorized);
    let conversation_id = if let Some(native_context) = native_context.as_ref() {
        match continuity
            .lock()
            .await
            .conversation_for_native_context(native_context, presented_household_context.as_ref())
        {
            Ok(conversation_id) => conversation_id,
            Err(outcome) => return Ok(outcome),
        }
    } else {
        continuity.lock().await.conversation_id.clone()
    };
    let snapshot = session.lock().await.clone();
    let credentials = match ensure_session
        .execute(snapshot.clone(), cancellation.child_token())
        .await
        .map_err(|error| turn_failure_from_session_error(&error))?
    {
        EnsureSessionOutcome::Current(credentials) => credentials,
        EnsureSessionOutcome::Refreshed(credentials) => {
            let mut current = session.lock().await;
            current.credentials = credentials.clone();
            current.reconciliation_required = false;
            credentials
        }
        EnsureSessionOutcome::CancelledBeforeDispatch => {
            return Ok(RunTurnOutcome::CancelledBeforeServerAcceptance);
        }
    };

    if cancellation.is_cancelled() {
        return Ok(RunTurnOutcome::CancelledBeforeServerAcceptance);
    }
    let mut context = match hosted_context.as_ref() {
        Some(hosted_context) => native_household_turn_context(hosted_context)
            .map_err(|error| TurnFailure::from_port_error(&error))?,
        None => match (context_service, local_state) {
            (Some(service), Some(state)) => {
                let selector = continuity.lock().await.household_scope.clone();
                build_household_turn_context(
                    &service,
                    &credentials,
                    &state,
                    selector.as_deref(),
                    cancellation.child_token(),
                )
                .await
                .map_err(|error| turn_failure_from_one_shot_error(&error))?
            }
            _ => TurnContext::default(),
        },
    };
    context.confirmation = confirmation;
    let request = TurnRequest {
        prompt,
        conversation_id,
        context,
        refresh: RefreshPolicy::Never,
    };
    let accepted = service
        .open_turn(
            request,
            credentials,
            OperationId::new(),
            cancellation.child_token(),
        )
        .await;
    // The request has either been rejected before acceptance or accepted with
    // the exact revision-bound owner context. Later scope changes apply only
    // to subsequent turns.
    drop(hosted_context);
    let mut accepted = match accepted {
        Ok(accepted) => accepted,
        Err(error) if error.code == "converse_cancelled_before_dispatch" => {
            return Ok(RunTurnOutcome::CancelledBeforeServerAcceptance);
        }
        Err(error) if error.outcome_uncertain => {
            return Ok(RunTurnOutcome::CancelledAfterDispatchOutcomeUnknown);
        }
        Err(error) => return Err(TurnFailure::from_port_error(&error)),
    };

    loop {
        let next = accepted.events.next();
        let event = tokio::select! {
            () = cancellation.cancelled() => {
                let _ = accepted.events.close().await;
                return Ok(RunTurnOutcome::CancelledAfterServerAcceptance);
            }
            event = next => event.map_err(|error| TurnFailure::from_port_error(&error))?,
        };
        let Some(event) = event else {
            let _ = accepted.events.close().await;
            return Err(TurnFailure::from_port_error(&PortError::uncertain(
                "stream_incomplete",
                "the response stream ended before a final result arrived",
            )));
        };
        let terminal = matches!(event, AgentEvent::Result { .. } | AgentEvent::Error { .. });
        if let AgentEvent::Result {
            conversation_id: Some(next_conversation),
            ..
        } = &event
        {
            let mut continuity = continuity.lock().await;
            continuity.conversation_id = Some(next_conversation.clone());
            if let Some(native_context) = native_context.as_ref() {
                continuity.native_context = Some(native_context.clone());
            }
        }
        if runtime_events
            .send(RuntimeEvent::TurnEvent {
                operation_id,
                event,
            })
            .await
            .is_err()
        {
            cancellation.cancel();
            let _ = accepted.events.close().await;
            return Ok(RunTurnOutcome::CancelledAfterServerAcceptance);
        }
        if terminal {
            accepted
                .events
                .close()
                .await
                .map_err(|error| TurnFailure::from_port_error(&error))?;
            return Ok(RunTurnOutcome::Completed);
        }
    }
}

fn turn_failure_from_session_error(error: &EnsureSessionError) -> TurnFailure {
    match error {
        EnsureSessionError::Service(error)
        | EnsureSessionError::ServiceReconciliationRequired(error)
        | EnsureSessionError::CredentialReconciliationRequired(error) => {
            TurnFailure::from_port_error(error)
        }
        EnsureSessionError::ReconciliationMarkerWrite { operation, .. } => {
            TurnFailure::from_port_error(operation)
        }
        EnsureSessionError::ReconciliationRequired => {
            TurnFailure::internal("session_reconciliation_required")
        }
    }
}

fn turn_failure_from_one_shot_error(error: &OneShotError) -> TurnFailure {
    let port_error = if error.outcome_uncertain {
        PortError::uncertain(error.code, &error.message)
    } else {
        PortError::new(error.code, &error.message)
    };
    TurnFailure::from_port_error(&port_error)
}

enum OnboardingOperationError {
    Failed(String),
    Cancelled(RunTurnOutcome),
}

async fn run_interactive_onboarding(
    profile: OnboardingProfileInput,
    service: Arc<HttpService>,
    ensure_session: Arc<EnsureSession>,
    session: Arc<Mutex<SessionSnapshot>>,
    authorization_scope: &str,
    cancellation: CancellationToken,
) -> Result<(), OnboardingOperationError> {
    for required_scope in ["profile:read", "profile:write"] {
        if !authorization_has_scope(authorization_scope, required_scope) {
            return Err(OnboardingOperationError::Failed(format!(
                "Additional authorization ({required_scope}) is required. Exit the TUI and run `heyfood login`, then restart onboarding."
            )));
        }
    }
    let profile_data = profile.profile_data().map_err(|message| {
        OnboardingOperationError::Failed(format!("The dietary profile is invalid: {message}"))
    })?;
    let snapshot = session.lock().await.clone();
    let credentials = match ensure_session
        .execute(snapshot, cancellation.child_token())
        .await
        .map_err(|error| OnboardingOperationError::Failed(terminal_safe_text(&error.to_string())))?
    {
        EnsureSessionOutcome::Current(credentials) => credentials,
        EnsureSessionOutcome::Refreshed(credentials) => {
            let mut current = session.lock().await;
            current.credentials = credentials.clone();
            current.reconciliation_required = false;
            credentials
        }
        EnsureSessionOutcome::CancelledBeforeDispatch => {
            return Err(OnboardingOperationError::Cancelled(
                RunTurnOutcome::CancelledBeforeServerAcceptance,
            ));
        }
    };
    onboarding_cancellation_checkpoint(&cancellation)?;
    ensure_profile_sync_consent(&service, &credentials, &cancellation).await?;
    // Consent is a separate mutation. Once its response is observed, a stop at
    // this boundary still proves that the profile upload was not dispatched.
    onboarding_cancellation_checkpoint(&cancellation)?;

    let expected_version =
        match service
            .download_profile(
                &credentials,
                "_self",
                OperationId::new(),
                cancellation.child_token(),
            )
            .await
        {
            Ok(document) => Some(document.get("version").and_then(Value::as_u64).ok_or_else(
                || {
                    OnboardingOperationError::Failed(
                        "The existing profile had no usable version; no profile was uploaded."
                            .into(),
                    )
                },
            )?),
            Err(error) if error.code == "resource_not_found" => None,
            Err(error) => return Err(onboarding_service_error(error)),
        };
    onboarding_cancellation_checkpoint(&cancellation)?;
    let uploaded = service
        .upload_profile(
            &credentials,
            "_self",
            &profile_data,
            expected_version,
            OperationId::new(),
            cancellation.child_token(),
        )
        .await
        .map_err(onboarding_service_error)?;
    if uploaded.get("version").and_then(Value::as_u64).is_none() {
        return Err(OnboardingOperationError::Cancelled(
            RunTurnOutcome::CancelledAfterDispatchOutcomeUnknown,
        ));
    }
    Ok(())
}

async fn ensure_profile_sync_consent(
    service: &HttpService,
    credentials: &SessionCredentials,
    cancellation: &CancellationToken,
) -> Result<(), OnboardingOperationError> {
    let consent = service
        .profile_consent_status(credentials, OperationId::new(), cancellation.child_token())
        .await
        .map_err(onboarding_service_error)?;
    let has_consent = consent
        .get("has_consent")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            OnboardingOperationError::Failed(
                "The profile-sync consent response was incomplete; no profile was uploaded.".into(),
            )
        })?;
    if has_consent {
        return Ok(());
    }
    onboarding_cancellation_checkpoint(cancellation)?;
    let granted = service
        .grant_profile_consent(credentials, OperationId::new(), cancellation.child_token())
        .await
        .map_err(onboarding_service_error)?;
    if granted.get("has_consent").and_then(Value::as_bool) != Some(true) {
        return Err(OnboardingOperationError::Failed(
            "Profile-sync consent was not confirmed; no profile was uploaded.".into(),
        ));
    }
    Ok(())
}

fn onboarding_cancellation_checkpoint(
    cancellation: &CancellationToken,
) -> Result<(), OnboardingOperationError> {
    if cancellation.is_cancelled() {
        Err(OnboardingOperationError::Cancelled(
            RunTurnOutcome::CancelledBeforeServerAcceptance,
        ))
    } else {
        Ok(())
    }
}

fn onboarding_service_error(error: heyfood_application::PortError) -> OnboardingOperationError {
    if error.outcome_uncertain {
        OnboardingOperationError::Cancelled(RunTurnOutcome::CancelledAfterDispatchOutcomeUnknown)
    } else if matches!(
        error.code,
        "request_cancelled_before_dispatch"
            | "request_cancelled_after_dispatch"
            | "response_cancelled"
    ) {
        OnboardingOperationError::Cancelled(RunTurnOutcome::CancelledBeforeServerAcceptance)
    } else {
        OnboardingOperationError::Failed(format!(
            "{}: {}",
            terminal_safe_text(error.code),
            terminal_safe_text(&error.message)
        ))
    }
}

struct InteractivePanelEnvironment {
    local_state: Option<Arc<ImportedPythonState>>,
    native_voice_available: bool,
}

async fn run_interactive_panel(
    panel: PanelRequest,
    service: Arc<HttpService>,
    ensure_session: Arc<EnsureSession>,
    session: Arc<Mutex<SessionSnapshot>>,
    authorization_scope: &str,
    environment: InteractivePanelEnvironment,
    cancellation: CancellationToken,
) -> Result<String, String> {
    let required_scope = match panel {
        PanelRequest::Grocery => Some("grocery:read"),
        PanelRequest::Watch => Some("menu:watch"),
        PanelRequest::Health => Some("health:read"),
        PanelRequest::Profile => Some("profile:read"),
        PanelRequest::Status | PanelRequest::Household | PanelRequest::Location => None,
    };
    if let Some(required_scope) = required_scope
        && !authorization_scope
            .split_whitespace()
            .any(|scope| scope == required_scope)
    {
        return Err(format!(
            "Additional authorization ({required_scope}) is required. Exit the TUI and run `heyfood login`, then reopen this panel."
        ));
    }

    let snapshot = session.lock().await.clone();
    if matches!(panel, PanelRequest::Household | PanelRequest::Location) {
        if environment.local_state.as_ref().is_some_and(|state| {
            state.account_user_id.as_deref() != Some(snapshot.credentials.account_id.as_str())
        }) {
            return Err("Saved local context belongs to a different account.".into());
        }
        return match panel {
            PanelRequest::Household => {
                Ok(render_household_panel(environment.local_state.as_deref()))
            }
            PanelRequest::Location => Ok(render_location_panel(environment.local_state.as_deref())),
            PanelRequest::Status
            | PanelRequest::Grocery
            | PanelRequest::Watch
            | PanelRequest::Health
            | PanelRequest::Profile => unreachable!(),
        };
    }
    let credentials = match ensure_session
        .execute(snapshot, cancellation.child_token())
        .await
        .map_err(|error| terminal_safe_text(&error.to_string()))?
    {
        EnsureSessionOutcome::Current(credentials) => credentials,
        EnsureSessionOutcome::Refreshed(credentials) => {
            let mut current = session.lock().await;
            current.credentials = credentials.clone();
            current.reconciliation_required = false;
            credentials
        }
        EnsureSessionOutcome::CancelledBeforeDispatch => {
            return Err("Panel loading was cancelled before dispatch.".into());
        }
    };
    if cancellation.is_cancelled() {
        return Err("Panel loading was cancelled before dispatch.".into());
    }

    match panel {
        PanelRequest::Status => {
            let status = ReadStatus::new(service.as_ref())
                .execute(
                    credentials,
                    authorization_scope,
                    environment.native_voice_available,
                    cancellation,
                )
                .await
                .map_err(panel_error)?;
            let profile = match status.profile {
                ProfileReadinessStatus::NotAuthorized => "not authorized",
                ProfileReadinessStatus::AuthorizedConsentGranted => {
                    "authorized · sync consent granted"
                }
                ProfileReadinessStatus::AuthorizedConsentNotGranted => {
                    "authorized · sync consent not granted"
                }
            };
            let grocery = match status.grocery {
                OptionalCapabilityStatus::Authorized => "available · authorized",
                OptionalCapabilityStatus::AuthorizationRequired => {
                    "available · authorization required"
                }
                OptionalCapabilityStatus::NotAdvertised => "not advertised by service",
            };
            let menu_watch = match status.menu_watch {
                OptionalCapabilityStatus::Authorized => "authorized · create/list/remove available",
                OptionalCapabilityStatus::AuthorizationRequired
                | OptionalCapabilityStatus::NotAdvertised => "authorization required",
            };
            let voice = match status.voice {
                VoiceReadinessStatus::AuthorizedCaptureAvailable => {
                    "native capture available · transcription authorized · permission checked on use"
                }
                VoiceReadinessStatus::AuthorizationRequiredCaptureAvailable => {
                    "native capture available · transcription authorization required"
                }
                VoiceReadinessStatus::AuthorizedCaptureUnavailable => {
                    "transcription authorized · native capture unavailable in this artifact"
                }
                VoiceReadinessStatus::AuthorizationRequiredCaptureUnavailable => {
                    "transcription authorization required · native capture unavailable in this artifact"
                }
            };
            Ok(format!(
                "Session: active\nService: reachable\nProfile: {profile}\nGrocery: {grocery}\nMenu Watch: {menu_watch}\nHealth integrations: deferred from v0.6.3\nVoice: {voice}"
            ))
        }
        PanelRequest::Grocery => {
            let capabilities = DiscoverCapabilities::new(service.as_ref())
                .execute(cancellation.child_token())
                .await
                .map_err(panel_error)?;
            let list = ReadActiveGroceryDisplay::new(service.as_ref())
                .execute(
                    capabilities.clone(),
                    credentials.clone(),
                    OperationId::new(),
                    cancellation.child_token(),
                )
                .await
                .map_err(panel_error)?;
            let exclusions = ReadGroceryExclusions::new(service.as_ref())
                .execute(capabilities, credentials, OperationId::new(), cancellation)
                .await
                .map_err(panel_error)?;
            let mut output = render_grocery_list(&list, OutputMode::HumanPlain);
            output.push('\n');
            output.push_str(&render_grocery_exclusions(
                &exclusions,
                OutputMode::HumanPlain,
            ));
            Ok(output)
        }
        PanelRequest::Watch => {
            let watches = ListMenuWatches::new(service.as_ref())
                .execute(credentials, OperationId::new(), cancellation)
                .await
                .map_err(panel_error)?;
            Ok(render_menu_watch_list(&watches, OutputMode::HumanPlain))
        }
        PanelRequest::Health => {
            let integrations = service
                .health_integrations(&credentials, OperationId::new(), cancellation.child_token())
                .await
                .map_err(panel_error)?;
            let context = service
                .health_context(&credentials, OperationId::new(), cancellation)
                .await
                .map_err(panel_error)?;
            let mut output = String::from("Connections\n");
            if integrations.integrations.is_empty() {
                output.push_str("No health integrations connected.\n");
            } else {
                for integration in integrations.integrations {
                    let provider = serde_json::to_value(integration.provider)
                        .ok()
                        .and_then(|value| value.as_str().map(str::to_owned))
                        .unwrap_or_else(|| "provider".into());
                    let status = serde_json::to_value(integration.status)
                        .ok()
                        .and_then(|value| value.as_str().map(str::to_owned))
                        .unwrap_or_else(|| "unknown".into());
                    output.push_str(&format!("• {provider}: {status}\n"));
                }
            }
            output.push('\n');
            output.push_str(&render_health_context(&context, OutputMode::HumanPlain));
            output.push_str("\nHealth context is informational and is not a diagnosis.\n");
            Ok(output)
        }
        PanelRequest::Profile => {
            let consent = service
                .profile_consent_status(
                    &credentials,
                    OperationId::new(),
                    cancellation.child_token(),
                )
                .await
                .map_err(panel_error)?;
            if !consent
                .get("has_consent")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return Ok(
                    "Profile sync consent: not granted\nNo synced dietary profile was read.".into(),
                );
            }
            match service
                .download_profile(&credentials, "_self", OperationId::new(), cancellation)
                .await
            {
                Ok(profile) => Ok(render_profile_panel(&profile)),
                Err(error) if error.code == "resource_not_found" => {
                    Ok("Profile sync consent: granted\nNo synced dietary profile exists.".into())
                }
                Err(error) => Err(panel_error(error)),
            }
        }
        PanelRequest::Household | PanelRequest::Location => unreachable!(),
    }
}

fn authorization_has_scope(scope: &str, required: &str) -> bool {
    scope.split_whitespace().any(|scope| scope == required)
}

fn interactive_voice_availability(
    native_voice_available: bool,
    authorization_scope: &str,
) -> VoiceAvailability {
    match (
        native_voice_available,
        authorization_has_scope(authorization_scope, "audio:transcribe"),
    ) {
        (true, true) => VoiceAvailability::Ready,
        (true, false) => VoiceAvailability::AuthorizationRequired,
        (false, _) => VoiceAvailability::Unavailable,
    }
}

fn render_household_panel(state: Option<&ImportedPythonState>) -> String {
    let household = state
        .and_then(|state| state.account_scoped.get("household"))
        .and_then(Value::as_object);
    let members = household
        .and_then(|household| household.get("members"))
        .and_then(Value::as_array);
    let active_scope = household
        .and_then(|household| household.get("active_scope"))
        .and_then(Value::as_str)
        .unwrap_or("_self");
    let scope_label = if active_scope == "_self" {
        "Me".to_owned()
    } else if active_scope == "__everyone__" {
        "Everyone".to_owned()
    } else {
        members
            .into_iter()
            .flatten()
            .find(|member| member.get("id").and_then(Value::as_str) == Some(active_scope))
            .and_then(|member| member.get("name").and_then(Value::as_str))
            .map(terminal_safe_text)
            .unwrap_or_else(|| terminal_safe_text(active_scope))
    };
    let mut output = format!("Active scope: {scope_label}\n");
    let mut count = 0_usize;
    for member in members.into_iter().flatten() {
        let Some(name) = member.get("name").and_then(Value::as_str) else {
            continue;
        };
        let relationship = member
            .get("relationship")
            .and_then(Value::as_str)
            .unwrap_or("member");
        output.push_str(&format!(
            "• {} — {}\n",
            terminal_safe_text(name),
            terminal_safe_text(relationship)
        ));
        count = count.saturating_add(1);
    }
    if count == 0 {
        output.push_str("• Me — self\nNo additional household members are saved.\n");
    }
    output
}

fn render_location_panel(state: Option<&ImportedPythonState>) -> String {
    let location = state
        .and_then(|state| state.account_scoped.get("location"))
        .and_then(Value::as_object);
    let Some(location) = location else {
        return "No default location is saved.".into();
    };
    let label = location
        .get("label")
        .and_then(Value::as_str)
        .map(terminal_safe_text)
        .unwrap_or_else(|| "Saved coordinates".into());
    let latitude = location.get("latitude").and_then(Value::as_f64);
    let longitude = location.get("longitude").and_then(Value::as_f64);
    match (latitude, longitude) {
        (Some(latitude), Some(longitude))
            if latitude.is_finite()
                && longitude.is_finite()
                && (-90.0..=90.0).contains(&latitude)
                && (-180.0..=180.0).contains(&longitude) =>
        {
            format!("{label}\nLatitude: {latitude:.5}\nLongitude: {longitude:.5}")
        }
        _ => "Saved location data is incomplete and was not applied.".into(),
    }
}

fn render_profile_panel(document: &Value) -> String {
    let profile = document
        .get("profile_data")
        .and_then(Value::as_object)
        .or_else(|| document.as_object());
    let Some(profile) = profile else {
        return "Profile sync consent: granted\nThe dietary profile response is empty.".into();
    };
    let mut output = String::from("Profile sync consent: granted\n");
    if let Some(version) = document.get("version").and_then(Value::as_u64) {
        output.push_str(&format!("Version: {version}\n"));
    }
    let sections = [
        ("Diet styles", &["diet_style_ids", "preferences"][..]),
        (
            "Allergies and restrictions",
            &["allergy_ids", "restrictions"],
        ),
        ("Health conditions", &["health_condition_ids"]),
        ("Ingredients to avoid", &["avoid_ingredients"]),
        ("Cuisine preferences", &["cuisine_preferences"]),
    ];
    let mut populated = false;
    for (label, keys) in sections {
        let values = keys
            .iter()
            .find_map(|key| profile.get(*key).and_then(Value::as_array))
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(terminal_safe_text)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        if !values.is_empty() {
            output.push_str(&format!("{label}: {}\n", values.join(", ")));
            populated = true;
        }
    }
    if let Some(activity) = profile.get("activity_level").and_then(Value::as_str) {
        output.push_str(&format!("Activity: {}\n", terminal_safe_text(activity)));
        populated = true;
    }
    if !populated {
        output.push_str("The synced dietary profile is empty.\n");
    }
    output
}

fn panel_error(error: heyfood_application::PortError) -> String {
    format!(
        "{}: {}",
        terminal_safe_text(error.code),
        terminal_safe_text(&error.message)
    )
}

/// Runtime supervisor boundary used only after bootstrap has validated every
/// required input. Implementations must enqueue work and return promptly; the
/// retained terminal thread must never perform network IO.
pub trait QualifiedTurnDriver {
    /// Attach process-signal forwarding to the terminal event queue before the
    /// alternate screen is entered.
    fn start_session(&mut self, _events: mpsc::Sender<RuntimeEvent>) -> io::Result<()> {
        Ok(())
    }

    fn start_turn(
        &mut self,
        operation_id: u64,
        prompt: String,
        presented_household_context: Option<PresentedHouseholdContextV1>,
        events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()>;

    fn start_confirmation(
        &mut self,
        _operation_id: u64,
        _command: AgentConfirmationCommandWire,
        _presented_household_context: Option<PresentedHouseholdContextV1>,
        _events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "interactive confirmations are unavailable in this driver",
        ))
    }

    fn start_panel(
        &mut self,
        _operation_id: u64,
        _panel: PanelRequest,
        _events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "interactive panels are unavailable in this driver",
        ))
    }

    fn start_household_management_load(
        &mut self,
        _operation_id: HouseholdOperationIdV1,
        _session_mode_generation: HouseholdModeGenerationV1,
        _expected_account_binding_digest: HouseholdAccountBindingDigestV1,
        _reducer_correlation: HouseholdReducerCorrelationV1,
        _purpose: HouseholdManagementLoadPurposeV1,
        _events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "native household management is unavailable in this driver",
        ))
    }

    fn start_household_member_create(
        &mut self,
        _binding: HouseholdOperationBindingV1,
        _draft: BoundedHouseholdMemberDraftV1,
        _profile: OnboardingProfileInput,
        _events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "native household member creation is unavailable in this driver",
        ))
    }

    fn start_household_member_profile_save(
        &mut self,
        _binding: HouseholdOperationBindingV1,
        _subject: HouseholdSubjectId,
        _expected_profile_revision: Option<ProfileRevision>,
        _profile: OnboardingProfileInput,
        _events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "native household member onboarding is unavailable in this driver",
        ))
    }

    fn start_native_household_scope_selection(
        &mut self,
        _binding: HouseholdOperationBindingV1,
        _scope: HouseholdScope,
        _events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "native household scope selection is unavailable in this driver",
        ))
    }

    fn start_household_context_apply(
        &mut self,
        _binding: HouseholdOperationBindingV1,
        _resulting_household_revision: HouseholdRevision,
        _affected_subject: Option<HouseholdSubjectId>,
        _active_scope: HouseholdScope,
        _bounded_active_label: String,
        _events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "native household context application is unavailable in this driver",
        ))
    }

    fn cancel_household_operation(
        &mut self,
        _binding: &HouseholdOperationBindingV1,
    ) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "native household operation cancellation is unavailable in this driver",
        ))
    }

    fn start_owner_profile_actions(
        &mut self,
        _operation_id: u64,
        _purpose: OwnerProfileActionLoadPurposeV1,
        _events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "owner Profile actions are unavailable in this driver",
        ))
    }

    fn start_owner_profile_consent(
        &mut self,
        _operation_id: u64,
        _events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "owner Profile consent is unavailable in this driver",
        ))
    }

    fn start_owner_profile_retry(
        &mut self,
        _operation_id: u64,
        _action: OwnerProfileRetryActionV1,
        _intent: OwnerSyncIntentHandleV1,
        _events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "owner Profile sync retry is unavailable in this driver",
        ))
    }

    fn start_household_scope(
        &mut self,
        _operation_id: u64,
        _selector: String,
        _events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "household targeting is unavailable in this driver",
        ))
    }

    fn start_onboarding(
        &mut self,
        _operation_id: u64,
        _profile: OnboardingProfileInput,
        _events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "dietary onboarding is unavailable in this driver",
        ))
    }

    fn start_voice(
        &mut self,
        _operation_id: u64,
        _events: mpsc::Sender<RuntimeEvent>,
    ) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "voice input is unavailable in this driver",
        ))
    }

    fn stop_voice(&mut self, _operation_id: u64) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "voice input is unavailable in this driver",
        ))
    }

    fn cancel_voice(&mut self, operation_id: u64) -> io::Result<()> {
        self.cancel_turn(operation_id)
    }

    fn cancel_turn(&mut self, operation_id: u64) -> io::Result<()>;

    /// Forget process-local conversation continuity without touching persisted
    /// credentials or server-side data.
    fn reset_conversation(&mut self) -> io::Result<()> {
        Ok(())
    }

    /// Cancel any remaining operations, close their transports, and join every
    /// owned worker before the deadline. Returning `Ok` certifies that no turn
    /// task or socket remains owned by this driver.
    fn shutdown_and_join(&mut self, timeout: Duration) -> io::Result<()>;
}

#[derive(Debug)]
pub enum CompositionError {
    Tui(TuiError),
    Driver(io::Error),
    TuiAndDriver { tui: TuiError, driver: io::Error },
}

impl fmt::Display for CompositionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tui(error) => error.fmt(formatter),
            Self::Driver(error) => write!(formatter, "turn supervisor failed: {error}"),
            Self::TuiAndDriver { tui, driver } => write!(
                formatter,
                "terminal session failed ({tui}) and turn supervisor shutdown also failed: {driver}"
            ),
        }
    }
}

impl std::error::Error for CompositionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Tui(error) => Some(error),
            Self::Driver(error) => Some(error),
            Self::TuiAndDriver { driver, .. } => Some(driver),
        }
    }
}

/// Enter the terminal only after the caller has constructed a qualified driver
/// from explicit, validated native state.
pub fn run_qualified_session(
    driver: &mut impl QualifiedTurnDriver,
) -> Result<ExitReason, CompositionError> {
    let (runtime_sender, mut runtime_receiver) = mpsc::channel(64);
    driver
        .start_session(runtime_sender.clone())
        .map_err(CompositionError::Driver)?;
    let terminal = heyfood_tui::run_terminal(&mut runtime_receiver, |effect| {
        route_effect(driver, &runtime_sender, effect).map_err(|error| match error {
            CompositionError::Driver(error) => error,
            CompositionError::Tui(_) | CompositionError::TuiAndDriver { .. } => {
                unreachable!("effect routing does not enter the TUI")
            }
        })
    });
    finish_session(
        terminal,
        driver.shutdown_and_join(QUALIFIED_SHUTDOWN_TIMEOUT),
    )
}

fn finish_session(
    terminal: Result<ExitReason, TuiError>,
    shutdown: io::Result<()>,
) -> Result<ExitReason, CompositionError> {
    match (terminal, shutdown) {
        (Ok(reason), Ok(())) => Ok(reason),
        (Err(error), Ok(())) => Err(CompositionError::Tui(error)),
        (Ok(_), Err(error)) => Err(CompositionError::Driver(error)),
        (Err(tui), Err(driver)) => Err(CompositionError::TuiAndDriver { tui, driver }),
    }
}

fn route_effect(
    driver: &mut impl QualifiedTurnDriver,
    runtime_sender: &mpsc::Sender<RuntimeEvent>,
    effect: Effect,
) -> Result<(), CompositionError> {
    match effect {
        Effect::LoadHouseholdManagementV1 {
            operation_id,
            session_mode_generation,
            expected_account_binding_digest,
            reducer_correlation,
            purpose,
        } => driver
            .start_household_management_load(
                operation_id,
                session_mode_generation,
                expected_account_binding_digest,
                reducer_correlation,
                purpose,
                runtime_sender.clone(),
            )
            .map_err(CompositionError::Driver),
        Effect::CreateMemberWithDeclaredProfileV1 {
            binding,
            bounded_member_draft,
            onboarding_profile_input,
        } => driver
            .start_household_member_create(
                binding,
                bounded_member_draft,
                *onboarding_profile_input,
                runtime_sender.clone(),
            )
            .map_err(CompositionError::Driver),
        Effect::SaveMemberDeclaredProfileV1 {
            binding,
            subject,
            expected_profile_revision,
            onboarding_profile_input,
        } => driver
            .start_household_member_profile_save(
                binding,
                subject,
                expected_profile_revision,
                *onboarding_profile_input,
                runtime_sender.clone(),
            )
            .map_err(CompositionError::Driver),
        Effect::SelectHouseholdScopeV1 {
            binding,
            selected_scope,
        } => driver
            .start_native_household_scope_selection(binding, selected_scope, runtime_sender.clone())
            .map_err(CompositionError::Driver),
        Effect::ApplyCommittedHouseholdContextV1 {
            binding,
            resulting_household_revision,
            affected_subject,
            active_scope,
            bounded_active_label,
        } => driver
            .start_household_context_apply(
                binding,
                resulting_household_revision,
                affected_subject,
                active_scope,
                bounded_active_label,
                runtime_sender.clone(),
            )
            .map_err(CompositionError::Driver),
        Effect::CancelHouseholdOperationV1 { binding } => driver
            .cancel_household_operation(&binding)
            .map_err(CompositionError::Driver),
        Effect::SaveOnboarding {
            operation_id,
            profile,
        } => driver
            .start_onboarding(operation_id, *profile, runtime_sender.clone())
            .map_err(CompositionError::Driver),
        Effect::SubmitTurn {
            operation_id,
            prompt,
            presented_household_context,
        } => driver
            .start_turn(
                operation_id,
                prompt,
                presented_household_context,
                runtime_sender.clone(),
            )
            .map_err(CompositionError::Driver),
        Effect::ConfirmAction {
            operation_id,
            command,
            presented_household_context,
        } => driver
            .start_confirmation(
                operation_id,
                command,
                presented_household_context,
                runtime_sender.clone(),
            )
            .map_err(CompositionError::Driver),
        Effect::OpenPanel {
            operation_id,
            panel,
        } => driver
            .start_panel(operation_id, panel, runtime_sender.clone())
            .map_err(CompositionError::Driver),
        Effect::LoadOwnerProfileActionsV1 {
            operation_id,
            purpose,
        } => driver
            .start_owner_profile_actions(operation_id, purpose, runtime_sender.clone())
            .map_err(CompositionError::Driver),
        Effect::GrantOwnerProfileConsentV1 { operation_id } => driver
            .start_owner_profile_consent(operation_id, runtime_sender.clone())
            .map_err(CompositionError::Driver),
        Effect::RetryOwnerProfileSyncV1 {
            operation_id,
            action,
            intent,
        } => driver
            .start_owner_profile_retry(operation_id, action, intent, runtime_sender.clone())
            .map_err(CompositionError::Driver),
        Effect::SelectHousehold {
            operation_id,
            selector,
        } => driver
            .start_household_scope(operation_id, selector, runtime_sender.clone())
            .map_err(CompositionError::Driver),
        Effect::StartVoice { operation_id } => driver
            .start_voice(operation_id, runtime_sender.clone())
            .map_err(CompositionError::Driver),
        Effect::StopVoice { operation_id } => driver
            .stop_voice(operation_id)
            .map_err(CompositionError::Driver),
        Effect::CancelVoice { operation_id } => driver
            .cancel_voice(operation_id)
            .map_err(CompositionError::Driver),
        Effect::CancelTurn { operation_id } => driver
            .cancel_turn(operation_id)
            .map_err(CompositionError::Driver),
        Effect::ResetConversation => driver
            .reset_conversation()
            .map_err(CompositionError::Driver),
        Effect::Exit(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::{BTreeMap, VecDeque},
        path::PathBuf,
        sync::Mutex as StdMutex,
        sync::atomic::{AtomicUsize, Ordering},
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    use heyfood_agent_runtime::CliAuthContext;
    use heyfood_agent_runtime::OwnerSyncOutcomeUncertainReasonV1;
    use heyfood_application::{
        AcceptedTurn, AudioCapture, BoxFuture, ClockPort, CredentialCommit, CredentialPort,
        EventStream, PortError,
    };
    use heyfood_core::{
        AccountId, AgentEvent, CommitId, CredentialVersion, NetworkPolicy, RefreshOutcome,
        RefreshRequest, SensitiveString, ServiceUrl,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    use heyfood_platform::PythonStateImporter;

    struct LogTempRoot(PathBuf);

    impl LogTempRoot {
        fn new(name: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "heyfood-prepared-log-{name}-{}-{nonce}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for LogTempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn classify_owner_response(
        status: u16,
        body: impl Into<Vec<u8>>,
    ) -> OwnerSyncDispatchClassificationV1 {
        classify_owner_sync_transport_v1(OwnerSyncTransportResultV1::Response {
            status,
            body: body.into(),
        })
    }

    #[test]
    fn grocery_text_exports_require_a_protected_file_for_human_output() {
        let private_content = "onion for maya-uuid\n";
        for (format, export) in [
            (
                "markdown",
                GroceryExport::Markdown(private_content.to_owned()),
            ),
            ("text", GroceryExport::Text(private_content.to_owned())),
        ] {
            let error = render_grocery_export_stdout(export.clone(), OutputMode::HumanPlain)
                .expect_err("human export without --out must fail closed");
            assert_eq!(error.code, "grocery_export_requires_out");
            assert_eq!(
                error.message,
                GROCERY_EXPORT_REQUIRES_PROTECTED_FILE_MESSAGE
            );
            assert!(!error.message.contains("maya-uuid"));

            let rendered = render_grocery_export_stdout(export.clone(), OutputMode::Json)
                .expect("machine export is one JSON value");
            let decoded: Value = serde_json::from_str(&rendered).unwrap();
            assert_eq!(decoded["format"], format);
            assert_eq!(decoded["content"], private_content);
            assert_eq!(
                grocery_export_bytes(&export).unwrap(),
                private_content.as_bytes()
            );
        }
    }

    #[test]
    fn owner_sync_classifier_requires_the_exact_bounded_success_schema() {
        let success =
            br#"{"member_id":"_self","updated_at":"2026-07-30T12:00:00.000Z","version":7}"#;
        assert!(matches!(
            classify_owner_response(200, success),
            OwnerSyncDispatchClassificationV1::DefiniteSuccess {
                remote_version: 7,
                ..
            }
        ));

        for invalid in [
            br#""#.as_slice(),
            br#"null"#,
            br#"[]"#,
            br#"{"member_id":"member","updated_at":"2026-07-30T12:00:00.000Z","version":7}"#,
            br#"{"member_id":"_self","updated_at":"2026-07-30T12:00:00Z","version":7}"#,
            br#"{"member_id":"_self","updated_at":"2026-07-30T12:00:00.000Z","version":0}"#,
            br#"{"member_id":"_self","updated_at":"2026-07-30T12:00:00.000Z","version":7.0}"#,
            br#"{"extra":true,"member_id":"_self","updated_at":"2026-07-30T12:00:00.000Z","version":7}"#,
            br#"{"member_id":"_self","member_id":"_self","updated_at":"2026-07-30T12:00:00.000Z","version":7}"#,
        ] {
            assert_eq!(
                classify_owner_response(200, invalid),
                OwnerSyncDispatchClassificationV1::OutcomeUncertain,
                "{:?}",
                String::from_utf8_lossy(invalid)
            );
        }
    }

    #[test]
    fn owner_sync_classifier_is_total_for_every_status_and_ignores_error_body() {
        let valid_success =
            br#"{"member_id":"_self","updated_at":"2026-07-30T12:00:00.000Z","version":1}"#;
        for status in 0_u16..=u16::MAX {
            let classification = classify_owner_response(status, valid_success);
            let expected = match status {
                200..=299 => matches!(
                    classification,
                    OwnerSyncDispatchClassificationV1::DefiniteSuccess { .. }
                ),
                400 | 422 => matches!(
                    classification,
                    OwnerSyncDispatchClassificationV1::DefiniteFailure {
                        error: LastDefiniteOwnerSyncErrorV1::Validation
                    }
                ),
                401 => matches!(
                    classification,
                    OwnerSyncDispatchClassificationV1::DefiniteFailure {
                        error: LastDefiniteOwnerSyncErrorV1::Unauthorized
                    }
                ),
                403 => matches!(
                    classification,
                    OwnerSyncDispatchClassificationV1::DefiniteFailure {
                        error: LastDefiniteOwnerSyncErrorV1::Forbidden
                    }
                ),
                404 => matches!(
                    classification,
                    OwnerSyncDispatchClassificationV1::DefiniteFailure {
                        error: LastDefiniteOwnerSyncErrorV1::NotFound
                    }
                ),
                409 => matches!(
                    classification,
                    OwnerSyncDispatchClassificationV1::VersionConflict
                ),
                _ => matches!(
                    classification,
                    OwnerSyncDispatchClassificationV1::OutcomeUncertain
                ),
            };
            assert!(expected, "unclassified status {status}: {classification:?}");
        }

        for status in [400, 401, 403, 404, 409, 422] {
            assert_eq!(
                classify_owner_response(status, b"sentinel error body A".to_vec()),
                classify_owner_response(status, b"{\"different\":\"sentinel B\"}".to_vec())
            );
        }
    }

    #[test]
    fn owner_sync_classifier_preserves_only_the_pre_send_cancellation_distinction() {
        assert_eq!(
            classify_owner_sync_transport_v1(OwnerSyncTransportResultV1::CancelledBeforeSend),
            OwnerSyncDispatchClassificationV1::CancelledBeforeSend
        );
        for reason in [
            OwnerSyncOutcomeUncertainReasonV1::Timeout,
            OwnerSyncOutcomeUncertainReasonV1::CancelledAfterSend,
            OwnerSyncOutcomeUncertainReasonV1::Transport,
            OwnerSyncOutcomeUncertainReasonV1::BodyRead,
            OwnerSyncOutcomeUncertainReasonV1::BodyTooLarge,
        ] {
            assert_eq!(
                classify_owner_sync_transport_v1(OwnerSyncTransportResultV1::OutcomeUncertain {
                    reason
                }),
                OwnerSyncDispatchClassificationV1::OutcomeUncertain
            );
        }
    }

    fn safe_log_preview(name: &str, household: Value) -> (LogTempRoot, PythonStatePreview) {
        let root = LogTempRoot::new(name);
        let source = root.0.join("config.json");
        std::fs::write(
            &source,
            serde_json::to_vec(&json!({
                "account_user_id": "one-shot-account",
                "first_name": "Justin",
                "household": household,
                "household_local_profiles": {},
                "household_profile_outbox": {}
            }))
            .unwrap(),
        )
        .unwrap();
        let importer = PythonStateImporter::under(&source, root.0.join("native"));
        importer.import().unwrap();
        (root, importer.preview_state().unwrap())
    }

    fn no_source_log_preview(name: &str) -> (LogTempRoot, PythonStatePreview) {
        let root = LogTempRoot::new(name);
        let importer =
            PythonStateImporter::under(root.0.join("missing.json"), root.0.join("native"));
        (root, importer.preview_state().unwrap())
    }

    fn protected_log_preview(name: &str) -> (LogTempRoot, PythonStatePreview) {
        let root = LogTempRoot::new(name);
        let source = root.0.join("config.json");
        std::fs::write(&source, b"{credential-canary-not-json").unwrap();
        let importer = PythonStateImporter::under(&source, root.0.join("native"));
        (root, importer.preview_state().unwrap())
    }

    fn log_arguments(selector: Option<&str>, meal: &[&str]) -> LogArgs {
        LogArgs {
            meal: meal.iter().map(|value| (*value).to_owned()).collect(),
            meal_type: Some(MealType::Breakfast),
            checking_for: selector.map(str::to_owned),
        }
    }

    fn sarah_household(active_scope: &str) -> Value {
        json!({
            "active_scope": active_scope,
            "members": [
                {"id": "_self", "name": "Justin", "relationship": "self", "archived": false},
                {"id": "member-sarah", "name": "Sarah", "relationship": "partner", "archived": false}
            ]
        })
    }

    #[test]
    fn household_log_human_agent_error_always_uses_reviewed_copy() {
        let error = OneShotError {
            code: "agent_error",
            message: "A server-selected explanation.".to_owned(),
            outcome_uncertain: true,
        };

        let sanitized = sanitize_household_log_error(error, OutputMode::HumanPlain, &[]);

        assert_eq!(sanitized.code, "agent_error");
        assert_eq!(sanitized.message, HOUSEHOLD_LOG_HUMAN_ERROR_MESSAGE);
        assert!(sanitized.outcome_uncertain);
    }

    #[test]
    fn household_log_human_server_error_with_generic_private_id_uses_reviewed_copy() {
        let error = OneShotError::new(
            "service_error",
            "Server rejected member 3f1c9c2e-2f5a-4a5b-8f1e-9d2b7c6a4e01.",
        );

        let sanitized = sanitize_household_log_error(error, OutputMode::HumanAnsi, &[]);

        assert_eq!(sanitized.code, "service_error");
        assert_eq!(sanitized.message, HOUSEHOLD_LOG_HUMAN_ERROR_MESSAGE);
        assert!(!sanitized.outcome_uncertain);
    }

    #[test]
    fn household_log_human_server_error_with_whitespace_transformed_roster_id_is_sanitized() {
        let error = OneShotError {
            code: "service_error",
            message: "Server rejected opaque- member-\nseven.".to_owned(),
            outcome_uncertain: true,
        };
        let private_household_ids = vec!["opaque-member-seven".to_owned()];

        let sanitized =
            sanitize_household_log_error(error, OutputMode::HumanPlain, &private_household_ids);

        assert_eq!(sanitized.code, "service_error");
        assert_eq!(sanitized.message, HOUSEHOLD_LOG_HUMAN_ERROR_MESSAGE);
        assert!(sanitized.outcome_uncertain);
    }

    #[test]
    fn household_log_human_server_error_with_only_roster_id_prefix_uses_reviewed_copy() {
        let error = OneShotError {
            code: "service_error",
            message: "Server rejected opaque-member.".to_owned(),
            outcome_uncertain: true,
        };
        let private_household_ids = vec!["opaque-member-seven".to_owned()];

        let sanitized =
            sanitize_household_log_error(error, OutputMode::HumanPlain, &private_household_ids);

        assert_eq!(sanitized.code, "service_error");
        assert_eq!(sanitized.message, HOUSEHOLD_LOG_HUMAN_ERROR_MESSAGE);
        assert!(sanitized.outcome_uncertain);
    }

    #[test]
    fn household_log_json_error_keeps_exact_machine_semantics() {
        let error = OneShotError {
            code: "agent_error",
            message: "Server rejected opaque- member-\nseven and _self.".to_owned(),
            outcome_uncertain: true,
        };
        let private_household_ids = vec!["opaque-member-seven".to_owned()];

        let sanitized =
            sanitize_household_log_error(error.clone(), OutputMode::Json, &private_household_ids);

        assert_eq!(sanitized, error);
    }

    #[test]
    fn household_log_human_success_never_promotes_partial_only_text() {
        let result = heyfood_application::OneShotTurnResult {
            document: json!({"text": "Prepared for legacyOpa"}),
            conversation_id: Some("conversation-1".into()),
            partial_text_promoted: true,
            streamed_choice_value_authorities: Vec::new(),
        };

        let human =
            render_household_log_result(&result, OutputMode::HumanPlain, &["legacyOpaque7"]);
        assert_eq!(human.trim_end(), UNRENDERABLE_AGENT_RESULT_MESSAGE);
        assert!(!human.contains("legacyOpa"));

        let machine = render_household_log_result(&result, OutputMode::Json, &["legacyOpaque7"]);
        assert_eq!(
            serde_json::from_str::<Value>(&machine).unwrap(),
            result.document
        );
    }

    #[test]
    fn household_log_human_terminal_text_rejects_a_known_id_prefix() {
        let result = heyfood_application::OneShotTurnResult {
            document: json!({"message": "Prepared for legacyOpa"}),
            conversation_id: None,
            partial_text_promoted: false,
            streamed_choice_value_authorities: Vec::new(),
        };

        let human =
            render_household_log_result(&result, OutputMode::HumanPlain, &["legacyOpaque7"]);
        assert_eq!(human.trim_end(), UNRENDERABLE_AGENT_RESULT_MESSAGE);
        assert!(!human.contains("legacyOpa"));
    }

    #[test]
    fn household_log_human_result_retains_replaced_choice_value_authority() {
        let result = heyfood_application::OneShotTurnResult {
            document: json!({
                "message": "Prepared for foreignOpaque7",
                "choices": {
                    "choices": ["Continue"],
                    "choice_details": [{"label": "Continue", "value": "next"}],
                    "allow_multiple": false
                }
            }),
            conversation_id: None,
            partial_text_promoted: false,
            streamed_choice_value_authorities: vec!["foreignOpaque7".into(), "next".into()],
        };

        let human = render_household_log_result(&result, OutputMode::HumanPlain, &[]);
        assert_eq!(human.trim_end(), UNRENDERABLE_AGENT_RESULT_MESSAGE);
        assert!(!human.contains("foreignOpaque7"));

        let machine = render_household_log_result(&result, OutputMode::Json, &[]);
        assert_eq!(
            serde_json::from_str::<Value>(&machine).unwrap(),
            result.document
        );
    }

    #[test]
    fn prepared_log_review_uses_saved_non_self_scope_when_for_is_omitted() {
        let (_root, preview) = safe_log_preview("saved-sarah", sarah_household("member-sarah"));
        let prepared =
            prepare_log_command(log_arguments(None, &["oatmeal"]), &[], preview).unwrap();
        let review = prepared.review_document();
        assert!(review.contains("Household target: \"Sarah\""));
        assert!(!review.contains("member-id-utf8-hex"));
        assert_eq!(prepared.target.raw_id, "member-sarah");
    }

    #[test]
    fn prepared_log_no_source_omitted_for_reviews_self_without_authentication() {
        let (_root, preview) = no_source_log_preview("no-source-self");
        let prepared =
            prepare_log_command(log_arguments(None, &["oatmeal"]), &[], preview).unwrap();
        let review = prepared.review_document();
        assert!(review.contains("Household target: \"Me\""));
        assert!(!review.contains("scope=_self"));
    }

    #[test]
    fn prepared_log_no_source_member_or_everyone_fails_before_review() {
        for selector in ["member-sarah", "everyone"] {
            let (_root, preview) = no_source_log_preview(selector);
            let error =
                prepare_log_command(log_arguments(Some(selector), &["oatmeal"]), &[], preview)
                    .unwrap_err();
            assert_eq!(error.code, "household_state_unavailable");
        }
    }

    #[test]
    fn prepared_log_uninspected_mixed_source_explicit_self_reviews_without_source_read() {
        let (_root, preview) = protected_log_preview("protected-self");
        let prepared =
            prepare_log_command(log_arguments(Some("self"), &["oatmeal"]), &[], preview).unwrap();
        let review = prepared.review_document();
        assert!(review.contains("Household target: \"Me\""));
        assert!(!review.contains("scope=_self"));
    }

    #[test]
    fn prepared_log_protected_source_omitted_member_or_everyone_fails_before_review() {
        for selector in [None, Some("member-sarah"), Some("everyone")] {
            let (_root, preview) = protected_log_preview("protected-deny");
            let error = prepare_log_command(log_arguments(selector, &["oatmeal"]), &[], preview)
                .unwrap_err();
            assert_eq!(error.code, "household_state_protected");
        }
    }

    #[test]
    fn prepared_log_review_uses_reversible_ascii_label_without_member_id() {
        let (_root, preview) = safe_log_preview(
            "ascii-review",
            json!({
                "active_scope": "m\u{00e9}mber",
                "members": [
                    {"id": "m\u{00e9}mber", "name": "S\u{00e1}ra \"Q\"", "archived": false}
                ]
            }),
        );
        let prepared =
            prepare_log_command(log_arguments(None, &["oatmeal"]), &[], preview).unwrap();
        let review = prepared.review_document();
        assert!(review.contains("\"S\\u00E1ra \\\"Q\\\"\""));
        assert!(!review.contains("member-id-utf8-hex"));
        assert!(!review.contains("6dc3a96d626572"));
        assert!(review.is_ascii());
    }

    #[test]
    fn prepared_log_everyone_review_promises_one_owner_meal_without_id_tokens() {
        let (_root, preview) = safe_log_preview(
            "everyone-owner-attribution",
            sarah_household("__everyone__"),
        );
        let prepared =
            prepare_log_command(log_arguments(None, &["oatmeal"]), &[], preview).unwrap();
        let review = prepared.review_document();
        assert!(review.contains("Household target: \"Everyone\""));
        assert!(review.contains("Meal write: one meal for owner \"Justin\""));
        for hidden in ["member-id-utf8-hex", "scope=__everyone__", "member-sarah"] {
            assert!(!review.contains(hidden));
        }
    }

    #[test]
    fn prepared_log_label_tokens_round_trip_and_are_pairwise_injective_across_accepted_corpus() {
        let mut labels = BTreeSet::from([
            "A".to_owned(),
            "\"".to_owned(),
            "\\".to_owned(),
            "A B".to_owned(),
            "\u{00e9}".to_owned(),
            "\u{4e2d}".to_owned(),
            "\u{200d}".to_owned(),
            "\u{1f642}".to_owned(),
            "\u{1d11e}".to_owned(),
            "\u{10ffff}".to_owned(),
            "\"\\\u{00e9}\u{1f642}".to_owned(),
        ]);
        for scalar in '\u{20}'..='\u{7e}' {
            labels.insert(format!("A{scalar}Z"));
        }
        for scalar in [
            '\u{00a1}',
            '\u{00e9}',
            '\u{03a9}',
            '\u{061b}',
            '\u{200d}',
            '\u{4e2d}',
            '\u{1d11e}',
            '\u{1f642}',
            '\u{10ffff}',
        ] {
            labels.insert(format!("A{scalar}Z"));
        }
        for scalar_count in 1..=80 {
            labels.insert("a".repeat(scalar_count));
            labels.insert("\u{1f642}".repeat(scalar_count));
            labels.insert(format!(
                "A{}Z",
                ["a", "\"", "\\", "\u{00e9}", "\u{4e2d}", "\u{1f642}"][scalar_count % 6]
                    .repeat(scalar_count.saturating_sub(2))
            ));
        }

        let mut rendered_to_label = BTreeMap::new();
        for label in &labels {
            validate_stored_member_name(label).unwrap();
            let rendered = ascii_json_string(label);
            assert!(rendered.is_ascii());
            let decoded: String =
                serde_json::from_str(&rendered).expect("canonical label is valid JSON");
            assert_eq!(decoded.as_bytes(), label.as_bytes());
            if let Some(previous) = rendered_to_label.insert(rendered, label.clone()) {
                assert_eq!(
                    previous, *label,
                    "distinct accepted UTF-8 labels rendered identically"
                );
            }
        }
        assert_eq!(rendered_to_label.len(), labels.len());
    }

    #[test]
    fn prepared_log_label_renderer_freezes_escape_and_length_boundaries() {
        let canonical = "\"\\\u{00e9}\u{1f642}";
        let rendered = ascii_json_string(canonical);
        assert_eq!(rendered, "\"\\\"\\\\\\u00E9\\uD83D\\uDE42\"");
        assert_eq!(
            serde_json::from_str::<String>(&rendered).unwrap(),
            canonical
        );

        let controls = "\u{0000}\u{001f}\u{007f}\u{009b}";
        let escaped_controls = ascii_json_string(controls);
        assert_eq!(escaped_controls, "\"\\u0000\\u001F\\u007F\\u009B\"");
        assert_eq!(
            serde_json::from_str::<String>(&escaped_controls).unwrap(),
            controls
        );

        for accepted in [
            "a".to_owned(),
            "a".repeat(80),
            "\u{1f642}".repeat(80),
            "\"".to_owned(),
            "\\".to_owned(),
        ] {
            validate_stored_member_name(&accepted).unwrap();
            let token = ascii_json_string(&accepted);
            assert_eq!(
                serde_json::from_str::<String>(&token).unwrap().as_bytes(),
                accepted.as_bytes()
            );
        }
        for rejected in [
            String::new(),
            "a".repeat(81),
            format!("{}a", "\u{1f642}".repeat(80)),
            " Name".to_owned(),
            "Name\u{2003}".to_owned(),
            "A\u{0000}Z".to_owned(),
            "A\u{001f}Z".to_owned(),
            "A\u{007f}Z".to_owned(),
            "A\u{009b}Z".to_owned(),
            "A\u{061c}Z".to_owned(),
            "A\u{200e}Z".to_owned(),
            "A\u{2028}Z".to_owned(),
            "A\u{2066}Z".to_owned(),
            "A\u{feff}Z".to_owned(),
        ] {
            assert!(validate_stored_member_name(&rejected).is_err());
        }
    }

    #[test]
    fn prepared_log_distinct_member_ids_have_distinct_review_tokens() {
        let left = canonical_display(LogTargetMode::Member, "Same", "member-a");
        let right = canonical_display(LogTargetMode::Member, "Same", "member-b");
        assert_ne!(left.stable_id_token, right.stable_id_token);
    }

    #[test]
    fn prepared_log_rejects_duplicate_member_id_before_review() {
        let (_root, preview) = safe_log_preview(
            "duplicate-id",
            json!({
                "active_scope": "member-a",
                "members": [
                    {"id": "member-a", "name": "A"},
                    {"id": "member-a", "name": "B"}
                ]
            }),
        );
        assert_eq!(
            prepare_log_command(log_arguments(None, &["oatmeal"]), &[], preview)
                .unwrap_err()
                .code,
            "household_state_invalid"
        );
    }

    #[test]
    fn prepared_log_strict_decoder_preserves_missing_self_without_synthesizing_a_row_or_name() {
        let state = ImportedPythonState {
            account_user_id: Some("one-shot-account".into()),
            global: BTreeMap::new(),
            account_scoped: BTreeMap::from([(
                "household".into(),
                json!({
                    "active_scope": "member-a",
                    "members": [{"id": "member-a", "name": "Alex", "archived": false}]
                }),
            )]),
        };
        let frozen = strict_frozen_household(&state).unwrap();
        assert_eq!(frozen.members.len(), 1);
        assert_eq!(frozen.members[0].id, "member-a");
        assert!(frozen.member("_self").is_none());

        let mut self_scoped = state;
        self_scoped
            .account_scoped
            .get_mut("household")
            .and_then(Value::as_object_mut)
            .unwrap()
            .insert("active_scope".into(), Value::String("_self".into()));
        let frozen = strict_frozen_household(&self_scoped).unwrap();
        assert!(frozen.member("_self").is_none());
        assert_eq!(frozen.active_scope, "_self");
    }

    #[test]
    fn prepared_log_strict_decoder_rejects_duplicate_self_rows() {
        let state = ImportedPythonState {
            account_user_id: Some("one-shot-account".into()),
            global: BTreeMap::new(),
            account_scoped: BTreeMap::from([(
                "household".into(),
                json!({
                    "active_scope": "_self",
                    "members": [
                        {"id": "_self", "name": "Owner"},
                        {"id": "_self", "name": "Other"}
                    ]
                }),
            )]),
        };
        let Err(error) = strict_frozen_household(&state) else {
            panic!("duplicate self rows were accepted");
        };
        assert_eq!(error.code, "household_state_invalid");
    }

    #[test]
    fn prepared_log_duplicate_name_requires_exact_stable_id() {
        let household = json!({
            "active_scope": "_self",
            "members": [
                {"id": "member-a", "name": "Sam"},
                {"id": "member-b", "name": "SAM"}
            ]
        });
        let (_root, preview) = safe_log_preview("duplicate-name", household.clone());
        let error = prepare_log_command(log_arguments(Some("sam"), &["oatmeal"]), &[], preview)
            .unwrap_err();
        assert_eq!(error.code, "household_target_ambiguous");
        assert!(error.message.contains("give members unique names"));
        assert!(!error.message.contains("ID"));
        let (_root, preview) = safe_log_preview("exact-id", household);
        let prepared =
            prepare_log_command(log_arguments(Some("member-b"), &["oatmeal"]), &[], preview)
                .unwrap();
        assert_eq!(prepared.target.raw_id, "member-b");
    }

    #[test]
    fn prepared_log_rejects_missing_unknown_or_archived_active_scope_without_self_fallback() {
        let cases = [
            json!({"members": [{"id": "member-a", "name": "A"}]}),
            json!({"active_scope": "missing", "members": [{"id": "member-a", "name": "A"}]}),
            json!({"active_scope": "member-a", "members": [{"id": "member-a", "name": "A", "archived": true}]}),
        ];
        for (index, household) in cases.into_iter().enumerate() {
            let (_root, preview) = safe_log_preview(&format!("active-scope-{index}"), household);
            assert_eq!(
                prepare_log_command(log_arguments(None, &["oatmeal"]), &[], preview)
                    .unwrap_err()
                    .code,
                "household_active_scope_invalid"
            );
        }
    }

    #[test]
    fn prepared_log_rejects_member_id_byte_bounds_trim_control_ansi_separator_and_reserved_cases() {
        for (index, id) in [
            "",
            " member",
            "member ",
            "member\u{001b}",
            "member\u{009b}",
            "member\u{2028}",
            "member/name",
            "member\\name",
            ".",
            "..",
            "self",
            "EVERYONE",
            &"x".repeat(129),
        ]
        .into_iter()
        .enumerate()
        {
            let state = ImportedPythonState {
                account_user_id: Some("one-shot-account".into()),
                global: BTreeMap::new(),
                account_scoped: BTreeMap::from([(
                    "household".into(),
                    json!({"active_scope": "_self", "members": [{"id": id, "name": "Name"}]}),
                )]),
            };
            assert!(
                strict_frozen_household(&state).is_err(),
                "invalid ID case {index} was accepted"
            );
        }
    }

    #[test]
    fn prepared_log_rejects_name_scalar_byte_trim_control_and_ansi_cases() {
        for name in [
            "".to_owned(),
            " Name".to_owned(),
            "Name ".to_owned(),
            "Name\u{001b}".to_owned(),
            "Name\u{009b}".to_owned(),
            "Name\u{2029}".to_owned(),
            "x".repeat(81),
            "\u{1f600}".repeat(81),
        ] {
            let state = ImportedPythonState {
                account_user_id: Some("one-shot-account".into()),
                global: BTreeMap::new(),
                account_scoped: BTreeMap::from([(
                    "household".into(),
                    json!({"active_scope": "_self", "members": [{"id": "member-a", "name": name}]}),
                )]),
            };
            assert!(strict_frozen_household(&state).is_err());
        }
    }

    #[test]
    fn prepared_log_rejects_malformed_roster_row_instead_of_dropping_it() {
        let state = ImportedPythonState {
            account_user_id: Some("one-shot-account".into()),
            global: BTreeMap::new(),
            account_scoped: BTreeMap::from([(
                "household".into(),
                json!({"active_scope": "_self", "members": [null]}),
            )]),
        };
        let Err(error) = strict_frozen_household(&state) else {
            panic!("malformed roster row was accepted");
        };
        assert_eq!(error.code, "household_state_invalid");
    }

    #[test]
    fn prepared_log_rejects_invalid_meal_before_review() {
        let (_root, preview) = no_source_log_preview("invalid-meal");
        assert_eq!(
            prepare_log_command(log_arguments(None, &[]), &[], preview)
                .unwrap_err()
                .code,
            "invalid_meal"
        );
    }

    #[test]
    fn prepared_log_debug_redacts_sensitive_fields() {
        let (_root, preview) = safe_log_preview("debug-redaction", sarah_household("member-sarah"));
        let prepared =
            prepare_log_command(log_arguments(None, &["secret-oatmeal"]), &[], preview).unwrap();
        let debug = format!("{prepared:?}");
        for secret in ["secret-oatmeal", "Sarah", "member-sarah", "Justin"] {
            assert!(!debug.contains(secret));
        }
    }

    #[test]
    fn prepared_log_exact_stable_id_disambiguates_duplicate_name() {
        let household = json!({
            "active_scope": "_self",
            "members": [
                {"id": "member-a", "name": "Sam"},
                {"id": "member-b", "name": "Sam"}
            ]
        });
        let (_root, preview) = safe_log_preview("duplicate-exact", household);
        let prepared =
            prepare_log_command(log_arguments(Some("member-a"), &["oatmeal"]), &[], preview)
                .unwrap();
        assert_eq!(prepared.target.raw_id, "member-a");
    }

    #[test]
    fn prepared_log_member_id_tokens_round_trip_and_are_pairwise_injective_for_every_byte_length() {
        fn decode(token: &str) -> Vec<u8> {
            token
                .strip_prefix("member-id-utf8-hex=")
                .expect("member token domain")
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| {
                    u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16)
                        .expect("lowercase hex byte")
                })
                .collect()
        }

        let mut accepted = Vec::new();
        for byte_len in 1..=128 {
            let first = format!("a{}", "x".repeat(byte_len - 1));
            let second = format!("b{}", "x".repeat(byte_len - 1));
            accepted.push(first);
            accepted.push(second);
            if byte_len >= 2 {
                accepted.push(format!("\u{00e9}{}", "x".repeat(byte_len - 2)));
            }
            if byte_len >= 3 {
                accepted.push(format!("\u{4e2d}{}", "x".repeat(byte_len - 3)));
            }
            if byte_len >= 4 {
                accepted.push(format!("\u{1f642}{}", "x".repeat(byte_len - 4)));
            }
        }

        let mut tokens = BTreeSet::new();
        for id in &accepted {
            validate_stored_member_id(id).unwrap();
            assert!((1..=128).contains(&id.len()));
            let token = canonical_display(LogTargetMode::Member, "Same", id).stable_id_token;
            assert_eq!(decode(&token), id.as_bytes());
            assert!(tokens.insert(token), "distinct accepted IDs collided");
        }
        assert_eq!(tokens.len(), accepted.len());
    }

    struct FixedClock;

    impl ClockPort for FixedClock {
        fn unix_timestamp(&self) -> i64 {
            0
        }
    }

    #[derive(Clone, Default)]
    struct FixtureAudioCapture {
        calls: Arc<AtomicUsize>,
    }

    impl AudioCapturePort for FixtureAudioCapture {
        fn available(&self) -> bool {
            true
        }

        fn capture(
            &self,
            stop: CancellationToken,
            cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<AudioCapture, PortError>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Box::pin(async move {
                tokio::select! {
                    () = stop.cancelled() => {
                        let mut wav_bytes = vec![0_u8; 46];
                        wav_bytes[..4].copy_from_slice(b"RIFF");
                        wav_bytes[4..8].copy_from_slice(&38_u32.to_le_bytes());
                        wav_bytes[8..12].copy_from_slice(b"WAVE");
                        wav_bytes[12..16].copy_from_slice(b"fmt ");
                        wav_bytes[16..20].copy_from_slice(&16_u32.to_le_bytes());
                        wav_bytes[20..22].copy_from_slice(&1_u16.to_le_bytes());
                        wav_bytes[22..24].copy_from_slice(&1_u16.to_le_bytes());
                        wav_bytes[24..28].copy_from_slice(&16_000_u32.to_le_bytes());
                        wav_bytes[28..32].copy_from_slice(&32_000_u32.to_le_bytes());
                        wav_bytes[32..34].copy_from_slice(&2_u16.to_le_bytes());
                        wav_bytes[34..36].copy_from_slice(&16_u16.to_le_bytes());
                        wav_bytes[36..40].copy_from_slice(b"data");
                        wav_bytes[40..44].copy_from_slice(&2_u32.to_le_bytes());
                        Ok(AudioCapture {
                            wav_bytes,
                            sample_rate_hz: 16_000,
                            duration_millis: 1,
                            truncated: false,
                            overflowed: false,
                        })
                    }
                    () = cancellation.cancelled() => Err(PortError::new(
                        "voice_capture_cancelled",
                        "the fixture recording was cancelled",
                    )),
                }
            })
        }
    }

    struct MemoryCredentialPort;

    impl CredentialPort for MemoryCredentialPort {
        fn load(&self) -> BoxFuture<'_, Result<Option<SessionCredentials>, PortError>> {
            Box::pin(async { Ok(None) })
        }

        fn commit(&self, _commit: CredentialCommit) -> BoxFuture<'_, Result<(), PortError>> {
            Box::pin(async { Ok(()) })
        }

        fn mark_reconciliation_required(
            &self,
            _commit_id: CommitId,
        ) -> BoxFuture<'_, Result<(), PortError>> {
            Box::pin(async { Ok(()) })
        }

        fn clear_reconciliation_required(
            &self,
            _commit_id: CommitId,
        ) -> BoxFuture<'_, Result<(), PortError>> {
            Box::pin(async { Ok(()) })
        }
    }

    struct FixtureStream {
        events: VecDeque<AgentEvent>,
    }

    impl EventStream for FixtureStream {
        fn next(&mut self) -> BoxFuture<'_, Result<Option<AgentEvent>, PortError>> {
            Box::pin(async { Ok(self.events.pop_front()) })
        }

        fn close(self: Box<Self>) -> BoxFuture<'static, Result<(), PortError>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[derive(Default)]
    struct FixtureService {
        requests: StdMutex<Vec<TurnRequest>>,
    }

    impl ServicePort for FixtureService {
        fn refresh_session(
            &self,
            _request: RefreshRequest,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<RefreshOutcome, PortError>> {
            Box::pin(async {
                Err(PortError::new(
                    "unexpected_refresh",
                    "fixture credentials must remain current",
                ))
            })
        }

        fn open_turn(
            &self,
            request: TurnRequest,
            _credentials: SessionCredentials,
            _operation_id: OperationId,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<AcceptedTurn, PortError>> {
            self.requests.lock().unwrap().push(request);
            Box::pin(async {
                Ok(AcceptedTurn {
                    events: Box::new(FixtureStream {
                        events: VecDeque::from([
                            AgentEvent::Partial {
                                text: "Hello ".into(),
                            },
                            AgentEvent::Result {
                                document: serde_json::json!({"text": "Hello there"}),
                                conversation_id: Some("conversation-1".into()),
                            },
                        ]),
                    }),
                })
            })
        }
    }

    struct RotatingSessionProvider {
        calls: Arc<AtomicUsize>,
        services: StdMutex<VecDeque<Arc<FixtureService>>>,
    }

    impl InteractiveSessionProvider for RotatingSessionProvider {
        fn prepare(
            &self,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<InteractiveSessionPreparation, OneShotError>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let service = self
                .services
                .lock()
                .unwrap()
                .pop_front()
                .expect("one fresh service per operation");
            Box::pin(async move {
                let service_port: Arc<dyn ServicePort> = service;
                let ensure_session = Arc::new(EnsureSession::new(
                    service_port.clone(),
                    Arc::new(MemoryCredentialPort),
                    Arc::new(FixedClock),
                ));
                Ok(InteractiveSessionPreparation::from_service(
                    service_port,
                    ensure_session,
                    SessionSnapshot {
                        credentials: fixture_credentials(),
                        reconciliation_required: false,
                    },
                    "profile:read",
                ))
            })
        }
    }

    struct RejectedSessionProvider;

    impl InteractiveSessionProvider for RejectedSessionProvider {
        fn prepare(
            &self,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<InteractiveSessionPreparation, OneShotError>> {
            Box::pin(async {
                Err(OneShotError::new(
                    "login_required",
                    "private authorization rejection",
                ))
            })
        }
    }

    struct ChangedAccountSessionProvider {
        service: Arc<FixtureService>,
    }

    impl InteractiveSessionProvider for ChangedAccountSessionProvider {
        fn prepare(
            &self,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<InteractiveSessionPreparation, OneShotError>> {
            let service = self.service.clone();
            Box::pin(async move {
                let service_port: Arc<dyn ServicePort> = service;
                let ensure_session = Arc::new(EnsureSession::new(
                    service_port.clone(),
                    Arc::new(MemoryCredentialPort),
                    Arc::new(FixedClock),
                ));
                Ok(InteractiveSessionPreparation::from_service(
                    service_port,
                    ensure_session,
                    SessionSnapshot {
                        credentials: fixture_credentials_for("account-2"),
                        reconciliation_required: false,
                    },
                    "profile:read",
                ))
            })
        }
    }

    struct FreshHttpSessionProvider {
        service: Arc<HttpService>,
        authorization_scope: Arc<str>,
    }

    impl InteractiveSessionProvider for FreshHttpSessionProvider {
        fn prepare(
            &self,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<InteractiveSessionPreparation, OneShotError>> {
            let service = self.service.clone();
            let authorization_scope = self.authorization_scope.clone();
            Box::pin(async move {
                let service_port: Arc<dyn ServicePort> = service.clone();
                let ensure_session = Arc::new(EnsureSession::new(
                    service_port,
                    Arc::new(MemoryCredentialPort),
                    Arc::new(FixedClock),
                ));
                Ok(InteractiveSessionPreparation::new(
                    service,
                    ensure_session,
                    SessionSnapshot {
                        credentials: fixture_credentials(),
                        reconciliation_required: false,
                    },
                    authorization_scope,
                ))
            })
        }
    }

    fn fixture_credentials() -> SessionCredentials {
        fixture_credentials_for("account-1")
    }

    fn fixture_credentials_for(account_id: &str) -> SessionCredentials {
        SessionCredentials::from_unix_expiry(
            AccountId::parse(account_id).unwrap(),
            SensitiveString::new("access"),
            SensitiveString::new("refresh"),
            CredentialVersion::new(1),
            4_102_444_800,
        )
        .unwrap()
    }

    async fn read_complete_http_request_bytes(socket: &mut TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let header_end = loop {
            let mut buffer = [0_u8; 1024];
            let read = socket.read(&mut buffer).await.unwrap();
            assert!(read > 0);
            request.extend_from_slice(&buffer[..read]);
            if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
        };
        let headers = String::from_utf8(request[..header_end].to_vec()).unwrap();
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        while request.len() < header_end + content_length {
            let mut buffer = [0_u8; 1024];
            let read = socket.read(&mut buffer).await.unwrap();
            assert!(read > 0);
            request.extend_from_slice(&buffer[..read]);
        }
        request
    }

    async fn read_complete_http_request(socket: &mut TcpStream) -> String {
        String::from_utf8(read_complete_http_request_bytes(socket).await).unwrap()
    }

    async fn write_json_response(socket: &mut TcpStream, status: &str, body: Value) {
        let body = body.to_string();
        socket
            .write_all(
                format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn voice_vertical_transcribes_capture_without_submitting_an_agent_turn() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_complete_http_request_bytes(&mut socket).await;
            let request_text = String::from_utf8_lossy(&request);
            assert!(request_text.starts_with("POST /v1/audio/transcriptions HTTP/1.1\r\n"));
            assert!(
                request_text
                    .to_ascii_lowercase()
                    .contains("authorization: bearer channel-access")
            );
            assert!(request_text.contains("name=\"purpose\"\r\n\r\nask\r\n"));
            assert!(request_text.contains("name=\"file\"; filename=\"audio.wav\""));
            assert!(request.windows(4).any(|window| window == b"RIFF"));
            write_json_response(
                &mut socket,
                "200 OK",
                json!({
                    "transcript": "What should I make for dinner?",
                    "duration_seconds": 1.0,
                    "language": "en-US",
                    "model_version": "fixture-1"
                }),
            )
            .await;
            assert!(
                tokio::time::timeout(Duration::from_millis(25), listener.accept())
                    .await
                    .is_err(),
                "transcription must not implicitly submit an agent turn"
            );
        });
        let service_url =
            ServiceUrl::parse(&format!("http://{address}"), NetworkPolicy::DEVELOPMENT).unwrap();
        let service = Arc::new(
            HttpService::new(service_url, NetworkPolicy::DEVELOPMENT, Default::default())
                .unwrap()
                .with_cli_auth(
                    CliAuthContext::new(
                        "interactive-device",
                        SensitiveString::new("channel-access"),
                        None,
                    )
                    .unwrap(),
                ),
        );
        let capture = Arc::new(FixtureAudioCapture::default());
        let stop = CancellationToken::new();
        stop.cancel();
        let (events, _receiver) = mpsc::channel(8);
        let event = run_interactive_voice(
            7,
            capture.clone(),
            service,
            stop,
            CancellationToken::new(),
            events,
            None,
        )
        .await;
        assert!(matches!(
            event,
            RuntimeEvent::VoiceTranscriptReady {
                operation_id: 7,
                transcript
            } if transcript == "What should I make for dinner?"
        ));
        assert_eq!(capture.calls.load(Ordering::Relaxed), 1);
        server.await.unwrap();
    }

    #[test]
    fn voice_scope_preflight_never_opens_the_microphone() {
        let service_url =
            ServiceUrl::parse("http://127.0.0.1:1", NetworkPolicy::DEVELOPMENT).unwrap();
        let service = Arc::new(
            HttpService::new(service_url, NetworkPolicy::DEVELOPMENT, Default::default()).unwrap(),
        );
        let service_port: Arc<dyn ServicePort> = service.clone();
        let ensure_session = Arc::new(EnsureSession::new(
            service_port,
            Arc::new(MemoryCredentialPort),
            Arc::new(FixedClock),
        ));
        let capture = Arc::new(FixtureAudioCapture::default());
        let mut driver = InteractiveTurnDriver::new_http(
            service,
            ensure_session,
            SessionSnapshot {
                credentials: fixture_credentials(),
                reconciliation_required: false,
            },
            "profile:read",
        )
        .unwrap()
        .with_audio_capture(capture.clone());
        let (events, mut receiver) = mpsc::channel(8);
        driver.start_voice(11, events).unwrap();
        assert_eq!(
            driver.runtime.block_on(receiver.recv()).unwrap(),
            RuntimeEvent::VoiceAvailability(VoiceAvailability::AuthorizationRequired)
        );
        let event = driver.runtime.block_on(receiver.recv()).unwrap();
        assert!(matches!(
            event,
            RuntimeEvent::VoiceFailed {
                operation_id: 11,
                message
            } if message.contains("audio:transcribe") && message.contains("no microphone was opened")
        ));
        assert_eq!(capture.calls.load(Ordering::Relaxed), 0);
        driver
            .shutdown_and_join(QUALIFIED_SHUTDOWN_TIMEOUT)
            .unwrap();
    }

    #[test]
    fn fresh_voice_scope_can_authorize_when_launch_scope_is_stale() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let service_url = ServiceUrl::parse(
            &format!("http://{}", listener.local_addr().unwrap()),
            NetworkPolicy::DEVELOPMENT,
        )
        .unwrap();
        let service = Arc::new(
            HttpService::new(service_url, NetworkPolicy::DEVELOPMENT, Default::default()).unwrap(),
        );
        let service_port: Arc<dyn ServicePort> = service.clone();
        let ensure_session = Arc::new(EnsureSession::new(
            service_port,
            Arc::new(MemoryCredentialPort),
            Arc::new(FixedClock),
        ));
        let capture = Arc::new(FixtureAudioCapture::default());
        let mut driver = InteractiveTurnDriver::new_http(
            service.clone(),
            ensure_session,
            SessionSnapshot {
                credentials: fixture_credentials(),
                reconciliation_required: false,
            },
            "profile:read",
        )
        .unwrap()
        .with_session_provider(Arc::new(FreshHttpSessionProvider {
            service,
            authorization_scope: Arc::from("profile:read audio:transcribe"),
        }))
        .with_audio_capture(capture.clone());
        let (events, mut receiver) = mpsc::channel(8);

        driver.start_voice(12, events).unwrap();
        assert_eq!(
            driver.runtime.block_on(receiver.recv()).unwrap(),
            RuntimeEvent::VoiceAvailability(VoiceAvailability::Ready)
        );
        for _ in 0..100 {
            if capture.calls.load(Ordering::Relaxed) == 1 {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(capture.calls.load(Ordering::Relaxed), 1);
        driver.cancel_voice(12).unwrap();
        assert!(matches!(
            driver.runtime.block_on(receiver.recv()),
            Some(RuntimeEvent::VoiceCancelled { operation_id: 12 })
        ));
        driver
            .shutdown_and_join(QUALIFIED_SHUTDOWN_TIMEOUT)
            .unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock,
            "cancelling during capture must not dispatch transcription"
        );
    }

    #[test]
    fn missing_fresh_voice_scope_opens_neither_microphone_nor_network() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let service_url = ServiceUrl::parse(
            &format!("http://{}", listener.local_addr().unwrap()),
            NetworkPolicy::DEVELOPMENT,
        )
        .unwrap();
        let service = Arc::new(
            HttpService::new(service_url, NetworkPolicy::DEVELOPMENT, Default::default()).unwrap(),
        );
        let service_port: Arc<dyn ServicePort> = service.clone();
        let ensure_session = Arc::new(EnsureSession::new(
            service_port,
            Arc::new(MemoryCredentialPort),
            Arc::new(FixedClock),
        ));
        let capture = Arc::new(FixtureAudioCapture::default());
        let mut driver = InteractiveTurnDriver::new_http(
            service.clone(),
            ensure_session,
            SessionSnapshot {
                credentials: fixture_credentials(),
                reconciliation_required: false,
            },
            "profile:read audio:transcribe",
        )
        .unwrap()
        .with_session_provider(Arc::new(FreshHttpSessionProvider {
            service,
            authorization_scope: Arc::from("profile:read"),
        }))
        .with_audio_capture(capture.clone());
        let (events, mut receiver) = mpsc::channel(8);

        driver.start_voice(13, events).unwrap();
        assert_eq!(
            driver.runtime.block_on(receiver.recv()).unwrap(),
            RuntimeEvent::VoiceAvailability(VoiceAvailability::AuthorizationRequired)
        );
        assert!(matches!(
            driver.runtime.block_on(receiver.recv()),
            Some(RuntimeEvent::VoiceFailed {
                operation_id: 13,
                message
            }) if message.contains("audio:transcribe") && message.contains("no microphone was opened")
        ));
        driver
            .shutdown_and_join(QUALIFIED_SHUTDOWN_TIMEOUT)
            .unwrap();
        assert_eq!(capture.calls.load(Ordering::Relaxed), 0);
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock,
            "missing fresh scope must not dispatch transcription"
        );
    }

    #[derive(Default)]
    struct ControlledDriver {
        started: Vec<(u64, String)>,
        confirmed: Vec<(u64, AgentConfirmationCommandWire)>,
        cancelled: Vec<u64>,
        joined: bool,
    }

    impl QualifiedTurnDriver for ControlledDriver {
        fn start_turn(
            &mut self,
            operation_id: u64,
            prompt: String,
            _presented_household_context: Option<PresentedHouseholdContextV1>,
            events: mpsc::Sender<RuntimeEvent>,
        ) -> io::Result<()> {
            self.started.push((operation_id, prompt));
            events
                .try_send(RuntimeEvent::TurnEvent {
                    operation_id,
                    event: AgentEvent::Partial {
                        text: "controlled partial".into(),
                    },
                })
                .map_err(io::Error::other)
        }

        fn cancel_turn(&mut self, operation_id: u64) -> io::Result<()> {
            self.cancelled.push(operation_id);
            Ok(())
        }

        fn start_confirmation(
            &mut self,
            operation_id: u64,
            command: AgentConfirmationCommandWire,
            _presented_household_context: Option<PresentedHouseholdContextV1>,
            _events: mpsc::Sender<RuntimeEvent>,
        ) -> io::Result<()> {
            self.confirmed.push((operation_id, command));
            Ok(())
        }

        fn shutdown_and_join(&mut self, _timeout: Duration) -> io::Result<()> {
            self.joined = true;
            Ok(())
        }
    }

    #[test]
    fn interactive_driver_streams_and_retains_conversation_in_memory() {
        let service = Arc::new(FixtureService::default());
        let service_port: Arc<dyn ServicePort> = service.clone();
        let ensure_session = Arc::new(EnsureSession::new(
            service_port.clone(),
            Arc::new(MemoryCredentialPort),
            Arc::new(FixedClock),
        ));
        let mut driver = InteractiveTurnDriver::new(
            service_port,
            ensure_session,
            SessionSnapshot {
                credentials: fixture_credentials(),
                reconciliation_required: false,
            },
        )
        .unwrap();
        let (sender, mut receiver) = mpsc::channel(16);
        driver
            .start_session(sender.clone())
            .expect("native signal forwarding starts with the session");

        driver
            .start_turn(1, "first question".into(), None, sender.clone())
            .unwrap();
        let mut first_events = Vec::new();
        loop {
            let event = receiver.blocking_recv().expect("first turn event");
            let finished = matches!(
                event,
                RuntimeEvent::TurnFinished {
                    operation_id: 1,
                    outcome: RunTurnOutcome::Completed
                }
            );
            first_events.push(event);
            if finished {
                break;
            }
        }
        assert!(first_events.iter().any(|event| matches!(
            event,
            RuntimeEvent::TurnEvent {
                operation_id: 1,
                event: AgentEvent::Partial { text }
            } if text == "Hello "
        )));

        for _ in 0..100 {
            if driver.turns.iter().all(|turn| turn.task.is_finished()) {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        driver
            .start_turn(2, "follow up".into(), None, sender)
            .expect("completed turn is reaped before the next turn");
        loop {
            if matches!(
                receiver.blocking_recv().expect("second turn event"),
                RuntimeEvent::TurnFinished {
                    operation_id: 2,
                    outcome: RunTurnOutcome::Completed
                }
            ) {
                break;
            }
        }
        driver.shutdown_and_join(Duration::from_secs(1)).unwrap();

        let requests = service.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].conversation_id, None);
        assert_eq!(
            requests[1].conversation_id.as_deref(),
            Some("conversation-1")
        );
    }

    #[test]
    fn interactive_driver_reprepares_native_authority_before_every_turn() {
        let fallback_service = Arc::new(FixtureService::default());
        let first_service = Arc::new(FixtureService::default());
        let second_service = Arc::new(FixtureService::default());
        let service_port: Arc<dyn ServicePort> = fallback_service.clone();
        let ensure_session = Arc::new(EnsureSession::new(
            service_port.clone(),
            Arc::new(MemoryCredentialPort),
            Arc::new(FixedClock),
        ));
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(RotatingSessionProvider {
            calls: calls.clone(),
            services: StdMutex::new(VecDeque::from([
                first_service.clone(),
                second_service.clone(),
            ])),
        });
        let mut driver = InteractiveTurnDriver::new(
            service_port,
            ensure_session,
            SessionSnapshot {
                credentials: fixture_credentials(),
                reconciliation_required: false,
            },
        )
        .unwrap()
        .with_session_provider(provider);
        let (sender, mut receiver) = mpsc::channel(16);

        for (operation_id, prompt) in [(1, "first"), (2, "after expiry")] {
            driver
                .start_turn(operation_id, prompt.into(), None, sender.clone())
                .unwrap();
            loop {
                if matches!(
                    receiver.blocking_recv(),
                    Some(RuntimeEvent::TurnFinished {
                        operation_id: finished,
                        outcome: RunTurnOutcome::Completed
                    }) if finished == operation_id
                ) {
                    break;
                }
            }
            for _ in 0..100 {
                if driver.turns.iter().all(|turn| turn.task.is_finished()) {
                    break;
                }
                thread::sleep(Duration::from_millis(1));
            }
        }

        driver.shutdown_and_join(Duration::from_secs(1)).unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 2);
        assert!(fallback_service.requests.lock().unwrap().is_empty());
        assert_eq!(first_service.requests.lock().unwrap().len(), 1);
        assert_eq!(second_service.requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn interactive_driver_does_not_dispatch_when_sign_in_refresh_is_rejected() {
        let fallback_service = Arc::new(FixtureService::default());
        let service_port: Arc<dyn ServicePort> = fallback_service.clone();
        let ensure_session = Arc::new(EnsureSession::new(
            service_port.clone(),
            Arc::new(MemoryCredentialPort),
            Arc::new(FixedClock),
        ));
        let mut driver = InteractiveTurnDriver::new(
            service_port,
            ensure_session,
            SessionSnapshot {
                credentials: fixture_credentials(),
                reconciliation_required: false,
            },
        )
        .unwrap()
        .with_session_provider(Arc::new(RejectedSessionProvider));
        let (sender, mut receiver) = mpsc::channel(16);

        driver
            .start_turn(1, "What can I eat?".into(), None, sender)
            .unwrap();
        loop {
            if matches!(
                receiver.blocking_recv(),
                Some(RuntimeEvent::TurnFailed {
                    operation_id: 1,
                    failure: TurnFailure {
                        kind: TurnFailureKind::AuthenticationRequired,
                        ..
                    },
                })
            ) {
                break;
            }
        }

        driver.shutdown_and_join(Duration::from_secs(1)).unwrap();
        assert!(fallback_service.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn interactive_driver_does_not_carry_continuity_into_a_replaced_account() {
        let fallback_service = Arc::new(FixtureService::default());
        let replacement_service = Arc::new(FixtureService::default());
        let service_port: Arc<dyn ServicePort> = fallback_service.clone();
        let ensure_session = Arc::new(EnsureSession::new(
            service_port.clone(),
            Arc::new(MemoryCredentialPort),
            Arc::new(FixedClock),
        ));
        let mut driver = InteractiveTurnDriver::new(
            service_port,
            ensure_session,
            SessionSnapshot {
                credentials: fixture_credentials(),
                reconciliation_required: false,
            },
        )
        .unwrap()
        .with_session_provider(Arc::new(ChangedAccountSessionProvider {
            service: replacement_service.clone(),
        }));
        let (sender, mut receiver) = mpsc::channel(16);

        driver
            .start_turn(1, "What can I eat?".into(), None, sender)
            .unwrap();
        loop {
            if matches!(
                receiver.blocking_recv(),
                Some(RuntimeEvent::TurnFailed {
                    operation_id: 1,
                    failure: TurnFailure {
                        kind: TurnFailureKind::AuthenticationChanged,
                        ..
                    },
                })
            ) {
                break;
            }
        }

        driver.shutdown_and_join(Duration::from_secs(1)).unwrap();
        assert!(fallback_service.requests.lock().unwrap().is_empty());
        assert!(replacement_service.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn nonlegacy_onboarding_never_falls_through_to_the_released_remote_first_path() {
        for (mode, expected_message) in [
            (
                ProfilePresentationModeV1::NativeEnabled,
                "Native owner onboarding is unavailable until the local household session is ready.",
            ),
            (
                ProfilePresentationModeV1::NativeRollbackReadOnly,
                "Native household state is read-only in rollback mode; profile changes are unavailable.",
            ),
        ] {
            let service = Arc::new(FixtureService::default());
            let service_port: Arc<dyn ServicePort> = service.clone();
            let ensure_session = Arc::new(EnsureSession::new(
                service_port.clone(),
                Arc::new(MemoryCredentialPort),
                Arc::new(FixedClock),
            ));
            let mut driver = InteractiveTurnDriver::new(
                service_port,
                ensure_session,
                SessionSnapshot {
                    credentials: fixture_credentials(),
                    reconciliation_required: false,
                },
            )
            .unwrap()
            .with_profile_presentation_mode(mode);
            let (sender, mut receiver) = mpsc::channel(1);

            driver
                .start_onboarding(7, OnboardingProfileInput::default(), sender)
                .unwrap();

            assert!(matches!(
                receiver.blocking_recv(),
                Some(RuntimeEvent::OnboardingFailed {
                    operation_id: 7,
                    message
                }) if message == expected_message
            ));
            assert!(service.requests.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn interactive_household_target_resolves_to_a_process_local_member_id() {
        let service = Arc::new(FixtureService::default());
        let service_port: Arc<dyn ServicePort> = service.clone();
        let ensure_session = Arc::new(EnsureSession::new(
            service_port.clone(),
            Arc::new(MemoryCredentialPort),
            Arc::new(FixedClock),
        ));
        let state = ImportedPythonState {
            account_user_id: Some("account-1".into()),
            global: BTreeMap::new(),
            account_scoped: BTreeMap::from([(
                "household".into(),
                serde_json::json!({
                    "members": [{
                        "id": "member-sarah",
                        "name": "Sarah",
                        "relationship": "partner"
                    }]
                }),
            )]),
        };
        let mut driver = InteractiveTurnDriver::new(
            service_port,
            ensure_session,
            SessionSnapshot {
                credentials: fixture_credentials(),
                reconciliation_required: false,
            },
        )
        .unwrap()
        .with_local_state(Some(state));
        driver.runtime.block_on(async {
            driver.continuity.lock().await.conversation_id = Some("prior-household-turn".into());
        });
        let (sender, mut receiver) = mpsc::channel(4);
        driver
            .start_household_scope(1, "Sarah".into(), sender.clone())
            .unwrap();
        assert!(matches!(
            receiver.blocking_recv(),
            Some(RuntimeEvent::HouseholdScopeReady {
                operation_id: 1,
                label
            }) if label == "Sarah"
        ));
        driver.runtime.block_on(async {
            let continuity = driver.continuity.lock().await;
            assert_eq!(continuity.household_scope.as_deref(), Some("member-sarah"));
            assert_eq!(continuity.conversation_id, None);
        });
        for _ in 0..100 {
            if driver.turns.iter().all(|turn| turn.task.is_finished()) {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        driver
            .start_turn(2, "fresh household question".into(), None, sender)
            .unwrap();
        loop {
            if matches!(
                receiver.blocking_recv(),
                Some(RuntimeEvent::TurnFinished {
                    operation_id: 2,
                    outcome: RunTurnOutcome::Completed
                })
            ) {
                break;
            }
        }
        driver.shutdown_and_join(Duration::from_secs(1)).unwrap();
        assert_eq!(service.requests.lock().unwrap()[0].conversation_id, None);
    }

    #[tokio::test]
    async fn interactive_turn_sends_the_selected_household_context() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut consent, _) = listener.accept().await.unwrap();
            let request = read_complete_http_request(&mut consent).await;
            assert!(request.starts_with("GET /v1/profile/consent HTTP/1.1\r\n"));
            let body = r#"{"has_consent":true,"consent_version":1}"#;
            consent
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();

            let (mut profile, _) = listener.accept().await.unwrap();
            let request = read_complete_http_request(&mut profile).await;
            assert!(
                request.starts_with("GET /v1/profile/sync?member_id=member-sarah HTTP/1.1\r\n")
            );
            let body = serde_json::json!({
                "member_id": "member-sarah",
                "version": 3,
                "updated_at": "2026-07-22T00:00:00Z",
                "profile_data": {
                    "preferences": ["vegetarian"],
                    "avoid_ingredients": ["peanuts"]
                }
            })
            .to_string();
            profile
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();

            let (mut converse, _) = listener.accept().await.unwrap();
            let request = read_complete_http_request(&mut converse).await;
            assert!(request.starts_with("POST /v1/agent/converse HTTP/1.1\r\n"));
            let body = request.split_once("\r\n\r\n").unwrap().1;
            let body: Value = serde_json::from_str(body).unwrap();
            assert_eq!(body["dietary_context"]["name"], "Sarah");
            assert_eq!(body["dietary_context"]["preferences"][0], "vegetarian");
            assert_eq!(body["meal_context"]["active_member_id"], "member-sarah");
            assert_eq!(body["meal_context"]["active_member_name"], "Sarah");
            assert_eq!(
                body["device_context"]["household"]["members"][1]["id"],
                "member-sarah"
            );
            converse
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\nevent: result\ndata: {\"message\":\"done\"}\n\n",
                )
                .await
                .unwrap();
        });

        let service_url =
            ServiceUrl::parse(&format!("http://{address}"), NetworkPolicy::DEVELOPMENT).unwrap();
        let service = Arc::new(
            HttpService::new(service_url, NetworkPolicy::DEVELOPMENT, Default::default())
                .unwrap()
                .with_cli_auth(
                    CliAuthContext::new(
                        "interactive-device",
                        SensitiveString::new("channel-access"),
                        None,
                    )
                    .unwrap(),
                ),
        );
        let service_port: Arc<dyn ServicePort> = service.clone();
        let ensure_session = Arc::new(EnsureSession::new(
            service_port.clone(),
            Arc::new(MemoryCredentialPort),
            Arc::new(FixedClock),
        ));
        let state = Arc::new(ImportedPythonState {
            account_user_id: Some("account-1".into()),
            global: BTreeMap::new(),
            account_scoped: BTreeMap::from([(
                "household".into(),
                serde_json::json!({
                    "members": [{
                        "id": "member-sarah",
                        "name": "Sarah",
                        "relationship": "partner"
                    }]
                }),
            )]),
        });
        let (events, _receiver) = mpsc::channel(8);
        let outcome = run_interactive_turn(
            1,
            "What should Sarah eat?".into(),
            None,
            service_port,
            ensure_session,
            Arc::new(Mutex::new(SessionSnapshot {
                credentials: fixture_credentials(),
                reconciliation_required: false,
            })),
            Arc::new(Mutex::new(InteractiveContinuity {
                conversation_id: None,
                household_scope: Some("member-sarah".into()),
                ..InteractiveContinuity::default()
            })),
            Some(service),
            Some(state),
            None,
            None,
            CancellationToken::new(),
            events,
        )
        .await
        .unwrap();
        assert_eq!(outcome, RunTurnOutcome::Completed);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn interactive_onboarding_grants_consent_then_uses_the_observed_profile_version() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let responses = [
                ("GET /v1/profile/consent ", json!({"has_consent": false})),
                (
                    "POST /v1/profile/consent ",
                    json!({"has_consent": true, "consent_version": 1}),
                ),
                (
                    "GET /v1/profile/sync?member_id=_self ",
                    json!({"member_id": "_self", "version": 7, "profile_data": {}}),
                ),
                (
                    "PUT /v1/profile/sync ",
                    json!({"member_id": "_self", "version": 8}),
                ),
            ];
            for (index, (expected, response)) in responses.into_iter().enumerate() {
                let (mut socket, _) = listener.accept().await.unwrap();
                let request = read_complete_http_request(&mut socket).await;
                assert!(request.starts_with(expected));
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("authorization: bearer access")
                );
                if index == 1 {
                    let body: Value =
                        serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
                    assert_eq!(body, json!({"consent_version": 1}));
                }
                if index == 3 {
                    let body: Value =
                        serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
                    assert_eq!(body["member_id"], "_self");
                    assert_eq!(body["expected_version"], 7);
                    assert_eq!(body["profile_data"]["diet_style_ids"], json!(["vegan"]));
                    assert_eq!(body["profile_data"]["selection_provenance_version"], 1);
                }
                let response = response.to_string();
                socket
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                            response.len()
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
        });

        let service_url =
            ServiceUrl::parse(&format!("http://{address}"), NetworkPolicy::DEVELOPMENT).unwrap();
        let service = Arc::new(
            HttpService::new(service_url, NetworkPolicy::DEVELOPMENT, Default::default()).unwrap(),
        );
        let service_port: Arc<dyn ServicePort> = service.clone();
        let result = run_interactive_onboarding(
            OnboardingProfileInput {
                diet_style_ids: vec!["vegan".into()],
                ..OnboardingProfileInput::default()
            },
            service,
            Arc::new(EnsureSession::new(
                service_port,
                Arc::new(MemoryCredentialPort),
                Arc::new(FixedClock),
            )),
            Arc::new(Mutex::new(SessionSnapshot {
                credentials: fixture_credentials(),
                reconciliation_required: false,
            })),
            "profile:read profile:write",
            CancellationToken::new(),
        )
        .await;
        assert!(result.is_ok());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn interactive_onboarding_pre_dispatch_cancellation_opens_no_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let service_url =
            ServiceUrl::parse(&format!("http://{address}"), NetworkPolicy::DEVELOPMENT).unwrap();
        let service = Arc::new(
            HttpService::new(service_url, NetworkPolicy::DEVELOPMENT, Default::default()).unwrap(),
        );
        let service_port: Arc<dyn ServicePort> = service.clone();
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let result = run_interactive_onboarding(
            OnboardingProfileInput::default(),
            service,
            Arc::new(EnsureSession::new(
                service_port,
                Arc::new(MemoryCredentialPort),
                Arc::new(FixedClock),
            )),
            Arc::new(Mutex::new(SessionSnapshot {
                credentials: fixture_credentials(),
                reconciliation_required: false,
            })),
            "profile:read profile:write",
            cancellation,
        )
        .await;

        assert!(matches!(
            result,
            Err(OnboardingOperationError::Cancelled(
                RunTurnOutcome::CancelledBeforeServerAcceptance
            ))
        ));
        assert!(
            tokio::time::timeout(Duration::from_millis(25), listener.accept())
                .await
                .is_err(),
            "pre-dispatch cancellation must not open a connection"
        );
    }

    #[tokio::test]
    async fn cancellation_after_consent_proves_profile_upload_was_not_dispatched() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut status, _) = listener.accept().await.unwrap();
            assert!(
                read_complete_http_request(&mut status)
                    .await
                    .starts_with("GET /v1/profile/consent ")
            );
            write_json_response(&mut status, "200 OK", json!({"has_consent": false})).await;

            let (mut grant, _) = listener.accept().await.unwrap();
            assert!(
                read_complete_http_request(&mut grant)
                    .await
                    .starts_with("POST /v1/profile/consent ")
            );
            write_json_response(
                &mut grant,
                "200 OK",
                json!({"has_consent": true, "consent_version": 1}),
            )
            .await;
            listener
        });
        let service_url =
            ServiceUrl::parse(&format!("http://{address}"), NetworkPolicy::DEVELOPMENT).unwrap();
        let service =
            HttpService::new(service_url, NetworkPolicy::DEVELOPMENT, Default::default()).unwrap();
        let cancellation = CancellationToken::new();

        let consent_result =
            ensure_profile_sync_consent(&service, &fixture_credentials(), &cancellation).await;
        assert!(consent_result.is_ok());
        let listener = server.await.unwrap();
        cancellation.cancel();

        assert!(matches!(
            onboarding_cancellation_checkpoint(&cancellation),
            Err(OnboardingOperationError::Cancelled(
                RunTurnOutcome::CancelledBeforeServerAcceptance
            ))
        ));
        assert!(
            tokio::time::timeout(Duration::from_millis(25), listener.accept())
                .await
                .is_err(),
            "the profile upload must not be dispatched after the consent boundary cancellation"
        );
    }

    #[tokio::test]
    async fn cancellation_after_profile_upload_dispatch_is_outcome_unknown() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let cancellation = CancellationToken::new();
        let server_cancellation = cancellation.clone();
        let server = tokio::spawn(async move {
            let (mut consent, _) = listener.accept().await.unwrap();
            assert!(
                read_complete_http_request(&mut consent)
                    .await
                    .starts_with("GET /v1/profile/consent ")
            );
            write_json_response(&mut consent, "200 OK", json!({"has_consent": true})).await;

            let (mut profile, _) = listener.accept().await.unwrap();
            assert!(
                read_complete_http_request(&mut profile)
                    .await
                    .starts_with("GET /v1/profile/sync?member_id=_self ")
            );
            write_json_response(&mut profile, "404 Not Found", json!({})).await;

            let (mut upload, _) = listener.accept().await.unwrap();
            assert!(
                read_complete_http_request(&mut upload)
                    .await
                    .starts_with("PUT /v1/profile/sync ")
            );
            server_cancellation.cancel();
            tokio::time::sleep(Duration::from_millis(25)).await;
        });
        let service_url =
            ServiceUrl::parse(&format!("http://{address}"), NetworkPolicy::DEVELOPMENT).unwrap();
        let service = Arc::new(
            HttpService::new(service_url, NetworkPolicy::DEVELOPMENT, Default::default()).unwrap(),
        );
        let service_port: Arc<dyn ServicePort> = service.clone();

        let result = run_interactive_onboarding(
            OnboardingProfileInput::default(),
            service,
            Arc::new(EnsureSession::new(
                service_port,
                Arc::new(MemoryCredentialPort),
                Arc::new(FixedClock),
            )),
            Arc::new(Mutex::new(SessionSnapshot {
                credentials: fixture_credentials(),
                reconciliation_required: false,
            })),
            "profile:read profile:write",
            cancellation,
        )
        .await;

        assert!(matches!(
            result,
            Err(OnboardingOperationError::Cancelled(
                RunTurnOutcome::CancelledAfterDispatchOutcomeUnknown
            ))
        ));
        server.await.unwrap();
    }

    #[test]
    fn determinate_onboarding_response_is_not_reclassified_by_a_later_cancel() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let result = onboarding_service_error(PortError::new(
            "version_conflict",
            "the resource version changed",
        ));
        assert!(matches!(
            result,
            OnboardingOperationError::Failed(message) if message.starts_with("version_conflict:")
        ));
    }

    #[tokio::test]
    async fn interactive_onboarding_rejects_missing_write_scope_before_network_io() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let service_url =
            ServiceUrl::parse(&format!("http://{address}"), NetworkPolicy::DEVELOPMENT).unwrap();
        let service = Arc::new(
            HttpService::new(service_url, NetworkPolicy::DEVELOPMENT, Default::default()).unwrap(),
        );
        let service_port: Arc<dyn ServicePort> = service.clone();
        let result = run_interactive_onboarding(
            OnboardingProfileInput::default(),
            service,
            Arc::new(EnsureSession::new(
                service_port,
                Arc::new(MemoryCredentialPort),
                Arc::new(FixedClock),
            )),
            Arc::new(Mutex::new(SessionSnapshot {
                credentials: fixture_credentials(),
                reconciliation_required: false,
            })),
            "profile:read",
            CancellationToken::new(),
        )
        .await;
        assert!(matches!(
            result,
            Err(OnboardingOperationError::Failed(message)) if message.contains("profile:write")
        ));
        assert!(
            tokio::time::timeout(Duration::from_millis(25), listener.accept())
                .await
                .is_err(),
            "missing authorization must fail before opening a connection"
        );
    }

    #[test]
    fn controlled_driver_is_available_as_a_test_seam_without_a_binary_flag() {
        let (sender, mut receiver) = mpsc::channel(4);
        let mut driver = ControlledDriver::default();
        route_effect(
            &mut driver,
            &sender,
            Effect::SubmitTurn {
                operation_id: 7,
                prompt: "lunch".into(),
                presented_household_context: None,
            },
        )
        .unwrap();
        assert_eq!(driver.started, [(7, "lunch".into())]);
        assert!(matches!(
            receiver.try_recv(),
            Ok(RuntimeEvent::TurnEvent {
                operation_id: 7,
                event: AgentEvent::Partial { .. }
            })
        ));

        route_effect(
            &mut driver,
            &sender,
            Effect::ConfirmAction {
                operation_id: 8,
                command: AgentConfirmationCommandWire {
                    confirmation_id: heyfood_core::GroceryConfirmationId::parse(
                        "00000000-0000-4000-8000-000000000001",
                    )
                    .unwrap(),
                    idempotency_key: heyfood_core::GroceryIdempotencyKey::parse(
                        "00000000-0000-4000-8000-000000000002",
                    )
                    .unwrap(),
                    decision: heyfood_core::ConfirmationDecisionWire::Cancel,
                    edits: None,
                },
                presented_household_context: None,
            },
        )
        .unwrap();
        assert_eq!(driver.confirmed.len(), 1);
        assert_eq!(
            driver.confirmed[0].1.decision,
            heyfood_core::ConfirmationDecisionWire::Cancel
        );

        route_effect(&mut driver, &sender, Effect::CancelTurn { operation_id: 7 }).unwrap();
        assert_eq!(driver.cancelled, [7]);
    }

    #[test]
    fn platform_pre_journal_household_cancellation_is_classified_before_commit() {
        let error = PortError::new(
            "household_operation_cancelled",
            "sensitive cancellation detail",
        );
        assert_eq!(
            household_mutation_failure_v1(&error),
            HouseholdMutationFailureV1::BeforeCommitCancelled
        );
    }

    #[tokio::test]
    async fn interactive_panels_render_authenticated_and_account_bound_results() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let responses = [
            (
                "/v1/auth/capabilities",
                serde_json::json!({
                    "schema_version": 1,
                    "self_registration": {
                        "status": "available",
                        "regions": ["US"],
                        "identity_methods": ["sms", "email"]
                    },
                    "authorization": {
                        "loopback_pkce": true,
                        "device_code": true,
                        "identity_methods": ["sms", "email"]
                    },
                    "profile_readiness": true,
                    "application_capabilities": {"grocery": "v1"}
                }),
            ),
            (
                "/v1/grocery/list",
                serde_json::json!({
                    "id": "11111111-1111-4111-8111-111111111111",
                    "title": "Weekly groceries",
                    "state": "active",
                    "version": 3,
                    "items": [],
                    "created_at": "2026-07-22T00:00:00Z",
                    "updated_at": "2026-07-22T00:00:00Z"
                }),
            ),
            (
                "/v1/grocery/exclusions",
                serde_json::json!({"exclusions": ["pork", "raw onion"]}),
            ),
            (
                "/v1/menu/watch",
                serde_json::json!({
                    "watches": [{
                        "id": "00000000-0000-4000-8000-000000000010",
                        "restaurant_id": "0c1cb790-0000-4000-8000-000000000000",
                        "cadence": {"weekday": 3, "hour": 9},
                        "tz": "America/Chicago",
                        "active": true,
                        "notify": true,
                        "next_run_at": "2026-07-30T14:00:00Z",
                        "last_run_at": null,
                        "last_snapshot_id": null,
                        "created_at": "2026-07-23T12:00:00Z"
                    }],
                    "count": 1
                }),
            ),
            (
                "/v1/integrations",
                serde_json::json!({
                    "integrations": [{
                        "provider": "oura",
                        "status": "connected",
                        "connected_at": "2026-07-21T00:00:00Z",
                        "last_sync_at": "2026-07-22T00:00:00Z",
                        "scopes": []
                    }]
                }),
            ),
            (
                "/v1/health/context",
                serde_json::json!({
                    "status": "connected",
                    "provider": "oura",
                    "stale_since": null,
                    "data_freshness_hours": 2,
                    "sleep_avg": 82,
                    "readiness_avg": 78,
                    "activity_avg": 75,
                    "sleep_label": "good",
                    "readiness_label": "good",
                    "activity_label": "good",
                    "steps_avg": 8100,
                    "active_calories_avg": 540,
                    "stress_label": null,
                    "deep_sleep_label": null,
                    "goals": []
                }),
            ),
            (
                "/v1/profile/consent",
                serde_json::json!({"has_consent": true, "consent_version": 1}),
            ),
            (
                "/v1/profile/sync?member_id=_self",
                serde_json::json!({
                    "member_id": "_self",
                    "version": 7,
                    "updated_at": "2026-07-22T00:00:00Z",
                    "profile_data": {
                        "diet_style_ids": ["vegetarian"],
                        "allergy_ids": ["peanuts"],
                        "health_condition_ids": [],
                        "avoid_ingredients": ["raw onion"],
                        "cuisine_preferences": ["thai"],
                        "activity_level": "moderate"
                    }
                }),
            ),
            (
                "/v1/auth/capabilities",
                serde_json::json!({
                    "schema_version": 1,
                    "self_registration": {
                        "status": "available",
                        "regions": ["US"],
                        "identity_methods": ["sms", "email"]
                    },
                    "authorization": {
                        "loopback_pkce": true,
                        "device_code": true,
                        "identity_methods": ["sms", "email"]
                    },
                    "profile_readiness": true,
                    "application_capabilities": {"grocery": "v1"}
                }),
            ),
            (
                "/v1/profile/consent",
                serde_json::json!({"has_consent": true, "consent_version": 1}),
            ),
        ];
        let server = tokio::spawn(async move {
            for (expected_path, body) in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut buffer = [0_u8; 1024];
                    let read = socket.read(&mut buffer).await.unwrap();
                    assert!(read > 0);
                    request.extend_from_slice(&buffer[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8(request).unwrap();
                assert!(request.starts_with(&format!("GET {expected_path} HTTP/1.1\r\n")));
                if expected_path != "/v1/auth/capabilities" {
                    assert!(
                        request
                            .to_ascii_lowercase()
                            .contains("authorization: bearer access")
                    );
                }
                let body = body.to_string();
                socket
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
        });

        let service_url =
            ServiceUrl::parse(&format!("http://{address}"), NetworkPolicy::DEVELOPMENT).unwrap();
        let service = Arc::new(
            HttpService::new(service_url, NetworkPolicy::DEVELOPMENT, Default::default()).unwrap(),
        );
        let service_port: Arc<dyn ServicePort> = service.clone();
        let ensure_session = Arc::new(EnsureSession::new(
            service_port,
            Arc::new(MemoryCredentialPort),
            Arc::new(FixedClock),
        ));
        let session = Arc::new(Mutex::new(SessionSnapshot {
            credentials: fixture_credentials(),
            reconciliation_required: false,
        }));
        let missing_scope = run_interactive_panel(
            PanelRequest::Grocery,
            service.clone(),
            ensure_session.clone(),
            session.clone(),
            "health:read",
            InteractivePanelEnvironment {
                local_state: None,
                native_voice_available: false,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(missing_scope.contains("grocery:read"));

        let grocery = run_interactive_panel(
            PanelRequest::Grocery,
            service.clone(),
            ensure_session.clone(),
            session.clone(),
            "grocery:read health:read",
            InteractivePanelEnvironment {
                local_state: None,
                native_voice_available: false,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(grocery.contains("Weekly groceries  version 3"));
        assert!(grocery.contains("No grocery items."));
        assert!(grocery.contains("Never buy\n• pork\n• raw onion"));

        let watch = run_interactive_panel(
            PanelRequest::Watch,
            service.clone(),
            ensure_session.clone(),
            session.clone(),
            "menu:watch",
            InteractivePanelEnvironment {
                local_state: None,
                native_voice_available: false,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(watch.contains("Menu Watch"));
        assert!(watch.contains("Thursday 09:00 · active"));
        assert!(watch.contains("awaiting first successful baseline"));

        let health = run_interactive_panel(
            PanelRequest::Health,
            service.clone(),
            ensure_session.clone(),
            session.clone(),
            "grocery:read health:read",
            InteractivePanelEnvironment {
                local_state: None,
                native_voice_available: false,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(health.contains("• oura: connected"));
        assert!(health.contains("Health context: connected"));
        assert!(health.contains("Health context is informational and is not a diagnosis."));

        let profile = run_interactive_panel(
            PanelRequest::Profile,
            service.clone(),
            ensure_session.clone(),
            session.clone(),
            "profile:read",
            InteractivePanelEnvironment {
                local_state: None,
                native_voice_available: false,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(profile.contains("Profile sync consent: granted"));
        assert!(profile.contains("Version: 7"));
        assert!(profile.contains("Diet styles: vegetarian"));

        let status = run_interactive_panel(
            PanelRequest::Status,
            service.clone(),
            ensure_session.clone(),
            session.clone(),
            "profile:read grocery:read menu:watch health:read audio:transcribe",
            InteractivePanelEnvironment {
                local_state: None,
                native_voice_available: true,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(status.contains("Session: active"));
        assert!(status.contains("Service: reachable"));
        assert!(status.contains("Profile: authorized · sync consent granted"));
        assert!(status.contains("Grocery: available · authorized"));
        assert!(status.contains("Menu Watch: authorized · create/list/remove available"));
        assert!(status.contains("Health integrations: deferred from v0.6.3"));
        assert!(status.contains(
            "Voice: native capture available · transcription authorized · permission checked on use"
        ));

        let local_state = Arc::new(ImportedPythonState {
            account_user_id: Some("account-1".into()),
            global: BTreeMap::new(),
            account_scoped: BTreeMap::from([
                (
                    "household".into(),
                    serde_json::json!({
                        "active_scope": "member-sarah",
                        "members": [{
                            "id": "member-sarah",
                            "name": "Sarah",
                            "relationship": "partner"
                        }]
                    }),
                ),
                (
                    "location".into(),
                    serde_json::json!({
                        "label": "San Luis Obispo, CA",
                        "latitude": 35.2828,
                        "longitude": -120.6596
                    }),
                ),
            ]),
        });
        let household = run_interactive_panel(
            PanelRequest::Household,
            service.clone(),
            ensure_session.clone(),
            session.clone(),
            "",
            InteractivePanelEnvironment {
                local_state: Some(local_state.clone()),
                native_voice_available: false,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(household.contains("Active scope: Sarah"));
        assert!(household.contains("Sarah — partner"));
        let location = run_interactive_panel(
            PanelRequest::Location,
            service,
            ensure_session,
            session,
            "",
            InteractivePanelEnvironment {
                local_state: Some(local_state),
                native_voice_available: false,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(location.contains("San Luis Obispo, CA"));
        assert!(location.contains("Latitude: 35.28280"));
        server.await.unwrap();
    }

    #[test]
    fn supervisor_shutdown_failure_cannot_be_reported_as_a_clean_exit() {
        let error = finish_session(
            Ok(ExitReason::Requested),
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "worker did not join",
            )),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            CompositionError::Driver(error) if error.kind() == io::ErrorKind::TimedOut
        ));
    }
}
