# heyfood interactive terminal session plan

**Status:** Draft v3 — two review rounds folded in; execution requires independent approval
**Baseline:** `frntrllc/heyfood` `main` at `9c6b91929143180252ad1b644aea273729a1f1b9` (`heyfood 0.3.2`)
**Primary user:** a developer running bare `heyfood` in an interactive terminal
**Release target:** `0.5.0` after all gates pass; `v0.4.0` and `v0.4.1` are unsupported incident releases
**License:** Apache-2.0

## Executive decision

Build a real interactive terminal application for bare `heyfood`.

On a supported TTY, one command will open a persistent, styled session with:

- conversation scrollback;
- a multi-line composer anchored at the bottom;
- live turn progress and incremental response rendering;
- discoverable slash commands and completion;
- first-run registration, login, and onboarding inside the same session journey;
- native voice capture and transcript review without dropping back to the shell;
- reliable cancellation, resizing, suspend/resume, and terminal restoration.

The session shell will use `prompt_toolkit` 3.x. Typer remains the public
one-shot command router, Rich remains the one-shot renderer, and the stable
`--json` contract remains unchanged. A shared application layer will prevent
the interactive session and one-shot commands from becoming two independent
implementations of hello.food behavior.

This is a new interaction architecture, not another styling pass over the
current `Prompt.ask()` loop.

## Why this plan exists

Release `0.3.2` corrected the repeated-banner and conversational styling
defects. It did not make `heyfood` an interactive terminal application.

Today, `src/heyfood_cli/commands/agent.py::chat()` is a synchronous
`while True` loop around Rich `Prompt.ask()`. Each prompt and response is
written into ordinary terminal scrollback. There is no retained viewport,
fixed composer, centralized input dispatcher, application event loop,
completion surface, or safe background rendering path. Bare `heyfood` does
handle registration, login, onboarding, and then chat, but those stages are a
sequence of separate line prompts rather than one coherent application.

The user-facing consequence is structural: colors and spacing can improve the
transcript, but cannot produce the persistent, responsive experience of tools
such as Claude Code or Grok Build.

## Evidence and reference review

### Current heyfood 0.3.2

- `src/heyfood_cli/main.py` detects a bare interactive invocation.
- `src/heyfood_cli/commands/auth.py::run_bare_first_run()` resolves
  registration, authentication, onboarding, and then enters `chat()`.
- `src/heyfood_cli/commands/agent.py::_ask_agent()` combines request
  construction, streaming transport, progress, rendering, persistence, and
  error handling.
- `src/heyfood_cli/commands/agent.py::chat()` owns a line-oriented input loop
  with `/exit`, `/new`, `/household`, and `/for NAME` branches.
- `src/heyfood_cli/client.py::stream_agent()` exposes the streaming protocol,
  but the current terminal surface does not render it as a non-blocking live
  session.
- `src/heyfood_cli/presentation.py`, `render.py`, and `theme.py` already provide
  valuable semantic presentation contracts that must be reused.
- `src/heyfood_cli/banner.py` and packaged `data/banner.txt` plus
  `data/banner.palette.json` are the canonical identity assets.
- Native voice already has capture, policy, transcription, and review modules;
  the session must integrate these rather than replace them.

### Grok Build reference implementation

The local Apache-2.0 Grok Build source at `~/Dev/grok-build`, reviewed at exact
commit `b189869b7755d2b482969acf6c92da3ecfeffd36`, demonstrates the interaction
patterns worth adapting:

- a thin event loop delegates input, state, and drawing to an application view;
- explicit application states replace scattered booleans;
- long-running work returns typed events to the UI instead of blocking input;
- a retained scrollback model is distinct from the visible terminal frame;
- a command registry provides canonical names, aliases, descriptions, usage,
  visibility gates, and completion triggers;
- terminal takeover and child/browser handoff are centralized and reversible;
- render ticks are demand-driven so an idle session does not busy-loop;
- animation, voice state, cancellation, and late-event suppression are modeled
  as lifecycles;
- keyboard behavior accounts for terminal capability differences;
- full-screen and scrollback-native fallbacks are treated as different
  presentation modes over shared session behavior.

heyfood must not copy Grok's coding-agent scope, Rust architecture, source
layout, or complexity. The default is pattern-only learning with no copied
source. Any direct adaptation requires separate approval plus an origin ledger
naming the Grok source path and commit, heyfood destination, modifications,
applicable copyright/attribution, and review of Grok's `LICENSE` and relevant
third-party notices before adapted code enters a commit.

## Product contract

### Default journey

For a supported interactive terminal:

1. The user runs `heyfood`.
2. A contained hey.food startup treatment establishes identity once.
3. The application checks authentication and profile readiness without
   freezing input or corrupting the viewport.
4. A new user is clearly offered registration or login.
5. Registration continues immediately into onboarding.
6. An authenticated, onboarded user lands in an idle composer with one concise
   suggestion and visible command discovery.
7. Typed or dictated input becomes a live conversational turn.
8. The session remains open until the user deliberately exits.

Existing developers can still use one-shot commands such as `heyfood ask`,
`heyfood restaurants`, and `heyfood profile`. Automation continues to use
`--json`; the TUI never becomes an automation dependency.

