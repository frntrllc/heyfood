# Rust Phase 1 qualification report

Phase 1 is implemented at code lineage
`891270e362f3bf45ed1a8daec213bd107651f2b6`. The full local workspace,
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

## Remaining Phase 1 gates

The evidence SHA must pass hosted macOS, Linux, and Windows CI, including the
native credential jobs, and then receive independent exact-SHA Rust/platform/
security approval. Until both complete, PR #18 remains draft and Phase 2
remains unauthorized.
