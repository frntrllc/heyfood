# Grocery backend contract freeze

This namespace is the import boundary for authoritative grocery companion
contracts. The checked-in C3 confirmation contract and C4
application-capability/scope contracts are exact copies from their merged
hellofood commits. The `phase-a/` subtree is an exact 14-file mirror of the
deployed Grocery Phase-A contract set frozen by hellofood PR #115.
`cargo xtask import-grocery-contracts --source-repo PATH` reproduces both sets,
while the stable-contract and dedicated grocery validators enforce the source,
deployed, manifest, per-file, target, and aggregate SHA-256 values recorded in
`provenance.json`.

PR #107's merge `70d79bf6d859ff7d45738663b52a9a1074e62738` is an ancestor of
the exact deployed commit `f7b0eebca879840995226ede9ea715dc8702313a`. PR
#115's squash merge `7871b20ae609a0fffd82b2a35efd39cf3385825d`
authoritatively records their byte-identical 14-file tree and aggregate digest
`781a14b9d05d70a4da245e2d80c24b0b040aa7ec742f852c65ca3815cc583911`.

This is a fixture import only. The Rust-side import review and separate Phase 2
authorization remain required before generating final wire DTOs or adding
Grocery REST/tool bindings. Fresh least-privilege positive and conflict canaries
remain required before runtime activation.
