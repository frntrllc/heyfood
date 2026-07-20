# heyfood Rust native client and interactive TUI plan

**Status:** Draft v5 — final Python-oracle, health, and Phase A hardening reconciliation; exact-SHA re-review required
**Baseline:** final unpublished Python `0.4.0` candidate at `73494a57468dac83b4904ce6c390e36926f5c6fe`; the last public Python release remains `0.3.2`
**Reference plan:** `docs/plans/2026-07-19-heyfood-interactive-terminal-session-plan.md` at approved commit `56a4dca136a6d6f9ad3b5e99fa812ea433448d22`
**Reference implementation:** local Apache-2.0 Grok Build checkout at `b189869b7755d2b482969acf6c92da3ecfeffd36`
**Active companion:** `frntrllc/hellofood` Platform P0, Grocery Phase A/Kroger, Security D2, and Health H1-H3 workstreams dated 2026-07-19
**Primary user:** a developer using hello.food throughout the working day from a terminal
**Replacement target:** `0.4.0`, released only when the complete Rust client passes every gate
**License:** Apache-2.0

## Executive decision

Build the durable heyfood client in Rust.

Bare `heyfood` will become a native, fully interactive terminal application
modeled on the strongest interaction patterns in the Grok Build TUI:

- a persistent conversation viewport;
- a multi-line composer anchored at the bottom;
- responsive streaming while the composer remains usable;
- typed actions, effects, and supervised background operations;
- discoverable slash commands, completion, command help, and keyboard hints;
- registration, authentication, and dietary onboarding in one continuous
  first-run journey;
- first-class voice capture and transcript review;
- deterministic cancellation, terminal restoration, resize, and crash safety;
- self-contained binaries for macOS, Linux, and Windows.

The intelligence, dietary graph, user data, and agent service remain hosted by
hello.food. “Agent runtime” in this plan means the native client runtime around
the existing authenticated `/v1/agent/converse` SSE service. It does not mean
embedding a local model, shell executor, coding agent, or plugin/tool runtime.

This is an owner-directed big-bang replacement. The Python source remains in
the development branch only long enough to export contracts and act as a test
oracle. The completed cutover removes the Python implementation, packaging,
and release workflows in the same reviewed change that installs the Rust
workspace as the sole heyfood product. There is no supported split runtime,
parallel public channel, or gradual command-by-command migration.

The Rust artifact is not released until it passes every parity, security,
platform, installation, and end-to-end gate. Internal build artifacts may be
used for qualification, but users never receive a partial Rust client.

For grocery, this plan supersedes every Python/PyPI/released-`0.3.x` client
implementation clause in the historical grocery CLI surface directive. That
document remains product/contract input only; Platform P0 and Grocery Phase A
stay backend-owned, while all terminal implementation ships in native `0.4.0`.

For health, the Rust program owns H1/H2 one-shot and TUI surfaces over the
server-backed Oura contracts and later consumes provider-neutral Apple Health
H3 rollups. The CLI never reads HealthKit directly and never stores provider
OAuth tokens. Health H1/H2 is part of the native client program; H3 remains
capability-gated on separately reviewed mobile/backend consent, upload,
retention, encryption, and deletion contracts.

Upon independent approval, this plan supersedes the Python/`prompt_toolkit`
implementation choice in the reference plan. The reference plan's product,
privacy, cancellation, auth, voice, compatibility, testing, and rollout
requirements remain inherited unless this document explicitly replaces them.

## Why Rust is the right long-term choice

The product goal is no longer “improve a Python prompt loop.” The interactive
terminal is a primary hello.food client that we intend to keep investing in.
Building the full state machine, cancellation model, terminal recovery,
renderer, onboarding, and voice integration in Python first would duplicate
the hardest work if we later moved to a native client.

Rust materially improves the intended product through:

- one native executable without a Python, virtualenv, pip, or pipx requirement;
- Tokio-supervised asynchronous I/O and explicit cancellation ownership;
- Ratatui/Crossterm control over retained layout, input, resize, and restoration;
- a typed domain/application/runtime boundary shared by TUI and one-shot CLI;
- predictable packaging for developer workstations, SSH hosts, containers, and
  locked-down environments;
- first-class macOS, Linux, and Windows release targets;
- lower startup overhead and fewer interpreter/dependency conflicts;
- a foundation appropriate for a long-lived interactive client.

Rust does not make portability automatic. Credential stores, audio, browser
handoff, filesystem semantics, terminal capabilities, console signals, code
signing, TLS roots, proxies, and installers still require explicit platform
adapters and qualification.

## Current-system facts the rewrite must preserve

The final Python oracle is a mature compatibility surface, not a prototype. It
includes all public `0.3.2` behavior plus the merged-but-unpublished
`channels list/disconnect` and official production OAuth-client behavior:

- approximately 13,000 production Python lines and 9,000 test lines;
- 45 command handlers across agent, auth, channels, profile, household, restaurant,
  recipe, meal, voice, configuration, conversation, and account workflows;
- 643 passing tests at the audited baseline;
- stable `--json` output, error envelopes, exit codes, and deprecated `--raw`
  compatibility;
- registration intent, loopback PKCE, RFC 8628-style device flow, session
  refresh/revocation, profile readiness, and immediate onboarding;
- household scopes, local child profiles, consented adult profile sync, repair
  outbox, and account-bound local state;
- native and browser voice paths with `audio:transcribe` scope and processor
  consent policy;
- restaurant/menu polling, recipes, meals, account deletion, diagnostics, and
  named local/production contexts;
- account-owned AI-channel listing and disconnection;
- HTTPS enforcement and exact-loopback-only development exceptions;
- OS keyring support with a documented owner-only `0600` file fallback;
- PyPI trusted publishing, `pipx`, and the hosted `install.sh` channel.

The rewrite must port behavior and contracts, not translate files line by line.
The current `_ask_agent()` combines validation, scope resolution, payload
construction, SSE consumption, household effects, persistence, and rendering.
The Rust design separates those responsibilities before building breadth.

Phase 0's rebaseline audit verified the final oracle independently: 643
collected node IDs and 643 passes; normalized node-ID SHA-256
`49e6fe5429174a9ad8f6cf47d209365ca852ebed5b6f6fa7f86b824dfa4b0cd3`;
45 command leaves; 56 versioned `0.4.0` help fixtures; and the legacy `0.3.0`
fixture set. The current exporter does not yet export complete JSON/API/SSE
contracts, so it must be repaired before use. The 26-row called-endpoint fixture includes
channel listing but omits the real `GET /.well-known/oauth-authorization-server`
request because its extractor accepts only `/v1/`; the stable Phase 0 contract
therefore contains 27 HTTP rows plus browser navigations and local listeners
rather than perpetuating that blind spot.

The exact current asset hashes are recorded in the migration evidence, with
private canonical provenance pinned to clean monorepo commit
`27cab29dd3d17bb844462c8ec5340585b859b0ae`: dietary options
`40a26e22d7e729289ef5bf4052af841adc76029711d89c89f046eba87d533556`,
banner text `8f97c59f5eba7075891cb1aa31c300ea776a0ac57117ef290a7f7c6a07e4c50e`,
and palette `22978be9dd03ca5a194617940d0b78a495bdf7f3ecc40729af2f1322cca0d73e`.

## Product contract

### The single-command journey

On a supported interactive terminal:

1. The user installs one native artifact.
2. The user runs `heyfood`.
3. A contained hey.food ASCII animation establishes identity once.
4. The client checks local credentials and profile readiness without freezing
   the interface.
5. A new user sees **Create account** as the primary path and **Sign in** as the
   secondary path.
6. Registration proceeds directly through authentication and dietary
   onboarding.
7. The user lands in a persistent composer with concise command discovery.
8. Typed or dictated input becomes a live personalized dietary turn.
9. The session remains active until deliberate exit.

An existing user with valid credentials goes directly to the composer. A user
on SSH receives short-code device authorization rather than a broken remote
loopback callback. Automation continues through one-shot commands and `--json`.

### Target session layout

```text
┌─ hey.food ─────────────────────────────── Me · San Francisco ─┐
│                                                               │
│  You                                                          │
│  I ate a hamburger and had three beers for lunch.             │
│                                                               │
│  hey.food                                                     │
│  That meal may conflict with ...                              │
│                                                               │
│  Checking your dietary profile…                               │
│                                                               │
├───────────────────────────────────────────────────────────────┤
│  Ask about food, a meal, a restaurant, or a recipe…           │
│  >                                                            │
├───────────────────────────────────────────────────────────────┤
│  / commands · Ctrl+Space voice · Ctrl+C stop · ? help        │
└───────────────────────────────────────────────────────────────┘
```

The frame illustrates information hierarchy, not mandatory heavy borders.
Whitespace, responsive layout, the existing palette, and semantic status color
are the design contract. The giant banner never appears in an ordinary turn.

### Keyboard and composer contract

- `Enter` submits a non-empty prompt.
- `Shift+Enter` and `Alt+Enter` insert a newline when distinguishable;
  `Esc` then `Enter` and `Ctrl+J` are reliable fallbacks.
- Bracketed paste inserts multi-line content without accidental submission.
- `Up`/`Down` traverse the buffer first and current-process history at buffer
  boundaries.
- `Ctrl+R` searches current-process history only.
- `Tab` completes slash commands and supported arguments.
- `Ctrl+Space` starts/stops voice; `F8` is the fallback.
- Keyboard `Ctrl+C` clears a draft, cancels an active operation, or requires a
  second idle press to exit.
- An external process `SIGINT`/console interrupt cancels once, restores the
  terminal, and exits with the platform-equivalent interrupted status.
- `Ctrl+D` on an empty composer requests exit.
- `?` and `/help` open contextual help.
- Mouse input is optional; every action is keyboard-accessible.

### Initial slash registry

| Command | Purpose |
|---|---|
| `/help` | Contextual commands and keyboard help |
| `/new` | Start a fresh conversation |
| `/voice` | Start/stop capture or explain unavailable support |
| `/for NAME` | Target later turns to a household member |
| `/household` | Inspect or change household targeting |
| `/profile` | Profile readiness and profile actions |
| `/location` | Inspect or update location context |
| `/status` | Auth, service, scope, profile, and voice readiness |
| `/clear` | Clear visible scrollback without claiming server deletion |
| `/exit` | Restore the terminal and exit |

Each registry entry has a canonical name, aliases, usage, description,
argument parser, availability predicate, and typed action. Slash handlers call
application use cases; they never re-enter the Clap parser or spawn `heyfood`.

### Viewport, focus, and response interaction contract

“Grok-quality” is a testable interaction contract, not a visual aspiration:

- The composer owns focus by default. `PageUp`/`PageDown` scroll the transcript
  without moving composer history; `Ctrl+Home` reaches the first entry and
  `Ctrl+End`/`End` returns to the live tail.
- New streamed content follows the tail only while the viewport is already at
  the tail. If the user scrolls away, the viewport never yanks them downward;
  an accessible **N new lines** indicator appears and activates with `End`.
- Long prose, nested lists, safety explanations, and code blocks reflow at the
  current width without hiding the composer. Pathological unbroken input is
  clipped or safely wrapped within documented bounds rather than corrupting
  layout.
- `Tab`/`Shift+Tab` move among actionable links, choices, and confirmations;
  arrows move within a choice group; `Enter` activates and `Esc` returns focus
  without silently choosing a dietary or account action.
