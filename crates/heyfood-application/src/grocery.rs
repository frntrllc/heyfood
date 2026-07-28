//! Grocery application boundaries over the imported and independently approved
//! Phase-A authority. Runtime adapters remain capability-gated and must not
//! activate before the production canary gate passes.

use std::{collections::BTreeMap, sync::Arc};

use heyfood_core::{
    AccountId, ContextFingerprint, FrozenGroceryPreconditions, GroceryCapability,
    GroceryConfirmation, GroceryConfirmationCommand, GroceryEntityId, GroceryItemStateWire,
    GroceryListVersion, GrocerySafetyStatus, HouseholdContextHashVersion, OperationId,
    SensitiveString, SessionCredentials,
};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::{BoxFuture, CapabilitySnapshot, PortError};

#[derive(Clone, PartialEq, Serialize)]
pub struct GroceryDisplaySource {
    pub source_type: String,
    pub source_ref: Option<String>,
    pub source_detail: Option<String>,
}

#[derive(Clone, PartialEq, Serialize)]
pub struct GroceryDisplayMemberFlag {
    pub member_id: String,
    pub status: GrocerySafetyStatus,
    pub reason: Option<String>,
    pub substitutions: Vec<String>,
}

#[derive(Clone, PartialEq, Serialize)]
pub struct GroceryDisplaySafety {
    pub basis: String,
    pub status: GrocerySafetyStatus,
    pub member_flags: Vec<GroceryDisplayMemberFlag>,
    pub model_version: Option<String>,
    pub rules_version: Option<String>,
    pub confidence: Option<f64>,
    pub context_hash: Option<String>,
    pub context_hash_version: Option<i64>,
    pub label_hint: String,
}

#[derive(Clone, PartialEq, Serialize)]
pub struct GroceryDisplayItem {
    pub id: String,
    pub requested_name: String,
    pub canonical_name: String,
    pub quantity: Option<f64>,
    pub unit: Option<String>,
    pub package_quantity: Option<i64>,
    pub note: Option<String>,
    pub state: GroceryItemStateWire,
    pub intended_for: Option<String>,
    pub sources: Vec<GroceryDisplaySource>,
    pub safety: Option<GroceryDisplaySafety>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, PartialEq, Serialize)]
pub struct GroceryDisplayList {
    pub id: String,
    pub title: String,
    pub state: String,
    pub version: u64,
    pub items: Vec<GroceryDisplayItem>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GroceryExclusions {
    pub exclusions: Vec<String>,
}

/// Deployed display-read seam. Unlike `GroceryPort`, this deliberately carries
/// no context fingerprint and grants no mutation authority.
pub trait GroceryReadPort: Send + Sync {
    fn read_active_display(
        &self,
        capabilities: CapabilitySnapshot,
        credentials: SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<GroceryDisplayList, PortError>>;

    fn read_exclusions(
        &self,
        capabilities: CapabilitySnapshot,
        credentials: SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<GroceryExclusions, PortError>>;
}

pub struct ReadActiveGroceryDisplay<'a> {
    port: &'a dyn GroceryReadPort,
}

impl<'a> ReadActiveGroceryDisplay<'a> {
    #[must_use]
    pub const fn new(port: &'a dyn GroceryReadPort) -> Self {
        Self { port }
    }

    pub async fn execute(
        &self,
        capabilities: CapabilitySnapshot,
        credentials: SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<GroceryDisplayList, PortError> {
        if cancellation.is_cancelled() {
            return Err(PortError::new(
                "grocery_read_cancelled_before_dispatch",
                "Grocery read was cancelled before dispatch",
            ));
        }
        self.port
            .read_active_display(capabilities, credentials, operation_id, cancellation)
            .await
    }
}

pub struct ReadGroceryExclusions<'a> {
    port: &'a dyn GroceryReadPort,
}

impl<'a> ReadGroceryExclusions<'a> {
    #[must_use]
    pub const fn new(port: &'a dyn GroceryReadPort) -> Self {
        Self { port }
    }

