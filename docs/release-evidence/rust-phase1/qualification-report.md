# Rust Phase 1 qualification report

Phase 1 is implemented and independently approved at exact code lineage
`09191ca5d3f3254eb6d2deb750e9f65a2c77df7a` and immutable tree
`c3ff635dd5c2dd7e344054261b41bfad617fe4da`. Local qualification and hosted
macOS, Linux, and Windows CI passed. Phase 2 is not authorized by this report.

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

Grocery Phase A is deployed at backend commit
`f7b0eebca879840995226ede9ea715dc8702313a`; schema revision `096`, runtime
readiness, `capabilities.grocery = "v1"`, and the public Grocery scopes are
live. No final Grocery request/response DTO, REST call, or tool binding was
added here. Those remain blocked until an approved contract import pins the
Grocery merge and deployed provenance, per-file hashes, and regenerated
aggregate digest. The later Rust implementation must pass fresh
least-privilege list, prepare/cancel, stale-version-conflict, and active-list
non-mutation canaries before Grocery activation.

No Kroger or other provider OAuth-token type/storage was added. That remains
blocked until Security D2's purpose-specific versioned integration-key custody
is deployed and reviewed. Health H1/H2 ports model server-held provider-neutral
state only; H3 remains false unless a separate runtime capability is proven.

## Phase 1 disposition

GO. Exact-head Rust CI run
[`29799184345`](https://github.com/frntrllc/heyfood/actions/runs/29799184345)
passed 39 jobs with one intentional conditional skip and zero failures; general
CI run
[`29799185069`](https://github.com/frntrllc/heyfood/actions/runs/29799185069)
passed 14 of 14 jobs. Windows broad-ACE replacement, atomic persistence,
concurrent writes, Credential Manager, broker attestation, PTY cancellation,
signal/restoration, and optimized performance qualification all passed. The
independent Rust specialist subagent approved the same immutable SHA with no
remaining code findings.

PR #18 remains draft. This disposition approves Phase 1 only; Phase 2 remains
unauthorized. The final Grocery contract-import/activation and Kroger
token-storage gates above remain in force.
