# Rust Phase 0 remediation evidence

**Evidence date:** 2026-07-20

**Exact code lineage measured:** `a105d285d6421bde58d312506a994c4f3dfa6a24`

**Status:** local remediation evidence is green; updated hosted CI and independent Phase 0 approval are pending. Cutover is not authorized.

## What the remediation proves

The internal `phase0_qualification` Cargo test executable is not a `[[bin]]` target and cannot be installed as a user command. It composes the native file stores, frozen Python-compatible refresh and complete converse request contracts, `RunTurn`, the Reqwest client configured for Rustls, bounded SSE normalization, the Ratatui model/renderer, bounded cancellation and file-lock acquisition, task join, PTY/ConPTY terminal entry, catchable signals where the host exposes them, and terminal restoration. The shipped `heyfood` executable remains fail-closed with exit 78.

Controlled HTTP uses a loopback listener, so this evidence proves the Rustls-configured client composition but does not claim a live TLS handshake, production service, proxy, or public-root qualification.

The harness consumes:

- `crates/heyfood-agent-runtime/tests/fixtures/python_backend_refresh.json`, the auth lane's frozen Python/backend refresh and fallback oracle;
- `crates/heyfood-bin/tests/fixtures/python-exported-turn.v1.json`, pinned to the final unpublished Python `0.4.0` oracle at `73494a57468dac83b4904ce6c390e36926f5c6fe` and its SSE lifecycle test.

## Local results

Host: macOS 26.5 (25F71), Apple Silicon arm64; `rustc 1.94.0 (4a4ef493e 2026-03-02)`; Cargo 1.94.0.

| Gate | Result |
|---|---|
| Internal qualification executable | 4 passed; 1 ignored helper (the helper is executed by the parent PTY matrix) |
| Python refresh contract -> persisted rotation -> SSE -> RunTurn -> Ratatui | passed |
| Complete Python converse headers/body fixture | passed, including app/device/API-key/request-ID headers and household/location context |
| Cancellation after peer-consumed POST body with withheld headers | passed; explicit dispatched/outcome-unknown result, no unsafe before-acceptance classification |
| Cancellation after application-observed SSE acceptance | peer EOF/reset observed; turn and controlled server joined within 3 seconds |
| SSE memory limits | typed line, event, and aggregate failures passed |
| Durable lock acquisition | blocking work isolated from async executor and failed with typed timeout within 2 seconds under contention |
| Credential commit replay after restart | passed against persistent commit identity with the original expected version |
| TUI finishing/single-flight and render invalidation | terminal content renders while submission remains closed through `TurnFinished`; idle redraw removed and frame ceiling enforced |
| macOS PTY catchable-signal matrix | SIGINT, SIGTERM, SIGHUP passed |
| Terminal restoration | alternate screen left, bracketed paste disabled, canonical mode restored after every signal case |
| Read-only Python local-state import | 5 passed: source immutability, account binding, local-state preservation, credential exclusion, idempotency/conflict refusal, keyring/unbound/unknown dispositions, malformed/symlink fail-closed behavior |
| Windows credential cross-check | Reversible file credentials remain fail-closed; default and `native-credentials` platform targets pass cross-target Clippy, and the full Credential Manager vertical is selected by the hosted Windows qualification job |
| Controlled first-frame probe | 30 warm samples; p95 1,942 µs |
| Controlled input-to-frame probe | 2,000 samples with 500 semantic entries; p95 7,382 µs |
| `cargo audit --deny warnings` | passed with Cargo Audit 0.22.2 |
| `cargo deny check` | passed with Cargo Deny 0.20.2; non-fatal duplicate/unmatched-license warnings remain visible |
| Dependency DAG | passed exact internal `=0.4.0` versions, path-only sources, exact edges, and direct `crates/` containment |
| Asset integrity | passed; two independent provenance reviews remain pending |
| Asset approval gate | failed closed as intended because those two reviews are pending |
| Phase 0 inventory | valid; unresolved requirements remain blockers |

