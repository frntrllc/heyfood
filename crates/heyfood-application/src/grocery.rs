//! Grocery application boundaries over the imported and independently approved
//! Phase-A authority. Runtime adapters remain capability-gated and must not
//! activate before the production canary gate passes.

use std::collections::BTreeMap;

use heyfood_core::{
    AccountId, ContextFingerprint, FrozenGroceryPreconditions, GroceryCapability,
    GroceryConfirmation, GroceryConfirmationCommand, GroceryEntityId, GroceryListVersion,
    HouseholdContextHashVersion, OperationId, SensitiveString, SessionCredentials,
};
use tokio_util::sync::CancellationToken;

use crate::{BoxFuture, PortError};

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
