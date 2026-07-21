# Rust Phase 1 qualification report

The prior Phase 1 candidate was measured at code lineage
`a3b953ec571099c5f34f1d91aac29c2cc2bfa901`, then superseded by a REQUEST
CHANGES verdict at evidence head `326c50c6`. Remediation and new exact-SHA
qualification are in progress. Phase 2 is not authorized by this report.

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

## REQUEST CHANGES remediation

- Frozen C3 semantics now retain distinct canonical confirmation and
  idempotency identities, explicit legacy-accept/accept-with-edits/cancel
  decisions, nullable household context hash version, and the exact
  cancel/edit/precondition/replay error cases. The provisional port carries the
  complete semantic command without defining a final REST DTO or endpoint.
- Grocery edit objects are bounded and redact from diagnostics. Fixture-driven
  tests cover legacy accept, cancel, edits, duplicate accept, accept after
  cancel, list replacement/version drift, context-hash drift, and independent
  hash-version drift.
- Health context freshness and integration connection status are distinct
  frozen enums; cross-domain ingress is rejected by fixture-driven tests.
- Windows persistence constructs a fresh protected DACL with one current-owner
  full-control ACE, then independently reads and verifies the persisted DACL.
  The Windows-only regression begins with explicit Everyone and BUILTIN Users
  grants on both a directory and a file.

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

REQUEST CHANGES. A subsequent independent review at evidence head `326c50c6`
identified two P1 defects: incomplete frozen C3 confirmation semantics and
incomplete removal of explicit foreign Windows ACL entries. It also identified
overbroad Health status domains and stale review-packet metadata. The prior GO
is superseded until those findings are corrected, a new exact remediation SHA
passes hosted qualification, and the Rust specialist approves that SHA.

PR #18 remains draft. Phase 2 remains unauthorized. The final Grocery Phase-A
wire and Kroger token-storage gates above remain in force.
