# DG-R2 status after agent-native Phase 0 inventory

The current client dispatch inventory is complete. Server replay and deployed
reconciliation evidence remains incomplete, so this document does not
authorize agent mutation or a later phase exit.

The authoritative machine-readable inventory is
`docs/release-evidence/agent-native-phase0/dg-r2-dispatch-inventory.json`.
Its gate checks live in
`crates/heyfood-agent-runtime/tests/agent_phase0_dg_r2_inventory.rs`.

## Current result

- 24 POST, PUT, or DELETE routes compiled into the Rust workspace are
  classified.
- 21 are public or feature-reachable; three Health routes are compiled but the
  supported product rejects them before credential access.
- Every row defines reachability, operation class, client retry rule, observed
  or missing server replay contract, reconciliation path, source anchor,
  evidence, and blockers.
- `X-Request-ID` is explicitly not classified as idempotency authority.
- No row permits a blind retry after dispatch.
- Direct meal, Grocery, and Menu Watch mutation routes remain
  human-terminal-only and cannot become agent fallbacks.

## Evidence already present

- Session refresh and credential rotation have durable reconciliation markers
  and cancellation-after-acceptance tests.
- `/v1/agent/converse` has no automatic retry; cancellation, timeout, transport
  loss, EOF, and bounded-stream failures after dispatch remain uncertain.
- Grocery request fixtures cover every current prepare/confirm payload and one
  observed dispatch.
- Menu Watch create conflict taxonomy plus cancellation, disconnect, invalid
  body, and no-retry behavior is tested.
- Profile consent/sync, audio transcription, OAuth staging, promotion, and
  account-bound persistence have focused contract tests.

## Server/deployment evidence still required by later phases

Twelve rows retain explicit server-contract blockers. They principally require:

- device-authorization recovery behavior after transport loss;
- channel-session identical-grant replay/fingerprint behavior;
- conversational hosted-tool mutation and reconciliation classification;
- identical profile-consent replay behavior;
- Grocery proposal replay, mismatch, stale-list/context, and confirmation
  replay evidence;
- Menu Watch duplicate-create and repeated-delete behavior; and
- privacy-safe deployed canaries proving no duplicate proposal, screening,
  watch, or committed mutation.

Until those artifacts are frozen and independently reviewed, uncertain
dispatches remain non-retryable and no MCP protected mutation may be exposed.
