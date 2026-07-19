# heyfood Rust native client and interactive TUI plan

**Status:** Draft v2 — owner-directed big-bang replacement; execution requires independent approval
**Baseline:** `frntrllc/heyfood` `main` at `9c6b91929143180252ad1b644aea273729a1f1b9` (`heyfood 0.3.2`)
**Reference plan:** `docs/plans/2026-07-19-heyfood-interactive-terminal-session-plan.md` at approved commit `56a4dca136a6d6f9ad3b5e99fa812ea433448d22`
**Reference implementation:** local Apache-2.0 Grok Build checkout at `b189869b7755d2b482969acf6c92da3ecfeffd36`
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

The `0.3.2` Python release is a mature compatibility surface, not a prototype:

- approximately 13,000 production Python lines and 9,000 test lines;
- 43 command handlers across agent, auth, profile, household, restaurant,
  recipe, meal, voice, configuration, conversation, and account workflows;
- 601 passing tests at the audited baseline;
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
- HTTPS enforcement and exact-loopback-only development exceptions;
- OS keyring support with a documented owner-only `0600` file fallback;
- PyPI trusted publishing, `pipx`, and the hosted `install.sh` channel.

The rewrite must port behavior and contracts, not translate files line by line.
The current `_ask_agent()` combines validation, scope resolution, payload
construction, SSE consumption, household effects, persistence, and rendering.
The Rust design separates those responsibilities before building breadth.

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
8. **The terminal is borrowed.** Every normal, cancelled, panicked, signaled,
   and suspended path restores it.
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
└── heyfood-bin/
    └── src/main.rs
