# Rust Phase 0 spike evidence

**Evidence date:** 2026-07-19

**Integration base measured:** `3d4ac7a2a23c357d845b06cf90a58c7b498b0485`

**Status:** local macOS qualification complete for the evidence below; GitHub CI has not yet run this evidence/CI commit; Phase 0 exit approval is not claimed.

## Exact integration lineage

The evidence was collected from the following first-parent descendant lineage:

| Commit | Purpose |
|---|---|
| `7d980f177c605e35fa95492ade115c05222c1414` | Rust workspace scaffold and dependency barrier |
| `64872de45da689db643cd7122971b485e482d999` | Rust build-artifact ignore policy |
| `d20f490c1dd9d1808120e169347ed33af3983761` | Phase 0 migration/contracts/assets freeze |
| `e21ac0e7bc002abd6f8a90ca61f4774465009507` | Core/application contracts and DG-R1 use case |
| `d05acfa103c122afff056676fe28556713622cd7` | Retained Ratatui vertical and fail-closed binary seam |
| `8e1536d0061a4ca5e8a1ebf8dfd3752e5890e988` | HTTP/SSE and native persistence adapters |
| `3d4ac7a2a23c357d845b06cf90a58c7b498b0485` | Dependency-policy remediation after audit findings |

The focused CI/validator/evidence commit is a direct descendant of `3d4ac7a`; its exact SHA is recorded in the review handoff because a document cannot contain the SHA of the commit that contains itself without changing that SHA.

The immutable Python compatibility baseline referenced by the ledger is `9c6b91929143180252ad1b644aea273729a1f1b9`, tree `b3cf49317b7ccbb42389411c819a925d3e8be3b9`, package version `0.3.2`.

### Dependency-policy deviation and remediation

A dependency audit during integration found RUSTSEC-2024-0436 (`paste`, unmaintained) and RUSTSEC-2026-0002 (`lru`, unsound) in the initially resolved Phase 0 graph. Commit `3d4ac7a2a23c357d845b06cf90a58c7b498b0485` therefore intentionally deviates from the original UI dependency pins: Ratatui moved from 0.29 to 0.30.2 and Crossterm moved from 0.28 to 0.29. The same commit refreshed `Cargo.lock` and added exact `=0.4.0` versions to internal path dependencies. All local compile, Clippy, default-feature tests, optional-feature checks, measurements, and validators in this report ran against that remediated integration tree. The dependency-policy workstream then ran `/tmp/heyfood-rust-tools/bin/cargo-audit audit --deny warnings` and `/tmp/heyfood-rust-tools/bin/cargo-deny check`; both exited 0. Cargo Deny still emitted non-fatal unmatched-license and duplicate-version warnings, while its advisories, bans, licenses, and sources checks completed successfully.

## Architecture seams proven

- `heyfood-core` owns dependency-light typed wire/domain contracts, conservative URL policy, secret redaction, credential generation, operation identity, and terminal-event vocabulary.
- `heyfood-application` owns the UI-independent `RunTurn` orchestration, refresh/stream cancellation boundaries, immutable workflow inputs, the single-writer generation check, and the non-cancellable durable commit boundary.
- `heyfood-agent-runtime` owns authenticated HTTP, refresh, conversational POST, SSE normalization, uncertain-POST no-retry behavior, and socket teardown on cancellation.
- `heyfood-platform` owns repository-local test seams for atomic owner-only persistence, locking, idempotent versioned credential rotation, reconciliation markers, and cross-process writes. The native-keychain feature compiles separately; real keychain behavior is not qualified here.
- `heyfood-tui` owns the retained reducer/model, bounded scrollback, short-poll event loop, responsive 40/80/120-column rendering, cancellation effects, and terminal-mode restoration guard.
- `heyfood-bin` is the composition boundary. The checked-in executable deliberately exits with `EX_CONFIG`/78 before touching terminal state until validated credentials and bootstrap state are wired. This is a qualification binary, not a shippable native client.
- `xtask` enforces the approved workspace dependency DAG and validates the language-neutral migration freeze, stable outbound-surface contract, and runtime assets without Python, shell hash tools, or new Cargo dependencies.

## Local macOS results