### Target layout

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
│  Ask about food, a meal, or a restaurant…                     │
│  >                                                            │
├───────────────────────────────────────────────────────────────┤
│  / for commands · Ctrl+Space voice · Ctrl+C stop · ? help    │
└───────────────────────────────────────────────────────────────┘
```

The frame is illustrative, not a mandate for a heavy border. The shipped
design should use whitespace, the existing hey.food palette, and restrained
separators. Ordinary assistant prose must never be boxed or preceded by the
full ASCII banner.

### Composer and keyboard contract

- `Enter` submits a non-empty prompt.
- `Shift+Enter` and `Alt+Enter` insert a newline when the terminal reports
  those chords; `Esc` then `Enter` and `Ctrl+J` are documented fallbacks.
- Bracketed paste inserts multi-line content without accidental submission.
- `Up` and `Down` navigate current-session history only when movement within
  the current buffer is no longer possible.
- `Ctrl+R` searches current-session prompt history.
- `Tab` completes slash commands and relevant command arguments.
- `Ctrl+Space` begins or ends voice capture; `F8` is the fallback for terminals
  or operating systems that reserve `Ctrl+Space`.
- `Ctrl+C` clears a non-empty draft first; while a turn is active it cancels
  that turn and keeps the session open; while idle with an empty composer it
  requires a second press within a short window to exit.
- `Ctrl+D` on an empty composer requests exit. It never submits or truncates a
  non-empty draft.
- `?` or `/help` opens contextual help.
- Mouse input is optional. Every action must be keyboard-accessible.

Bindings must be capability-tested in Apple Terminal, iTerm2, Terminal.app
over SSH, VS Code Terminal, and common Linux terminals. Unsupported chords
must have visible fallbacks; the implementation must not pretend terminals
emit key distinctions they do not support.

### Slash-command contract

Create a typed command registry rather than expanding the `if` chain in
`chat()`. The initial built-ins are:

| Command | Purpose |
|---|---|
| `/help` | Show commands and contextual keyboard help |
| `/new` | Start a new conversation after confirmation when needed |
| `/voice` | Start/stop voice capture or explain unavailable voice support |
| `/for NAME` | Target subsequent turns to a household member |
| `/household` | Show or change household targeting |
| `/profile` | Summarize profile readiness and offer profile actions |
| `/location` | View or update the active location context |
| `/status` | Show auth, API reachability, scopes, profile, and voice readiness |
| `/clear` | Clear the visible transcript without deleting server history |
| `/exit` | Close the interactive session safely |

Every registry entry defines a canonical name, aliases, usage, description,
argument parser, availability predicate, and handler. A command that is not
available remains discoverable when an explanation or setup path is useful;
security- or capability-gated commands fail closed.

## Architecture decisions

### A1 — `prompt_toolkit` is the terminal application foundation

Add `prompt-toolkit` as a core dependency. Phase 0 begins with a provisional
`>=3.0,<4` evaluation range, then records a tested minimum and a bounded minor
upper range before implementation dependencies are committed. Test both bounds
on Python 3.11–3.13 and record package license metadata. It provides the
Python-native primitives required here: an application loop, retained layout,
buffer, keybindings, completion, resize handling, formatted text, async
invalidation, alternate-screen support, bracketed paste, and terminal
restoration.

Textual is not selected for this iteration because heyfood needs a focused
conversation surface rather than a general widget framework, and its larger
runtime/presentation model would duplicate more of the existing CLI. Building
directly on curses would transfer too much input, compatibility, and recovery
risk into product code.

Phase 0 must still prove the selected version on all supported Python and
terminal targets before feature construction. A failed compatibility spike
returns this decision to review; it does not authorize an improvised terminal
engine.

### A2 — Typer and the TUI own different entry surfaces

- Typer remains the parser/router for one-shot commands and machine output.
- Bare interactive `heyfood` launches `InteractiveSessionApp`.
- `heyfood chat` launches the same application by default.
- `heyfood chat --classic` preserves a line-oriented, accessible fallback for
  at least one minor release.
- `HEYFOOD_NO_TUI=1`, `TERM=dumb`, or an unsupported terminal selects classic
  mode. There is no portable automatic screen-reader detector; documentation
  and first-run help explicitly identify `--classic` and `HEYFOOD_NO_TUI=1` as
  the accessibility opt-outs.
- Non-TTY bare invocation remains side-effect-free and prints concise help; it
  never starts browser auth, registration, or onboarding.

The TUI must not invoke Typer command functions or spawn `heyfood` subprocesses
to perform slash commands. Both surfaces call shared application services.

### A3 — Extract a shared turn application service

Refactor `_ask_agent()` into a UI-independent `TurnService` that owns:

- payload construction and validated context;
- conversation and target selection;
- a new tracing `X-Request-ID` for each transport attempt;
- a stable logical-operation ID, idempotency key, and request fingerprint
  across an explicitly safe replay, when the backend contract supports them;
- preservation of any server-issued pending-confirmation ID/key as a separate
  contract;
- client streaming and event normalization;
- explicit service failure recognition;
- conversation ID persistence;
- cancellation and timeout outcomes;
- redacted diagnostics.

The existing one-shot renderer subscribes synchronously. The session app runs
blocking work outside the UI loop and forwards typed `SessionEvent` objects
through bounded async queues. The UI event loop must never block on HTTP,
browser auth, keyring, microphone capture, transcription, or file I/O.

Operation IDs and late-event suppression are defense-in-depth, not
cancellation. Phase 0 defines a `CancellableOperation` contract for agent SSE,
loopback auth, device polling, browser voice, native capture, and
transcription. Each operation owns:

- a cancellation token/event;
- finite connect/read/write/pool or polling deadlines;
- the close handle for its HTTP response/client, callback server, poller, or
  microphone stream;
- a bounded event queue with an explicit backpressure/coalescing policy;
- an idempotent `cancel()` that closes the owned resource;
- a bounded join/exit deadline and a terminal outcome of completed, cancelled,
  timed out, or failed.

The initial close budget is 500 ms for microphone capture, 2 seconds for an
HTTP stream/callback server/device poller, and 3 seconds for total application
exit. HTTP connects, writes, and pool acquisition use finite deadlines; an SSE
read uses a finite heartbeat/inactivity deadline agreed with the service rather
than `timeout=None`. Phase 0 may tighten these values with evidence but may not
make them unbounded.

Phase 0 inventories every operation that can leave the UI loop—including
ordinary HTTP requests, token refresh, slash-command requests, onboarding
saves, keyring access, configuration reads/writes, and diagnostics—and assigns
exactly one execution class:

| Class | Contract |
|---|---|
| Cooperatively cancellable | Own a close handle/token, finite deadline, bounded join, and no commit after cancellation |
| Serialized atomic persistence | Run through the sole application state writer; use temp-file + flush/fsync + atomic replace; cancellation waits for the current sub-500 ms commit section, preserving either the old or new complete state |
| Process-isolated blocking adapter | Run behind a supervised helper process when a library/backend exposes no cancellation or deadline; communicate over anonymous pipes, never argv/environment; terminate on the documented bound and reconcile uncertain completion |

Ordinary and refresh HTTP calls use per-operation clients with finite timeouts
and owned close handles. Filesystem configuration persistence uses the atomic
state-writer class. OS keyring backends are not placed in an unkillable Python
worker thread: Phase 0 proves a process-isolated credential broker on supported
platforms, including secure pipe handling, backend prompts, timeout/termination,
and read-after-write reconciliation. If that proof fails, the hard three-second
exit claim and the affected interactive auth design return to independent
review; implementation may not quietly leak a thread or move keyring calls onto
the UI loop.

For an uncertain credential write, the parent records no secret or success
claim. A later broker read reconciles whether the backend committed. For an
atomic config write, the application accepts only the complete old or complete
new file. No secret is written to diagnostics, temp artifacts, argv, or the
environment.

The session may not rely on cancelling an `asyncio` future to stop a blocking
thread. Exit tests must prove there are no surviving operation threads,
sockets, callback listeners, pollers, or microphone streams.

`X-Request-ID` is tracing, not replay protection. Until Production
`/v1/agent/converse` is verified to accept a stable idempotency key plus
fingerprint, heyfood performs no automatic conversational POST retry after an
uncertain response. If the server lacks this contract, a separately reviewed
backend prerequisite must add it before reconnect/retry is enabled; the CLI
may still provide a user-initiated new turn with clear uncertainty guidance.

### A4 — State is explicit and reducible

Define a single session state model with, at minimum:

- `STARTING`;
- `AUTH_REQUIRED`;
- `AUTHORIZING`;
- `ONBOARDING_REQUIRED`;
- `ONBOARDING`;
- `IDLE`;
- `SUBMITTING`;
- `STREAMING`;
- `VOICE_STARTING`;
- `VOICE_RECORDING`;
- `TRANSCRIBING`;
- `VOICE_REVIEW`;
- `CANCELLING`;
- `ERROR`;
- `EXITING`.

Events and actions are typed. Each background operation carries an operation
ID and the cancellable resource contract from A3; late events from cancelled
or replaced work are ignored. State transitions are unit-testable without a
terminal or network.

### A4.1 — Stateful work is single-flight

The composer may remain editable during background work, but heyfood runs one
stateful workflow at a time. Responsiveness does not authorize concurrent
mutation.

| While a stateful turn/auth/onboarding workflow is active | Policy |
|---|---|
| Draft editing, scrolling, `/help` | Allowed; local only |
| Submit another prompt | Keep at most one memory-only queued draft; it is not sent until the active operation reaches a terminal state |
| `/status` | Show a local immutable snapshot; defer any network refresh |
| `/for`, `/new`, `/location`, mutating `/profile` | Require cancel-and-join of the active workflow, then explicit execution; never overlap or silently queue |
| Auth refresh required by the active request | Part of that operation and committed through the state writer before its terminal outcome |
| Registration or onboarding | Exclusive; no conversational turn or mutating command may start |

Each operation receives an immutable config/auth/scope snapshot and its own
`HelloFoodClient`. Workers never share a mutable client or `ConfigStore`.
Workers return proposed local mutations to one application-owned state writer,
which serializes them and applies them only when the operation generation still
matches. UI late-event suppression therefore cannot hide a stale disk write.

If an existing contract requires a local-first effect, such as accepting a
pending household confirmation before a converse stream completes, the state
writer records the effect as its own committed event. Cancellation or an
uncertain server outcome explicitly tells the user what changed locally,
refreshes the authoritative snapshot, and never lets the cancelled operation
overwrite the replacement conversation ID or household scope.

### A5 — Presentation remains semantic

Replace the current partial presentation seam with a UI-neutral
`PresentationDocument` builder. Every supported agent result type, error,
choice, progress event, and continuation side effect must become a semantic
document before rendering. Semantic selection accepts no `Console`, performs
no writes, and imports neither Rich nor `prompt_toolkit`.

Rich and interactive formatted-text renderers are leaf adapters over identical
documents. Add fixtures asserting document equality before renderer-specific
goldens. Service Markdown is restricted to an explicit allowlist (paragraphs,
emphasis, lists, code, and validated links); unsupported constructs degrade to
literal/plain text. No renderer may independently rediscover dietary result
types or success/failure semantics.

Service text is always data:

- never interpret it as Rich markup;
- never emit embedded terminal escapes;
- sanitize control characters other than intentional newlines/tabs;
- validate links before adding terminal hyperlink sequences;
- bound untrusted line and block sizes before layout.

The two renderers must share themes, status vocabulary, error semantics, and
golden fixtures so they cannot drift into different dietary guidance products.

### A6 — The ASCII identity treatment has one contained lifecycle

The canonical banner remains `data/banner.txt` with colors from
`data/banner.palette.json`. The interactive startup may animate the hey.food
identity in place, then replace it with the compact session header.

- Animation is timer-driven; it never blocks startup with `sleep()`.
- Idle sessions request no animation ticks.
- The full banner appears at most once per interactive process.
- Response turns, slash commands, errors, voice review, JSON, pipes, CI, dumb
  terminals, and classic mode never show the full banner.
- `NO_COLOR`, `HEYFOOD_NO_BANNER`, `HEYFOOD_NO_ANIMATION`, narrow width, and a
  reduced-motion preference receive static or no-animation treatment.
- Resize or interruption during animation leaves no frame fragments or hidden
  cursor.

The landing-page animation is a visual reference. The CLI animation must be
generated from canonical packaged assets or an explicitly versioned frame
manifest; it must not depend on browser code at runtime.

### A7 — Registration, login, and onboarding are in-session workflows

Reuse the established registration, device authorization, email OTP, profile,
and onboarding services. Separate their application logic from their current
Rich prompts so the TUI can present them as stateful panels/overlays.

- A net-new user sees `Create account` as the primary action and `Sign in` as
  the secondary action.
- Successful registration proceeds immediately to onboarding.
- Only the `webbrowser.open` handoff uses `prompt_toolkit` terminal suspension
  or `run_in_terminal()`. Authorization waiting remains a cancellable
  background operation with live in-session status.
- A local desktop TTY defaults to loopback PKCE. SSH, headless use, loopback
  bind failure, or suppression of browser launch selects/offers RFC 8628-style
  device authorization instead; the CLI never tries to launch a local browser
  for a remote callback.
- The verification URL and short code remain visible and copyable until the
  operation completes, expires, or is cancelled.
- Cancellation closes the loopback callback server or device poller within the
  A3 exit bound.
- The app redraws on return and clearly handles expired, denied, or abandoned
  codes.
- The canonical public origin is `https://auth.hello.food`; deployment-provider
  hostnames must never appear in normal user journeys.
