use std::sync::Arc;

use heyfood_application::{
    BoxFuture, GroceryListSnapshot, GroceryMutationIntent, GroceryPort, PortError,
    PreparedGroceryMutation, ReadActiveGroceryList,
};
use heyfood_core::{
    AccountId, ContextFingerprint, CredentialVersion, FrozenGroceryPreconditions,
    GroceryCapability, GroceryConfirmationCommand, GroceryEntityId, GroceryListVersion,
    OperationId, SensitiveString, SessionCredentials,
};
use sha2::{Digest, Sha256};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const PROOF_MANIFEST: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/release-evidence/agent-native-phase0/thin-proof-manifest.json"
));
const PROOF_MANIFEST_SHA256: &str =
    "7f34ad48c614a0e8b17b8b1830d3ae8a62a4107d549c62d6ad71f74263413577";

#[derive(Clone, Copy)]
enum ReadBehavior {
    Complete,
    WaitForCancellation,
}

struct FixtureGroceryPort {
    expected_account: AccountId,
    snapshot: GroceryListSnapshot,
    behavior: ReadBehavior,
    started: Arc<Notify>,
}

impl GroceryPort for FixtureGroceryPort {
    fn capability(
        &self,
        _credentials: SessionCredentials,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<GroceryCapability, PortError>> {
        Box::pin(async {
            Err(PortError::new(
                "unexpected_proof_call",
                "capability was not part of the bounded proof",
            ))
        })
    }

    fn read_active_list(
        &self,
        credentials: SessionCredentials,
        _operation_id: OperationId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<GroceryListSnapshot, PortError>> {
        Box::pin(async move {
            if credentials.account_id != self.expected_account {
                return Err(PortError::new(
                    "account_mismatch",
                    "fixture credentials belong to a different account",
                ));
            }
            match self.behavior {
                ReadBehavior::Complete => Ok(self.snapshot.clone()),
                ReadBehavior::WaitForCancellation => {
                    self.started.notify_one();
                    cancellation.cancelled().await;
                    Err(PortError::new(
                        "grocery_read_cancelled",
                        "Grocery read was cancelled",
                    ))
                }
            }
        })
    }

    fn prepare_mutation(
        &self,
        _credentials: SessionCredentials,
        _operation_id: OperationId,
        _expected: FrozenGroceryPreconditions,
        _intent: GroceryMutationIntent,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<PreparedGroceryMutation, PortError>> {
        Box::pin(async {
            Err(PortError::new(
                "unexpected_proof_call",
                "mutation preparation was not part of the bounded proof",
            ))
        })
    }

    fn decide_confirmation(
        &self,
        _credentials: SessionCredentials,
        _operation_id: OperationId,
        _command: GroceryConfirmationCommand,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<GroceryListSnapshot, PortError>> {
        Box::pin(async {
            Err(PortError::new(
                "unexpected_proof_call",
                "confirmation was not part of the bounded proof",
            ))
        })
    }
}

fn account_id() -> AccountId {
    AccountId::parse("phase0-fixture-account").expect("fixture account")
}

fn credentials() -> SessionCredentials {
    SessionCredentials::from_unix_expiry(
        account_id(),
        SensitiveString::new("fixture-access-token"),
        SensitiveString::new("fixture-refresh-token"),
        CredentialVersion::new(1),
        4_102_444_800,
    )
    .expect("fixture credentials")
}

fn snapshot() -> GroceryListSnapshot {
    GroceryListSnapshot {
        preconditions: FrozenGroceryPreconditions {
            list_id: GroceryEntityId::parse("10000000-0000-4000-8000-000000000001")
                .expect("list ID"),
            list_version: GroceryListVersion::new(7).expect("list version"),
            context_fingerprint: ContextFingerprint::parse("abcd-1234")
                .expect("context fingerprint"),
            household_context_hash_version: None,
        },
        items: vec![(
            GroceryEntityId::parse("10000000-0000-4000-8000-000000000002").expect("item ID"),
            SensitiveString::new("fixture ingredient"),
        )],
    }
}

fn assert_internal_manifest() {
    let text = std::str::from_utf8(PROOF_MANIFEST).expect("proof manifest UTF-8");
    let normalized = text.replace("\r\n", "\n");
    assert!(
        !normalized.contains('\r'),
        "proof manifest contains a non-line-ending carriage return"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized.as_bytes())),
        PROOF_MANIFEST_SHA256
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(PROOF_MANIFEST).expect("proof manifest JSON");
    assert_eq!(manifest["proof_only"], true);
    assert_eq!(manifest["public_command"], false);
    assert_eq!(manifest["schema_version"], 0);
}

#[tokio::test]
async fn bin_composes_an_account_bound_application_read_without_public_surface() {
    assert_internal_manifest();
    let expected = snapshot();
    let controller = ReadActiveGroceryList::new(Arc::new(FixtureGroceryPort {
        expected_account: account_id(),
        snapshot: expected.clone(),
        behavior: ReadBehavior::Complete,
        started: Arc::new(Notify::new()),
    }));

    let actual = controller
        .execute(credentials(), OperationId::new(), CancellationToken::new())
        .await
        .expect("bounded application read");

    assert_eq!(actual, expected);
}

#[tokio::test]
async fn bin_composition_forwards_cancellation_to_the_object_safe_port() {
    assert_internal_manifest();
    let started = Arc::new(Notify::new());
    let controller = ReadActiveGroceryList::new(Arc::new(FixtureGroceryPort {
        expected_account: account_id(),
        snapshot: snapshot(),
        behavior: ReadBehavior::WaitForCancellation,
        started: started.clone(),
    }));
    let cancellation = CancellationToken::new();
    let operation = tokio::spawn({
        let cancellation = cancellation.clone();
        async move {
            controller
                .execute(credentials(), OperationId::new(), cancellation)
                .await
        }
    });

    started.notified().await;
    cancellation.cancel();
    let error = operation
        .await
        .expect("proof task joins")
        .expect_err("cancelled read fails");

    assert_eq!(error.code, "grocery_read_cancelled");
    assert!(!error.outcome_uncertain);
}
