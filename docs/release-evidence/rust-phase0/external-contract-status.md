# Phase 0 external contract status

Observed read-only on 2026-07-19 in `/Users/justinhambleton/Dev/hellofood` and its `grocery-phase-a` worktree.

The backend prerequisite contracts P0 C1–C4 are merged on the companion repository's `origin/main` lineage:

| Contract | Commit | Status |
|---|---|---|
| C1 household context | `4b7bcdfaf80053c087f5aa68fea8bd5f78732160` | merged |
| C2 safety-result metadata | `fa23437c324b4e5d3d1c433b9c933b9f2dc2cbca` | merged |
| C3 structured confirmation | `9e0a9f220751270da56996ba7004ae25e67b06d0` | merged |
| C4 scope tiers/capabilities | `9e1011d75be9b919452c82cc7dd849bc3f5823a2` | merged |

Grocery Phase A is **not an authoritative companion contract yet**. The `feat/grocery-phase-a-backend` worktree is based at C4 but contains modified and untracked grocery REST/entity/tool schemas and fixtures. Those bytes have no committed source SHA, reviewed directive SHA, or stable aggregate digest. They must not be copied into Rust fixtures or encoded as client wire types.

Blocker: commit and independently review the companion Grocery Phase A contracts, then publish the authoritative contract paths, source commit SHA, digest algorithm/digest, and capability deployment status. Until then the Rust grocery boundary remains stopped; no backend contract was inferred or changed here.
