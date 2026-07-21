# Rust Phase 0 remediation evidence

**Evidence date:** 2026-07-20
**Exact code lineage measured:** `08cecb3a00bff6bbd670faf066105205f6e93b0b`
**Status:** local and exact hosted three-OS remediation evidence is green;
asset, Grok, and deterministic grocery C3/C4 provenance are specialist-approved;
overall Phase 0 approval is pending the final-delta exact-SHA review.
Cutover and Phase 1 are not authorized by this report.

## What the spike now proves

The internal `phase0_qualification` Cargo test executable is not a `[[bin]]`
target and cannot be installed as a user command. Its active-turn vertical
composes real TUI input/effects, the qualified supervisor seam, `RunTurn`, the
Reqwest/Rustls client, loopback HTTP/SSE, streaming UI delivery, Ctrl+C
cancellation, peer-observed socket close, `TurnFinished`, bounded worker/server
join, and PTY/ConPTY terminal restoration. The shipped `heyfood` executable
remains fail-closed with exit 78.

Additional direct proofs cover:

- catchable release-profile panic restoration (`panic = "unwind"`) and
  restoration-error propagation;
- cancellation while the UI event channel is full;
- 10,000-event/4 MiB turn-stream limits and 4 MiB UTF-8-safe semantic
  scrollback bounds;
- distinct presentation of before-acceptance, accepted, and
  dispatched/outcome-unknown cancellation;
- version-monotonic bounded credential replay, durable reconciliation, and
  explicit supervisor/signal-listener shutdown and join;
- owner-only Unix permissions and Windows DACL application;
- read-only/idempotent supported Python local-state import on every platform,
  with Python keyring secrets deliberately routed to reauthentication;
- a Windows qualification split between the real user Ctrl+C ConPTY path and a
  native `CREATE_NEW_CONSOLE` control-event delivery path.

Controlled HTTP uses loopback, so this is a Rustls-configured client composition
proof, not a public TLS/proxy/root-store qualification.

## Frozen inputs and current companion state

The harness consumes the frozen Python/backend refresh fixture and final
unpublished Python `0.4.0` converse/SSE oracle at
`73494a57468dac83b4904ce6c390e36926f5c6fe`.

Companion backend main is
`1a4a05b5799ba3050027171c1f98a2999c24df5c`: migration repair plus the
certifi-backed verified-TLS correction are merged, production is postflight
verified at sole head `095`, H1/H2 PR #79 and H3 backend/mobile PRs #96/#95 are
merged, and their health contracts are frozen under `fixtures/contracts/`.
Grocery PR #107 now supersedes PR #90 with a revision-096 candidate
at exact final-qualification head `f5bf2656`, claiming the authoritative-
snapshot/frozen-list/C1 and exact catalog corrections. Hosted checks and
independent review remain in progress; final Phase A provenance remains gated
on a current green reviewed merge, production migration, and live canaries.
Security PRs #108, #109, and #110 are merged; PR #110 still requires deployment
and exact-build postflight. No Kroger B1/B2 or Security D2 implementation is
visible. Exact merged C3/C4 schemas are now mirrored under
`fixtures/contracts/grocery-backend/` with deterministic import and validation.
`external-contract-status.md` records the exact boundaries.

## Clean local measurement

Measured from a detached, clean worktree at the exact code SHA on macOS 26.5
(25F71), Apple Silicon arm64; `rustc 1.94.0`; Cargo 1.94.0.

| Gate | Result |
|---|---|
| Internal qualification executable | 5 passed across the optimized functional and performance commands; 2 ignored helpers executed by their parent harnesses; 0 failed; 4.87 s combined test time |
| Supervised TUI → HTTP/SSE → cancel/socket close/join | passed |
| Python refresh/converse fixtures → persistence → SSE → RunTurn → Ratatui | passed |
| Stream/scrollback memory limits | typed failures and UTF-8-safe truncation passed |
| Cancellation with full UI channel | passed and returned the accepted-cancellation outcome |
| Durable lock/rotation/reconciliation/restart replay | passed |
| macOS PTY signal matrix | SIGINT, SIGTERM, SIGHUP passed |
| Terminal restoration | alternate screen left, bracketed paste disabled, canonical mode restored |
| Release-profile panic restoration | passed |
| Read-only Python local-state import | 5 passed; source immutable; credentials excluded; keyring disposition requires reauthentication |
| Windows hosted runtime | default/native-credential suites, real-console signal delivery, ConPTY restoration, and compilation passed |
| First-frame controlled probe | 30 samples; optimized p95 104 µs |
| Input-to-frame controlled probe | 2,000 samples with 500 semantic entries; optimized p95 470 µs |
| Workspace format/strict Clippy/tests | passed locally |
| Dependency DAG/contracts/assets/ledger/inventory validators | passed locally; assets, Grok ledger, and grocery freeze specialist-approved; overall Phase 0 review pending |

