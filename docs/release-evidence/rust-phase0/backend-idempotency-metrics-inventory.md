# Backend idempotency and release-metrics inventory

**Python client contract:** `fixtures/contracts/called-endpoints.json`, final
oracle `73494a57468dac83b4904ce6c390e36926f5c6fe` (27 endpoints).

**Backend observed:** `frntrllc/hellofood` main
`8633c0a5229178eefb1556edc6c136b0a88cff3f`.

This is a conservative client-safety inventory. A row classified
`no_automatic_retry` remains non-retriable even if an individual backend
implementation appears naturally idempotent; Rust may relax that only after a
reviewed server key/fingerprint/replay contract is frozen.

## Read-only surfaces

The ten `GET` rows in the frozen contract are read-only and may be retried only
within their explicit deadline and authentication policy: capabilities, profile
readiness, session identity, channel identity, household members, profile,
profile consent, daily meal summary, channel links, and RFC 8414 discovery.
Personal health reads added after the Python oracle are frozen separately in
`fixtures/contracts/health-h1h2.v1.json` and use `Cache-Control: no-store`.

## State-changing or POST-as-read surfaces

| Surface | Backend replay behavior observed | Rust retry/reconciliation rule |
|---|---|---|
| `POST /v1/auth/account-deletion/begin` | Backend module declares a permanent replay-safe transaction and returns the existing transaction while pending. | No blind retry after an unknown response; query the one-purpose status capability. |
| `POST /v1/auth/account-deletion/status` | Read/poll operation using a one-purpose in-memory status capability. | Bounded status polling is permitted. |
| `POST /v1/auth/account-deletion/cancel` | Cancels the identified transaction; repeated terminal-state handling is server-owned. | Reconcile through status; no new transaction is inferred. |
| `PUT /v1/profile/sync` | Merge/update keyed by authenticated account and member; no frozen request-id replay record. | `no_automatic_retry` after dispatch; refresh with `GET /v1/profile/sync`. |
| `POST /v1/profile/consent` | Sets profile-sync consent, but no frozen key/fingerprint/expiry contract exists. | `no_automatic_retry`; reconcile with consent GET. |
| `DELETE /v1/channel/links/{link_id}` | Resource-identity delete used as best-effort logout teardown. | May be repeated only during explicit teardown; absence is success-equivalent. |
| `POST /v1/agent/converse` | Streaming conversational POST; no server replay key is advertised. | Never retry an uncertain POST. Distinguish before-acceptance, accepted, and dispatched/outcome-unknown; reconcile by reading current conversation/state when available. |
| `POST /v1/audio/transcriptions` | Expensive multipart operation with rate limits and latency metadata; no replay key is frozen. | `no_automatic_retry`; user explicitly resubmits a new capture. |
| `POST /v1/channel/tools/{name}` | Polymorphic read/mutation dispatch; behavior depends on the named tool. | Default `no_automatic_retry`; a future generated tool contract must opt a read-only tool into retry. |
| `POST /v1/auth/session/refresh` | Rotates credentials. Rust Phase 0 durably versions the accepted rotation and marks uncertain reconciliation. | No transport retry after dispatch; re-exchange through the separately frozen channel-session path when policy permits. |
| `POST /v1/auth/session/revoke` | Identified session teardown. | Best-effort explicit logout only; local secret removal never waits indefinitely. |
| `POST /v1/auth/device/revoke` | Identified device teardown. | Best-effort explicit logout only. |
| `POST /v1/channel/oauth/cli/session` | Grant/session exchange can rotate credentials; no generic replay key is exposed. | No blind retry after dispatch; perform bounded re-exchange only through the frozen auth policy. |
| `POST /v1/channel/oauth/token` | OAuth token issuance/refresh with one-time or rotating material. | Follow protocol polling/refresh rules; never replay an accepted credential mutation from UI code. |
| `POST /v1/channel/oauth/register` | Dynamic client registration; backend rejects conflicting identifiers and tells the caller to generate a new identifier. | No blind retry with a changed payload; generate a new identifier only for the explicit conflict response. |
| `POST /v1/channel/oauth/device/authorize` | Creates an RFC 8628 device authorization. | One explicit authorization start; restart only after terminal expiry/error. |
| `POST /v1/channel/oauth/device/token` | RFC 8628 polling endpoint. | Poll only at the server interval, honoring `slow_down`, expiry, and cancellation. |

## Release metrics baseline

Phase 0 freezes the dimensions required for the later shadow/cutover baseline:

- client version, platform/architecture, command/use case, endpoint class, and
  authenticated scope tier;
- request count, success/error category, latency p50/p95/p99, and bounded
  timeout/cancellation category;
- auth refresh, channel re-exchange, durable credential reconciliation, and
  account/context generation changes;
- SSE accepted, disconnect/EOF, stream-limit rejection, terminal event, and
  dispatched/outcome-unknown counts;
- TUI startup/first-frame/input-to-frame, terminal restoration failure, and
  supervisor join timeout;
- grocery stale list/context/consent precondition failures once the
  authoritative Phase A contract lands;
- client-side metrics remain content-free: no prompt, response, dietary,
  health, credential, query, RAG, or verdict value is logged.

Production writers remain suspended, so Phase 0 does not fabricate a 24-hour
traffic sample. The later release candidate must capture these dimensions on
the deployed backend before cutover. Phase 0's requirement is the reviewed
inventory and conservative no-retry posture, both of which are now explicit.

## Native-state idempotency evidence

The internal qualification proves atomic config/state replacement, bounded
cross-process locks, version-monotonic credential rotation, durable
reconciliation markers, restart replay, and read-only Python local-state import.
Python keyring credentials are intentionally not copied: account binding is
preserved as a redacted disposition and the safe action is reauthentication.