The timing probes use Ratatui's controlled `TestBackend`. They are repeatable regression checks for composition/render work, not release-process launch, real terminal paint, end-to-end network latency, idle CPU, or steady-state RSS evidence.

## Exact local artifacts

Built with `cargo build --locked --package heyfood-bin`, `cargo build --locked --release --package heyfood-bin`, and `cargo test --locked --release --package heyfood-bin --test phase0_qualification --no-run` from a clean exact code checkout.

| Artifact | Shipped? | Bytes | SHA-256 |
|---|---:|---:|---|
| `target/debug/heyfood` | no | 444,104 | `de9463dbcdb7f0ae33e40d3f23e1c986552879d1bab54dcf3e18fd629f7549f5` |
| `target/release/heyfood` | eventual public name, currently fail-closed | 333,520 | `4ccbd4c46b3f787cc1f620449d8effbcef21c565f430899b41e9110d183a7b0d` |
| release `phase0_qualification` test executable | no | 4,087,792 | `faadacbb5e02e1b5bc1a54e5ad3b7b1a5e350052d95adb306ff23363eec93938` |

The machine-readable companion record is `qualification-evidence.json`.

## Hosted matrix

The updated workflow defines:

- the internal qualification executable on Ubuntu, macOS, and Windows;
- Phase 0 inventory validation on all three hosts;
- Cargo Audit with `--deny warnings` and Cargo Deny 0.20.2 via immutable action commit;
- an explicit workflow-dispatch approval mode that fails unless asset provenance contains an independent reviewer and exact reviewed commit SHA.

No hosted result is claimed for `a105d28`; the jobs must run on the evidence descendant before review. At exact head `7710571`, Legacy CI and 34 of 36 Rust jobs passed. The two failing Windows jobs both exposed the same additional portability defect: `LockFileEx` reports normal contention as raw OS error 33, which Rust/fs2 did not classify as `WouldBlock`, so the bounded retry loop failed immediately. This code lineage maps that exact Windows condition into the existing bounded retry/timeout behavior. It also retains the fail-closed reversible-file default and selects Credential Manager only in the explicit Windows qualification feature job. On Unix runners the PTY matrix delivers SIGINT/SIGTERM/SIGHUP. On Windows it exercises ConPTY entry/restoration via Ctrl+D; a real Windows console-close/control-event matrix remains a blocker.

## Phase 0 blockers

The authoritative inventory is `phase0-inventory.json`. Important blockers are:

- Grocery Phase A PR #90 has a substantial committed contract candidate, but it conflicts with current `main`, its PostgreSQL migration and aggregate CI gates fail, and the authoritative-household-snapshot and frozen-list-identity corrections remain required; there is no merge SHA, deployed capability, or approved aggregate digest;
- backend endpoint-by-endpoint idempotency and release-metrics provenance is not frozen;
- file-backed supported Python local state now has a read-only idempotent importer, and Windows reversible credential files remain fail-closed in favor of Credential Manager; selective reconciliation of local household state held in the Python keyring, application consumption/disposition UX, and hosted Windows native-credential evidence remain unresolved;
- dietary and brand provenance each still require an independent exact-SHA review;
- Grok source/provenance and license review is absent;
- platform minimums, release-hardware owners, protected signing environment, and exact Sigstore identity expressions are absent;
- real keychain, microphone, TLS/proxy, Windows control event, installed artifact, signing, and release hardware qualification remain incomplete;
- all 675 Python migration entries remain unmapped, so DG-R5 and Python deletion are not authorized.

The correct decision remains: retain the dormant Phase 0 foundation for exact-SHA review, run the updated hosted matrix, keep every blocker visible, and do not begin cutover or pin mutable grocery/health wire contracts. This evidence does not authorize Phase 1; Phase 1 belongs in a subsequent PR after the Phase 0 gates and independent review close.
