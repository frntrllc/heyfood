//! Phase 2 wire DTOs derived from the independently approved contract freezes.
//!
//! Grocery types are bound to the 14-file Phase-A import approved at
//! `47282aea7047b1f3bb0642fff9d09b106fa1bb0c`. Health H1/H2 types are bound to
//! `fixtures/contracts/health-h1h2.v1.json` and its exact source commit. These
//! types deliberately contain no retailer/provider credential representation.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::{
    GroceryConfirmationId, GroceryEntityId, GroceryIdempotencyKey, GroceryListVersion,
    GrocerySafetyStatus, HealthConnectionStatus, HealthFreshnessStatus, HealthProvider,
};

pub const GROCERY_WIRE_CONTRACT_VERSION: u16 = 1;
pub const GROCERY_WIRE_SCHEMA_SHA256: &str =
    "783472779f3f1209c1daca6d33088b36415e5ea8b51ec6113750d453e0654930";
pub const HEALTH_H1_H2_SOURCE_COMMIT: &str = "7cfadc55c103257b588b237c65fe7b5031a3f745";

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationCapabilitiesWire {
    pub schema_version: u16,
    pub self_registration: SelfRegistrationCapabilityWire,
    pub authorization: AuthorizationCapabilityWire,
    pub profile_readiness: bool,
    #[serde(default)]
    pub application_capabilities: BTreeMap<String, String>,
}

