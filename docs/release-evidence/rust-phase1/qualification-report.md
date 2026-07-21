# Rust Phase 1 qualification report

Phase 1 is implemented and independently approved at exact code lineage
`a3b953ec571099c5f34f1d91aac29c2cc2bfa901`. The full local workspace,
policy, contract, migration-freeze, native-feature, real macOS Keychain, and
process-broker qualification passed. Phase 2 is not authorized by this report.

## Implemented boundary

- `heyfood-core` now owns normalized errors, command-independent validation,
  terminal-safe bounded presentation documents, schema-versioned config,
  generic Grocery semantics, and provider-neutral Health semantics.
- `heyfood-application` now owns the single-flight supervisor, immutable
  operation snapshots/generations, the existing three-class state writer,
  provisional Grocery ports and ID-only account/list/version cache, and H1/H2
  Health ports.
- `heyfood-platform` now owns separate native config/data/cache/runtime paths,
  opaque per-account state directories, schema-3 account binding and bounded
  replay state, validated
  TTY/proxy/custom-CA policy, native keyrings plus owner-only fallback, and a
  deadline-bounded child-process credential broker using anonymous pipes and
  exact-parent executable attestation.
- The frozen Python `0.4.0` validation oracle is consumed by Rust tests. Python
  is not invoked by production Rust code.

## Durable and privacy evidence

Tests cover cancellation before dispatch, uncertain outcomes after dispatch,
cancel immediately after credential acceptance, while queued behind the state
writer, during the atomic adapter section, and immediately after adapter
commit. They also cover durable replay after restart, stale presentation
rejection, config and local-first repair markers, bounded file locks/replay
state/record inputs, atomic replacement, interrupted staging, account-switch
isolation, and read-only/idempotent Python non-secret import. Fixture rows—not
hard-coded duplicates—drive the Python validation differential.

The real macOS Keychain round trip proves initialize/load/rotation/deletion
without a credential file. Broker evidence separately proves direct external
invocation is rejected and a prompting/hung child is killed by its deadline
without an orphan. Credential, Grocery item, Health, browser-auth, normalized
SSE, and presentation values redact from diagnostics; the Grocery
item-reference cache stores only server UUIDs.

## Deliberate external gates

No final Grocery Phase-A request/response DTO, REST call, or tool binding was
added. Those remain blocked until exact deployment, regenerated aggregate
digest, `capabilities.grocery = "v1"`, and live capability/scope/confirmation/
conflict/non-mutation canaries establish authoritative provenance.

No Kroger or other provider OAuth-token type/storage was added. That remains
blocked until Security D2's purpose-specific versioned integration-key custody
is deployed and reviewed. Health H1/H2 ports model server-held provider-neutral
state only; H3 remains false unless a separate runtime capability is proven.

## Phase 1 disposition

GO. Exact-head Rust CI run
[`29795094072`](https://github.com/frntrllc/heyfood/actions/runs/29795094072)
passed 39 jobs with one intentional conditional skip and zero failures; general
CI run
[`29795094087`](https://github.com/frntrllc/heyfood/actions/runs/29795094087)
passed 14 of 14 jobs. The independent Rust specialist subagent approved the
same immutable SHA with no remaining P0, P1, or P2 Rust findings.

PR #18 remains draft. This disposition approves Phase 1 only; Phase 2 remains
unauthorized. The final Grocery Phase-A wire and Kroger token-storage gates
above remain in force.
