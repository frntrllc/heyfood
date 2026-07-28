//! Renderer-neutral Menu Watch use cases and outbound port contracts.

use heyfood_core::{MenuWatchId, OperationId, RestaurantId, SessionCredentials, WatchCadenceWire};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::{BoxFuture, PortError};

#[derive(Clone, PartialEq, Serialize)]
pub struct MenuWatchChangeSummary {
    pub added: u64,
    pub removed: u64,
    pub modified: u64,
    pub price_increases: u64,
    pub price_decreases: u64,
}

#[derive(Clone, PartialEq, Serialize)]
pub struct MenuWatchChangeEvent {
    pub changed_at: String,
    pub previous_snapshot_id: String,
    pub new_snapshot_id: String,
    pub summary: MenuWatchChangeSummary,
}

#[derive(Clone, PartialEq, Serialize)]
pub struct MenuWatchSnapshot {
    pub id: MenuWatchId,
    pub restaurant_id: RestaurantId,
    pub cadence: WatchCadenceWire,
    pub tz: String,
    pub active: bool,
    pub notify: bool,
    pub next_run_at: String,
    pub last_run_at: Option<String>,
    pub last_snapshot_id: Option<String>,
    pub created_at: String,
    pub menu_url: Option<String>,
    pub identity_verdict: Option<String>,
    pub identity_confidence: Option<f64>,
    pub identity_reasoning: Option<String>,
    pub identity_confirmed: Option<bool>,
    pub last_change: Option<MenuWatchChangeEvent>,
}

#[derive(Clone, PartialEq, Serialize)]
pub struct MenuWatchList {
    pub watches: Vec<MenuWatchSnapshot>,
    pub count: u64,
}

#[derive(Clone, Eq, PartialEq)]
pub struct CreateMenuWatchRequest {
    pub restaurant_id: RestaurantId,
    pub cadence: WatchCadenceWire,
    pub notify: bool,
    pub menu_url: Option<String>,
    pub confirm_menu_url: bool,
    pub tz: Option<String>,
}

pub trait MenuWatchReadPort: Send + Sync {
    fn list(
        &self,
        credentials: SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<MenuWatchList, PortError>>;
}

pub trait MenuWatchPort: MenuWatchReadPort {
    fn create(
        &self,
        credentials: SessionCredentials,
        operation_id: OperationId,
        request: CreateMenuWatchRequest,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<MenuWatchSnapshot, PortError>>;

    fn remove(
        &self,
        credentials: SessionCredentials,
        operation_id: OperationId,
        watch_id: MenuWatchId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<(), PortError>>;
}

pub struct ListMenuWatches<'a> {
    port: &'a dyn MenuWatchReadPort,
}

impl<'a> ListMenuWatches<'a> {
    #[must_use]
    pub const fn new(port: &'a dyn MenuWatchReadPort) -> Self {
        Self { port }
    }

    pub async fn execute(
        &self,
        credentials: SessionCredentials,
        operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> Result<MenuWatchList, PortError> {
        ensure_not_cancelled(&cancellation, "menu_watch_list_cancelled_before_dispatch")?;
        self.port
            .list(credentials, operation_id, cancellation)
            .await
    }
}

pub struct CreateMenuWatch<'a> {
    port: &'a dyn MenuWatchPort,
}

impl<'a> CreateMenuWatch<'a> {
    #[must_use]
    pub const fn new(port: &'a dyn MenuWatchPort) -> Self {
        Self { port }
    }

    pub async fn execute(
        &self,
        credentials: SessionCredentials,
        operation_id: OperationId,
        request: CreateMenuWatchRequest,
        cancellation: CancellationToken,
    ) -> Result<MenuWatchSnapshot, PortError> {
        ensure_not_cancelled(&cancellation, "menu_watch_create_cancelled_before_dispatch")?;
        self.port
            .create(credentials, operation_id, request, cancellation)
            .await
    }
}

pub struct RemoveMenuWatch<'a> {
    port: &'a dyn MenuWatchPort,
}

impl<'a> RemoveMenuWatch<'a> {
    #[must_use]
    pub const fn new(port: &'a dyn MenuWatchPort) -> Self {
        Self { port }
    }

