//! Provider-neutral health semantics. Provider credentials and raw samples are
//! intentionally not representable in this crate.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::SensitiveString;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthProvider {
    Oura,
    FunctionHealth,
    AppleHealth,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthConnectionStatus {
    Connected,
    Stale,
    NotConnected,
    Expired,
    Revoked,
    Disconnected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrendDirection {
    Improving,
    Stable,
    Declining,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HealthMetric {
    pub key: String,
    /// Health values are sensitive and redact from `Debug`/`Display` output.
    pub value: SensitiveString,
    pub label: Option<SensitiveString>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HealthTrend {
    pub metric: HealthMetric,
    pub direction: TrendDirection,
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct HealthFreshness {
    pub status: HealthConnectionStatus,
    pub provider: Option<HealthProvider>,
    pub data_freshness_hours: Option<u32>,
    pub stale_since: Option<String>,
}

impl fmt::Debug for HealthFreshness {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HealthFreshness([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HealthCapability {
    pub read: bool,
    pub manage_integrations: bool,
    pub h3_daily_aggregates: bool,
}

impl HealthCapability {
    pub const H1_H2_UNAVAILABLE: Self = Self {
        read: false,
        manage_integrations: false,
        h3_daily_aggregates: false,
    };

    #[must_use]
    pub const fn from_scopes(read: bool, manage_integrations: bool) -> Self {
        Self {
            read,
            manage_integrations,
            // H3 requires a separately reviewed server capability. Scopes alone
            // can never turn it on.
            h3_daily_aggregates: false,
        }
    }
}