- Copy mode permits keyboard selection and copy of response text, validated
  links, the device-flow URL, and the short code. A dedicated **Copy code**
  action remains available where terminal selection is unreliable. Copy never
  submits, opens, or logs the selected value.
- Empty, first-load, reconnecting, streaming, awaiting-choice, offline,
  cancelled, and terminal-error states each have a defined semantic snapshot,
  focus target, keyboard escape, and classic-mode equivalent.
- At 40 columns, the viewport collapses metadata before content or input. At
  larger widths, whitespace and restrained color establish hierarchy; giant
  banners, repeated frames, and noisy spinners remain prohibited.

Reducer, snapshot, and PTY/ConPTY tests cover these behaviors with long wrapped
responses, code, lists, links, multiple streamed updates while scrolled up,
choice/confirmation focus, device-code copy, every empty/loading/error state,
and resize during selection.

## Architectural principles

1. **One application core.** TUI, one-shot human output, and JSON call the same
   use cases.
2. **Pure decisions, supervised effects.** State transitions are synchronous
   and testable; network, storage, browser, audio, and timers are effects.
3. **One stateful workflow at a time.** A responsive composer does not permit
   concurrent mutation of conversation, household, auth, or profile state.
4. **Semantic presentation first.** Dietary results become renderer-neutral
   documents before TUI, ANSI, or JSON formatting.
5. **Machine output is sacred.** `--json` remains one ANSI-free value on stdout.
6. **Cancellation owns resources.** Dropping UI events is never mistaken for
   stopping HTTP, auth, audio, or persistence.
7. **Privacy by default.** Prompt history is process-memory-only; secrets and
   dietary content never enter logs.
8. **The terminal is borrowed.** Every normal, cancelled, panicked, catchably
   signaled, and suspended path restores it.
9. **Compatibility is measured.** Rust behavior is proven against language-
   neutral fixtures exported from the Python baseline.
10. **One complete cutover.** Python is a temporary development oracle, then
    the repository, installer, documentation, and release system switch to Rust
    together.

## Cargo workspace architecture

```text
Cargo.toml
Cargo.lock
rust-toolchain.toml
deny.toml
crates/
├── heyfood-core/
│   └── src/lib.rs
├── heyfood-agent-runtime/
│   └── src/lib.rs
├── heyfood-application/
│   └── src/lib.rs
├── heyfood-platform/
│   └── src/lib.rs
├── heyfood-voice/
│   └── src/lib.rs
├── heyfood-cli/
│   └── src/lib.rs
├── heyfood-tui/
│   └── src/lib.rs
├── heyfood-installer/
│   └── src/main.rs
└── heyfood-bin/
    └── src/main.rs
```

### Dependency DAG and port ownership

The workspace dependency graph is fixed before scaffolding:

```text
heyfood-core
    ▲
    ├── heyfood-application     # owns all outbound port traits
    │       ▲
    │       ├── heyfood-agent-runtime  # implements service/transcription ports
    │       ├── heyfood-platform       # implements config/credential/browser/clock ports
    │       ├── heyfood-voice          # implements capture/encoding ports only
    │       ├── heyfood-cli            # invokes use cases; leaf presentation
    │       └── heyfood-tui            # invokes use cases; leaf presentation
    │
    └───────────────────────────────────────────────────────────────
                          heyfood-bin depends on and wires every adapter/surface

heyfood-installer ──> heyfood-core  # standalone bootstrap/manifest verifier
```

- `heyfood-core` has no workspace dependencies.
- `heyfood-application -> heyfood-core` and defines `ServicePort`,
  `CredentialPort`, `ConfigPort`, `BrowserPort`, `ClockPort`, and
  `AudioCapturePort` and `ClipboardPort` traits plus use-case DTOs.
- `heyfood-agent-runtime -> heyfood-application + heyfood-core`; it implements
  authenticated API and transcription upload. It never imports CLI/TUI/platform.
- `heyfood-platform -> heyfood-application + heyfood-core`; it implements
  storage, credentials, browser, clock, TLS-root, and signal/console adapters.
- `heyfood-voice -> heyfood-application + heyfood-core`; it captures/encodes
  audio only. The application coordinates transcription through `ServicePort`,
  preventing voice/runtime cycles.
- `heyfood-cli` and `heyfood-tui -> heyfood-application + heyfood-core`; neither
  depends on concrete runtime/platform/voice adapters.
- `heyfood-installer -> heyfood-core`; it is a separate minimal release binary
  that verifies signed release metadata and installs an already-built target
  artifact. It imports no application, API, TUI, voice, or credential code.
- `heyfood-bin` is the only crate that depends on all implementations and
  constructs trait objects/generic compositions.
- `heyfood-platform` emits dependency-neutral `SignalEvent` and
  `NetworkPolicy` values defined in core/application; `heyfood-bin` translates
  signals into TUI actions and passes network policy into the runtime. The TUI
  retains sole ownership of terminal RAII, and runtime/platform never import
  one another.
- No lower layer imports a surface crate, and no adapter imports another
  adapter. `cargo metadata` plus a repository dependency-policy test enforces
  the DAG in CI.

### Phase 0 dependency and feature policy

- Rust 2024 edition; build toolchain and `rust-version` pinned to `1.94.0`, the
  locally verified stable toolchain. Changing it requires a reviewed toolchain
  update, not an implicit CI drift.
- Async: Tokio 1.x with only required `rt-multi-thread`, `macros`, `sync`,
  `time`, `signal`, `net`, `io-util`, `fs`, and `process` features; Tokio-util
  0.7 `CancellationToken`.
- TUI: Ratatui 0.30.2 and Crossterm 0.29 with `event-stream` and
  `bracketed-paste`; no Grok-internal Ratatui fork.
- CLI/data: Clap 4 derive/completion, Serde/Serde JSON 1, UUID 1, URL 2, time
  0.3, `secrecy` plus `zeroize` for secret-bearing values.
- HTTP/TLS: Reqwest 0.12 with default features off, Rustls, streaming, JSON,
  multipart, and native-root loading; no OpenSSL dependency. Standard native
  roots are primary, `HEYFOOD_CA_BUNDLE` is the explicit custom-CA path, and
  certificate/hostname validation cannot be disabled.
- SSE: `eventsource-stream` 0.2 or a smaller audited parser over Reqwest bytes;
  implicit reconnect is prohibited.
- Files: `directories`/platform APIs for paths, `fs2` or a reviewed equivalent
  for locks, and same-directory exclusive-create staging plus atomic replace.
- Credentials: evaluate `keyring` 3 with target-specific features; if it cannot
  satisfy bounded behavior, implement the internal broker with
  `security-framework` (macOS), `secret-service` (Linux), and `windows` APIs.
- Browser/callback: `webbrowser` or a minimal platform adapter for launch; a
  Tokio loopback listener with a bounded, single-purpose HTTP parser instead of
  a general web framework.
- Voice: CPAL 0.16 and Hound 3, target-gated; Linux ALSA/Pulse requirements are
  recorded before GA and absence produces browser/typed fallback.
- Diagnostics: Tracing 0.1 and a redacting subscriber; no OpenTelemetry/client
  analytics dependency in `0.4.0`.
- Tests: Insta 1, Proptest 1, a controlled HTTP fixture server, and
  portable-pty 0.9/ConPTY plus platform-native process tests.
- Release/security: `cargo-audit`, `cargo-deny`, `cargo-sbom` or a reviewed
  equivalent. Sigstore, DSSE, RFC 8785/JCS, and TUF verification libraries are
  confined to `heyfood-installer` and release tooling.
- All exact versions and checksums are committed in `Cargo.lock`. Git
  dependencies, wildcard versions, multiple TLS stacks, default-enabled
  telemetry, and build scripts that fetch network content are prohibited.
- Target-specific default features are explicit. `--all-features` CI must not
  accidentally combine mutually exclusive credential/audio backends; those
  combinations get named feature-matrix jobs instead.

### `heyfood-core`

Owns dependency-light product contracts:

- validated API/auth URLs and exact-loopback policy;
- configuration schema and migrations;
- authentication/session/profile/household/location/conversation value types;
- API request/response and SSE event types;
- provider-neutral health connection, freshness, trend, and integration-state
  values; no HealthKit or provider-token representation;
- safety status vocabulary;
- normalized error and exit categories;
- `PresentationDocument` and semantic blocks;
- command-independent validation;
- redaction and sensitive-value wrappers.

It imports no Ratatui, Crossterm, Clap, Reqwest, keyring, or audio dependency.

### `heyfood-agent-runtime`

Owns async service communication:

- Reqwest client construction with Rustls and platform root policy;
- metadata/capability discovery;
- session refresh and authenticated requests;
- finalized H1/H2 health-context and integration-management REST contracts;
- `/v1/agent/converse` SSE parsing and normalized runtime events;
- tracing `X-Request-ID` per attempt;
- logical operation ID, verified idempotency key/fingerprint, and existing
  pending-confirmation key as distinct contracts;
- finite connect/write/pool and SSE inactivity/heartbeat deadlines;
- `CancellationToken` ownership and response-drop connection close;
- no automatic uncertain POST replay until backend support is proven;
- bounded event channels and explicit backpressure/coalescing.

This is a hosted-service client runtime, not a local inference/tool runtime.

### `heyfood-application`

Owns UI-independent use cases and state coordination:

- register, login, logout, status, and account deletion;
- profile readiness, onboarding, and explicit profile-sync consent;
- ask, reply, conversation, confirmation, and household targeting;
- meals, items, restaurants, menus, recommendations, and recipes;
- grocery capability discovery, screened-list reads, optimistic mutations,
  confirmation, exclusions, weekly proposals, export, and item references;
- health status/context, Oura connect/sync/disconnect, completion polling, and
  health-aware turn preparation without making optional health scopes a
  prerequisite for ordinary conversation;
- configuration/context/location operations;
- single-flight workflow supervisor;
- immutable operation snapshots and generation IDs;
- one serialized state writer for config/auth/scope/conversation mutations;
- local-first effect disclosure and authoritative reconciliation;
- renderer-neutral progress, choices, failures, and presentation documents.

### `heyfood-platform`

Owns platform-specific adapters behind traits:

- config/data/cache paths;
- atomic file persistence and cross-platform locks;
- macOS Keychain, Linux Secret Service, and Windows Credential Manager;
- browser launch and loopback callback binding;
- bounded native clipboard access with capability detection, explicit maximum
  size, no background reads, sensitive-code confirmation, and a copy-mode/
  manual-selection fallback; OSC 52 is disabled unless separately reviewed;
- TTY/capability/SSH detection;
- Unix signals and Windows console control events;
- native certificate roots and enterprise proxy behavior;
- secure process-isolated credential broker when a backend cannot provide a
  bounded cancellable call;
- updater/install metadata primitives, without self-modifying during `0.4.0`.

The credential broker uses an internal binary mode with inherited anonymous
pipes. Secrets never appear in argv, environment, logs, or generic temporary
locations. File fallback uses an owner-only atomic staging file in the config
directory, then flush/fsync, replace, and parent-directory fsync where
supported.

### `heyfood-voice`

Owns voice policy and lifecycle:

- processor selection and explicit consent;
- native capture abstraction using platform-qualified audio backends;
- browser capture callback server fallback;
- WAV encoding and upload to the existing transcription endpoint;
- `RECORDING -> TRANSCRIBING -> REVIEW` state with one final transcript;
- immediate stream/server close on cancel;
- missing device/dependency/permission/scope/timeouts and typed fallback;
- no local transcript persistence.