- Existing fail-closed scope, capability, HTTPS, token-storage, and onboarding
  contracts remain unchanged.
- Profile-sync consent is a distinct, explicit, tested action. It is never
  implied by account creation, authentication, or onboarding submission.

### A8 — Voice is a first-class composer input

Integrate the existing voice modules behind a session `VoiceController`.

- `/voice`, `Ctrl+Space`, or `F8` starts capture from the composer.
- Recording state and elapsed time are visible without scrolling the composer
  away.
- `Esc` cancels; the same voice binding or `Enter` stops capture.
- `0.4.0` uses the capabilities that exist today:
  `VOICE_RECORDING -> TRANSCRIBING -> VOICE_REVIEW`, with elapsed recording
  time and one final transcript tied to the initiating operation ID. Partial
  transcription is not promised. A future streaming-transcription path needs
  a separately reviewed backend/client deliverable.
- The transcript enters an in-session review surface with Accept, Edit, Record
  again, Type instead, and Cancel.
- Accepting a transcript populates/submits through the same turn service as
  typed text and never prints a banner.
- Missing hardware, missing optional dependencies, denied permissions, lost
  scope, timeout, and transcription failure keep the session usable and offer
  typed fallback.
- The current default-no browser-vendor consent disclosure is preserved. The
  session never silently switches voice processors, and `Esc`/exit closes the
  microphone immediately through the A3 cancellation contract.
- Real-microphone release qualification remains mandatory on supported macOS
  hardware; a no-microphone test alone is insufficient.

