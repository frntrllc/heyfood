//! Minimal native configuration snapshot used by Phase 0 operations.

use serde::{Deserialize, Serialize};

use crate::ServiceUrl;

#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct ConfigRevision(u64);

impl ConfigRevision {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Immutable configuration captured at operation start.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClientConfig {
    pub active_context: String,
    pub api_url: ServiceUrl,
    pub auth_url: ServiceUrl,
    pub revision: ConfigRevision,
}