Host: macOS 26.5 (25F71), Apple Silicon `arm64`; `rustc 1.94.0 (4a4ef493e 2026-03-02)`, host `aarch64-apple-darwin`; Cargo 1.94.0.

### Tests and policy gates

| Command | Local result |
|---|---|
| `cargo fmt --all -- --check` | passed |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | passed |
| `cargo test --locked --workspace` | 49 passed, 0 failed, 0 ignored |
| `cargo clippy --locked --package heyfood-platform --all-targets --features native-credentials -- -D warnings` | passed on macOS |
| `cargo test --locked --package heyfood-platform --features native-credentials` | 5 passed; feature compiled on macOS; no live Keychain transaction |
| `cargo clippy --locked --package heyfood-voice --all-targets --features native-audio -- -D warnings` | passed on macOS |
| `cargo test --locked --package heyfood-voice --features native-audio` | compiled and passed its zero-test harness; no microphone/device qualification |
| `cargo xtask dependency-dag` | passed; approved 10-crate workspace and edges preserved |
| `cargo xtask verify-stable-contracts` | passed; 26 endpoints, 4 browser navigations, 2 local listeners |
| `cargo xtask verify-assets` | passed schema/shape/hash/provenance integrity for 4 assets; 2 reviews remain pending |
| `cargo xtask verify-migration-ledger` | passed freeze integrity; 633 entries = 601 pytest nodes + 32 non-pytest invariants; 0 mapped and 633 unmapped |
| `/tmp/heyfood-rust-tools/bin/cargo-audit audit --deny warnings` | passed after `3d4ac7a`; exit 0 |
| `/tmp/heyfood-rust-tools/bin/cargo-deny check` | passed after `3d4ac7a`; exit 0 with non-fatal unmatched-license/duplicate warnings |

The 49 workspace tests include 7 `xtask` tests. Three of those create corrupted scratch freezes and prove rejection of a changed pytest-node inventory hash, a changed stable-contract compatibility fixture hash, and a changed dietary asset/schema version. Phase 0's 633 unmapped ledger entries are intentionally accepted and reported, not presented as migration completion; DG-R5 still requires zero unmapped entries.

### Startup and artifact measurements

Both binaries were built from the same `3d4ac7a` integration tree plus the uncommitted evidence-only working delta, with default features:

- `cargo build --locked --package heyfood-bin`
- `cargo build --locked --release --package heyfood-bin`

Each startup sample measured wall time with Python `time.perf_counter_ns()` around a direct `subprocess.run`, with stdout/stderr discarded. One warm-up was followed by 30 warm launches. P95 is the nearest-rank 29th ordered sample. All 60 measured launches returned the intended fail-closed exit 78.

| Build | Minimum | Median | P95 | Maximum | Binary bytes | SHA-256 |
|---|---:|---:|---:|---:|---:|---|
| Debug | 1.713 ms | 2.022 ms | 2.710 ms | 2.795 ms | 444,152 | `bbc48bb75b381d05937a64e86926ab48e3b0e1d57950d7ea098b6d8cd448f834` |
| Release | 1.765 ms | 2.031 ms | 2.827 ms | 5.797 ms | 333,520 | `0a720323e874dc188d7386f7a6855952ba0a9b16767060922d72a392b4eef0af` |

`/usr/bin/time -l` reported 1,572,864 bytes maximum RSS and 1,048,888 bytes peak memory footprint for one debug fail-closed launch and the same values for one release fail-closed launch.

These values measure process launch through the deliberate pre-terminal refusal. They do **not** establish the first-visible-frame, steady-state RSS/idle CPU, input-latency, animation, or full authenticated-startup budgets. Those require an enabled, qualified composition path.

## Cancellation and resource evidence

The local suite provides executable evidence for the following bounded ownership seams:

- `cancellation_drops_the_sse_response_and_closes_the_peer_socket` observes peer EOF after cancellation, demonstrating that dropping the in-flight response closes the test TCP socket rather than leaving it alive.
- `cancellation_before_server_acceptance_does_not_mutate_credentials` proves a pre-accept cancellation cannot commit credentials.
- `cancellation_during_post_acceptance_commit_cannot_lose_rotated_credentials` proves the accepted rotation crosses a deliberately non-cancellable, idempotent commit boundary.
- `uncertain_conversational_post_is_never_retried` prevents duplicate side effects after an uncertain POST result.
- terminal tests cover restoration after normal return, body error, catchable panic, partial entry failure, explicit idempotent restore, and single-flush ordering.
- reducer/driver tests cover cancel-before-exit ordering, external-signal platform exit codes, double-exit behavior, stale-event rejection, and scrollback bounded by both semantic entries and rendered lines.
- persistence tests cover owner-only files, interrupted staging, reopen durability, reconciliation markers, idempotent rotation, and locked cross-process commits that leave a complete document.

No real microphone stream, live native keychain process, production HTTP peer, OS signal delivery, ConPTY, or authenticated end-to-end terminal session was used in this local evidence.

## Rust CI added, not yet executed

`.github/workflows/rust-ci.yml` defines immutable-pinned checkout steps (`actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0`, v7.0.0) and read-only permissions. Its jobs are:

- portable default-feature compile, rustfmt, Clippy, tests, dependency-DAG, stable-contract, asset/provenance, and migration-ledger jobs on `ubuntu-latest`, `macos-latest`, and `windows-latest`;
- explicit `native-credentials` and `native-audio` feature jobs on macOS and Windows.

Linux native audio is not implicitly enabled because the runner's ALSA development-library contract has not been established. Linux native credentials remain in the portable default-disabled build until a usable Secret Service/system-dependency qualification is designed. GitHub results must be linked after the workflow runs; this report does not claim that CI is green.

## Explicit gaps and stop conditions

- **Windows/ConPTY:** no Windows job has run yet, and no ConPTY interaction, resize, paste, signal, restoration, or Windows Terminal/PowerShell qualification exists. The workflow only establishes the opportunity to compile/test on a hosted Windows runner.
- **Linux:** no Rust workflow has run on hosted Linux yet; no common-terminal/SSH PTY, Linux ARM64, native audio/ALSA, Secret Service, file-permission, or idle-resource qualification exists.
- **Native keychains:** the macOS `native-credentials` feature compiled and the persistence suite passed, but no live Apple Keychain transaction was performed. Windows Credential Manager and Linux Secret Service are untested.
- **Native audio:** macOS feature compilation succeeded, but no microphone permission, device enumeration, capture, cancellation-close deadline, or WAV behavior was exercised. Windows is pending CI and Linux is intentionally not assumed.
- **Signing and distribution:** no macOS codesign/notarization, Windows signing, Sigstore identity/provenance, SBOM, release archive, installer manifest, rollback, or trust-bootstrap evidence exists.
- **Dependency policy follow-up:** the post-remediation Cargo Audit and Cargo Deny gates exited 0, closing RUSTSEC-2024-0436 and RUSTSEC-2026-0002 for the resolved graph. Cargo Deny's non-fatal unmatched-license and duplicate-version warnings still need deliberate cleanup or reviewed policy decisions; a warning-free dependency report is not claimed.
- **Provenance review:** both `assets/dietary/provenance.json` and `assets/brand/provenance.json` are structurally valid and their target/source bytes match declared hashes, but their review status is still `pending` with no reviewer or reviewed commit SHA. This blocks provenance approval.
- **Migration completion:** all 633 frozen Python test/invariant entries remain unmapped. The freeze is trustworthy, but DG-R5 is not satisfied and no Python deletion is authorized by this evidence.
- **Performance:** only fail-closed process startup and artifact/RSS samples were recorded. First frame, input-to-frame p95, steady-state RSS/CPU, rendering caps, 2,000-keystroke load, authenticated stream latency, and resource-close deadlines remain unmeasured.
- **Product path:** the binary intentionally cannot start an authenticated TUI. Real bootstrap/config/keychain creation, Python import, backend metrics/idempotency inventory, grocery companion contract provenance, and installed-artifact behavior remain future qualifications.

The correct Phase 0 decision at this SHA is therefore: retain the architectural spike and its frozen contracts for review, run and inspect the new three-OS workflow, close provenance/security/platform gaps, and do not claim cutover readiness or remove Python assets/tests.