Voice remains optional at runtime when hardware/platform support is absent, but
the GA matrix must provide a useful typed fallback and a qualified capture path
for every advertised platform.

### `heyfood-cli`

Owns Clap and one-shot presentation:

- the complete existing command tree and option grammar;
- `channels list/disconnect`, H1 `health status/show`, and H2
  `health connect/sync/disconnect oura`;
- human ANSI renderer over `PresentationDocument`;
- JSON renderer and stable error envelopes;
- exit-code mapping;
- shell completion generation;
- `--version`, `--verbose`, `--no-banner`, `--json`, `--raw`, `--no-input`, and
  other compatibility behavior;
- classic line-oriented chat fallback.

It contains no HTTP business rules or direct secret/config mutation.

### `heyfood-tui`

Owns the retained terminal application:

- Ratatui layout and Crossterm event translation;
- `AppModel`, `Action`, `Effect`, `RuntimeEvent`, and pure dispatch;
- Tokio `select!` event loop;
- bounded scrollback and semantic rendering;
- composer, current-process history, paste, completion, slash registry;
- auth/onboarding/voice panels and modals;
- provider-neutral health status/trend cards and Oura connection lifecycle;
- demand-driven animation ticks and invalidation;
- focus, responsive layout, accessibility/classic handoff;
- terminal guard, panic restoration, resize, suspend/resume, and signals.

### `heyfood-bin`

Owns composition only:

- parse top-level mode;
- construct configuration and platform adapters;
- initialize Tokio and redacted tracing;
- choose TUI, classic, or one-shot execution;
- install terminal/panic/signal guards;
- map the final typed outcome to a process exit.

`main.rs` should remain approximately 100–150 lines. The Grok binary is a
valuable composition-root reference, but its 2,800+ line `main.rs` is not the
target.

## TUI event and effect model

The central loop is intentionally thin:

```text
terminal input ─┐
runtime events ─┼─> Action -> dispatch(AppModel) -> [Effect] -> supervisor
auth/voice ─────┤                  │                    │
ticks/signals ──┘                  └──── render <───────┘
```

- `dispatch` performs synchronous state transitions and returns effects.
- Effects run under a supervisor with a cancellation token, operation ID,
  finite deadline, and terminal result.
- All application/runtime channels are bounded. Replaceable progress events
  coalesce; response fragments coalesce before enqueue; terminal/error events
  apply backpressure.
- Each stateful workflow owns an immutable auth/config/scope snapshot.
- Workers propose mutations; the sole state writer accepts them only for the
  current generation.
- One active stateful workflow is permitted. Draft editing and local help remain
  responsive. At most one follow-up draft is retained in memory and returned to
  the composer for explicit post-result submission; it is never auto-sent into
  a new choice or confirmation state.
- Mutating `/for`, `/new`, `/location`, `/profile`, auth, and onboarding require
  cancel-and-join before starting.
- Read-only status during a turn uses a local snapshot; it cannot refresh tokens
  or mutate config concurrently.
- Idle views request no animation tick. Startup animation runs at 12 fps or less
  and ordinary rendering is capped at 30 fps.

### Cancellation-safe durable mutations

A generation check may reject stale presentation and conversation-pointer
updates, but it must never discard a server-accepted security mutation or a
local-first durable commit. The state writer distinguishes three classes:

1. **Generation-scoped state** — viewport progress, draft-derived state,
   response fragments, and a conversation pointer proposed by a cancelled turn.
   These apply only to the current generation.
2. **Server-accepted durable state** — refresh-token rotation, authorization
   exchange, validated new credentials, logout/revocation bookkeeping, and
   server-confirmed profile/account identifiers. After the response is
   authenticated and validated, the writer persists this class regardless of
   whether the initiating view generation has gone stale.
3. **Local-first durable state** — disclosed household/profile effects, repair
   outbox records, migration markers, and other mutations that are committed
   before authoritative reconciliation. Once the local commit begins, it must
   either complete atomically or remain a recoverable journal/outbox entry.

Cancellation is fully effective while a request is in flight. Once a response
has crossed the defined server-acceptance boundary, cancellation suppresses
further UI work but waits for the serialized durable commit before advancing
generation or exiting. The non-cancellable region covers only the bounded
local lock/keyring/write/flush operation—never network I/O, user prompts, or an
unbounded credential-store call. A broker timeout or uncertain auth exchange
marks the session **reconciliation required** and forces verified refresh or
re-authentication; it does not silently restore the old token or delete the new
one. Each proposal carries operation ID, mutation class, expected account,
credential version, and commit ID so duplicate delivery is idempotent.

Deterministic tests cancel before response, immediately after server
acceptance, while queued behind another writer, during atomic replacement, and
immediately after commit. They prove a rotated token remains usable, old
credentials are not resurrected, local-first effects remain repairable, stale
conversation UI does not reappear, and process exit waits only for the bounded
commit deadline.

## Terminal lifecycle

`TerminalGuard` is the sole owner of:

- raw/cooked mode;
- alternate-screen entry/exit;
- cursor visibility/style;
- bracketed paste and optional mouse capture;
- keyboard enhancement negotiation;
- final byte ordering during restore.

The application owns input bindings and drawing, not terminal-mode sequences.
The guard restores on normal exit, returned errors, cancellation, panic hooks,
catchable Unix signals, catchable Windows console events, and child/browser
suspension. No client can run cleanup after `SIGKILL`, power loss, kernel panic,
or unconditional process termination; PTY tests verify catchable paths, and the
troubleshooting runbook documents `reset`/`stty sane` recovery for uncatchable
termination instead of making an impossible guarantee.

Signal behavior remains distinct from keyboard behavior:

- keyboard `Ctrl+C`: clear draft, cancel active work, or double-press idle exit;
- Unix `SIGINT`: one-shot cancel, restore, exit `130`;
- Unix `SIGTERM`: cancel, restore, exit `143`;
- Unix `SIGHUP`: cancel, best-effort restore, exit `129`;
- Unix `SIGTSTP`/resume: restore before suspend, re-enter and redraw after resume;
- Windows `CTRL_C_EVENT`, close, logoff, and shutdown events: cancel, restore,
  and return documented Windows process outcomes within platform constraints.

Panic output and redacted diagnostics appear only after restoration. Worker
tasks never write directly to stdout/stderr while the TUI owns the frame.

## Authentication and onboarding

Reuse the deployed hello.food services and scopes.

- Production API: `https://api.hello.food`.
- Public authorization origin: `https://auth.hello.food`.
- A local desktop TTY defaults to loopback PKCE.
- SSH/headless, loopback bind failure, or browser suppression selects/offers
  RFC 8628-style short-code device flow.
- Only browser launch suspends terminal rendering; authorization waiting remains
  a cancellable background operation with visible URL/code/status.
- Registration uses the existing create-account intent and immediately enters
  onboarding after authentication.
- Capability discovery, least-privilege scopes, HTTPS enforcement, token
  storage, refresh, revocation, expiry, and account binding remain fail closed.
- Profile-sync consent is a distinct explicit action; registration and
  onboarding never imply it.
- Cancellation closes callback listeners and pollers.
- Expired, denied, invalid, abandoned, offline, and partial-profile paths are
  first-class states, not generic failures.

### Loopback OAuth threat model

- Generate a fresh 256-bit `state` value and PKCE S256 verifier for every
  attempt; keep them in memory only and compare state in constant time.
- Bind an operating-system-assigned high port on exact loopback addresses only
  (`127.0.0.1` and, where supported, `::1`). Never bind wildcard, LAN, or a
  predictable fixed port, and never relax the production redirect-origin rule.
- Accept exactly one bounded `GET /oauth/callback` request. Reject other methods
  and paths, duplicate terminal results, malformed percent encoding, headers or
  request targets above 8 KiB, missing/duplicate parameters, and state mismatch.
- Treat the browser request as hostile: do not trust `Host`, forwarded headers,
  browser origin, query ordering, or callback text. The client validates only
  its exact listener, path, state, code/error shape, and PKCE token exchange.
- Close all listeners immediately after the first valid terminal result,
  cancellation, or deadline. A malicious invalid request cannot consume the
  one valid attempt; invalid requests are bounded and counted until the overall
  deadline/rate ceiling fires.
- The browser success page contains no token, profile, prompt, or dietary data.
  Authorization codes, verifiers, state, device codes, and callback URLs are
  redacted from logs and crash output.
- Device flow independently validates verification origin, code format,
  interval/backoff, expiry, and `slow_down`; it never accepts a code through
  the local callback endpoint.

## Presentation and dietary safety

Every supported service result becomes a renderer-neutral
`PresentationDocument` before output. It covers:

- ordinary assistant Markdown;
- safety verdicts and explanations;
- restaurant/menu/recipe/meal/item structures;
- household scope and confirmation choices;
- progress and continuation hints;
- explicit service failures and recovery guidance.

The TUI and one-shot ANSI renderer consume identical documents. JSON uses its
versioned wire contract and never scrapes formatted text.

Service text is untrusted data:

- sanitize terminal control characters and escape sequences;
- never interpret Rich markup;
- allowlist Markdown paragraphs, emphasis, lists, code, and validated links;
- degrade unsupported constructs to literal/plain text;
- bound lines, blocks, links, and total retained content;
- never allow a service payload to emit OSC/CSI terminal control.

The safety vocabulary remains `generally_safer`, `risky`, `avoid`, and
`unable_to_evaluate`. The client never upgrades uncertainty or substitutes the
word “safe.”

## Compatibility and migration contract

### Language-neutral fixture freeze

Before porting behavior, export reviewed fixtures from the final Python oracle
at `73494a57468dac83b4904ce6c390e36926f5c6fe` for:

- command tree, help, arguments, options, aliases, and deprecations;
- exit codes and error categories;
- JSON success/error documents;
- config schema, migrations, redaction, and account binding;
- auth requests, scopes, callbacks, device polling outcomes, and session data;
- every called endpoint, HTTP method, headers, payload, and SSE event;
- household/profile/onboarding/consent transitions;
- presentation documents and responsive snapshots;
- banner text/palette/frame manifest;
- installer and version output.

Fixtures live outside Python modules and are consumed by both Python and Rust
tests. Intentional differences require a versioned fixture update, migration
note, and independent review.

### Python test-migration ledger

The statement “643 tests passed” is not deletion evidence. Before any Python
test or implementation file is removed, freeze
`tests/migration/python-test-ledger.json` from the committed baseline using the
exact `pytest --collect-only` node IDs plus every non-pytest CI, release-script,
schema, documentation, and installed-artifact invariant. Each ledger entry has:

- `baseline_sha`, stable `invariant_id`, original node ID/path, and category;
- disposition: `rust_test`, `fixture`, `installed_artifact_qualification`,
  `platform_qualification`, or independently approved `retired`;
- one or more concrete Rust test IDs, fixture paths, evidence records, or
  qualification IDs;
- rationale, owner, reviewer, and reviewed commit SHA.

The checked-in alias makes `cargo xtask verify-migration-ledger` equivalent to
`cargo run -p xtask -- verify-migration-ledger`. It checks every collected
Python node and inventory invariant has exactly one reviewed disposition, every referenced
replacement exists, there are no dangling/duplicate mappings, and retirement
entries carry approval. The committed baseline collection and generated count
are immutable review evidence. DG-R5 requires zero unmapped items; aggregate
test totals or a green Rust suite cannot substitute for the ledger.