### A9 — Sensitive history is memory-only by default

Food, health, household, and profile prompts may be sensitive. Composer history
is retained only for the current process by default. The feature must not add a
plaintext prompt-history file, transcript log, crash dump, analytics payload,
or shell-history surrogate.

Any future cross-session history requires a separate reviewed design covering
explicit opt-in, retention, deletion, encryption/keyring behavior, migrations,
and recovery. Conversation IDs may continue using the existing protected local
state contract; message content may not be added to it by this project.

### A10 — Terminal ownership and recovery are centralized

`prompt_toolkit.Application` and its renderer are the sole owners of terminal
mode sequences. `TerminalSession` composes and supervises the application; it
must not duplicate raw/alternate-screen operations or install competing input
readers.

| Responsibility | Sole owner |
|---|---|
| Raw/cooked mode, alternate screen, cursor, bracketed paste | `prompt_toolkit` application/renderer |
| Resize detection, invalidation, frame drawing | `prompt_toolkit` application/renderer |
| Capability selection before application launch | `TerminalSession` |
| Background operation supervision and cancellation | `TerminalSession` + A3 operation handles |
| Browser/child suspension coordination | `TerminalSession` through supported `prompt_toolkit` APIs |
| Process-signal handlers and normalized exit codes | `TerminalSession`; keyboard bindings remain application input |
| State-normalized exit and post-restore crash output | `TerminalSession` |

Signal behavior is explicit and distinguishes input from process control:

- keyboard `Ctrl+C` is a `prompt_toolkit` key event: it clears a draft, cancels
  the active operation, or performs the documented double-press idle exit;
- an actual process-level `SIGINT` (for example `kill -INT`) is one-shot: cancel
  all owned operations, restore the terminal, and exit `130` within the exit
  budget; it never waits for a second keypress;
- `SIGTERM` cancels resources, lets the application restore, and exits `143`;
- `SIGHUP` cancels resources, restores where the TTY remains available, and
  exits `129`;
- `SIGTSTP`/resume uses supported application suspension on Unix and forces a
  capability/redraw check after resume;
- platforms or embedding environments where handlers cannot be installed use
  `prompt_toolkit` defaults plus `finally` cleanup and record that limitation in
  the terminal matrix.

Crash reporting happens only after terminal restoration and remains redacted.
No feature module may write ANSI mode sequences directly. Diagnostics use a
UI-neutral redacted event sink: one-shot mode may emit to stderr, while an
active TUI consumes events through its bounded queue or holds them for
post-restore output. Worker code never calls `Console.print()` over the active
frame.

## Proposed code organization

```text
src/heyfood_cli/
├── application/
│   ├── auth_flow.py          # reusable registration/login state operations
│   ├── onboarding_flow.py    # reusable onboarding state operations
│   └── turns.py              # TurnService and normalized turn events
└── session/
    ├── __init__.py
    ├── app.py                # composition root and application loop
    ├── state.py              # state, reducer, operation identities
    ├── events.py             # typed input/background/session events
    ├── layout.py             # root layout and responsive regions
    ├── composer.py           # buffer, history, paste, completion
    ├── commands.py           # slash registry and handlers
    ├── scrollback.py         # bounded semantic transcript model
    ├── renderer.py           # semantic block -> formatted-text adapter
    ├── terminal.py           # capability and restoration lifecycle
    ├── auth.py               # session auth/registration controller
    ├── onboarding.py         # session onboarding controller
    └── voice.py              # session voice controller
```

Phase 0 may propose a path change, but it must update this table and receive
review before implementation. Do not create one monolithic `tui.py` or move
network/business rules into widgets.

### Exact deliverables map

