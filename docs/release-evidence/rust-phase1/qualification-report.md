# Rust Phase 1 qualification report

Phase 1 is implemented at code lineage
`85ad15ae66538eafdfd1e3f4a34bf83ef680872b`. The full local workspace,
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
  opaque per-account state directories, schema-2 account binding, validated
  TTY/proxy/custom-CA policy, native keyrings plus owner-only fallback, and a
  deadline-bounded child-process credential broker using anonymous pipes.
- The frozen Python `0.4.0` validation oracle is consumed by Rust tests. Python
  is not invoked by production Rust code.

## Durable and privacy evidence

Tests cover cancellation before dispatch, uncertain outcomes after dispatch,
cancel immediately after credential acceptance, durable replay after restart,
stale presentation rejection, local-first repair records, bounded file locks,
atomic replacement, interrupted staging, account-switch isolation, and
read-only/idempotent Python non-secret import. The native broker round trip
proves initialize/load/rotation/deletion through the actual macOS Keychain with
no credential file. Credential, Grocery item, and Health values redact from
diagnostics; the Grocery item-reference cache stores only server UUIDs.

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