The called-endpoint/auth contract remains at a stable language-neutral path
(`fixtures/contracts/called-endpoints.json`) while any backend consumer still
depends on it. Coordinated consumers may migrate to the stable path before the
old generator is deleted; the big-bang client rewrite does not orphan a
cross-repository security check.

### Runtime asset ownership and provenance

Python currently carries runtime data that must not vanish with
`src/heyfood_cli`: dietary options, banner text, and the banner palette. Rust
GA moves them to explicit language-neutral sources:

```text
assets/dietary/dietary_options.v2.json
assets/dietary/provenance.json
assets/brand/banner.txt
assets/brand/banner.palette.json
assets/brand/banner.frames.json
schemas/dietary-options.v2.schema.json
schemas/banner-palette.v1.schema.json
schemas/banner-frames.v1.schema.json
```

The private monorepo `shared/dietary_options.json` remains the canonical
dietary source. An audited export validates its schema and copies an exact
snapshot into this public repository while recording source repository,
source commit, source file SHA-256, export tool version, review, and target
SHA-256 in `assets/dietary/provenance.json`. Public CI validates schema,
version, target hash, and provenance without requiring private-repository
access. A changed dietary snapshot requires the existing shared-contract review
across backend/mobile/client owners.

Brand source parity is anchored to the reviewed monorepo references
`docs/references/banner.txt` and `docs/references/banner.ts`; CI consumes the
language-neutral palette JSON, never parses TypeScript. The frame manifest is
deterministically generated from the text/palette sources and records its
schema and source hashes.

All final assets are embedded at compile time with `include_bytes!` or
`include_str!`; the installed executable does not depend on a source checkout
or loose data directory. Unit tests validate schemas and hashes, archive tests
inspect the compiled behavior, and installed-artifact tests exercise dietary
onboarding plus the animation with the repository absent. DG-R5 may delete the
Python asset copies only after exact byte/semantic parity and provenance pass.

### Command-family parity

Rust GA includes the existing families:

- `ask`, `reply`, `chat`, `log`, `item`;
- `login`, `register`, `logout`, `status`, `doctor`;
- `channels list/disconnect`;
- `profile`, `onboard`, `members`, `household`;
- `search`, `menu`, `get-menu`, `recommend`, `location`;
- `recipes search/save/saved`;
- `daily`;
- `conversation list/resume/clear`;
- `voice devices/status/set/reset`;
- `context list/show/use/set`;
- `config path/show/validate`;
- `account delete`.

The replacement also includes the active grocery client surface when the
deployed backend advertises `capabilities.grocery = "v1"`:

- `grocery show` (default), `add`, `remove`, `bought`, `weekly`, `export`, and
  `never`, plus the shared `confirm` continuation;
- stable `--json` documents for every operation and headless confirmation;
- conversational and voice grocery turns through the existing agent path;
- version-bound item-index convenience with server IDs always available;
- per-member informational safety annotations, substitutions,
  `intended_for`, provenance, and ingredient-basis label guidance.

Deterministic grammar is `grocery add "<item>"...`; recipe expansion uses
`grocery add --recipe <ref>...`; non-interactive/JSON `grocery weekly` requires
explicit `--recipe <ref>...`, while the TUI may offer a saved-recipe picker.
Free-form dish language routes through the shared agent turn, never a REST
parser pretending to understand it. The item index cache is owner-only, bound
to API origin/context + account + list ID + list version, and expires after 15
minutes. It clears on context switch, logout/account switch, active-list
replacement, successful mutation, and expiry, and is never authoritative;
server item IDs always work. Unknown capability versions are unsupported rather
than guessed compatible with v1.

Grocery scope requirements are command-specific:

| Operation | Required authority |
|---|---|
| capability discovery | none/session metadata only |
| `show`, `export`, exclusions read | `grocery:read` |
| prepare add/remove/bought/weekly/exclusion mutation | `grocery:write` plus `grocery:read` when the response or precondition reads list state |
| confirmation accept/edit/cancel | authority frozen into the prepared confirmation; write is always required for a commit |

Read-only users retain useful list/export access. The client requests write
authority only for a write-capable session or explicit re-authentication, and
tests absent/read-only/write-only/read+write combinations against the backend
contract rather than treating the two scopes as interchangeable.

`grocery export --out FILE` handles dietary/member annotations as sensitive
local data: owner-only creation, exclusive create by default, explicit
overwrite, symlink/reparse-point-safe destination handling, same-directory
atomic replacement, no content logging, and cleanup on failure. `--json`
continues to emit exactly one value on stdout; file progress/errors use stderr.

The backend/platform grocery team owns `HouseholdContextResolver`, typed
safety-result metadata, generalized confirmation, optional scope tiers,
capabilities, the grocery domain/application service, tools, and REST APIs.
The Rust program owns all CLI grammar, TUI/voice interaction, rendering,
fixtures consumed by the client, and released-artifact qualification. No
Python grocery client is implemented or published during the transition.

Platform P0 C1-C4 are merged, but Phase A cannot treat C1 as a safety boundary
until a narrow hardening change preserves native household inputs instead of
collapsing them to `_self`, reports complete consent truth, surfaces persisted
single-profile versions, and makes hashing independent of input/member order.
Phase A must pass the authoritative `HouseholdContextSnapshot` unchanged
through REST, conversation tools, screening, and confirmation; reconstructing
a weaker snapshot from `DietaryContext` or using a profile-only hash is
prohibited.

Every grocery proposal freezes the real list UUID, exact list version, and
authoritative context hash. Precondition evaluation is read-only: it cannot use
an active-list sentinel, create a missing list, or silently follow a replacement
list. Missing, replaced, stale, consent-revoked, or context-changed state fails
without mutation. Rust may build provisional application/presentation seams
while Phase A hardens, but it pins wire DTOs, fixtures, and provenance only to
the corrected reviewed Phase A merge SHA and deployed capability.

Crate ownership is explicit: core owns versioned grocery wire/semantic types;
agent-runtime owns capability discovery, scoped REST calls, and grocery tool
events; application owns optimistic-list and confirmation use cases; CLI owns
grammar/JSON/classic output; TUI owns list/card interaction and presentation.
Voice remains a generic input adapter and contains no grocery-specific code.
The final Python oracle cannot be a differential oracle for this net-new
grocery behavior;
authoritative backend fixtures plus reviewed Rust client snapshots are the
source of truth.

Rust requests `grocery:read`/`grocery:write` only after deployed RFC 8414 scope
intersection and versioned application-capability discovery prove support.
Capability absence is an ordinary typed unavailable state; an existing session
missing optional grocery scopes receives an explicit re-authentication path,
never a raw 403 or a broken ordinary conversation.

Kroger is the first retailer provider. Provider-token persistence is prohibited
until Security D2 delivers purpose-specific versioned integration keys,
bounded re-encryption, rollback compatibility, and reconnect reconciliation.
The backend sequence is corrected Phase A plus Security D2 live, then B1
provider foundations, then B2 Kroger binding. No current Rust work assumes an
Instacart-first contract or moves provider OAuth tokens into the client.

### Health integration contract

H1/H2 is an additive native surface over the server-backed health and
integration APIs:

- `health status` reports connected providers and honest
  connected/stale/not-connected state;
- `health show` renders provider-neutral freshness, rolling values, labels,
  and goals without persisting health content locally;
- `health connect oura` opens the server authorization URL and performs a
  bounded CLI completion poll;
- `health sync oura` requests a server-side refresh and reconciles its terminal
  state without retaining provider credentials;
- `health disconnect oura` uses explicit confirmation and returns an honest
  deletion/revocation result;
- health-aware asks use the ordinary agent path and remain functional when
  optional health scopes or providers are absent.

H1 reads require `health:read`; H2 management requires
`integrations:manage`. These scopes are requested only after RFC 8414
intersection proves the deployed server supports them. A real authenticated
production canary must prove the endpoints, least-privilege session, and
redirect/poll lifecycle before H1/H2 is declared releasable.

Apple Health H3 is provider-neutral from the Rust client's perspective.
HealthKit authorization and collection remain mobile-owned; mobile uploads
explicitly consented daily aggregates; the backend owns encrypted storage,
retention, provider merging, revocation, and deletion. Rust consumes only a
versioned server response after those contracts ship. Direct HealthKit access,
raw sample ingestion, and Apple credentials in the CLI are prohibited.

The final Python oracle cannot cover H1-H3 because its health CLI slice was
deliberately not implemented. Reviewed backend fixtures and installed-client
canaries are the source of truth for these additive commands.

No command is silently dropped because it is not visible in the TUI. One-shot
commands remain a first-class developer interface.

### Native local state

Returning-user continuity is a GA gate for supported non-secret and local-only
state. Rust owns the final configuration and credential schema, but the
replacement cannot silently discard child profiles, repair outbox entries,
household/context/location selection, conversation pointers, or pending
confirmation state. The following native invariants apply:

- Use the platform/XDG `heyfood` config directory and a versioned native schema.
- Enforce owner-only permissions/ACLs, account binding, secret separation,
  optimistic version checks, cross-process locking, same-directory staging,
  atomic replacement, flush/fsync where supported, bounded corruption backup,
  and redacted inspection.
- Keep credentials in the platform store or owner-only fallback, never in the
  ordinary config document. Switching accounts cannot reuse profile,
  household, conversation, scope, or pending-effect state from another account.
- A new install, interrupted first write, schema upgrade, corrupt/truncated
  file, concurrent processes, unavailable keyring, and disk-full/permission
  failure each have deterministic fail-closed behavior and recovery guidance.
- Provide a one-time importer for supported local Python `0.3.2` state. Preserve
  non-secret/local-only state; require a fresh login when secret/keyring import
  cannot be proven safe.
- Import is read-only against the source, explicit, idempotent, and never
  deletes the Python file/keyring entry.
- A failed or ambiguous secret import starts a clean Rust login journey rather
  than weakening credential or file protections. Unsupported non-secret records
  produce an actionable, redacted disposition report and block returning-user
  qualification rather than disappearing silently.
- Rust does not maintain backward-write compatibility with Python.

### Big-bang repository and product cutover

- Development may temporarily keep Python beside Rust solely to export fixtures
  and run differential tests. No production code path shells out to Python.
- Before deletion, tag the exact final Python baseline for historical recovery
  and preserve its language-neutral contracts.
- The cutover PR removes `src/heyfood_cli`, Python-only tests, `pyproject.toml`,
  Python packaging scripts, and Python release workflows after Rust parity is
  proven.
- The same reviewed cutover makes Cargo the repository root, native installers
  canonical, Rust documentation authoritative, and the native binary the only
  supported `heyfood` implementation.
- There is one public replacement release. Internal artifacts are qualification
  inputs, not a separate supported preview product.
- After the first native release, operational rollback means re-promoting a
  previously reviewed native artifact only when its manifest sequence and
  rollback floor permit it. The inaugural `0.4.0` has no prior native artifact;
  its distinct halt/fix-forward procedure is defined below.
- Backend, scopes, profile data, and production infrastructure are unaffected by
  the client-language cutover.

## Distribution strategy

### Initial GA matrix

| Platform | Target | Artifact |
|---|---|---|
| macOS Apple Silicon | `aarch64-apple-darwin` | signed/notarized tarball |
| macOS Intel | `x86_64-apple-darwin` | signed/notarized tarball |
| Linux x86-64 | `x86_64-unknown-linux-gnu` | tarball |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | tarball |
| Windows x86-64 | `x86_64-pc-windows-msvc` | Authenticode-signed `heyfood.exe` in a Sigstore-attested zip, installed per-user by `install.ps1` |