| Path | Change | Review responsibility |
|---|---|---|
| `pyproject.toml` | Add bounded `prompt-toolkit` dependency and PTY/performance test dependencies | Packaging and supply chain |
| `src/heyfood_cli/main.py` | Lazy TUI launch, root-option forwarding, classic/capability selection | CLI compatibility |
| `src/heyfood_cli/commands/agent.py` | Delegate ask/reply/chat behavior to shared turn/session services; preserve option contracts | Command compatibility |
| `src/heyfood_cli/commands/auth.py` | Delegate registration/login/onboarding operations without duplicating prompts | Auth compatibility |
| `src/heyfood_cli/client.py` | Cancellable SSE ownership, finite timeouts, separate tracing/idempotency headers after contract verification | Transport/security |
| `src/heyfood_cli/config.py` | Immutable operation snapshots and atomic writes through the sole state writer | State/persistence |
| `src/heyfood_cli/credentials.py` | Route keyring work through the process-isolated credential broker and reconcile uncertain writes | Credential security |
| `src/heyfood_cli/diagnostics.py` | Add a UI-neutral redacted event sink; one-shot may print stderr, TUI queues safe events or emits after restore | Diagnostics/terminal safety |
| `src/heyfood_cli/auth.py` | Expose cancellable loopback/device primitives and deterministic resource close | Auth/security |
| `src/heyfood_cli/auth_application.py` | Add transport selection and UI-neutral auth state operations | Auth architecture |
| `src/heyfood_cli/onboarding.py` | Expose UI-neutral onboarding actions and explicit profile-sync consent | Privacy/product |
| `src/heyfood_cli/commands/profiles.py` | Move current `run_onboarding()` and profile-sync consent UI into shared application operations while preserving one-shot prompts | Profile/onboarding compatibility |
| `src/heyfood_cli/presentation.py` | Define complete `PresentationDocument` builders for every supported result/event | Safety/presentation |
| `src/heyfood_cli/render.py` | Become the Rich leaf adapter over semantic documents | One-shot UX |
| `src/heyfood_cli/banner.py` | Expose canonical non-blocking animation frames/timing without owning terminal mode | Brand/terminal safety |
| `src/heyfood_cli/voice_capture.py` | Extract cancellable capture/transcribe/review operations; preserve processor consent | Voice/privacy |
| `src/heyfood_cli/voice.py` | Make browser callback-server wait cancellable and guarantee listener teardown | Voice/resource safety |
| `src/heyfood_cli/voice_native.py` | Guarantee immediate idempotent microphone close on cancellation | Voice/resource safety |
| `src/heyfood_cli/application/__init__.py` | Application-layer package boundary | Architecture |
| `src/heyfood_cli/application/auth_flow.py` | UI-neutral registration/login operations and outcomes | Auth architecture |
| `src/heyfood_cli/application/onboarding_flow.py` | UI-neutral onboarding operations and outcomes | Product architecture |
| `src/heyfood_cli/application/turns.py` | `TurnService`, logical operations, normalized events, cancellation | Transport/product |
| `src/heyfood_cli/application/state_writer.py` | Serialize generation-checked config/auth/scope/conversation persistence and atomic file commits | Concurrency/persistence |
| `src/heyfood_cli/application/credential_broker.py` | Supervise isolated keyring helper, secure anonymous-pipe protocol, timeout, termination, reconciliation | Credential lifecycle |
| `src/heyfood_cli/session/__init__.py` | Lazy interactive-session public entry point | Import isolation |
| `src/heyfood_cli/session/app.py` | Composition root and application loop | TUI architecture |
| `src/heyfood_cli/session/state.py` | State, reducer, operation identities | State correctness |
| `src/heyfood_cli/session/events.py` | Typed input/background/session events | State correctness |
| `src/heyfood_cli/session/layout.py` | Responsive retained layout | UX/accessibility |
| `src/heyfood_cli/session/composer.py` | Editing, paste, memory-only history, completion | Input/privacy |
| `src/heyfood_cli/session/commands.py` | Slash registry, availability, argument parsers, handlers | Command UX |
| `src/heyfood_cli/session/scrollback.py` | Bounded semantic transcript | Memory/privacy |
| `src/heyfood_cli/session/renderer.py` | `PresentationDocument` formatted-text leaf adapter | Presentation parity |
| `src/heyfood_cli/session/terminal.py` | Capability selection, application supervision, suspension, signal outcomes | Terminal safety |
| `src/heyfood_cli/session/auth.py` | TUI auth/registration controller | Auth UX |
| `src/heyfood_cli/session/onboarding.py` | TUI onboarding controller | Onboarding UX |
| `src/heyfood_cli/session/voice.py` | TUI voice state/controller | Voice UX |
| `tests/test_session_state.py` | Reducer, operation identity, late-event tests | State gate |
| `tests/test_session_cancellation.py` | Thread/socket/server/poller/microphone teardown fault tests | Resource gate |
| `tests/test_session_concurrency.py` | Single-flight, queued-draft, generation, turn/command/refresh/onboarding overlap tests | Concurrency gate |
| `tests/test_state_writer.py` | Serialized atomic writes, stale-generation rejection, interrupted commit recovery | Persistence gate |
| `tests/test_credential_broker.py` | Secret-safe IPC, prompt/timeout/kill, uncertain write reconciliation, no orphan process | Credential gate |
| `tests/test_session_terminal_pty.py` | Launch, keys, paste, resize, signals, suspend, restoration | Terminal gate |
| `tests/test_session_layout.py` | Width/theme/motion/accessibility golden tests | Visual gate |
| `tests/test_session_commands.py` | Registry, completion, aliases, availability, parsing | Command gate |
| `tests/test_presentation_documents.py` | Complete result mapping and semantic parity fixtures | Safety/presentation gate |
| `tests/test_auth_transport_selection.py` | Desktop/SSH/headless/bind-failure/device selection and cancellation | Auth gate |
| `tests/test_session_onboarding.py` | New, partial, consent, resume, and failure states | Onboarding gate |
| `tests/test_session_voice.py` | Recording/transcribing/review/cancel/processor-consent behavior | Voice gate |
| `tests/test_session_compatibility.py` | Root/chat option, lazy import, classic, exit-code compatibility | CLI gate |
| `tests/test_diagnostics.py` | Extend redaction and prove no worker `Console.print()` during an active TUI | Diagnostics gate |
| `tests/test_output_contract.py` | Extend real/simulated-TTY JSON isolation assertions | Automation gate |
| `tests/test_package_metadata.py` | Dependency bounds, licenses, wheel/sdist contents | Packaging gate |
| `tests/test_installer.py` | `pipx`, hosted installer, upgrade, rollback artifact | Release gate |
| `docs/interactive-session.md` | Keyboard, commands, terminal support, privacy, accessibility fallback | User documentation |
| `docs/CLI_CONTRACT.md` | Update interactive/classic/machine-output lifecycle contracts | Public contract |
| `docs/COMMAND_GRAMMAR.md` | Update bare/chat options, slash grammar, aliases, and fallback behavior | Command contract |
| `README.md` | Bare-command journey and classic fallback | Public documentation |
| `CHANGELOG.md` | `0.4.0` behavior and compatibility | Release documentation |
| `RELEASING.md` | Qualification, monitoring, rollback drill | Operations |
| `docs/references/grok-build-provenance.md` | Pinned SHA, pattern-only record, or approved origin ledger | License/provenance |

### Existing option compatibility map

| Existing surface | `0.4.0` behavior |
|---|---|
| bare `heyfood` | Launch TUI only on a supported interactive TTY; otherwise retain concise non-interactive intro with no side effects |
| root `--version` | Exit before importing `prompt_toolkit` or any session module |
| root `--no-banner` | Launch TUI without the startup banner/animation; never affect response rendering |
| root `--verbose` | Preserve redacted stderr diagnostics; no prompt/body content and no direct writes over the active frame |
| `heyfood chat [INITIAL...]` | Launch TUI and queue the joined initial message only after readiness; render it as user input |
| `chat --new` | Clear the existing conversation ID before the first session turn |
| `chat --no-input` | Continue to reject with the existing automation guidance and exit semantics |
| `chat --lat/--lng` | Require the valid pair and set initial session location exactly as today |
| `chat --near` | Resolve initial location before the first turn without blocking the UI loop |
| `chat --no-location` | Suppress saved location for session turns |
| `chat --for` | Resolve initial household scope before the first turn and preserve fresh-conversation behavior when scope changes |
| `chat --json` / deprecated `--raw` | Continue to reject interactive JSON and direct automation to `ask`/`reply --json` |
| `heyfood chat --classic` | New explicit line-oriented fallback over shared services |

All session and `prompt_toolkit` imports are lazy. One-shot commands, `--json`,
`--version`, non-TTY help, and shell completion must not initialize terminal
state or pay the interactive dependency's import/startup path.

## Deliverables

### D1 — Terminal foundation and capability contract

- Select and pin a tested minimum plus bounded minor `prompt_toolkit` range,
  test both bounds, and record its license metadata.
- Prove full-screen launch, resize, paste, suspend/resume, exception recovery,
  and fallback behavior across the supported Python matrix.
- Add explicit TTY/capability selection and `--classic`/`HEYFOOD_NO_TUI`.
- Document terminal support and reliable key fallbacks.

### D2 — Session kernel

- Implement typed state, events, actions, operation IDs, reducer, cancellable
  resource supervision, bounded queues/joins, and demand-driven invalidation.
- Implement single-flight workflows, immutable snapshots, the sole state
  writer, and the supervised credential broker.
- Centralize terminal lifecycle and signal behavior.
- Bound scrollback and avoid redraw busy loops.

### D3 — Viewport, composer, and visual system

- Build responsive header, scrollback, activity region, composer, completion,
  and footer/help surfaces.
- Add multi-line editing, memory-only history, paste safety, and accessible
  focus behavior.
- Build both leaf renderers over the complete UI-neutral
  `PresentationDocument` contract and existing palette.