```

### `heyfood-core`

Owns dependency-light product contracts:

- validated API/auth URLs and exact-loopback policy;
- configuration schema and migrations;
- authentication/session/profile/household/location/conversation value types;
- API request/response and SSE event types;
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
Unix signals, Windows console events, and child/browser suspension.

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

Before porting behavior, export reviewed fixtures from Python `0.3.2` for:

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

### Command-family parity

Rust GA includes the existing families:

- `ask`, `reply`, `chat`, `log`, `item`;
- `login`, `register`, `logout`, `status`, `doctor`;
- `profile`, `onboard`, `members`, `household`;
- `search`, `menu`, `get-menu`, `recommend`, `location`;
- `recipes search/save/saved`;
- `daily`;
- `conversation list/resume/clear`;
- `voice devices/status/set/reset`;
- `context list/show/use/set`;
- `config path/show/validate`;
- `account delete`.

No command is silently dropped because it is not visible in the TUI. One-shot
commands remain a first-class developer interface.

### Native local state

There is no current-user backward-compatibility gate. Rust owns the final
configuration and credential schema.

- Use the platform/XDG `heyfood` config directory and a versioned native schema.
- Preserve owner-only permissions/ACLs, account binding, secret separation,
  atomic writes, and redacted inspection.
- Provide a one-time best-effort importer for a local Python `0.3.2` config and
  its keyring entry as developer convenience, not as a GA blocker.
- Import is read-only against the source, explicit, idempotent, and never
  deletes the Python file/keyring entry.
- A failed or ambiguous import starts a clean Rust login journey rather than
  weakening credential or file protections.
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
- Operational rollback means publishing or re-promoting a previously reviewed
  native artifact. Restoring the archived Python tag is an emergency source
  recovery option, not a supported compatibility promise.
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
| Windows x86-64 | `x86_64-pc-windows-msvc` | signed zip/MSI or installer package |

Windows ARM64 and musl/static Linux are follow-up targets after demand and
dependency qualification. GA documentation must not claim them early.

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
- The installer manifest is versioned and signed separately from the script.

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
| `deny.toml` | License, source, advisory, duplicate policy |
| `crates/heyfood-core/**` | Domain/wire/config/presentation/error contracts |
| `crates/heyfood-agent-runtime/**` | Authenticated HTTP/SSE/cancellation runtime |
| `crates/heyfood-application/**` | Use cases, single-flight supervisor, state writer |
| `crates/heyfood-platform/**` | Credentials, paths, browser, locks, signals, TLS |
| `crates/heyfood-voice/**` | Native/browser voice policy and lifecycle |
| `crates/heyfood-cli/**` | Clap tree, classic/ANSI/JSON output, completions |
| `crates/heyfood-tui/**` | Ratatui application, event loop, views, input, terminal guard |
| `crates/heyfood-bin/src/main.rs` | Thin executable composition root |

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
| `fixtures/config/**` | Native schema plus optional one-time Python import fixtures |
| `fixtures/presentation/**` | Semantic documents and renderer snapshots |
| `crates/heyfood-core/tests/**` | Schema, validation, redaction, presentation parity |
| `crates/heyfood-agent-runtime/tests/**` | Wire, SSE, timeout, cancellation, no-replay tests |
| `crates/heyfood-application/tests/**` | Use-case, single-flight, generation, overlap tests |
| `crates/heyfood-platform/tests/**` | Credentials, permissions, atomic writes, signals |
| `crates/heyfood-voice/tests/**` | Capture/transcribe/review/consent/cancel tests |
| `crates/heyfood-cli/tests/**` | Help, grammar, JSON, exits, differential fixtures |
| `crates/heyfood-tui/tests/**` | Reducer, layout, input, restoration, PTY tests |
| `tests/rust_python_differential/**` | Installed Python versus Rust contract runner |
| `tests/release/**` | Artifact, installer, signature, exact-upgrade, and prior-native recovery tests |
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
| `scripts/release-manifest.*` | Signed manifest generation/verification |
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
| `RELEASING.md` | Signing, qualification, promotion, and prior-native recovery runbook |

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

Verify whether Production `/v1/agent/converse` accepts a stable idempotency key
and request fingerprint. `X-Request-ID` is tracing only. Until replay protection
is documented and tested, Rust performs no automatic retry after an uncertain
conversational POST. Existing pending-confirmation IDs remain separate.

### DG-R3 — Credential and native file safety

Prove macOS Keychain, Linux Secret Service/headless fallback, and Windows
Credential Manager behavior, including timeouts/prompts, process isolation if
needed, atomic config mutation, file locking, owner permissions/ACLs, and clean
recovery after interruption. The optional Python importer is evaluated
separately and cannot weaken the native schema or credential policy.

### DG-R4 — Native signing ownership

Before GA, FRNTR, LLC must have:

- Apple Developer ID/notarization credentials in a protected GitHub environment;
- a Windows signing path acceptable to SmartScreen/enterprise users;
- Sigstore OIDC release identity;
- protected tag/release approval and recovery ownership.

Signing credentials are never committed or exposed to untrusted pull-request
jobs.

### DG-R5 — Big-bang cutover completeness

Prove one reviewed change removes the Python implementation, Python tests and
packaging, PyPI workflows, and Python installer behavior; makes Cargo/Rust CI
authoritative; promotes native installers/documentation; preserves the final
Python tag and exported fixtures; and leaves no runtime or release path that can
silently invoke the obsolete Python client.

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

1. Freeze language-neutral contracts from Python `0.3.2`.
2. Create the Cargo workspace skeleton and dependency/security policy.
3. Implement only enough core/platform/runtime/TUI code for DG-R1.
4. Run the spike on macOS, Linux, and Windows CI.
5. Prove native config/credential creation and evaluate the optional read-only
   Python importer without making it an exit dependency.
6. Inventory backend idempotency and release metrics.
7. Pin the Grok reference/provenance record.
8. Record dependency licenses and platform system-library requirements.
9. Produce a written spike result with measured startup, input latency,
   cancellation, restoration, artifact size, and unresolved platform gaps.

**Exit gate:** DG-R1 passes; no terminal/resource leak exists; Python contracts
are consumable from Rust; the crate boundaries and platform strategy receive
independent architecture/security review. If rejected, stop before broad port.

### Phase 1 — Core, platform, and compatibility foundation

1. Implement `heyfood-core` schemas, errors, validation, redaction, native
   config versioning, and presentation documents.
2. Implement platform paths, locks, atomic writer, credential stores/broker,
   browser, TTY, TLS/proxy, and signal abstraction.
3. Implement the application supervisor, immutable snapshots, generations, and
   single state writer.
4. Build temporary Rust/Python differential harnesses from exported fixtures.
5. Prove clean native state, account switching, interrupted-write recovery, and
   secret/file permissions on every platform; test optional import separately.

**Exit gate:** DG-R3 passes; core fixtures match Python; no secret/dietary data
enters logs; state races and interrupted persistence tests pass; independent
platform/security review approves the foundation.

### Phase 2 — Native one-shot CLI and agent runtime

1. Implement authenticated Reqwest/Rustls client, capability discovery,
   refresh, SSE, deadlines, cancellation, and no-retry posture.
2. Port the complete Clap command tree by application use case.
3. Implement ANSI and JSON leaf renderers over shared documents.
4. Match command help, arguments, JSON, errors, exit codes, endpoints, payloads,
   scope, location, household, and confirmation fixtures.
5. Add shell completion and classic chat.

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
4. Add PTY/ConPTY tests for resize, narrow width, paste, key fallbacks, panic,
   signals, suspend/resume, and restoration.
5. Add classic/`NO_COLOR`/no-animation/no-TUI accessibility paths.

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

**Exit gate:** a net-new user runs one internal qualification binary,
registers, authenticates, onboards, and completes a useful typed turn without
command-topology knowledge; all command families are parity-complete;
independent auth/privacy/UX review passes.

### Phase 5 — Voice and native distribution

1. Implement native/browser capture, processor consent, transcription, review,
   edit/re-record/type/cancel, and submission through the shared turn path.
2. Qualify real hardware and permissions for the advertised matrix.
3. Build `install.sh`, `install.ps1`, signed manifest, hashes, SBOMs, provenance,
   macOS signing/notarization, Windows signing, and Linux artifacts.
4. Exercise clean install, exact version, same-version repair, prior-native
   recovery, uninstall, proxy/custom CA, and no-admin paths.
5. Build the complete DG-R5 repository/package/workflow deletion as a reviewed
   but not-yet-merged cutover change.

**Exit gate:** DG-R4/R5/R6 pass; voice is one continuous TUI turn; public-style
native artifacts install and recover cleanly; the cutover removes every Python
runtime/release path; independent voice, supply-chain, and release review
passes.

### Phase 6 — Final qualification, big-bang cutover, and GA

1. Produce signed internal release-candidate artifacts and run the complete
   platform/terminal matrix without publishing a supported partial product.
2. Compare privacy-safe backend aggregate auth, agent SSE, and transcription
   success/failure with the pre-cutover baseline; add no prompt/content
   telemetry.
3. Resolve every P0/P1 qualification finding and repeat affected review gates.
4. Tag/archive the final Python baseline and verify exported fixtures are
   complete.
5. Drill prior-native artifact recovery and emergency restoration of the
   archived Python source tag without treating it as a supported product path.
6. Merge the single DG-R5 cutover only after installed-artifact clean-user,
   returning-user, one-shot JSON, interactive typed, and real-hardware voice
   journeys pass.
7. Cut `0.4.0` from the Rust-only repository state.
8. Promote native `install.sh`/`install.ps1`, documentation, landing-page
   animations, and support runbooks atomically; retire PyPI guidance.
9. Observe for 24 hours with named release owner `admin@frntr.ai` and recover
   or stop promotion on any critical journey failure, terminal restoration
   defect, installer/signature failure, or failure rate above both 5% and twice
   the baseline at 20+ events.

**Exit gate:** public installed bytes match signed reviewed artifacts; the
repository and supported product are Rust-only; native installers are the sole
public path; no Python/PyPI workflow remains active; release review marks this
plan complete.

## Test and qualification matrix

### Rust quality gates

- `cargo fmt --check`;
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`;
- `cargo test --workspace --all-features` and platform-targeted test features;
- `cargo audit` and `cargo deny check`;
- no unsafe code by default; any `unsafe` platform boundary is isolated,
  documented with invariants, and independently reviewed;
- property tests for validation, native config/import, SSE parsing, and command
  grammar;
- deterministic snapshots for semantic documents and terminal frames.

### Platform CI

- macOS ARM64 native runner and Intel build/qualification;
- Ubuntu x86-64 plus ARM64 build/qualification;
- Windows Server x86-64 with ConPTY tests;
- supported terminal checks: Apple Terminal, iTerm2, VS Code Terminal, common
  Linux terminal, Windows Terminal, PowerShell, SSH TTY;
- 40/80/120/160 columns, resize during every state, UTF-8 fallback, bracketed
  paste, `NO_COLOR`, no animation, classic/no-TUI, pipe, and redirected output.

### Functional journeys

- new user registration -> auth -> onboarding -> first typed turn;
- returning user launch -> restored context -> turn;
- loopback, device, SSH, bind failure, expiry, denial, abandon, cancel;
- refresh, logout/revocation, account switch, and delete;
- every command family and JSON fixture;
- structured result, failure, timeout, disconnect, cancellation, confirmation,
  local-first effect, scope/location change, and queued draft;
- native/browser voice success, permissions, no device, scope loss, timeout,
  cancel, processor consent, and typed fallback;
- clean native state plus optional read-only Python config/keyring import.

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
10. Terminal state restores after normal exit, panic, signals, console events,
    suspend/resume, browser handoff, and faults.
11. Stateful workflows are single-flight; stale generations cannot overwrite
    auth, conversation, household, profile, or config state.
12. Prompt/transcript history remains memory-only and diagnostics are redacted.
13. Existing one-shot commands, options, JSON, exits, API payloads, and local
    state pass the approved compatibility matrix.
14. `--json` emits exactly one ANSI-free JSON value on stdout in real and
    simulated TTY conditions.
15. The reviewed cutover removes the Python implementation, Python packaging,
    PyPI workflows, and every runtime fallback to Python.
16. Native signed artifacts exist for the declared macOS, Linux, and Windows
    matrix.
17. First-party installers verify signed manifests, hashes, provenance, and
    install atomically without Python or administrator access.
18. Native `install.sh` and `install.ps1` are the sole supported public install
    paths; documentation no longer presents pipx/PyPI as current.
19. Public installed artifacts—not source checkouts—pass clean-user,
    returning-user, JSON, typed TUI, and real-hardware voice qualification.
20. The local intelligence boundary remains unchanged: proprietary backend,
    data, dietary graph, and service logic are not moved into the open client.

## Risks and controls

| Risk | Control |
|---|---|
| Big-bang cutover ships an incomplete client | No public partial release; full command/TUI/voice/platform parity and DG-R5 gate the single cutover |
| Rust scope expands into local agent platform | Explicit hosted-runtime boundary and non-goals |
| TUI state becomes monolithic | Crate boundaries, pure dispatch, supervised effects, thin binary |
| Terminal corruption | Sole terminal guard, ordered restoration, panic/signal/ConPTY PTY tests |
| Cancellation drops UI but leaks work | Owned cancellation tokens/resources, bounded joins, no late mutation |
| Concurrent effects corrupt scope/config | Single-flight use cases, immutable snapshots, generation-checked state writer |
| Native config/keyring loses access or corrupts state | Versioned schema, atomic writes, platform credential qualification, interruption tests; optional importer is non-destructive |
| Sensitive data leaks into logs/history | secrecy/redaction types, memory-only history, content-free diagnostics, tests |
| Service output injects terminal controls | Semantic allowlist parser and CSI/OSC sanitization |
| Rust behavior drifts from proven Python contracts during rewrite | Frozen language-neutral fixtures and temporary differential runner before Python deletion |
| Windows support is assumed rather than built | Explicit MSVC/ConPTY/credential/signing/audio gates |
| Linux native dependencies reduce portability | Record glibc/audio/system requirements, artifact qualification, defer musl claims |
| Installer supply-chain compromise | Signed manifest, Sigstore identity, hashes, pinned Actions, protected release |
| Grok source is copied indiscriminately | Pinned pattern-only default and origin-ledger/license gate |
| Timeline pressure weakens gates | Phase exit reviews; slip moves release date, not acceptance criteria |

## Effort and staffing reality

This is a product rewrite with compatibility migration, not a short refactor.
The current planning estimate is:

| Scope | Engineer effort |
|---|---:|
| Architecture/compatibility spike | 2–3 engineer-weeks |
| Rust MVP: auth, one turn, basic TUI, config | 8–12 cumulative engineer-weeks |
| Full commands and TUI, excluding complete voice/platform release | 16–22 cumulative engineer-weeks |
| Full parity, voice, Windows, installers, signing, qualification | **22–32 cumulative engineer-weeks** |

For two experienced engineers, this is approximately 13–18 calendar weeks;
for one experienced engineer, approximately 5–8 months. Automation and focused
parallel work may reduce elapsed time, but this plan does not trade away
compatibility, privacy, signing, or installed-artifact qualification to meet an
arbitrary date.

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
9. installers, signing, Python/PyPI deletion, big-bang cutover, and recovery
   gates;
10. scope, estimates, phases, and whether the plan actually delivers the
    requested delightful developer experience.

Material findings require a new commit and exact-SHA re-review. Implementation
may begin only at Phase 0 and only from an approved revision.