Windows ARM64 and musl/static Linux are follow-up targets after demand and
dependency qualification. GA documentation must not claim them early.

### Minimum supported runtime

- macOS 13 or newer on Apple Silicon and Intel. Both architectures receive a
  real-hardware RC smoke; cross-compilation alone is insufficient.
- Linux glibc 2.28 or newer, built on a controlled AlmaLinux 8/manylinux-class
  baseline, for x86-64 and ARM64. Both architectures receive real-hardware
  install, auth, TUI, keyring/fallback, and typed-turn smoke.
- Windows 11 23H2 or newer and Windows Server 2022 or newer on x86-64, using
  ConPTY where available and documented classic fallback otherwise.
- Standard `HTTP_PROXY`, `HTTPS_PROXY`, and `NO_PROXY` behavior plus the
  explicit `HEYFOOD_CA_BUNDLE` path are qualified on every platform.
- Linux native voice documents its ALSA/Pulse/PipeWire runtime bridge and Linux
  Secret Service documents its D-Bus/session requirement. Missing services
  produce bounded browser/typed and owner-only credential fallbacks rather than
  a hang or misleading support claim.

### Canonical installation

- macOS/Linux: `curl -fsSL https://hey.food/install.sh | sh` downloads an exact
  native artifact; it no longer bootstraps Python or pipx.
- Windows: `irm https://hey.food/install.ps1 | iex` with the same manifest,
  signature, hash, version, and downgrade policy.
- The installer selects exact OS/architecture, refuses unknown targets, avoids
  root/admin by default, validates an HTTPS-hosted signed manifest, verifies
  SHA-256 and Sigstore identity, and installs atomically.
- Default Unix location is `~/.local/bin/heyfood`; Windows uses a documented
  per-user application directory and PATH update.
- Existing binaries are backed up/replaced atomically only after verification.
- Installers support exact version pinning, non-interactive CI use, dry-run,
  and uninstall instructions without accepting arbitrary URLs or shell tokens.

Homebrew and WinGet are desirable follow-up channels, not substitutes for the
first-party signed installer.

### Installer trust bootstrap and verification order

The stable manifest is RFC 8785/JCS-canonical JSON with `schema_version`,
monotonic `sequence`, `channel`, `status` (`active` or `halted`), `version`,
`published_at`, `min_installer_version`, `rollback_floor`, and per-target URL,
byte size, SHA-256, Sigstore-bundle URL, SBOM URL, and platform-signature
requirements. Release tooling signs the exact canonical bytes and emits a
Sigstore bundle containing certificate, signature, Rekor inclusion proof, and
DSSE/SLSA provenance.

Verification pins all of the following:

- Fulcio issuer `https://token.actions.githubusercontent.com`;
- repository `frntrllc/heyfood`;
- exact workflow identity
  `https://github.com/frntrllc/heyfood/.github/workflows/rust-release.yml@refs/tags/v<VERSION>`,
  with `<VERSION>` equal to the signed manifest version—branch identities fail;
- the reviewed GitHub OIDC subject/audience and protected release environment;
- Sigstore TUF trust roots shipped in the verifier, plus the bundle's offline
  Rekor inclusion proof and integrated time. Online consistency/freshness is an
  additional check when reachable, not a reason to skip offline verification.

The one-line installer has an unavoidable first-hop trust boundary: before a
verifier exists locally, the user trusts DNS/TLS for `hey.food`. Documentation
states this honestly and offers a stronger manual path using a preinstalled
Sigstore verifier and the immutable signed script release. The convenience
scripts are immutable versioned release assets behind the stable HTTPS URL;
their hashes and Sigstore bundles are published beside them.

The scripts contain a reviewed per-target SHA-256 table for a small immutable
`heyfood-installer` bootstrap binary. They download that binary from an exact
GitHub release URL, verify its bootstrap hash and the applicable macOS Developer
ID/notarization or Windows Authenticode signature before execution, and never fetch
or execute a dynamic `cosign`, package manager, arbitrary URL, or shell token.
The native bootstrap has the pinned identities/trust roots above and performs:

1. TLS-fetch the exact canonical manifest and bundle;
2. verify JCS encoding, certificate identity/issuer, signature, Rekor proof,
   provenance subject, schema, channel, status, integrated time, and sequence;
3. select an exact supported target and reject a halted or incompatible release;
4. download to an exclusive temporary file in the destination filesystem;
5. verify declared byte size and SHA-256;
6. verify the artifact's own Sigstore bundle and provenance subject;
7. verify/staple-check Developer ID plus notarization on macOS, or Authenticode
   publisher/chain/timestamp on Windows;
8. atomically install only after every check passes, preserving the previous
   binary until replacement succeeds.

The highest accepted manifest sequence is stored locally. Automatic install or
repair rejects a lower sequence. An explicit exact-version downgrade requires
the still-valid signed artifact and cannot cross the signed `rollback_floor`.
A fresh offline client cannot prove it has the newest manifest; the installer
must say so and may use offline media only with a valid bundle and an explicit
version. Signing-identity rotation uses a dual-authorized transition manifest;
an emergency stable-script change still requires protected release evidence and
an immutable signed script asset.

### PyPI retirement

PyPI is not a Rust distribution target. `heyfood-cli 0.3.2` remains an immutable
historical artifact, but the native release does not publish a compatibility
wheel, launcher, install-time downloader, or sdist. The public website, README,
GitHub release, and support guidance switch to the signed native installers in
the same cutover. `pipx install heyfood-cli` is no longer a supported install
path after `0.4.0`.

### Signing and provenance

- Git tags remain protected and release environments require approval.
- GitHub Actions are pinned to immutable SHAs.
- Cargo.lock is committed.
- Release jobs build once per target and promote the exact tested bytes.
- Every artifact receives SHA-256, SPDX or CycloneDX SBOM, Sigstore provenance,
  and GitHub release attachment.
- macOS artifacts require FRNTR, LLC Developer ID signing and notarization.
- Windows GA requires FRNTR, LLC Authenticode/Trusted Signing or an explicitly
  reviewed equivalent; unsigned Windows GA is not allowed.
- `cargo audit`, `cargo deny`, licenses, sources, duplicate critical crates,
  and RustSec advisories gate releases.
- The installer manifest and immutable installer scripts are versioned and
  signed separately; the trust-bootstrap and verification order above are
  release-blocking tests, not documentation-only claims.

### Inaugural native-release recovery

`0.4.0` cannot claim “rollback to a prior native release.” Before cutover, the
team prepares and drills this exact recovery system:

- The signed stable manifest can be promoted to `status: halted` without
  deleting or replacing published evidence. A confirmed critical installer,
  signature, terminal-corruption, registration/auth, credential-loss, JSON, or
  dietary-presentation defect halts new stable installs within 15 minutes and
  removes the landing-page one-liner until a safe artifact is active.
- Already-installed `0.4.0` binaries are not remotely disabled. The default
  recovery is a signed `0.4.1` fix-forward built from the preserved source SHA,
  `Cargo.lock`, toolchain, workflow inputs, SBOM, and provenance pipeline. The
  release owner decides halt/fix-forward within 30 minutes; a candidate target
  is four hours for a deterministic client-only correction, but no deadline
  waives tests, signatures, or the affected review gate.
- A patch is appropriate only when the defect and affected state are understood
  and the corrected artifact passes targeted plus mandatory clean-install/auth/
  terminal/JSON checks. Uncertain credential or dietary-data behavior stays
  halted until reconciled.
- The final signed Python `0.3.2` source tag and immutable PyPI artifact remain
  an **owner-authorized emergency source recovery** only for widespread auth,
  data-access, or security failure when no safe Rust fix can meet the incident
  objective. Re-enabling Python would require a separate incident change,
  explicit public guidance, and security/release approval; it is not a hidden
  fallback, a normal rollback, or part of supported `0.4.0` operation.
- The drill proves the release owner can halt the stable manifest and website,
  publish an independently reviewed `0.4.1` candidate through the same protected
  pipeline, preserve append-only evidence, and reactivate only exact verified
  bytes. It must not execute or silently depend on Python at runtime.

## Grok Build learning and provenance

The reviewed Grok snapshot demonstrates patterns worth adapting:

- a thin `tokio::select!` event loop;
- action/effect separation;
- explicit voice/auth/turn state;
- retained scrollback distinct from visible frames;
- command registry with aliases, descriptions, usage, and visibility;
- terminal suspend/restore around child/browser ownership;
- demand-driven ticks;
- bounded task/event handling;
- PTY-driven lifecycle testing;
- platform-aware keyboard fallbacks.

Default policy is pattern-only learning with no copied source. Any direct code
adaptation requires prior approval and an origin ledger naming Grok source
path, pinned commit, heyfood destination, modifications, applicable copyright,
and LICENSE/NOTICE/third-party attribution review before the code enters a
commit. The Rust plan does not authorize copying Grok branding, coding-agent
features, telemetry, services, or internal dependencies.

## Exact deliverables map

### Workspace, architecture, and core

| Path | Deliverable |
|---|---|
| `Cargo.toml` | Workspace members, shared dependency/features/profile policy |
| `Cargo.lock` | Committed exact dependency graph |
| `rust-toolchain.toml` | Pinned stable toolchain and components |
| `.cargo/config.toml` | Portable `cargo xtask` alias to `cargo run -p xtask --` |
| `deny.toml` | License, source, advisory, duplicate policy |
| `crates/heyfood-core/**` | Domain/wire/config/presentation/error contracts |
| `crates/heyfood-agent-runtime/**` | Authenticated HTTP/SSE/cancellation runtime |
| `crates/heyfood-application/**` | Use cases, single-flight supervisor, state writer |
| `crates/heyfood-platform/**` | Credentials, paths, browser, locks, signals, TLS |
| `crates/heyfood-voice/**` | Native/browser voice policy and lifecycle |
| `crates/heyfood-cli/**` | Clap tree, classic/ANSI/JSON output, completions |
| `crates/heyfood-tui/**` | Ratatui application, event loop, views, input, terminal guard |
| `crates/heyfood-installer/**` | Minimal pinned-identity manifest/artifact verifier and atomic installer |
| `crates/heyfood-bin/src/main.rs` | Thin executable composition root |
| `assets/dietary/**` | Validated dietary snapshot and audited source provenance |
| `assets/brand/**` | Banner text, language-neutral palette, and deterministic frame manifest |
| `schemas/dietary-options.v2.schema.json` | Embedded dietary asset schema |
| `schemas/banner-*.schema.json` | Embedded banner palette/frame schemas |

### Compatibility, tests, and fixtures