- Add the contained startup animation from canonical banner assets.

### D4 — Turn orchestration and streaming

- Extract `TurnService` from `_ask_agent()` while preserving server contracts,
  except for any separately reviewed idempotency prerequisite identified in
  Phase 0.
- Stream typed events to one-shot and interactive renderers.
- Add real resource cancellation, late-event suppression, explicit failure
  handling, and connection uncertainty messaging.
- Keep tracing request IDs distinct from logical idempotency and confirmation
  keys; prohibit automatic retry until server replay protection is verified.
- Preserve conversation and household target semantics.

### D5 — Command registry and discoverability

- Implement the initial slash-command table and argument parsing.
- Add fuzzy/prefix completion, usage, aliases, availability gates, and help.
- Ensure handlers call application services rather than CLI functions.

### D6 — First-run registration, authentication, and onboarding

- Move existing prompt-independent logic into reusable application flows.
- Present create-account/sign-in and onboarding within the session journey.
- Suspend only for `webbrowser.open`; keep loopback/device waiting visible and
  cancellable, then redraw after handoff.
- Verify net-new, returning, expired-code, denied-code, offline, and partial
  profile paths against existing services.

### D7 — Voice integration

- Integrate recording, a non-streaming transcribing state, final transcript
  review, edit, retry, and typed fallback into the composer lifecycle.
- Preserve `audio:transcribe`, HTTPS, privacy, timeout, and device policy.
- Preserve explicit processor consent and immediate microphone close.
- Complete the real-hardware qualification.

### D8 — Compatibility, security, and release

- Keep one-shot human commands compatible.
- Keep `--json` byte-clean and schema-compatible.
- Add PTY, restoration, visual, performance, packaging, installer, and clean
  upgrade/downgrade tests.
- Release through the existing protected PyPI trusted-publishing path only
  after independent phase reviews.

## Execution phases and review gates

No phase begins until the preceding phase has a recorded independent review.
Reviewers must inspect the actual commit(s), tests, and artifacts, not only a
summary.

### Phase 0 — Architecture proof and contracts

1. Add a throwaway or test-only `prompt_toolkit` spike for viewport, composer,
   resize, bracketed paste, background event delivery, browser suspension, and
   terminal restoration.
2. Select the tested dependency bounds, record license metadata, and run the
   spike on Python 3.11–3.13 at both bounds.
3. Write the session state/event contract, terminal ownership table,
   `CancellableOperation` contract, queue bounds, close/join deadlines, and
   complete `PresentationDocument` inventory.
4. Inventory every offloaded blocking operation and assign its cancellable,
   atomic-persistence, or process-isolated class. Prove the keyring credential
   broker and atomic state-writer strategy on supported platforms, or return
   the exit/auth design to review.
5. Write the single-flight/concurrency table, immutable operation snapshot,
   generation check, and local-first-effect reconciliation contract.
6. Inspect Production/backend source for `/v1/agent/converse` idempotency-key
   and fingerprint support. Record verified headers/schema and replay
   semantics, or create a separate backend prerequisite and retain the strict
   no-automatic-retry policy.
7. Define classic-mode selection and supported terminal matrix.
8. Map `_ask_agent()`, auth, onboarding, voice, and presentation code into the
   shared application layer with no duplicated business behavior.
9. Record the pinned Grok SHA and pattern-only source-provenance decision.
10. Freeze the exact deliverables map or commit reviewed path amendments.
11. Map the Phase 6 aggregate metrics to existing privacy-safe backend logs or
   monitoring. If any required signal is absent, create a separately reviewed
   backend observability prerequisite; do not add covert CLI telemetry.

**Exit gate:** the spike proves the foundation on macOS and Linux CI, the
architecture has no network work on the UI loop, and an independent reviewer
approves dependency bounds, the complete blocking-operation inventory,
credential/persistence proof, resource cancellation, single-flight state
mutation, terminal ownership, idempotency posture, semantic renderer
completeness, privacy, and migration seams.

### Phase 1 — Session kernel and terminal safety

1. Implement `TerminalSession`, state, reducer, events, operation IDs, task
   supervision, bounded queues, cancellable resource handles, and
   demand-driven invalidation.
2. Implement the single application state writer, immutable snapshots,
   generation checks, atomic config persistence, and supervised credential
   broker.
3. Add safe launch/exit, separate key-event/process-signal behavior, resize,
   suspend/resume, and crash restoration.
4. Add bounded semantic scrollback with no sensitive disk persistence.
5. Add deterministic unit, subprocess, and PTY tests before visual features.

**Exit gate:** every intentional and injected abnormal exit restores the
terminal; cancel/exit leaves zero live operation threads, helper processes,
HTTP streams, callback listeners, device pollers, or microphone streams after
the defined bound; atomic persistence resolves to a complete old/new state;
keyboard `Ctrl+C` and process `SIGINT` pass distinct tests; idle CPU meets the
measured budget; late operations cannot mutate replacement state; independent
safety review passes.

### Phase 2 — Interactive shell and presentation

1. Implement responsive layout, composer, scrollback, activity area, footer,
   focus, keybindings, paste, and memory-only history.
2. Implement semantic interactive rendering and shared theme fixtures.
3. Implement slash registry, completion, `/help`, `/clear`, `/exit`, and
   contextual command visibility.
4. Implement the once-per-process, non-blocking banner animation.

**Exit gate:** viewport golden tests pass at 40, 80, 120, and 160 columns;
composer behavior passes PTY tests; `NO_COLOR` and reduced-motion behavior are
correct; the full banner never appears in a turn; accessibility/classic review
passes.

### Phase 3 — Live dietary conversation

1. Extract and integrate `TurnService`.
2. Render incremental response and progress events without blocking input.
3. Implement resource-closing cancellation, uncertainty guidance, explicit
   service failures, target context, `/new`, `/for`, `/household`, `/profile`,
   `/location`, and `/status`.
4. Preserve one-shot `ask`, `reply`, and classic chat behavior through the same
   service.
5. Enforce the A4.1 single-flight table and add overlap tests for turn + `/for`,
   turn + `/new`, queued draft, token refresh + exit, onboarding save + exit,
   and cancelled local-first confirmation turns.
6. Enable no automatic POST replay. If the separately reviewed backend
   idempotency prerequisite is delivered, test the stable logical key and
   fingerprint across an intentionally replayed transport attempt before any
   retry behavior is proposed.

**Exit gate:** typed turns work end to end against a controlled service;
cancellation is reliable; concurrent mutation is impossible; stale operations
cannot overwrite conversation/scope/config; local-first effects are disclosed
and reconciled; service failures never look successful; safety vocabulary and
structured guidance match one-shot output; independent product and code review
passes.

### Phase 4 — Net-new user journey