impl ApplicationCapabilitiesWire {
    #[must_use]
    pub fn application_version(&self, name: &str) -> Option<&str> {
        self.application_capabilities.get(name).map(String::as_str)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SelfRegistrationCapabilityWire {
    pub status: SelfRegistrationStatusWire,
    pub regions: Vec<String>,
    pub identity_methods: Vec<IdentityMethodWire>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelfRegistrationStatusWire {
    Available,
    Disabled,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationCapabilityWire {
    pub loopback_pkce: bool,
    pub device_code: bool,
    pub identity_methods: Vec<IdentityMethodWire>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityMethodWire {
    Sms,
    Email,
    Apple,
    Google,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroceryItemStateWire {
    Active,
    Purchased,
    Dismissed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroceryMutationOperationWire {
    AddItems,
    RemoveItems,
    UpdateItemState,
    AddExclusion,
    RemoveExclusion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroceryDecisionWire {
    Accept,
    Cancel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GroceryMutationStatusWire {
    Committed,
    Cancelled,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemSourceWire {
    pub source_type: String,
    #[serde(default)]
    pub source_ref: Option<String>,
    #[serde(default)]
    pub source_detail: Option<String>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberFlagWire {
    pub member_id: String,
    pub status: GrocerySafetyStatus,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub substitutions: Vec<String>,
}

fn ingredient_basis() -> String {
    "ingredient".into()
}

fn ingredient_label_hint() -> String {
    "Screened at ingredient level — verify the product label.".into()
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafetyAnnotationWire {
    #[serde(default = "ingredient_basis")]
    pub basis: String,
    pub status: GrocerySafetyStatus,
    #[serde(default)]
    pub member_flags: Vec<MemberFlagWire>,
    #[serde(default)]
    pub model_version: Option<String>,
    #[serde(default)]
    pub rules_version: Option<String>,
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub context_hash: Option<String>,
    #[serde(default)]
    pub context_hash_version: Option<i64>,
    #[serde(default = "ingredient_label_hint")]
    pub label_hint: String,
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GroceryItemWire {
    pub id: String,
    pub requested_name: String,
    pub canonical_name: String,
    #[serde(default)]
    pub quantity: Option<f64>,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub package_quantity: Option<i64>,
    #[serde(default)]
    pub note: Option<String>,
    pub state: GroceryItemStateWire,
    #[serde(default)]
    pub intended_for: Option<String>,
    #[serde(default)]
    pub sources: Vec<ItemSourceWire>,
    #[serde(default)]
    pub safety: Option<SafetyAnnotationWire>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GroceryListWire {
    pub id: String,
    pub title: String,
    pub state: String,
    pub version: u64,
    #[serde(default)]
    pub items: Vec<GroceryItemWire>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GroceryItemInputWire {
    pub name: String,
    #[serde(default)]
    pub quantity: Option<f64>,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub package_quantity: Option<i64>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub intended_for: Option<String>,
    #[serde(default = "manual_source")]
    pub source_type: String,
    #[serde(default)]
    pub source_ref: Option<String>,
    #[serde(default)]
    pub source_detail: Option<String>,
}

fn manual_source() -> String {
    "manual".into()
}

#[derive(Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AddItemsRequestWire {
    pub list_id: GroceryEntityId,
    pub expected_version: GroceryListVersion,
    pub items: Vec<GroceryItemInputWire>,
}

#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoveItemsRequestWire {
    pub list_id: GroceryEntityId,
    pub expected_version: GroceryListVersion,
    pub item_ids: Vec<String>,
}

#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateItemStateRequestWire {
    pub list_id: GroceryEntityId,
    pub expected_version: GroceryListVersion,
    pub item_id: String,
    pub state: GroceryItemStateWire,
}

#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExclusionMutationRequestWire {
    pub name: String,
    pub list_id: GroceryEntityId,
    pub expected_version: GroceryListVersion,
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProposedItemWire {
    pub requested_name: String,
    pub canonical_name: String,
    #[serde(default)]
    pub quantity: Option<f64>,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub package_quantity: Option<i64>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub intended_for: Option<String>,
    #[serde(default)]
    pub sources: Vec<ItemSourceWire>,
    #[serde(default)]
    pub safety: Option<SafetyAnnotationWire>,
}

#[derive(Clone, Eq, PartialEq)]
pub struct GroceryConfirmationToken(String);

impl GroceryConfirmationToken {
    pub const MIN_BYTES: usize = 32;
    pub const MAX_BYTES: usize = 131_072;

    pub fn parse(value: String) -> Result<Self, &'static str> {
        if !(Self::MIN_BYTES..=Self::MAX_BYTES).contains(&value.len())
            || value.chars().any(char::is_control)
        {
            return Err("grocery confirmation token is invalid");
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn expose_at_transport_boundary(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for GroceryConfirmationToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GroceryConfirmationToken([REDACTED])")
    }
}

impl Serialize for GroceryConfirmationToken {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for GroceryConfirmationToken {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GroceryMutationProposalWire {
    pub confirmation_id: GroceryConfirmationId,
    pub idempotency_key: GroceryIdempotencyKey,
    pub operation: GroceryMutationOperationWire,
    pub expires_at: String,
    pub structured_preview: Map<String, Value>,
    pub preconditions: Vec<Map<String, Value>>,
    pub confirmation_token: GroceryConfirmationToken,
}

impl fmt::Debug for GroceryMutationProposalWire {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GroceryMutationProposalWire")
            .field("confirmation_id", &self.confirmation_id)
            .field("idempotency_key", &self.idempotency_key)
            .field("operation", &self.operation)
            .field("expires_at", &self.expires_at)
            .field("structured_preview", &"[REDACTED]")
            .field("precondition_count", &self.preconditions.len())
            .field("confirmation_token", &self.confirmation_token)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GroceryMutationConfirmRequestWire {
    pub confirmation_token: GroceryConfirmationToken,
    pub decision: GroceryDecisionWire,
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GroceryMutationResultWire {
    pub status: GroceryMutationStatusWire,
    pub operation: GroceryMutationOperationWire,
    pub confirmation_id: GroceryConfirmationId,
    #[serde(default)]
    pub list: Option<GroceryListWire>,
    #[serde(default)]
    pub exclusions: Option<Vec<String>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionConflictDetailWire {
    #[serde(default = "version_conflict_reason")]
    pub reason: String,
    pub expected_version: u64,
    pub actual_version: u64,
}

fn version_conflict_reason() -> String {
    "version_conflict".into()
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HealthContextWire {
    pub status: HealthFreshnessStatus,
    #[serde(deserialize_with = "required_nullable")]
    pub provider: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub stale_since: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub data_freshness_hours: Option<u32>,
    #[serde(deserialize_with = "required_nullable")]
    pub sleep_avg: Option<i64>,
    #[serde(deserialize_with = "required_nullable")]
    pub readiness_avg: Option<i64>,
    #[serde(deserialize_with = "required_nullable")]
    pub activity_avg: Option<i64>,
    #[serde(deserialize_with = "required_nullable")]
    pub sleep_label: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub readiness_label: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub activity_label: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub steps_avg: Option<i64>,
    #[serde(deserialize_with = "required_nullable")]
    pub active_calories_avg: Option<i64>,
    #[serde(deserialize_with = "required_nullable")]
    pub stress_label: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub deep_sleep_label: Option<String>,
    #[serde(default)]
    pub goals: Vec<Value>,
}

fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

impl fmt::Debug for HealthContextWire {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HealthContextWire")
            .field("status", &self.status)
            .field("provider_present", &self.provider.is_some())
            .field("stale_since_present", &self.stale_since.is_some())
            .field("goals_count", &self.goals.len())
            .field("health_values", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationStatusWire {
    pub provider: HealthProvider,
    pub status: HealthConnectionStatus,
    pub connected_at: Option<String>,
    pub last_sync_at: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

impl fmt::Debug for IntegrationStatusWire {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IntegrationStatusWire")
            .field("provider", &self.provider)
            .field("status", &self.status)
            .field("timestamps", &"[REDACTED]")
            .field("scope_count", &self.scopes.len())
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationListWire {
    #[serde(default)]
    pub integrations: Vec<IntegrationStatusWire>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationAuthorizeRequestWire {
    pub device_id: String,
    pub provider: HealthProvider,
    pub redirect_target: IntegrationRedirectTargetWire,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationRedirectTargetWire {
    Mobile,
    Cli,
}

#[derive(Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationAuthorizeResponseWire {
    pub auth_url: String,
    pub provider: HealthProvider,
}

impl fmt::Debug for IntegrationAuthorizeResponseWire {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IntegrationAuthorizeResponseWire")
            .field("auth_url", &"[REDACTED]")
            .field("provider", &self.provider)
            .finish()
    }
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SuggestedGoalWire {
    pub target: String,
    pub direction: String,
    pub amount: Option<String>,
    pub priority: String,
    #[serde(default)]
    pub constraints: Vec<String>,
    pub source: String,
    pub source_detail: Option<String>,
}

impl fmt::Debug for SuggestedGoalWire {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SuggestedGoalWire([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationSyncResponseWire {
    pub provider: HealthProvider,
    #[serde(default)]
    pub suggested_goals: Vec<SuggestedGoalWire>,
    pub data_period_start: Option<String>,
    pub data_period_end: Option<String>,
}

impl fmt::Debug for IntegrationSyncResponseWire {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IntegrationSyncResponseWire")
            .field("provider", &self.provider)
            .field("goal_count", &self.suggested_goals.len())
            .field("period", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationDisconnectResponseWire {
    pub provider: HealthProvider,
    pub status: HealthConnectionStatus,
    pub message: String,
}

impl fmt::Debug for IntegrationDisconnectResponseWire {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IntegrationDisconnectResponseWire")
            .field("provider", &self.provider)
            .field("status", &self.status)
            .field("message", &"[REDACTED]")
            .finish()
    }
}
