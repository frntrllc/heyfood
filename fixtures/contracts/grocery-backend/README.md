# Grocery backend contract freeze

This namespace is the Phase 0 import boundary for authoritative grocery
companion contracts. The checked-in C3 confirmation contract and C4
application-capability/scope contracts are exact copies from their merged
hellofood commits. `cargo xtask import-grocery-contracts --source-repo PATH`
reproduces them, while the stable-contract and dedicated grocery validators
enforce every source/target SHA-256 value in `provenance.json`.

No Grocery Phase A wire artifact is frozen here. PR #107 is reviewed and
merged, but remains non-authoritative for Rust until Production moves from
`095` to `096`, the exact merge is deployed with a regenerated aggregate
digest, and the live backend advertises a proven `grocery: "v1"` capability.
Until those gates complete, the importer intentionally knows only the merged
C3/C4 inputs and cannot turn a backend merge into an authoritative DTO.
