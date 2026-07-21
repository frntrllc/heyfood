# Grok reference provenance inventory

The machine-readable pattern-origin ledger is
`grok-pattern-origin.json`. It pins the requested Grok Build reference at
`b189869b7755d2b482969acf6c92da3ecfeffd36`, including the binary entry point
at `crates/codegen/xai-grok-pager-bin/src/main.rs`, terminal restoration,
bounded PTY qualification, and cancellation/background-completion reference
paths with source SHA-256 values.

The pinned repository license is Apache-2.0. Phase 0 uses the reference only
for architectural patterns: every ledger row names the independently written
heyfood implementation and records `copied_bytes: false`. No Grok crate,
symbol, asset, source file, or byte snapshot is present in the heyfood Cargo
workspace.

The specialized Rust reviewer approved the ledger at exact heyfood commit
`d738f8c0a2f02f677e7cdd5cb764bff11941db56` after verifying the upstream
hashes/license and the no-copied-bytes conclusion. The machine-readable review
metadata records that identity and commit.
