# Backend idempotency and release-metrics inventory

The stable Python baseline freezes 26 outbound endpoint rows in `fixtures/contracts/called-endpoints.json`. That inventory proves method/path/header compatibility; it does not prove server-side replay semantics or release observability.

No independently reviewed companion artifact currently maps each state-changing endpoint to its idempotency key, fingerprint, storage/expiry behavior, uncertain-outcome reconciliation rule, and duplicate-response behavior. No approved pre-cutover baseline currently records request volume, success/failure/latency, auth refresh/reconciliation, SSE disconnect, cancellation, or client-version dimensions.

Phase 0 therefore records this requirement as blocked. A future inventory must be sourced from a committed companion-backend SHA and reviewed without editing or inventing backend behavior in this repository.

Native state creation is covered by the internal qualification harness using repository-local controlled directories. The optional read-only Python importer remains unevaluated and is not an exit dependency.
