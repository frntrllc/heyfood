# DG-R2 status for Phase 2 remediation

DG-R2 is **incomplete** and remains an exit-gate blocker.

This status is pinned to remediation product commit
`d1b2b92cfd47e9a1e2d60a206e477ca376d008e7` and does not claim deployed replay
or production-canary evidence.

## Evidence already present

- `/v1/agent/converse` has automatic Reqwest retries disabled. Tests cover
  cancellation and transport loss after request-body consumption, assert an
  uncertain outcome, and assert one observed request.
- Grocery POST tests assert exact imported request payloads and one dispatch for
  prepare add/remove/state and confirmation. Cancellation after dispatch remains
  uncertain and is not blindly replayed.
- Item evaluation uses a provider-neutral, non-mutating channel tool and has no
  automatic retry.
- `X-Request-ID` remains a tracing operation ID; this remediation does not claim
  that it is a server idempotency key.

## Evidence still required

- A reviewed endpoint table for converse and every Grocery POST defining server
  acceptance, replay key, fingerprint binding, mismatch behavior, and safe
  status/read reconciliation.
- Deployed identical-key replay and fingerprint-mismatch proof for proposal,
  screening, weekly, and confirmation paths.
- Timeout-after-proposal, disconnect-during-confirmation, replayed accept,
  stale-list/context, and cancellation-boundary tests against the deployed
  contract.
- Privacy-safe production canaries proving positive read, prepare/cancel,
  stale-version conflict, active-list non-mutation, and no duplicate screening,
  proposal, or committed mutation.

Until those artifacts exist and receive exact-SHA review, uncertain POSTs remain
non-retryable and Phase 2 remains HOLD.