1. Integrate registration, login, browser/device authorization, code expiry,
   profile readiness, and onboarding state.
2. Make create-account direction primary for users without an account.
3. Suspend only for browser launch; keep loopback/device waiting cancellable and
   visible inside the intact viewport.
4. Enforce desktop-loopback and SSH/headless/bind-failure device-flow selection,
   with copyable URL/code and deterministic callback/poller closure.
5. Keep profile-sync consent distinct from registration/auth/onboarding.
6. Exercise clean-machine and partial-state journeys.

**Exit gate:** a person with no account can run only `heyfood`, register,
authenticate, onboard, and ask a useful first question without learning command
topology; returning users resume directly; independent auth/security and UX
review passes.

### Phase 5 — Voice inside the session

1. Add voice bindings, elapsed recording status, explicit transcribing state,
   one final transcript, review, edit/retry, and submission through
   `TurnService`.
2. Validate missing dependencies/hardware, denied permissions, scope loss,
   timeout, cancellation, and terminal restoration.
3. Qualify a real microphone on macOS and preserve classic/one-shot voice.

**Exit gate:** voice and typed prompts are two inputs to the same composer and
turn path; cancel immediately closes the microphone; processor consent never
changes silently; no terminal corruption or sensitive persistence occurs;
independent voice/privacy review passes.

### Phase 6 — Default switch and public release

1. Ship an internal/pre-release wheel with the TUI explicitly enabled and
   complete the compatibility matrix.
2. Run the full existing suite plus PTY, wheel/sdist, installer, fresh install,
   upgrade from `0.3.2`, and classic fallback tests.
3. Make the TUI the default for bare `heyfood` and `heyfood chat` only after all
   gates pass.
4. Publish `0.4.0` through protected PyPI trusted publishing under the named
   FRNTR, LLC CLI release owner, `admin@frntr.ai`.
5. Verify PyPI attestations, GitHub release, hosted `install.sh`, clean public
   installation, and real production registration/login/onboarding/typed/voice
   smoke tests.
6. Update the landing-page animated examples only from verified shipped
   behavior.
7. Run a 30-minute pre-public installed-artifact observation and a 24-hour
   post-public observation using backend-only, privacy-safe aggregate metrics:
   auth start/complete/failure by transport, agent-converse HTTP/SSE
   completion/failure, and transcription completion/failure. Record the prior
   24-hour backend baseline where volume exists. Signals come from the Phase 0
   inventory of existing logs/monitoring or an independently approved backend
   observability prerequisite; never add CLI prompt content, dietary content,
   transcript content, or a new device identifier.
8. Trigger rollback when any clean installed-artifact critical journey fails,
   terminal restoration fails, or—at a minimum of 20 events—a monitored
   failure rate exceeds both 5% and twice its prior baseline. Low-volume periods
   remain governed by synthetic clean-user tests rather than inconclusive
   percentages.

**Exit gate:** the public artifact—not a source checkout—passes the entire
journey on supported terminals; a prepared classic-default patch and installed
artifact rollback drill are verified; the defined observation shows no auth,
turn, transcription, terminal-restoration, or installer regression; release
review closes the plan.

## Test and quality matrix

### Functional

- new user: register -> authorize -> onboard -> first typed turn;
- returning user: launch -> restored target/context -> turn;
- incomplete profile and interrupted onboarding recovery;
- email/device code expiration, denial, and browser abandonment;
- local loopback, SSH/headless device selection, loopback bind failure, and
  cancellation with callback/poller teardown;
- streaming success, structured result, explicit failure, timeout, disconnect,
  retry guidance, and cancellation;
- injected cancellation during blocked SSE reads, auth waits, transcription,
  and microphone capture with zero surviving owned resources;
- turn + `/for`, turn + `/new`, queued draft replacement, token refresh + exit,
  onboarding save + exit, and cancelled local-first confirmation overlap;
- keyring read/write prompt, timeout, broker termination, uncertain-write
  reconciliation, and atomic config interruption;
- slash parsing, aliases, completion, invalid arguments, unavailable commands;
- voice success, review/edit/rerecord, missing mic, missing dependency,
  permission denial, scope loss, timeout, and cancellation.

### Terminal

- Apple Terminal, iTerm2, VS Code Terminal, common Linux terminal, SSH TTY;
- 40/80/120/160 columns and resize during every active state;
- UTF-8 and non-UTF-8 fallback;
- bracketed multi-line paste and control-character sanitization;
- `NO_COLOR`, `HEYFOOD_NO_BANNER`, `HEYFOOD_NO_ANIMATION`,
  `HEYFOOD_NO_TUI`, `TERM=dumb`, CI, pipe, redirected output;
- exception, keyboard `Ctrl+C`, external process `SIGINT`, `Ctrl+D`, `SIGTERM`,
  `SIGHUP`, suspend/resume, browser handoff, and child failure restoration.

### Contracts

- `--json` emits exactly one JSON value on stdout, with no ANSI, banners,
  spinners, progress, prompts, or human hints, in non-TTY and simulated TTY;
- existing schemas and exit codes remain compatible;
- existing HTTPS enforcement, credential storage, scopes, and capability gates
  remain fail closed;
- worker operations use immutable snapshots and isolated clients; only the
  generation-checking application state writer mutates local state;
- service text cannot inject terminal escapes or markup;
- tracing IDs, logical idempotency keys/fingerprints, and pending-confirmation
  keys remain separate contracts; no uncertain conversational POST is retried
  automatically without verified replay protection;
- no prompt/transcript content is written to local history or diagnostics;
- wheel and sdist contain canonical banner/palette resources and required
  license files;
- `pipx`, `pip`, and hosted `install.sh` resolve the same signed release.

### Performance budgets

- Benchmark with a reproducible `pexpect`/PTY plus VT-screen parser harness on
  GitHub-hosted `macos-14` and `ubuntu-24.04`, Python 3.11–3.13. Record runner
  image, Python, dependency bound, terminal dimensions, and raw samples.
- First visible frame is within 250 ms at p95 across 20 warm launches,
  excluding network and browser authorization.
- Keystroke-to-frame latency is below 50 ms at p95 across 1,000 synthetic
  keystrokes with 500 retained scrollback entries and a concurrent simulated
  SSE turn.
- Rendering is capped at 30 frames/second, startup animation at 12 frames/
  second or less, and an idle session averages below 1% of one CPU core over 60
  seconds, median of three runs.
- composer remains responsive during HTTP, auth, onboarding, and transcription;
- scrollback is capped at 1,000 semantic entries and 20,000 rendered lines,
  dropping the oldest completed entries with one visible truncation marker;
- each background event queue is capped at 256 events; replaceable progress
  events coalesce to the newest value, response chunks coalesce before enqueue,
  and non-replaceable terminal/error events apply backpressure rather than
  dropping;
