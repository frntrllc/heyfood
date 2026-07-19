# Rust Phase 0 remediation evidence

**Evidence date:** 2026-07-19

**Exact code lineage measured:** `72d5b3b2f1b11b807c2f7ba71690291f29c6cccf`

**Status:** local remediation evidence is green; updated hosted CI and independent Phase 0 approval are pending. Cutover is not authorized.

## What the remediation proves

The internal `phase0_qualification` Cargo test executable is not a `[[bin]]` target and cannot be installed as a user command. It composes the native file stores, frozen Python-compatible refresh headers/body/response, `RunTurn`, the Reqwest client configured for Rustls, SSE normalization, the Ratatui model/renderer, bounded cancellation, task join, PTY/ConPTY terminal entry, catchable signals where the host exposes them, and terminal restoration. The shipped `heyfood` executable remains fail-closed with exit 78.

Controlled HTTP uses a loopback listener, so this evidence proves the Rustls-configured client composition but does not claim a live TLS handshake, production service, proxy, or public-root qualification.

The harness consumes:

- `crates/heyfood-agent-runtime/tests/fixtures/python_backend_refresh.json`, the auth lane's frozen Python/backend refresh and fallback oracle;
- `crates/heyfood-bin/tests/fixtures/python-exported-turn.v1.json`, pinned to Python baseline `9c6b91929143180252ad1b644aea273729a1f1b9` and its SSE lifecycle test.

## Local results

Host: macOS 26.5 (25F71), Apple Silicon arm64; `rustc 1.94.0 (4a4ef493e 2026-03-02)`; Cargo 1.94.0.

| Gate | Result |
|---|---|
| Internal qualification executable | 4 passed; 1 ignored helper (the helper is executed by the parent PTY matrix) |
| Python refresh contract -> persisted rotation -> SSE -> RunTurn -> Ratatui | passed |
| Cancellation after application-observed SSE acceptance | peer EOF/reset observed; turn and controlled server joined within 3 seconds |
| macOS PTY catchable-signal matrix | SIGINT, SIGTERM, SIGHUP passed |
| Terminal restoration | alternate screen left, bracketed paste disabled, canonical mode restored after every signal case |
| Controlled first-frame probe | 30 warm samples; p95 1,289 µs |
| Controlled input-to-frame probe | 2,000 samples with 500 semantic entries; p95 7,456 µs |
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
| `target/debug/heyfood` | no | 444,104 | `d8ac6a19428b20264843b7badc603786b90f14cd1cfeed7af6527b07f1767797` |
| `target/release/heyfood` | eventual public name, currently fail-closed | 333,520 | `230b902310654f5d7ff6a5b3b674dc0af1ce3885c6cd486220309d6e8eba6be8` |
| release `phase0_qualification` test executable | no | 4,054,608 | `cdfdd0b7aba7dd2f70d5b56577794a7c1d21962b68ede26ba850a0cc38876d62` |

The machine-readable companion record is `qualification-evidence.json`.

## Hosted matrix

The updated workflow defines:

- the internal qualification executable on Ubuntu, macOS, and Windows;
- Phase 0 inventory validation on all three hosts;
- Cargo Audit with `--deny warnings` and Cargo Deny 0.20.2 via immutable action commit;
- an explicit workflow-dispatch approval mode that fails unless asset provenance contains an independent reviewer and exact reviewed commit SHA.

No hosted result is claimed for `72d5b3b`; the jobs must run on the evidence descendant before review. On Unix runners the PTY matrix delivers SIGINT/SIGTERM/SIGHUP. On Windows it exercises ConPTY entry/restoration via Ctrl+D; a real Windows console-close/control-event matrix remains a blocker.

## Phase 0 blockers

The authoritative inventory is `phase0-inventory.json`. Important blockers are:

- grocery Phase A schemas/fixtures exist only as mutable, uncommitted companion-worktree bytes; C1-C4 are merged, but there is no authoritative reviewed Grocery Phase A SHA or digest;
- backend endpoint-by-endpoint idempotency and release-metrics provenance is not frozen;
- the optional read-only Python importer is not evaluated;
- dietary and brand provenance each still require an independent exact-SHA review;
- Grok source/provenance and license review is absent;
- platform minimums, release-hardware owners, protected signing environment, and exact Sigstore identity expressions are absent;
- real keychain, microphone, TLS/proxy, Windows control event, installed artifact, signing, and release hardware qualification remain incomplete;
- all 633 Python migration entries remain unmapped, so DG-R5 and Python deletion are not authorized.

The correct decision remains: retain the Phase 0 spike for exact-SHA review, run the updated hosted matrix, keep every blocker visible, and do not begin cutover or encode mutable grocery draft contracts.