| Path | Deliverable |
|---|---|
| `src/heyfood_cli/**` | Temporary fixture/test oracle, then removed by the reviewed cutover |
| `pyproject.toml` | Temporary Python test packaging, then removed by the reviewed cutover |
| `schemas/v1/heyfood-output.schema.json` | Stable machine-output schema consumed by Rust tests |
| `schemas/v1/transcription.schema.json` | Stable transcription contract consumed by Rust tests |
| `scripts/regenerate_compat_fixtures.py` | Extend into deterministic language-neutral fixture exporter |
| `scripts/smoke_installed_cli.py` | Temporary differential helper, then replaced by native installed-artifact smoke tooling |
| `scripts/verify_artifacts.py` | Temporary helper, then replaced by native archive/manifest/SBOM/provenance verification |
| `fixtures/compat/**` | Language-neutral Python/Rust interface fixtures |
| `fixtures/api/**` | Requests, responses, SSE streams, error/timeout cases |
| `fixtures/contracts/grocery-backend/**` | Mirrored authoritative capability, scope, entity, confirmation, conflict, safety, tool, and export wire contracts with backend source SHA/provenance |
| `fixtures/grocery-client/**` | Rust-owned `--json`, semantic presentation, TUI, classic, and error snapshots |
| `fixtures/contracts/health-backend/**` | H1/H2 provider-neutral context/integration contracts and later H3 capability fixtures with backend source SHA/provenance |
| `fixtures/health-client/**` | Rust-owned health JSON, semantic presentation, TUI, classic, consent, and error snapshots |
| `fixtures/config/**` | Native schema plus required one-time Python state-import fixtures and reauthentication dispositions |
| `fixtures/presentation/**` | Semantic documents and renderer snapshots |
| `fixtures/contracts/called-endpoints.json` | Stable client/backend API and auth contract inventory |
| `tests/migration/python-test-ledger.json` | Exact Python node/invariant-to-Rust disposition ledger |
| `crates/xtask/**` | Ledger, asset, dependency-DAG, inventory, and release verification tasks |
| `crates/heyfood-core/tests/**` | Schema, validation, redaction, presentation parity |
| `crates/heyfood-agent-runtime/tests/**` | Wire, SSE, timeout, cancellation, no-replay tests |
| `crates/heyfood-application/tests/**` | Use-case, single-flight, generation, overlap tests |
| `crates/heyfood-application/tests/grocery_*` | Grocery optimistic-concurrency, confirmation, capability, and no-auto-write tests |
| `crates/heyfood-application/tests/health_*` | Optional-scope, provider-state, polling, disconnect, and health-aware-turn tests |
| `crates/heyfood-platform/tests/**` | Credentials, permissions, atomic writes, signals |
| `crates/heyfood-voice/tests/**` | Capture/transcribe/review/consent/cancel tests |
| `crates/heyfood-cli/tests/**` | Help, grammar, JSON, exits, differential fixtures |
| `crates/heyfood-tui/tests/**` | Reducer, layout, input, restoration, PTY tests |
| `tests/rust_python_differential/**` | Installed Python versus Rust contract runner |
| `tests/release/**` | Artifact, trust bootstrap, signature, exact-upgrade, halt/fix-forward tests |
| `tests/hardware/**` | Recorded real microphone and platform qualification protocol |

### Release, installer, and documentation

| Path | Deliverable |
|---|---|
| `.github/workflows/rust-ci.yml` | fmt/clippy/test/audit/deny/platform matrix |
| `.github/workflows/rust-release.yml` | signed native artifact build/promotion |
| `.github/workflows/rust-post-release-smoke.yml` | installed public artifact matrix |
| `.github/workflows/ci.yml` | Temporary Python oracle CI, removed/replaced by Rust CI in the cutover |
| `.github/workflows/release.yml` | Python/PyPI release workflow removed in the cutover |
| `.github/workflows/post-release-smoke.yml` | Python/PyPI smoke removed in favor of native public smoke |
| `install.sh` | Native macOS/Linux verified installer |
| `install.ps1` | Native Windows verified installer |
| `scripts/release-manifest.*` | JCS manifest, Sigstore/DSSE provenance, sequence, halt/promotion tooling |
| `packaging/macos/**` | signing/notarization packaging |
| `packaging/windows/**` | Authenticode/installer packaging |
| `docs/CLI_CONTRACT.md` | Language-independent public CLI contract |
| `docs/COMMAND_GRAMMAR.md` | Clap/one-shot/slash grammar |
| `docs/interactive-session.md` | TUI keys, commands, accessibility, privacy |
| `docs/platform-support.md` | Exact platform/terminal/audio matrix |
| `docs/rust-cutover.md` | Big-bang source/package/installer/release cutover and emergency recovery |
| `docs/references/grok-build-provenance.md` | Pinned pattern/adaptation ledger |
| `README.md` | Native install and single-command journey |
| `CHANGELOG.md` | Complete Rust replacement and removed PyPI path |
| `RELEASING.md` | Signing, qualification, promotion, inaugural halt/fix-forward, and later rollback runbook |

## Decision gates

### DG-R1 — Vertical architecture proof

Before broad porting, prove one complete Rust path:

1. create/read the native config and credential state safely;
2. authenticate/refresh against a controlled service;
3. open `/v1/agent/converse` SSE;
4. render incremental text in a minimal Ratatui viewport;
5. keep the composer responsive;
6. cancel and prove the connection/task/resources close;
7. restore after normal exit, panic, keyboard cancel, Unix signal, and Windows
   console event;
8. run on the initial target CI matrix;
9. build checksummed native artifacts;
10. compare the operation's request/events/result with Python-exported fixtures.

Failure returns the architecture to review. It does not authorize bypassing
terminal, cancellation, platform, or compatibility gates.

### DG-R2 — Idempotency and retry

Verify every expensive or mutating POST, including `/v1/agent/converse` and
grocery prepare/add/remove/state/weekly/screening/confirmation operations.
Mirrored contracts distinguish logical operation ID, idempotency key, request
fingerprint, expected list version, and confirmation ID; `X-Request-ID` remains
tracing only. Each endpoint explicitly declares whether identical-key replay is
safe, whether fingerprint mismatch is rejected, and where server acceptance
occurs. Existing pending-confirmation IDs do not substitute for proposal-
creation idempotency.

Until an endpoint's replay protection is deployed, mirrored, and tested, Rust
performs no automatic retry after an uncertain POST. It reports unknown outcome
and reconciles by a safe read/status operation when the contract provides one.
Tests cover timeout before acceptance, timeout after grocery proposal creation,
disconnect during confirmation, identical-key replay, fingerprint mismatch,
replayed accept, stale list/context version, and cancellation at every boundary;
they prove no duplicate screening cost, proposal, or committed mutation.

### DG-R3 — Credential and native file safety

Prove macOS Keychain, Linux Secret Service/headless fallback, and Windows
Credential Manager behavior, including timeouts/prompts, process isolation if
needed, atomic config mutation, file locking, owner permissions/ACLs, and clean
recovery after interruption. The required Python state importer is evaluated
separately and cannot weaken the native schema or credential policy; unsafe
secret transfer still requires reauthentication.

### DG-R4 — Native signing ownership

Before GA, FRNTR, LLC must have:

- Apple Developer ID/notarization credentials in a protected GitHub environment;
- a Windows signing path acceptable to SmartScreen/enterprise users;
- Sigstore OIDC release identity;
- protected tag/release approval and recovery ownership.

Signing credentials are never committed or exposed to untrusted pull-request
jobs.

### DG-R5 — Big-bang cutover completeness

Prove one reviewed change satisfies a machine-readable cutover inventory, not a
handwritten “remove Python” assertion. At minimum it accounts for:

- `src/heyfood_cli/**`, `tests/**`, `pyproject.toml`, Python schemas, fixture
  exporters, artifact/smoke scripts, optional `voice`/`keyring` extras, Python
  dev tooling, and every runtime asset formerly under the Python package;
- `.github/workflows/ci.yml`, `release.yml`, `post-release-smoke.yml`, trusted
  PyPI publishing/environment references, caches, artifact names, badges, and
  required-check names;
- `install.sh`, `install.sh.sha256`, PyPI/pip/pipx behavior, version discovery,
  repair/uninstall behavior, and website/landing-page install snippets;
- `README.md`, `CHANGELOG.md`, `DEVELOPMENT.md`, `RELEASING.md`, `SUPPORT.md`,
  `SECURITY.md`, `LICENSE`, all `docs/**` CLI/command/dietary/JSON documents,
  issue templates, release notes, and repository metadata/topics;
- the 675-entry migration ledger, stable called-endpoint contract, embedded asset
  schemas/provenance, final Python tag, and immutable historical release links.

The Rust replacement must make Cargo/Rust CI authoritative, promote the native
installers/documentation, and leave no runtime or release path that can silently
invoke Python. A repository-wide forbidden-reference scan fails on
`heyfood_cli`, active `pytest`, active `pipx`/PyPI instructions, Python version
lookups, old workflow/check names, optional Python extras, or
`install.sh.sha256`, except inside explicitly allowlisted immutable history,
migration evidence, and the emergency-source runbook. The inventory validator,
ledger validator, installed-archive tests, and cross-repository contract check
all pass at the exact cutover SHA.

### DG-R6 — Voice platform matrix

Qualify real capture on macOS ARM64/x86 where available, Linux x86/ARM, and
Windows x86-64, or explicitly document browser/typed fallback for a platform.
The client never advertises native voice where the shipped artifact cannot use
it.

## Execution phases and mandatory reviews

No phase begins until the previous phase's actual commits, tests, artifacts,
and evidence receive independent review.

### Phase 0 — Contracts and vertical Rust spike

**Estimated effort:** 2–3 calendar weeks for a focused experienced team.

1. Freeze language-neutral contracts from the final Python oracle at
   `73494a57468dac83b4904ce6c390e36926f5c6fe`.
2. Freeze the exact Python test/invariant migration ledger and stable
   called-endpoint contract before implementation changes collection results.
3. Export dietary/banner assets with schemas, source hashes, and provenance.
4. Create the Cargo workspace skeleton, enforced dependency DAG, exact feature
   matrix, toolchain, dependency/security policy, and installer-verifier crate.
5. Implement only enough core/platform/runtime/TUI code for DG-R1.
6. Run the spike on macOS, Linux, and Windows CI.
7. Prove native config/credential creation and the read-only migration of
   supported non-secret/local-only Python state; separately prove that unsafe
   credential import falls back to reauthentication without source mutation.
8. Inventory backend idempotency and release metrics.
9. Establish the grocery contract import/provenance tool and fixture namespace;
   consume Platform P0 C3/C4 schemas as they land and record C1–C4 plus Grocery
   Phase A as external dependencies without serializing the Rust spike behind
   unfinished backend work. Authoritative Phase A fixtures must be frozen before
   Phase 2 grocery implementation, and the deployed `grocery: "v1"` capability
   is mandatory before Phase 6 cutover.
   Phase 0 records the companion directive/PR state and marks current DTOs
   provisional. Generic Phase 1 ports and semantic types may proceed, but final
   wire types and Phase 2 grocery calls require authoritative generated backend
   fixtures, the corrected Phase A source SHA, and aggregate digest.
10. Freeze H1/H2 provider-neutral backend fixtures and production-scope/routing
    evidence; record H3 mobile/backend contracts as a separately capability-
    gated dependency rather than inventing Apple Health wire types.
11. Pin the Grok reference/provenance record.
12. Record dependency licenses, platform minimums, system-library requirements,
    release-hardware ownership, and signing/trust-bootstrap prerequisites,
    including the exact protected GitHub environment name and OIDC subject/
    audience expressions used by the pinned Sigstore identity.
13. Produce a written spike result with measured startup, input latency,
   cancellation, restoration, artifact size, and unresolved platform gaps.

**Exit gate:** DG-R1 passes; no terminal/resource leak exists; the migration
ledger, endpoint contract, and embedded-asset provenance are frozen; Python
contracts are consumable from Rust; the enforced crate DAG, dependency policy,
platform minimums, and installer trust design receive independent architecture/
security review. If rejected, stop before broad port.