- total RSS stays below 150 MiB and grows by less than 25 MiB while cycling
  10,000 bounded synthetic events after warm-up;
- cancellation, credential broker termination, and state-writer completion
  meet their 500 ms/2 second resource-close and 3 second total-exit budgets in
  every fault-injection test;
- no unbounded task, queue, event, response-fragment, or history accumulation.

## Acceptance criteria

1. Bare `heyfood` on a supported TTY opens a persistent interactive session
   with a bottom-anchored composer.
2. A net-new user completes registration and onboarding from that single
   command and reaches a useful first turn.
3. A returning user reaches an idle composer without unnecessary prompts.
4. Responses stream into retained scrollback while the composer and
   cancellation controls remain responsive.
5. Slash commands are discoverable, completable, documented, and executed
   through shared application services.
6. Typed and voice input converge on the same turn pipeline and presentation.
7. The full hey.food ASCII identity appears at most once during startup and
   never in ordinary responses.
8. Colors, spacing, safety statuses, structured results, and error semantics
   match the existing presentation contract and landing-page experience.
9. Terminal state is restored after every normal, cancelled, suspended, and
   exceptional exit covered by the matrix.
10. Sensitive prompt/transcript content is memory-only unless separately
    authorized by a future reviewed design.
11. One-shot commands, classic chat, non-TTY behavior, exit codes, and JSON
    schemas remain compatible.
12. `--json` remains ANSI-free and human-output-free under both real and
    simulated TTY conditions.
13. Installation through PyPI and `https://hey.food/install.sh` produces the
    same independently verified `0.4.0` artifact.
14. The released installed wheel passes clean-machine registration, login,
    onboarding, typed-turn, and real-hardware voice smoke tests.
15. Cancelling or exiting closes every owned network/auth/voice resource and
    keyring helper process within the defined budget; atomic persistence
    resolves completely; late-event suppression is not used as a substitute.
16. SSH/headless login selects the short-code device flow and never launches a
    browser against a remote loopback callback.
17. Every result and progress type becomes one renderer-neutral semantic
    document used by both Rich and the TUI.
18. Release and rollback decisions use the named owner, windows, privacy-safe
    metrics, thresholds, and installed-artifact drill in Phase 6.
19. Only one stateful workflow runs at a time; stale generations cannot
    overwrite conversation, household, auth, onboarding, or config state.
20. Keyboard `Ctrl+C` follows interactive semantics, while external `SIGINT`
    always cancels, restores, and exits `130` without a second press.

## Risks and controls

| Risk | Control |
|---|---|
| Terminal corruption on crash or browser handoff | One terminal lifecycle boundary, `finally` restoration, PTY fault injection, no direct ANSI mode writes elsewhere |
| UI freezes or resources survive cancellation | Cancellable resource owners, bounded queues/joins, operation IDs, no blocking work on application loop, teardown fault tests |
| Keyring backend blocks or finishes after caller uncertainty | Supervised process-isolated broker, secret-safe pipes, hard termination, read-after-write reconciliation, no worker thread |
| Concurrent turn/command/auth state corrupts scope or conversation | Single-flight workflows, immutable snapshots, isolated clients, sole generation-checking state writer, overlap tests |
| TUI and one-shot behavior drift | Shared application services and semantic presentation fixtures |
| Health/dietary content leaks through history or logs | Memory-only history, redacted diagnostics, explicit future review for persistence |
| Terminal escape/markup injection from service content | Control sanitization, literal service text, validated links, bounded content |
| Key chords vary by emulator | Capability matrix and documented fallback chords |
| Accessibility regression from alternate screen | `--classic`, `HEYFOOD_NO_TUI`, no-TUI detection, keyboard-only operation, review with screen-reader workflow |
| Startup animation becomes repeated branding/noise | Once-per-process lifecycle and negative tests on every command/turn path |
| New dependency expands supply-chain risk | Tested minimum and bounded minor range, both-bound CI, dependency/license/security review, release artifact checks, trusted publishing |
| Registration or auth regression | Reuse established services, retain fail-closed gates, live clean-user release smoke |
| Over-copying Grok Build | Pinned reference SHA, pattern-only default, and approved path/commit origin ledger plus license/notice review for any direct adaptation |
| Scope expands into a coding-agent harness | Enforce non-goals and phase deliverables; separate future proposals |

## Rollout and rollback

- Develop behind an internal opt-in while the classic path remains default.
- Produce a pre-release wheel for terminal and clean-user qualification.
- Switch bare `heyfood` only after Phase 5 approval; do not partially expose
  first-run users to an unfinished shell.
- Keep `heyfood chat --classic` and `HEYFOOD_NO_TUI=1` through at least the
  first `0.4.x` minor window.
- Before `0.4.0`, prepare and dry-run (without publishing) the exact change that
  makes classic mode default. A rollback republishes and requires no downgrade
  to `0.3.2`; the FRNTR, LLC CLI release owner (`admin@frntr.ai`) cuts a new
  `0.4.1` or later patch restoring classic default behavior while preserving
  user credentials and conversation identifiers.
- The rollback drill installs the built patch into a clean `pipx` home, proves
  bare/classic/JSON behavior, verifies the hosted installer resolves the new
  PyPI patch, and records GitHub/PyPI artifact hashes and attestations.
- Release decisions use the 30-minute/24-hour backend aggregate observations
  and thresholds in Phase 6 plus installed-artifact synthetic journeys. The CLI
  adds no telemetry to support this rollout.
- No backend rollback, auth weakening, scope widening, or data migration is
  authorized by this CLI rollback plan.

## Non-goals

- Building a general coding-agent harness, tool runner, shell executor,
  multi-agent dashboard, plugin runtime, or repository permission model.
- Copying Grok Build's Rust implementation or recreating all of its features.
- Replacing Typer or Rich for one-shot commands.
- Changing backend dietary evaluation, safety vocabulary, profile semantics,
  authentication policy, or production infrastructure. A missing converse
  idempotency contract is handled only through a separate reviewed backend
  prerequisite, not silently inside this CLI plan.
- Persisting full prompt or transcript history locally.
- Making mouse interaction required.
- Rendering the full banner during responses or one-shot commands.
- Changing machine-output schemas under the guise of a terminal redesign.

## Independent review protocol

This document must be committed before review. The reviewer receives the exact
commit SHA and evaluates:

1. current-code accuracy;
2. feasibility of the `prompt_toolkit` and shared-service architecture;
3. terminal lifecycle and failure recovery;
4. authentication/onboarding and voice integration;
5. privacy, escape-injection, credential, and machine-output protections;
6. phase order, gates, test coverage, rollout, and rollback;
7. whether the scope achieves the requested single-command experience without
   turning heyfood into an unrelated coding-agent product.

Any material review finding is folded into a new commit and the revised exact
commit is reviewed again. Implementation may not begin from an unreviewed
revision.