    pub async fn execute(
        &self,
        credentials: SessionCredentials,
        operation_id: OperationId,
        watch_id: MenuWatchId,
        cancellation: CancellationToken,
    ) -> Result<(), PortError> {
        ensure_not_cancelled(&cancellation, "menu_watch_remove_cancelled_before_dispatch")?;
        self.port
            .remove(credentials, operation_id, watch_id, cancellation)
            .await
    }
}

fn ensure_not_cancelled(
    cancellation: &CancellationToken,
    code: &'static str,
) -> Result<(), PortError> {
    if cancellation.is_cancelled() {
        Err(PortError::new(
            code,
            "Menu Watch operation was cancelled before dispatch",
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use heyfood_core::{AccountId, CredentialVersion, SensitiveString, WatchHour, WatchWeekday};
    use serde_json::json;

    use super::*;

    struct RejectingPort {
        calls: AtomicUsize,
    }

    impl RejectingPort {
        fn called<T>(&self) -> BoxFuture<'_, Result<T, PortError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {
                Err(PortError::new(
                    "unexpected_dispatch",
                    "test port must not be called",
                ))
            })
        }
    }

    impl MenuWatchReadPort for RejectingPort {
        fn list(
            &self,
            _credentials: SessionCredentials,
            _operation_id: OperationId,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<MenuWatchList, PortError>> {
            self.called()
        }
    }

    impl MenuWatchPort for RejectingPort {
        fn create(
            &self,
            _credentials: SessionCredentials,
            _operation_id: OperationId,
            _request: CreateMenuWatchRequest,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<MenuWatchSnapshot, PortError>> {
            self.called()
        }

        fn remove(
            &self,
            _credentials: SessionCredentials,
            _operation_id: OperationId,
            _watch_id: MenuWatchId,
            _cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<(), PortError>> {
            self.called()
        }
    }

    fn credentials() -> SessionCredentials {
        SessionCredentials::from_unix_expiry(
            AccountId::parse("menu-watch-test").unwrap(),
            SensitiveString::new("access"),
            SensitiveString::new("refresh"),
            CredentialVersion::new(1),
            4_102_444_800,
        )
        .unwrap()
    }

    fn create_request() -> CreateMenuWatchRequest {
        CreateMenuWatchRequest {
            restaurant_id: RestaurantId::parse("0c1cb790-0000-4000-8000-000000000000").unwrap(),
            cadence: WatchCadenceWire {
                weekday: WatchWeekday::new(3).unwrap(),
                hour: WatchHour::new(9).unwrap(),
            },
            notify: true,
            menu_url: None,
            confirm_menu_url: false,
            tz: Some("America/Chicago".into()),
        }
    }

    #[test]
    fn renderer_neutral_snapshot_preserves_the_wire_json_shape() {
        let snapshot = MenuWatchList {
            watches: vec![MenuWatchSnapshot {
                id: MenuWatchId::parse("00000000-0000-4000-8000-000000000010").unwrap(),
                restaurant_id: RestaurantId::parse("0c1cb790-0000-4000-8000-000000000000").unwrap(),
                cadence: WatchCadenceWire {
                    weekday: WatchWeekday::new(3).unwrap(),
                    hour: WatchHour::new(9).unwrap(),
                },
                tz: "America/Chicago".into(),
                active: true,
                notify: true,
                next_run_at: "2026-07-30T14:00:00Z".into(),
                last_run_at: None,
                last_snapshot_id: Some("snapshot-new".into()),
                created_at: "2026-07-23T12:00:00Z".into(),
                menu_url: None,
                identity_verdict: Some("verified".into()),
                identity_confidence: Some(0.97),
                identity_reasoning: Some("name and location matched".into()),
                identity_confirmed: Some(true),
                last_change: Some(MenuWatchChangeEvent {
                    changed_at: "2026-07-24T14:05:00Z".into(),
                    previous_snapshot_id: "snapshot-old".into(),
                    new_snapshot_id: "snapshot-new".into(),
                    summary: MenuWatchChangeSummary {
                        added: 17,
                        removed: 12,
                        modified: 50,
                        price_increases: 50,
                        price_decreases: 0,
                    },
                }),
            }],
            count: 1,
        };

        assert_eq!(
            serde_json::to_value(snapshot).unwrap(),
            json!({
                "watches": [{
                    "id": "00000000-0000-4000-8000-000000000010",
                    "restaurant_id": "0c1cb790-0000-4000-8000-000000000000",
                    "cadence": {"weekday": 3, "hour": 9},
                    "tz": "America/Chicago",
                    "active": true,
                    "notify": true,
                    "next_run_at": "2026-07-30T14:00:00Z",
                    "last_run_at": null,
                    "last_snapshot_id": "snapshot-new",
                    "created_at": "2026-07-23T12:00:00Z",
                    "menu_url": null,
                    "identity_verdict": "verified",
                    "identity_confidence": 0.97,
                    "identity_reasoning": "name and location matched",
                    "identity_confirmed": true,
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
            })
        );
    }

    #[tokio::test]
    async fn all_controllers_reject_pre_dispatch_cancellation() {
        let port = RejectingPort {
            calls: AtomicUsize::new(0),
        };
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let list_error = match ListMenuWatches::new(&port)
            .execute(
                credentials(),
                OperationId::new(),
                cancellation.child_token(),
            )
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("cancelled list must fail before dispatch"),
        };
        let create_error = match CreateMenuWatch::new(&port)
            .execute(
                credentials(),
                OperationId::new(),
                create_request(),
                cancellation.child_token(),
            )
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("cancelled create must fail before dispatch"),
        };
        let remove_error = match RemoveMenuWatch::new(&port)
            .execute(
                credentials(),
                OperationId::new(),
                MenuWatchId::parse("00000000-0000-4000-8000-000000000010").unwrap(),
                cancellation,
            )
            .await
        {
            Err(error) => error,
            Ok(()) => panic!("cancelled remove must fail before dispatch"),
        };

        assert_eq!(list_error.code, "menu_watch_list_cancelled_before_dispatch");
        assert_eq!(
            create_error.code,
            "menu_watch_create_cancelled_before_dispatch"
        );
        assert_eq!(
            remove_error.code,
            "menu_watch_remove_cancelled_before_dispatch"
        );
        assert_eq!(port.calls.load(Ordering::SeqCst), 0);
    }
}
