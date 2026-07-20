# Backend idempotency and release-metrics inventory

The final Python oracle freezes 26 outbound endpoint rows, and the stable
`fixtures/contracts/called-endpoints.json` closes the extractor's RFC 8414
discovery gap for 27 total rows. That inventory proves method/path/header
compatibility; it does not prove server-side replay semantics or release
observability.

No independently reviewed companion artifact currently maps each state-changing endpoint to its idempotency key, fingerprint, storage/expiry behavior, uncertain-outcome reconciliation rule, and duplicate-response behavior. No approved pre-cutover baseline currently records request volume, success/failure/latency, auth refresh/reconciliation, SSE disconnect, cancellation, or client-version dimensions.

Phase 0 therefore records this requirement as blocked. A future inventory must be sourced from a committed companion-backend SHA and reviewed without editing or inventing backend behavior in this repository.

Native state creation is covered by the internal qualification harness using
repository-local controlled directories. File-backed supported Python state now
has a read-only, idempotent importer with redacted dispositions and mandatory
credential reauthentication. It preserves account-bound context, location,
household/local-child state, repair outbox data, and conversation/confirmation
state without mutating the source. The requirement remains blocked on selective
reconciliation of local household state held in the Python keyring, application
consumption/disposition UX, hosted-platform evidence, and a private Windows ACL
writer; Windows fails closed rather than creating an inadequately protected
dietary-state file.
