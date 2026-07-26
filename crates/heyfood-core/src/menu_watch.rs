//! Typed Menu Watch contracts imported from hellofood backend main.
//!
//! The frozen source is `fixtures/contracts/menu-watch/menu_watch.py`. The
//! account-owned list response includes the latest durable change summary,
//! source selection, and identity evidence needed by the installed client.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

pub const MENU_WATCH_SCOPE: &str = "menu:watch";
pub const MENU_WATCH_SOURCE_COMMIT: &str = "b2bc30b4984c09dc33107fde4db1723d31292886";
pub const MENU_WATCH_SOURCE_SHA256: &str =
    "f8eb36955f14b3a1423b45e8dbf4ee62ed736e8fa1745f3fe18c2cd65758c582";

macro_rules! uuid_identifier {
    ($name:ident, $message:literal) => {
        #[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(Uuid);

        impl $name {
            pub fn parse(value: &str) -> Result<Self, &'static str> {
                Uuid::parse_str(value).map(Self).map_err(|_| $message)
            }

            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_tuple(stringify!($name))
                    .field(&self.0.hyphenated().to_string())
                    .finish()
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0.hyphenated().to_string())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(&value).map_err(serde::de::Error::custom)
            }
        }
    };
}

uuid_identifier!(MenuWatchId, "menu watch ID must be a UUID");
uuid_identifier!(RestaurantId, "restaurant ID must be a UUID");

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct WatchWeekday(u8);

impl WatchWeekday {
    pub const fn new(value: u8) -> Result<Self, &'static str> {
        if value <= 6 {
            Ok(Self(value))
        } else {
            Err("watch weekday must be between Monday (0) and Sunday (6)")
        }
    }

    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl<'de> Deserialize<'de> for WatchWeekday {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(u8::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct WatchHour(u8);

impl WatchHour {
    pub const fn new(value: u8) -> Result<Self, &'static str> {
        if value <= 23 {
            Ok(Self(value))
        } else {
            Err("watch hour must be between 0 and 23")
        }
    }

    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl<'de> Deserialize<'de> for WatchHour {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(u8::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct WatchCadenceWire {
    pub weekday: WatchWeekday,
    pub hour: WatchHour,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MenuWatchCreateRequestWire {
    pub restaurant_id: RestaurantId,
    pub cadence: WatchCadenceWire,
    pub notify: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub menu_url: Option<String>,
    pub confirm_menu_url: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tz: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct MenuWatchChangeSummaryWire {
    #[serde(default)]
    pub added: u64,
    #[serde(default)]
    pub removed: u64,
    #[serde(default)]
    pub modified: u64,
    #[serde(default)]
    pub price_increases: u64,
    #[serde(default)]
    pub price_decreases: u64,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct MenuWatchChangeEventWire {
    pub changed_at: String,
    pub previous_snapshot_id: String,
    pub new_snapshot_id: String,
    pub summary: MenuWatchChangeSummaryWire,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct MenuWatchResponseWire {
    pub id: MenuWatchId,
    pub restaurant_id: RestaurantId,
    pub cadence: WatchCadenceWire,
    pub tz: String,
    pub active: bool,
    pub notify: bool,
    pub next_run_at: String,
    #[serde(default)]
    pub last_run_at: Option<String>,
    #[serde(default)]
    pub last_snapshot_id: Option<String>,
    pub created_at: String,
    #[serde(default)]
    pub menu_url: Option<String>,
    #[serde(default)]
    pub identity_verdict: Option<String>,
    #[serde(default)]
    pub identity_confidence: Option<f64>,
    #[serde(default)]
    pub identity_reasoning: Option<String>,
    #[serde(default)]
    pub identity_confirmed: Option<bool>,
    #[serde(default)]
    pub last_change: Option<MenuWatchChangeEventWire>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct MenuWatchListResponseWire {
    #[serde(default)]
    pub watches: Vec<MenuWatchResponseWire>,
    #[serde(default)]
    pub count: u64,
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::*;

    const SOURCE: &[u8] = include_bytes!("../../../fixtures/contracts/menu-watch/menu_watch.py");

    #[test]
    fn frozen_backend_source_has_exact_reviewed_digest() {
        assert_eq!(
            format!("{:x}", Sha256::digest(SOURCE)),
            MENU_WATCH_SOURCE_SHA256
        );
        assert_eq!(SOURCE.first(), Some(&b'"'));
    }

    #[test]
    fn request_and_response_follow_frozen_contract() {
        let request = MenuWatchCreateRequestWire {
            restaurant_id: RestaurantId::parse("0c1cb790-0000-4000-8000-000000000000").unwrap(),
            cadence: WatchCadenceWire {
                weekday: WatchWeekday::new(3).unwrap(),
                hour: WatchHour::new(9).unwrap(),
            },
            notify: true,
            menu_url: None,
            confirm_menu_url: false,
            tz: Some("America/Chicago".into()),
        };
        let encoded = serde_json::to_value(request).unwrap();
        assert_eq!(encoded["cadence"]["weekday"], 3);
        assert_eq!(encoded["notify"], true);
        assert!(encoded.get("menu_url").is_none());

        let response: MenuWatchListResponseWire = serde_json::from_value(serde_json::json!({
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
                "created_at": "2026-07-23T12:00:00Z",
                "menu_url": "https://ordering.example/abby",
                "identity_verdict": "verified",
                "identity_confidence": 0.97,
                "identity_reasoning": "name and location matched",
                "identity_confirmed": false,
                "last_change": {
                    "changed_at": "2026-07-24T14:05:00Z",
                    "previous_snapshot_id": "snapshot-old",
                    "new_snapshot_id": "snapshot-new",
                    "summary": {
                        "added": 17,
                        "removed": 12,
                        "modified": 50,
                        "price_increases": 50,
                        "price_decreases": 0
                    }
                }
            }],
            "count": 1
        }))
        .unwrap();
        assert_eq!(response.count, 1);
        assert_eq!(response.watches[0].cadence.weekday.get(), 3);
        assert_eq!(
            response.watches[0].menu_url.as_deref(),
            Some("https://ordering.example/abby")
        );
        assert_eq!(
            response.watches[0]
                .last_change
                .as_ref()
                .map(|event| event.summary.added),
            Some(17)
        );
    }

    #[test]
    fn cadence_rejects_values_outside_backend_bounds() {
        assert!(
            serde_json::from_value::<WatchCadenceWire>(
                serde_json::json!({"weekday": 7, "hour": 9})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<WatchCadenceWire>(
                serde_json::json!({"weekday": 3, "hour": 24})
            )
            .is_err()
        );
    }
}
