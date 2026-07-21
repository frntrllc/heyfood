//! Versioned native configuration captured by application operations.

use serde::{Deserialize, Deserializer, Serialize};

use crate::ServiceUrl;

/// The on-disk native configuration schema. This is deliberately independent
/// from the monotonically increasing user configuration revision.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConfigSchemaVersion(u16);

impl ConfigSchemaVersion {
    #[must_use]
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Schema 2 added explicit account binding; schema 3 bounds the durable replay
/// window. The ordinary configuration document remains credential-free.
pub const CURRENT_CONFIG_SCHEMA: ConfigSchemaVersion = ConfigSchemaVersion::new(3);

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
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ClientConfig {
    pub active_context: String,
    pub api_url: ServiceUrl,
    pub auth_url: ServiceUrl,
    pub revision: ConfigRevision,
}

impl<'de> Deserialize<'de> for ClientConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawClientConfig {
            active_context: String,
            api_url: ServiceUrl,
            auth_url: ServiceUrl,
            revision: ConfigRevision,
        }

        let raw = RawClientConfig::deserialize(deserializer)?;
        let config = Self {
            active_context: raw.active_context,
            api_url: raw.api_url,
            auth_url: raw.auth_url,
            revision: raw.revision,
        };
        config.validate().map_err(serde::de::Error::custom)?;
        Ok(config)
    }
}

impl ClientConfig {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.active_context.is_empty() {
            return Err("active context must not be empty");
        }
        if self.active_context.len() > 128 {
            return Err("active context exceeds 128 bytes");
        }
        if self.active_context.trim() != self.active_context {
            return Err("active context must not contain surrounding whitespace");
        }
        if self.active_context.chars().any(char::is_control) {
            return Err("active context must not contain control characters");
        }
        Ok(())
    }
}