### Phase 1 — Core, platform, and compatibility foundation

1. Implement `heyfood-core` schemas, errors, validation, redaction, native
   config versioning, and presentation documents.
2. Implement platform paths, locks, atomic writer, credential stores/broker,
   browser, TTY, TLS/proxy, and signal abstraction.
3. Implement the application supervisor, immutable snapshots, generations, and
   single state writer.
4. Implement the three mutation classes and cancellation-safe durable commit
   boundary, including refresh-token rotation and local-first repair tests.
5. Build temporary Rust/Python differential harnesses from exported fixtures.
6. Prove clean native state, account switching, interrupted-write recovery, and
   secret/file permissions on every platform; test required non-secret/local-
   only import and safe reauthentication dispositions separately.
7. Add generic grocery capability/entity/safety/confirmation/error semantics,
   provisional application ports, and the account-bound item-reference cache
   policy. Generate final wire DTOs only from the corrected authoritative Phase
   A fixtures; do not duplicate backend canonicalization, screening, retention,
   or purchase-history models.
8. Add provider-neutral health connection/freshness/trend semantics and H1/H2
   application ports from reviewed backend contracts. Keep H3 behind an
   unresolved capability boundary until its mobile/backend contract is final.

**Exit gate:** DG-R3 passes; core fixtures match Python; no secret/dietary data
enters logs; state races, interrupted persistence, refresh-token rotation, and
cancellation-after-server-acceptance tests pass; independent platform/security
review approves the foundation.

### Phase 2 — Native one-shot CLI and agent runtime

1. Implement authenticated Reqwest/Rustls client, capability discovery,
   refresh, SSE, deadlines, cancellation, and no-retry posture.
2. Port the complete Clap command tree by application use case.
3. Implement ANSI and JSON leaf renderers over shared documents.
4. Match command help, arguments, JSON, errors, exit codes, endpoints, payloads,
   scope, location, household, and confirmation fixtures.
5. Add shell completion and classic chat.
6. Implement the capability-gated `grocery` command family, optimistic list
   versioning, stable item IDs/index map, export, and structured headless
   confirmation against fixtures; do not enable it against an unadvertised
   backend.
7. Implement H1/H2 `health status/show/connect/sync/disconnect` against frozen
   provider-neutral fixtures, with bounded polling, explicit confirmation, and
   optional-scope reauthentication.

**Exit gate:** DG-R2 is recorded; all one-shot command families meet the
approved parity matrix or carry an explicit blocking exception; JSON is one
ANSI-free value under real/simulated TTY; independent API/compatibility review
passes.

### Phase 3 — Interactive TUI and typed conversation

1. Implement terminal guard, pure reducer, action/effect dispatch, supervisor,
   bounded channels, responsive layout, scrollback, composer, history, paste,
   completion, slash registry, help, and startup animation.
2. Integrate typed turns, streaming, progress, cancellation, confirmations,
   household scope, location, new/resume conversation, errors, and retry
   uncertainty guidance.
3. Enforce single-flight behavior and queued-draft explicit resubmission.
4. Add the screened grocery-list viewport and confirmation/edit/cancel flow:
   visible provenance, ingredient-basis disclaimer, per-member informational
   annotations, substitutions, intended-for emphasis, stale-list conflict, and
   narrow-terminal behavior. No mutation commits on natural-language assent.
5. Add provider-neutral health status/trend cards and the Oura connect/sync/
   disconnect lifecycle without persisting health values in TUI history.
6. Add PTY/ConPTY tests for resize, narrow width, paste, key fallbacks, panic,
   signals, suspend/resume, restoration, scroll/follow-tail, unseen indicators,
   copy, focus, long wrapped/code/list content, choices, and error states.
7. Add classic/`NO_COLOR`/no-animation/no-TUI accessibility paths.

**Exit gate:** the internal qualification binary delivers a responsive
persistent TUI; terminal restoration and resource budgets pass every platform;
semantic output matches one-shot behavior; independent
TUI/product/accessibility review passes.

### Phase 4 — Registration, onboarding, and complete workflows

1. Integrate create-account/sign-in, loopback PKCE, device flow, expiry/denial,
   session refresh, profile readiness, onboarding, and explicit sync consent.
2. Port profile, household, account, context, config, conversation, restaurant,
   menu, recipe, item, and meal TUI actions where interactive access adds value.
3. Preserve one-shot command parity for every workflow.
4. Exercise clean-machine, returning, partial-profile, interrupted onboarding,
   account switch, SSH, offline, and service-failure journeys.
5. Exercise typed conversational grocery journeys through the shared turn path,
   including capability absence, missing optional scopes, household annotations,
   confirmation, cancellation, and expiry.
6. Exercise H1/H2 first-connect, returning, stale, disconnected, missing-scope,
   cancelled-browser, sync-failure, and health-aware-turn journeys.

**Exit gate:** a net-new user runs one internal qualification binary,
registers, authenticates, onboards, and completes a useful typed turn without
command-topology knowledge; all command families are parity-complete;
independent auth/privacy/UX review passes.

### Phase 5 — Voice and native distribution

1. Implement native/browser capture, processor consent, transcription, review,
   edit/re-record/type/cancel, and submission through the shared turn path.
2. Exercise real-microphone grocery turns through that generic path, including
   transcript review and structured confirmation; add no grocery-specific audio
   capture or consent implementation.
3. Qualify real hardware and permissions for the advertised matrix.
4. Build the native bootstrap verifier, immutable `install.sh`/`install.ps1`,
   JCS manifest, Sigstore bundles, hashes, SBOMs, provenance, macOS signing/
   notarization, Windows signing, and Linux artifacts.
5. Exercise clean install, exact version, same-version repair, downgrade-floor,
   manifest halt, `0.4.1` fix-forward rehearsal, uninstall, proxy/custom CA,
   offline limitations, trust-root/identity rotation, and no-admin paths.
6. Build the complete DG-R5 repository/package/workflow deletion as a reviewed
   but not-yet-merged cutover change.

**Exit gate:** DG-R4/R5/R6 pass; voice is one continuous TUI turn; public-style
native artifacts pass trust-bootstrap, install, signed halt/fix-forward, and
platform-signature drills; the ledger has zero unmapped entries and the cutover
removes every Python runtime/release path; independent voice, supply-chain, and
release review passes.

### Phase 6 — Final qualification, big-bang cutover, and GA

1. Produce signed internal release-candidate artifacts and run the complete
   platform/terminal matrix without publishing a supported partial product.
2. Compare privacy-safe backend aggregate auth, agent SSE, transcription,
   health context/integration management, and grocery capability/REST/
   confirmation/conflict/screening success/failure with the pre-cutover
   baseline; add no prompt, health value, grocery-item, member, or content
   telemetry.
3. Resolve every P0/P1 qualification finding and repeat affected review gates.
4. Tag/archive the final Python baseline and verify exported fixtures are
   complete.
5. Drill inaugural signed-manifest halt and `0.4.1` fix-forward; separately
   validate that the archived Python source tag is retrievable for an
   owner-authorized emergency change without treating it as a runtime fallback
   or supported product path.
6. Verify Production advertises `capabilities.grocery = "v1"`, the deployed
   Phase A contract digest matches the mirrored fixture provenance, and a
   least-privilege released-candidate session receives the optional scopes.
   Also verify the released-candidate H1/H2 session receives only deployed
   `health:read`/`integrations:manage` authority and completes the authenticated
   provider-neutral canary.
7. Merge the single DG-R5 cutover only after installed-artifact clean-user,
   returning-user, one-shot JSON, interactive typed, real-hardware voice, and
   capability-advertised grocery journeys pass.
8. Cut `0.4.0` from the Rust-only repository state.
9. Promote native `install.sh`/`install.ps1`, documentation, landing-page
   animations, and support runbooks atomically; retire PyPI guidance.
10. Observe for 24 hours with named release owner `admin@frntr.ai` and recover
   or stop promotion on any critical journey failure, terminal restoration
   defect, installer/signature failure, wrong-list grocery mutation, broken or
   bypassed confirmation, or failure rate above both 5% and twice the baseline
   at 20+ events.

**Exit gate:** public installed bytes match signed reviewed artifacts; the
repository and supported product are Rust-only; native installers are the sole
public path; no Python/PyPI workflow remains active; release review marks this
plan complete.

## Test and qualification matrix

### Rust quality gates

- `cargo fmt --check`;
- `cargo clippy --workspace --all-targets -- -D warnings` plus every named,
  valid target feature combination; mutually exclusive platform backends are
  never forced into an artificial `--all-features` build;
- `cargo test --workspace` plus the same platform-targeted feature matrix;
- `cargo audit` and `cargo deny check`;
- no unsafe code by default; any `unsafe` platform boundary is isolated,
  documented with invariants, and independently reviewed;
- property tests for validation, native config/import, SSE parsing, and command
  grammar;
- deterministic snapshots for semantic documents and terminal frames.
- migration-ledger, dependency-DAG, forbidden-reference, embedded-asset schema/
  provenance, JCS manifest, Sigstore identity, and archive-content validators.

### Platform CI

- macOS ARM64 native runner and Intel build/qualification;
- Ubuntu x86-64 plus ARM64 build/qualification;
- Windows Server x86-64 with ConPTY tests;
- supported terminal checks: Apple Terminal, iTerm2, VS Code Terminal, common
  Linux terminal, Windows Terminal, PowerShell, SSH TTY;
- 40/80/120/160 columns, resize during every state, UTF-8 fallback, bracketed
  paste, `NO_COLOR`, no animation, classic/no-TUI, pipe, and redirected output.
- real Apple Silicon, Intel Mac, Linux x86-64, Linux ARM64, and Windows x86-64
  RC evidence at the declared minimums; emulation is supplementary only.

### Functional journeys

- new user registration -> auth -> onboarding -> first typed turn;
- returning user launch -> restored context -> turn;
- loopback, device, SSH, bind failure, expiry, denial, abandon, cancel;
- malicious loopback request/path/method/size/state/duplicate cases, valid
  callback after invalid traffic, PKCE mismatch, and listener deadline/closure;
- refresh, logout/revocation, account switch, and delete;
- every command family and JSON fixture;
- structured result, failure, timeout, disconnect, cancellation, confirmation,
  local-first effect, scope/location change, and queued draft;
- native/browser voice success, permissions, no device, scope loss, timeout,
  cancel, processor consent, and typed fallback;
- grocery capability absent/present, optional-scope re-authentication, show,
  add/remove/bought/weekly/never/export, list-version conflict, structured
  accept/edit/cancel, idempotent replay, member annotations, ingredient-basis
  wording, conversational and real-microphone voice journeys;
- grocery unknown capability, absent/read-only/write-only/read+write scope
  combinations, origin/account/list/version/expiry cache invalidation, secure
  export create/overwrite/symlink/failure behavior, and uncertain-POST outcomes;
- health not-connected/connected/stale, optional scope reauthentication, Oura
  connect callback/poll/sync/disconnect, provider absence, literal rendering,
  no local health persistence, and health-aware asks;
- clean native state plus required non-secret/local-only Python state import and
  safe credential reauthentication fallback;
- manifest bootstrap, wrong issuer/repository/workflow/tag, tampered JCS,
  invalid/offline Rekor proof, target/hash/size mismatch, downgrade, halt,
  identity rotation, atomic replacement interruption, and `0.4.1` fix-forward.

### Resource and performance budgets

