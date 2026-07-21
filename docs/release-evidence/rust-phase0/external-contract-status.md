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
at `70d79bf6d859ff7d45738663b52a9a1074e62738`; the exact-build production
postflight reports `095` as the sole current head with both health tables
present. That main SHA includes merged
[PR #109](https://github.com/frntrllc/hellofood/pull/109), which binds 095
health rows to aggregate-only, zero-plaintext, decryptable-envelope
attestation evidence, and merged
[PR #110](https://github.com/frntrllc/hellofood/pull/110), which corrects the
PostgreSQL-18 NOT NULL catalog interpretation while preserving exact semantic
constraint enforcement, and merged Grocery Phase A PR #107. The database
migration no longer blocks Rust work. Application-fleet
alignment and the separately controlled writer/gate/AWS-credential actions
remain backend operational work, not Rust Phase 0 blockers.

Deployment and exact-build postflight of the PR #110 merge remain backend
reactivation gates rather than reasons to stop the generic Rust Phase 0
foundation.

## Grocery Phase A and Kroger

[Grocery Phase A PR #107](https://github.com/frntrllc/hellofood/pull/107)
supersedes conflicting PR #90 and squash-merged as
`70d79bf6d859ff7d45738663b52a9a1074e62738` from exact reviewed source head
`8cd7baf2c683bf5ad286af32c26d96bdb1742f86`, with migration `096` over `095`
and PR #110 ancestry. Its contract closes the two mandatory corrections:

- thread the authoritative `HouseholdContextSnapshot` through REST,
  conversation tools, screening, proposal, and confirmation rather than
  reconstructing a profile-only approximation;
- freeze the real grocery-list UUID, exact list version, and authoritative
  context hash at proposal time, then make confirmation precondition checks
  read-only.

The candidate also claims the C1 native `_self`, complete-consent, persisted
single-profile version, order-independent hash hardening, and an exact Grocery
catalog proof covering 51 columns, 16 semantic constraints, 14 indexes, and
account-initializer function/trigger semantics, including PostgreSQL 18's exact
34 validated/enforced Grocery NOT NULL constraints. Its reported local
qualification is 5,396 hermetic tests, 87 PostgreSQL 18 data-protection tests,
and 39 PostgreSQL 16 tests with 48 PostgreSQL-18-only skips; required hosted
and independent review gates passed before merge.

The merge is still not an authoritative Rust import. Production remains at
`095`, with no Grocery tables or runtime-attestation row. A read-only first-
attestation dry run reported zero aggregate issues, but the separately
authorized write failed closed because the isolated migration runner lacked an
attestation signing key; that consumed authorization must not be retried. Final
Phase A contract provenance cannot be pinned until a fresh authorized signed
attestation succeeds, Production migrates `095 → 096`, the exact merge is
deployed with a regenerated fixture digest, and live `grocery:v1` capability,
scope, confirmation, conflict, and non-mutation canaries pass.

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