    pub async fn execute(
        &self,
        capabilities: CapabilitySnapshot,
        credentials: SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<GroceryExclusions, PortError> {
        if cancellation.is_cancelled() {
            return Err(PortError::new(
                "grocery_exclusions_cancelled_before_dispatch",
                "Grocery exclusions read was cancelled before dispatch",
            ));
        }
        self.port
            .read_exclusions(capabilities, credentials, operation_id, cancellation)
            .await
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroceryListSnapshot {
    pub preconditions: FrozenGroceryPreconditions,
    /// Item labels are sensitive and redact from diagnostics.
    pub items: Vec<(GroceryEntityId, SensitiveString)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GroceryMutationIntent {
    Add { items: Vec<SensitiveString> },
    Remove { item_ids: Vec<GroceryEntityId> },
    MarkBought { item_ids: Vec<GroceryEntityId> },
    WeeklyFromRecipes { recipe_ids: Vec<GroceryEntityId> },
    NeverBuy { item_ids: Vec<GroceryEntityId> },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedGroceryMutation {
    pub confirmation: GroceryConfirmation,
}

/// Provider-neutral service seam. Phase 2 adapters may bind this to final
/// contract-derived DTOs; runtime activation remains separately gated.
pub trait GroceryPort: Send + Sync {
    fn capability(
        &self,
        credentials: SessionCredentials,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<GroceryCapability, PortError>>;

    fn read_active_list(
        &self,
        credentials: SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<GroceryListSnapshot, PortError>>;

    fn prepare_mutation(
        &self,
        credentials: SessionCredentials,
        operation_id: OperationId,
        expected: FrozenGroceryPreconditions,
        intent: GroceryMutationIntent,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<PreparedGroceryMutation, PortError>>;

    fn decide_confirmation(
        &self,
        credentials: SessionCredentials,
        operation_id: OperationId,
        command: GroceryConfirmationCommand,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<GroceryListSnapshot, PortError>>;
}

/// Renderer-neutral active-list read shared by future CLI, TUI, and MCP
/// adapters.
///
/// This Phase 0 controller deliberately exposes no proposal or confirmation
/// authority. The caller supplies already-reconciled account credentials and a
/// cancellation token owned by the surrounding operation supervisor.
#[derive(Clone)]
pub struct ReadActiveGroceryList {
    port: Arc<dyn GroceryPort>,
}

impl ReadActiveGroceryList {
    #[must_use]
    pub fn new(port: Arc<dyn GroceryPort>) -> Self {
        Self { port }
    }

    pub async fn execute(
        &self,
        credentials: SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<GroceryListSnapshot, PortError> {
        if cancellation.is_cancelled() {
            return Err(PortError::new(
                "grocery_read_cancelled_before_dispatch",
                "Grocery read was cancelled before dispatch",
            ));
        }
        self.port
            .read_active_list(credentials, operation_id, cancellation)
            .await
    }
}

/// Exact ownership key for the short-lived item-index convenience cache.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroceryCacheKey {
    pub api_origin: String,
    pub context: String,
    pub account_id: AccountId,
    pub list_id: GroceryEntityId,
    pub list_version: GroceryListVersion,
    pub context_fingerprint: ContextFingerprint,
    pub household_context_hash_version: Option<HouseholdContextHashVersion>,
}

impl GroceryCacheKey {
    pub fn new(
        api_origin: impl Into<String>,
        context: impl Into<String>,
        account_id: AccountId,
        preconditions: &FrozenGroceryPreconditions,
    ) -> Result<Self, &'static str> {
        let api_origin = api_origin.into();
        let api_origin =
            heyfood_core::ServiceUrl::parse(&api_origin, heyfood_core::NetworkPolicy::DEVELOPMENT)
                .map_err(|_| "grocery cache origin is not an approved service origin")?
                .to_string();
        let context = context.into();
        if context.is_empty() || context.len() > 128 || context.chars().any(char::is_control) {
            return Err("grocery cache context is invalid");
        }
        Ok(Self {
            api_origin,
            context,
            account_id,
            list_id: preconditions.list_id,
            list_version: preconditions.list_version,
            context_fingerprint: preconditions.context_fingerprint.clone(),
            household_context_hash_version: preconditions.household_context_hash_version,
        })
    }
}

/// Non-authoritative index-to-server-ID cache. It deliberately stores no item
/// names, annotations, member data, or purchase history.
#[derive(Default)]
pub struct GroceryItemReferenceCache {
    entry: Option<CacheEntry>,
}

struct CacheEntry {
    key: GroceryCacheKey,
    expires_at_unix: i64,
    references: BTreeMap<u32, GroceryEntityId>,
}

impl GroceryItemReferenceCache {
    pub const LIFETIME_SECONDS: i64 = 15 * 60;

    pub fn replace(
        &mut self,
        key: GroceryCacheKey,
        now_unix: i64,
        references: impl IntoIterator<Item = (u32, GroceryEntityId)>,
    ) {
        self.entry = Some(CacheEntry {
            key,
            expires_at_unix: now_unix.saturating_add(Self::LIFETIME_SECONDS),
            references: references.into_iter().collect(),
        });
    }

    #[must_use]
    pub fn resolve(
        &mut self,
        key: &GroceryCacheKey,
        now_unix: i64,
        index: u32,
    ) -> Option<GroceryEntityId> {
        let entry = self.entry.as_ref()?;
        if now_unix >= entry.expires_at_unix || &entry.key != key {
            self.entry = None;
            return None;
        }
        entry.references.get(&index).copied()
    }

    pub fn invalidate(&mut self) {
        self.entry = None;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use heyfood_core::{CredentialVersion, GroceryCapability};

    use super::*;
    use crate::RegistrationAvailability;

    struct RejectingReadPort {
        calls: AtomicUsize,
    }

    impl RejectingReadPort {
        fn called<T>(&self) -> BoxFuture<'_, Result<T, PortError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {
                Err(PortError::new(
                    "unexpected_dispatch",
                    "test read port must not be called",
                ))
            })
        }
    }

    impl GroceryReadPort for RejectingReadPort {
        fn read_active_display(
            &self,
            _capabilities: CapabilitySnapshot,
            _credentials: SessionCredentials,
            _operation_id: OperationId,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<GroceryDisplayList, PortError>> {
            self.called()
        }

        fn read_exclusions(
            &self,
            _capabilities: CapabilitySnapshot,
            _credentials: SessionCredentials,
            _operation_id: OperationId,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<GroceryExclusions, PortError>> {
            self.called()
        }
    }

    fn capabilities() -> CapabilitySnapshot {
        CapabilitySnapshot {
            schema_version: 1,
            registration: RegistrationAvailability::Disabled,
            profile_readiness: true,
            loopback_pkce: true,
            device_code: true,
            grocery: GroceryCapability::V1,
        }
    }

    fn credentials() -> SessionCredentials {
        SessionCredentials::from_unix_expiry(
            AccountId::parse("grocery-display-test").unwrap(),
            SensitiveString::new("access"),
            SensitiveString::new("refresh"),
            CredentialVersion::new(1),
            4_102_444_800,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn deployed_display_controllers_stop_before_a_cancelled_dispatch() {
        let port = RejectingReadPort {
            calls: AtomicUsize::new(0),
        };
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let list_error = match ReadActiveGroceryDisplay::new(&port)
            .execute(
                capabilities(),
                credentials(),
                OperationId::new(),
                cancellation.child_token(),
            )
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("cancelled display read must not dispatch"),
        };
        let exclusions_error = match ReadGroceryExclusions::new(&port)
            .execute(
                capabilities(),
                credentials(),
                OperationId::new(),
                cancellation,
            )
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("cancelled exclusions read must not dispatch"),
        };

        assert_eq!(list_error.code, "grocery_read_cancelled_before_dispatch");
        assert_eq!(
            exclusions_error.code,
            "grocery_exclusions_cancelled_before_dispatch"
        );
        assert_eq!(port.calls.load(Ordering::SeqCst), 0);
    }
}
