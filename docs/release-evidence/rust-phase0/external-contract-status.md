# Phase 0 external contract status

Observed read-only on 2026-07-20 from GitHub and the companion checkout. The
local `/Users/justinhambleton/Dev/hellofood` worktree contains unrelated user
changes and was not modified.

The backend prerequisite contracts P0 C1–C4 are merged on the companion repository's `origin/main` lineage:

| Contract | Commit | Status |
|---|---|---|
| C1 household context | `4b7bcdfaf80053c087f5aa68fea8bd5f78732160` | merged |
| C2 safety-result metadata | `fa23437c324b4e5d3d1c433b9c933b9f2dc2cbca` | merged |
| C3 structured confirmation | `9e0a9f220751270da56996ba7004ae25e67b06d0` | merged |
| C4 scope tiers/capabilities | `9e1011d75be9b919452c82cc7dd849bc3f5823a2` | merged |

The production platform P0 cutover through schema `094` is independently
approved, and the dry-run blockers fixed by
[PR #100](https://github.com/frntrllc/hellofood/pull/100) and
[PR #101](https://github.com/frntrllc/hellofood/pull/101) are merged. This
removes the database migration as a dependency for continued Rust Phase 0
work. It does not authorize production writers: current backend `main` must
still be deployed with the schema-094 encryption/audit gates enabled, and the
broadly shared submission-storage AWS credential must be scoped and rotated
before writers resume.

## Grocery Phase A and Kroger

[Grocery Phase A PR #90](https://github.com/frntrllc/hellofood/pull/90)
is a substantial committed candidate at
`07303432e18cc1d6bfe943d6979c9091c7fbde9f`. It includes REST, entity, tool,
confirmation, scope, export, conflict, and founding-scenario contracts. It is
not authoritative yet:

- its recorded base is C4 at `9e1011d75be9b919452c82cc7dd849bc3f5823a2`,
  while current `main` is `3d7896542f8c6cb3a6d23d32392cd2f9017667d4`;
- GitHub reports the PR as conflicting (`mergeable_state: dirty`);
- `postgres-migrations` and the aggregate `ci-gate` fail; the PR owns migration
  `093` while current `main` already owns later migration history, so it must be
  rebased and allocated the next post-main revision;
- Grocery must receive the authoritative `HouseholdContextSnapshot` end-to-end
  rather than reconstructing a profile-only approximation;
- confirmation must freeze the real grocery-list UUID, exact list version, and
  authoritative context hash, and precondition checks must be read-only;
- C1 still requires its narrow hardening for native household inputs, complete
  consent truth, persisted single-profile versions, and order-independent
  context hashing.

There is no visible B1 provider-foundation or B2 Kroger-binding PR. Security D2
must deliver purpose-specific, versioned integration-key custody before Kroger
OAuth tokens may be stored. The required backend sequence remains: correct and
deploy Phase A plus Security D2, then B1, then B2.

## Health H1-H3

No visible H1/H2 Oura or H3 Apple Health implementation PR is available to pin
as an authoritative Rust wire contract. Rust may define provider-neutral core
semantics and application ports now. It must not invent production DTOs, read
HealthKit directly, or store provider OAuth tokens. H1/H2 fixtures must pin a
reviewed backend source SHA and deployed scopes; H3 remains capability-gated on
the separately reviewed mobile/backend consent, aggregation, retention,
encryption, revocation, and deletion contracts.

Blocker: merge the C1 and Phase A corrections, obtain green PostgreSQL and
aggregate gates, independently review the companion contracts, and publish the
merge SHA, contract paths, digest algorithm/digest, deployed capability, and
scope metadata. Until then Rust can implement only provider-neutral or generic
semantic/application boundaries; no draft grocery or health wire contract is
copied or treated as stable here.