The timing probes use Ratatui's controlled `TestBackend`; they are regression
checks, not release-process startup, real terminal paint, network latency, idle
CPU, or steady-state RSS measurements.

## Exact local artifacts

Built from the clean exact worktree with the commands recorded in
`qualification-evidence.json`.

| Artifact | Shipped? | Bytes | SHA-256 |
|---|---:|---:|---|
| `target/debug/heyfood` | no | 444,344 | `b3439a86c557be5bcdb854fde0230536720a635e6d86559f6d0971022d5ed879` |
| `target/release/heyfood` | no; currently fail-closed | 333,760 | `da520e525e69ef412310b667922968c031ea075a0d18b64ecf8c7f8505ceb4ac` |
| release `phase0_qualification` executable | no | 4,237,056 | `8a1f0b0ccd17415203951bd25f47cf2cbaf414c3ddd6bed5c55ff059e820199c` |

## Hosted matrix

The immediate parent remediation SHA `be6695414abebc495e334b90551a958ccdb3af15`
passed 44 Rust jobs, including Ubuntu/macOS qualification and Cargo Audit/Deny.
Windows exposed three concrete issues: the SID parser omitted the leading `S`,
the default-feature matrix invoked a native-credential helper, and a ConPTY
process cannot be attached by its PID for `GenerateConsoleCtrlEvent`. Code SHA
`05ccda2` fixed all three, then its Windows run proved ConPTY restoration but
exposed an idle signal-supervisor join after the EOF path. Exact code SHA
`cf6aaf7` added explicit cancellation for that no-signal path; its Windows run
then reached the real-console child and exposed signal installation outside the
Tokio runtime. Code SHA `3b5c057` installed the signal source inside the
runtime. Code SHA `5851538` also follows the Windows console contract by
broadcasting within the sender's attached isolated console instead of using an
unsupported nonzero Ctrl+C process-group target. Code SHA
`695528d` additionally runs the unchanged 25 ms input budget in an optimized
build, avoiding invalid debug/shared-runner comparisons while retaining the
budget. Code SHA `50d3533` also creates the isolated Windows control-test
fixture before launching its child. Code SHA `d5c3558` moved the native sender
into the child's isolated console and serialized terminal-owning qualification
parents. Code SHA `580546f` switched to the programmatically supported
`CTRL_BREAK_EVENT`. Code SHA `989c77e` made a PowerShell/.NET host allocate the
real console before launching the Rust child; its hosted Windows run proved
native event delivery, but exposed an active-TUI fixture race in which
cancellation could precede the first streamed frame. Exact code SHA `08cecb3`
synchronizes cancellation on the rendered SSE marker and passed ten consecutive
optimized local repetitions. Its exact hosted run has 35 successful Rust CI
jobs and one expected skipped protected-environment provenance-approval job;
the companion CI workflow passed all 14 jobs. PR merge SHA `bd26707` and code
head `08cecb3` have the identical tree `3b99bb0`.

## Remaining Phase 0 gates

The authoritative machine-readable inventory has zero classified requirement
blockers. The specialized Rust reviewer already approved both first-party
asset provenance records and the Grok pattern-only ledger at `d738f8c`. The
remaining Phase 0 gate is an exact-SHA re-review of the deterministic grocery
C3/C4 import/provenance remediation and the assembled zero-blocker result.

The grocery correction/deployment sequence, Kroger B1/B2, Security D2, final
wire DTOs, signed installers, real-hardware RCs, and all 675 migration-ledger
mappings remain explicit later-phase or cutover gates. Per the authoritative
plan, recording those unfinished external dependencies does not serialize the
Phase 0 spike or generic Phase 1 foundation.

The next decision is mechanical: commit this exact evidence candidate, pass its
hosted matrix, and send that same SHA to the specialized reviewer. Only a GO
verdict permits asking the owner to authorize Phase 1.
