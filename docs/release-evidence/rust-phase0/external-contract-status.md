# Phase 0 external contract status

Observed read-only on 2026-07-20 from GitHub and the companion checkout. The
local `/Users/justinhambleton/Dev/hellofood` worktree contains unrelated user
changes and was not modified.

The backend prerequisite contracts P0 C1–C4 are merged:

| Contract | Commit | Status |
|---|---|---|
| C1 household context | `4b7bcdfaf80053c087f5aa68fea8bd5f78732160` | merged; residual hardening remains required before Grocery uses it as a safety boundary |
| C2 safety-result metadata | `fa23437c324b4e5d3d1c433b9c933b9f2dc2cbca` | merged |
| C3 structured confirmation | `9e0a9f220751270da56996ba7004ae25e67b06d0` | merged |
| C4 scope tiers/capabilities | `9e1011d75be9b919452c82cc7dd849bc3f5823a2` | merged |

Production P0 remediation PRs
[100](https://github.com/frntrllc/hellofood/pull/100) and
[101](https://github.com/frntrllc/hellofood/pull/101) are merged. Migration
history repair [PR #102](https://github.com/frntrllc/hellofood/pull/102) restored
the `092 → 094 → 095` lineage. The certifi-backed verified-TLS corrections in
[PR #105](https://github.com/frntrllc/hellofood/pull/105) and
[PR #106](https://github.com/frntrllc/hellofood/pull/106), followed by the
validated-constraint readiness correction in
[PR #108](https://github.com/frntrllc/hellofood/pull/108), place current `main`
at `f752a057fb1cf75abe9bcb6ab4aafdc11687db73`; the exact-build production
postflight reports `095` as the sole current head with both health tables
present. The database migration no longer blocks Rust work. Application-fleet
alignment and the separately controlled writer/gate/AWS-credential actions
remain backend operational work, not Rust Phase 0 blockers.

One follow-up security hardening change remains active and not yet merged:
[PR #109](https://github.com/frntrllc/hellofood/pull/109) binds 095 health rows
to aggregate-only, zero-plaintext, decryptable-envelope attestation evidence.
It is a backend deployment/reactivation gate rather than a reason to stop the
generic Rust Phase 0 foundation.

## Grocery Phase A and Kroger

[Grocery Phase A PR #107](https://github.com/frntrllc/hellofood/pull/107)
supersedes conflicting PR #90. It is a mergeable reconstruction at
`ca9c793c5928c6af3f57f505393a61bdd81c7c46` with migration `096` over `095`,
but is now behind the newly merged PR #108 `main`. Its candidate contract
claims to close the two mandatory corrections:

- thread the authoritative `HouseholdContextSnapshot` through REST,
  conversation tools, screening, proposal, and confirmation rather than
  reconstructing a profile-only approximation;
- freeze the real grocery-list UUID, exact list version, and authoritative
  context hash at proposal time, then make confirmation precondition checks
  read-only.

The candidate also claims the C1 native `_self`, complete-consent, persisted
single-profile version, and order-independent hash hardening. Its reported
local Grocery, scope/household/confirmation, and migration-history suites pass.
It is not authoritative yet: its hosted hermetic, PostgreSQL migration, auth,
aggregate CI, and public-preview checks are red, and final contract provenance
cannot be pinned until it is current and green, then merged, migrated in
production from `095 → 096`, and proven by live `grocery:v1` canaries.

No visible B1 provider-foundation or B2 Kroger-binding PR exists. Security D2
also remains absent: current backend integration encryption still derives its
Fernet key from the general application secret rather than versioned,
purpose-specific integration-key custody. Kroger token storage and final Rust
grocery wire DTOs therefore remain blocked. This does not serialize generic
Phase 1 core/platform work; the plan explicitly treats Grocery Phase A as a
provisional external dependency until the corrected merge SHA and digest exist.

## Health H1–H3

Health is no longer an unavailable future contract:

| Slice | PR / merge commit | Rust Phase 0 disposition |
|---|---|---|
| H1/H2 CLI health read and Oura connect/sync/disconnect | [#79](https://github.com/frntrllc/hellofood/pull/79), `7cfadc55c103257b588b237c65fe7b5031a3f745` | Frozen language-neutral semantics and scope/routing evidence in `fixtures/contracts/health-h1h2.v1.json`. |
| H3 Apple Health backend daily rollups | [#96](https://github.com/frntrllc/hellofood/pull/96), `400c5cafb3beb0237e75f85e93d228fbbbd3dadf` | Canonical backend JSON contract copied byte-for-byte to `fixtures/contracts/health-h3-daily-sync.v1.json`. |
| H3 Apple Health mobile collection/sync | [#95](https://github.com/frntrllc/hellofood/pull/95), `dbea9c3cc8af4610b7b6bf3f3e64ad44e7fe428a` | Recorded as the separate mobile capability/consent implementation; Rust must not read HealthKit. |

`fixtures/contracts/health-contract-provenance.json` pins all source paths,
source hashes, merge commits, target paths, and target hashes. Phase 1 may
implement provider-neutral health connection, freshness/staleness, read, and
application-port semantics against these freezes. It must not store provider
OAuth tokens, perform client-side health aggregation, or treat H3 as available
unless the backend advertises the reviewed capability.

## Phase boundary

Phase 0 satisfies the plan by recording these external states and freezing the
merged health contracts. Grocery remains explicitly provisional and Kroger
remains blocked at its later implementation boundary. Neither state blocks
specialized Phase 0 review or generic Phase 1 authorization.