- first visible local frame within 100 ms p95 across 30 warm native launches;
- local keystroke-to-frame below 25 ms p95 across 2,000 synthetic keystrokes
  with 500 scrollback entries and simulated SSE;
- idle below 0.5% of one CPU core over 60 seconds, median of three runs;
- normal rendering capped at 30 fps and startup animation at 12 fps;
- bounded scrollback: 1,000 semantic entries/20,000 rendered lines;
- runtime channels capped and documented, with no unbounded user/service data;
- microphone close within 500 ms, network/callback/poller close within 2
  seconds, and total normal cancellation exit within 3 seconds;
- no remaining task, child/broker, socket, listener, audio stream, or terminal
  mode after exit;
- release binary size and RSS recorded by target in Phase 0, with regression
  budgets fixed from measured evidence before Phase 1.

## Acceptance criteria

1. Bare native `heyfood` opens a persistent, styled, responsive TUI.
2. A brand-new user can register, authenticate, onboard, and ask a useful first
   question without learning the command tree.
3. Returning users reach the composer without unnecessary prompts.
4. Typed and voice input share one application/turn/presentation path.
5. The composer remains responsive during streaming and cancellation.
6. Slash commands are discoverable, completable, typed, and use shared use cases.
7. The ASCII identity animates at most once and never appears in responses.
8. Every service result uses one semantic document across TUI and one-shot ANSI.
9. Service content cannot inject terminal escapes or control sequences.
10. Terminal state restores after normal exit, panic, all supported catchable
    signals/console events, suspend/resume, browser handoff, and returned faults;
    uncatchable termination is documented without a false cleanup guarantee.
11. Stateful workflows are single-flight; stale generations cannot overwrite
    generation-scoped state, while server-accepted auth rotation and local-first
    durable commits cannot be lost because a view was cancelled.
12. Prompt/transcript history remains memory-only and diagnostics are redacted.
13. Existing one-shot commands, options, JSON, exits, API payloads, and local
    state pass the approved compatibility matrix.
14. `--json` emits exactly one ANSI-free JSON value on stdout in real and
    simulated TTY conditions.
15. The reviewed cutover removes the Python implementation, Python packaging,
    PyPI workflows, and every runtime fallback to Python.
16. Native signed artifacts exist for the declared macOS, Linux, and Windows
    matrix.
17. First-party installers verify the pinned issuer/repository/tag-workflow
    identity, canonical signed manifest, Rekor proof, hashes, provenance, native
    platform signature, sequence, and rollback floor, then install atomically
    without Python or administrator access.
18. Native `install.sh` and `install.ps1` are the sole supported public install
    paths; documentation no longer presents pipx/PyPI as current.
19. Public installed artifacts—not source checkouts—pass clean-user,
    returning-user, JSON, typed TUI, and real-hardware voice qualification.
20. The local intelligence boundary remains unchanged: proprietary backend,
    data, dietary graph, and service logic are not moved into the open client.
21. Every Python test and non-test invariant has a reviewed migration-ledger
    disposition, and no runtime asset or cross-repository API contract is
    orphaned by Python deletion.
22. The inaugural-release halt/fix-forward drill succeeds before `0.4.0`; the
    plan never claims a nonexistent prior-native rollback.
23. Production advertises grocery v1 before `0.4.0` cutover, and the installed
    Rust artifact provides the complete one-shot, JSON, TUI, conversational, and
    voice grocery surface; every write requires generalized structured
    confirmation and stale list versions fail without mutation. Capability-
    absent behavior remains tested for development/older servers without
    weakening ordinary CLI use.

## Risks and controls

| Risk | Control |
|---|---|
| Big-bang cutover ships an incomplete client | No public partial release; full command/TUI/voice/platform parity and DG-R5 gate the single cutover |
| Grocery backend and Rust client evolve against different assumptions | Backend team retains platform/domain ownership; versioned language-neutral fixtures and deployed capability gate every Rust surface |
| Grocery work causes disposable Python implementation | Explicit prohibition on Python grocery client/release work; client effort starts in shared Rust application and presentation crates |
| Rust scope expands into local agent platform | Explicit hosted-runtime boundary and non-goals |
| TUI state becomes monolithic | Crate boundaries, pure dispatch, supervised effects, thin binary |
| Terminal corruption | Sole terminal guard, ordered restoration, panic/signal/ConPTY PTY tests |
| Cancellation drops UI but leaks work | Owned cancellation tokens/resources, bounded joins, no late mutation |
| Cancellation loses a rotated token or accepted local-first effect | Mutation classes, bounded non-cancellable durable commit, idempotent commit IDs, reconciliation-required state |
| Concurrent effects corrupt scope/config | Single-flight use cases, immutable snapshots, serialized class-aware state writer |
| Native config/keyring loses access or corrupts state | Versioned schema, atomic writes, platform credential qualification, interruption tests; required state importer is non-destructive and unsafe secret transfer requires reauthentication |
| Sensitive data leaks into logs/history | secrecy/redaction types, memory-only history, content-free diagnostics, tests |
| Service output injects terminal controls | Semantic allowlist parser and CSI/OSC sanitization |
| Rust behavior drifts from proven Python contracts during rewrite | Frozen language-neutral fixtures and temporary differential runner before Python deletion |
| Green Rust tests conceal an unmigrated Python invariant | Exact node/invariant migration ledger with zero-unmapped DG-R5 gate |
| Python deletion drops dietary or banner runtime data | Language-neutral embedded assets, schemas, source hashes, and audited provenance |
| Windows support is assumed rather than built | Explicit MSVC/ConPTY/credential/signing/audio gates |
| Linux native dependencies reduce portability | Record glibc/audio/system requirements, artifact qualification, defer musl claims |
| Installer supply-chain compromise | Pinned bootstrap hash, JCS manifest, exact Sigstore issuer/repository/tag-workflow identity, Rekor proof, platform signature, protected release |
| Critical defect in first native GA has no native rollback | Signed manifest halt, landing-page withdrawal, rehearsed `0.4.1` fix-forward, separately authorized emergency source recovery |
| Grok source is copied indiscriminately | Pinned pattern-only default and origin-ledger/license gate |
| Timeline pressure weakens gates | Phase exit reviews; slip moves release date, not acceptance criteria |

## Accelerated execution challenge

The team will attempt a 72-hour **active-engineering** challenge using parallel,
non-overlapping ownership. Review/dependency wait time pauses the challenge
clock. Phase N+1 work cannot begin, merge, or be retained as assumed production
design until the exact Phase N commit, evidence, and artifacts receive the
mandatory independent approval. A rejected or pending gate stops every later
lane; agents may improve the rejected phase only.

The expedited checkpoints are:

| Active target | Allowed work | Required checkpoint and stop condition |
|---|---|---|
| Hours 0–12 | Phase 0 contracts, ledger, assets, workspace policy, and DG-R1 vertical spike only | Exact Phase 0 SHA, spike report, CI/artifacts, DG-R1 review. Stop until approved. |
| Hours 12–28 | Phase 1 core/application/platform foundation only | Exact Phase 1 SHA, DG-R3 evidence including durable-token cancellation. Stop until approved. |
| Hours 28–44 | Phase 2 runtime/one-shot/JSON/command breadth; grocery only after authoritative reviewed companion fixtures are pinned | Exact Phase 2 SHA, DG-R2 evidence for converse and every grocery POST, compatibility review. Stop until approved. |
| Hours 44–56 | Phase 3 retained TUI, typed conversation, grocery list/cards | Exact Phase 3 SHA, PTY/accessibility/product review. Stop until approved. |
| Hours 56–64 | Phase 4 auth, registration, onboarding, complete typed workflows | Exact Phase 4 SHA, auth/privacy/UX review. Stop until approved. |
| Hours 64–72 | Phase 5 generic voice and internal signed-distribution candidate | Exact Phase 5 SHA, DG-R4/R5/R6 evidence and voice/supply-chain review. Stop until approved. |

Unfinished or unreviewed Grocery Phase A contracts stop the grocery portion at
the Phase 2 boundary; workers do not encode mutable draft assumptions. Phase 6
cutover remains outside the sprint until Production capability, signing,
hardware, installed-artifact, and release evidence exist. The 72-hour challenge
therefore targets an approved internal candidate, not a pre-authorized GA.

The sprint succeeds only if each retained phase was independently approved and
every unclosed acceptance criterion is explicit. Agents own disjoint files/
crates, integrate only within the currently authorized phase, and never delete
the Python oracle before DG-R5. If expedited review cannot complete, the sprint
pauses rather than working around the gate.

## Effort and staffing reality

This is a product rewrite with compatibility migration, not a short refactor.
The current planning estimate is:

| Scope | Engineer effort |
|---|---:|
| Architecture/compatibility spike | 2–3 engineer-weeks |
| Rust MVP: auth, one turn, basic TUI, config | 8–12 cumulative engineer-weeks |
| Full commands and TUI, excluding complete voice/platform release | 18–26 cumulative engineer-weeks |
| Full parity, grocery, voice, Windows, installers, signing, qualification | **36–54 cumulative engineer-weeks** |

For two experienced engineers with access to release hardware and signing
owners, this is approximately 20–30 calendar weeks; for one experienced
engineer, approximately 9–13 months. Grocery adds explicit contingency for
backend contract coordination, optimistic concurrency, confirmation, client
surfaces, and live qualification. The range also includes contingency
for the 675-entry migration ledger, release-review rework, Sigstore/bootstrap design,
Apple/Windows signing queues, real ARM/Intel hardware, Linux audio/keyring
variance, installer trust tests, and inaugural recovery rehearsal. Automation
and focused parallel work may reduce elapsed time, but this plan does not trade
away compatibility, privacy, signing, or installed-artifact qualification to
meet an arbitrary date.

## Non-goals

- Embedding a local LLM, coding agent, shell executor, ACP server, tool/plugin
  runtime, repository permissions, or multi-agent dashboard.
- Copying Grok Build's source tree, feature breadth, telemetry, branding, or
  service dependencies.
- Moving proprietary hello.food intelligence, data, dietary graphs, or backend
  logic into the open-source client.
- Changing dietary safety vocabulary or backend evaluation behavior.
- Weakening auth scopes, HTTPS, consent, credential, or profile protections.
- Persisting prompt/transcript history locally.
- Removing one-shot CLI or JSON in favor of TUI-only behavior.
- Claiming Windows ARM64, musl Linux, or unsupported terminals before testing.
- Publishing an unsigned Windows GA or unnotarized macOS GA.
- Publishing any incomplete/parallel preview instead of the single qualified
  replacement.

## Independent review protocol

This plan is committed before review. The reviewer receives the exact commit
SHA and validates:

1. current Python and Grok evidence;
2. Rust crate boundaries and dependency direction;
3. Tokio cancellation, single-flight mutation, and persistence correctness;
4. Ratatui/Crossterm terminal lifecycle and cross-platform feasibility;
5. full CLI/API/config/JSON compatibility and migration strategy;
6. registration/auth/onboarding and profile-consent behavior;
7. voice/platform adapter feasibility;
8. privacy, credential, escape-injection, and supply-chain security;
9. installer trust bootstrap, signing, inaugural halt/fix-forward, Python/PyPI
   deletion, migration ledger, embedded assets, and big-bang cutover gates;
10. scope, estimates, phases, and whether viewport/focus/copy/streaming states
    actually deliver the requested delightful developer experience.

Material findings require a new commit and exact-SHA re-review. Implementation
may begin only at Phase 0 and only from an approved revision.
