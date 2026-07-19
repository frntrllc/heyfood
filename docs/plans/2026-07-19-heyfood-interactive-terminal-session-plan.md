# heyfood interactive terminal session plan

**Status:** Draft v1 — committed review candidate; execution requires independent approval
**Baseline:** `frntrllc/heyfood` `main` at `9c6b91929143180252ad1b644aea273729a1f1b9` (`heyfood 0.3.2`)
**Primary user:** a developer running bare `heyfood` in an interactive terminal
**Release target:** `0.4.0` after all gates pass
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

The local Apache-2.0 Grok Build source at `~/Dev/grok-build` demonstrates the
interaction patterns worth adapting:

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
layout, or complexity. We are learning from interaction and reliability
patterns. Any future direct source adaptation requires a provenance note and
Apache-2.0 attribution/NOTICE review before merge.

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

Add `prompt-toolkit>=3.0,<4` as a core dependency. It provides the Python-native
primitives required here: an application loop, retained layout, buffer,
keybindings, completion, resize handling, formatted text, async invalidation,
alternate-screen support, bracketed paste, and terminal restoration.

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
- `HEYFOOD_NO_TUI=1`, `TERM=dumb`, an unsupported terminal, or a screen-reader
  preference selects classic mode.
- Non-TTY bare invocation remains side-effect-free and prints concise help; it
  never starts browser auth, registration, or onboarding.

The TUI must not invoke Typer command functions or spawn `heyfood` subprocesses
to perform slash commands. Both surfaces call shared application services.

### A3 — Extract a shared turn application service

Refactor `_ask_agent()` into a UI-independent `TurnService` that owns:

- payload construction and validated context;
- conversation and target selection;
- idempotency/request identifiers;
- client streaming and event normalization;
- explicit service failure recognition;
- conversation ID persistence;
- cancellation and timeout outcomes;
- redacted diagnostics.

The existing one-shot renderer subscribes synchronously. The session app runs
the blocking HTTP iterator in a worker thread and forwards typed `SessionEvent`
objects through an async queue. The UI event loop must never block on HTTP,
browser auth, keyring, microphone capture, transcription, or file I/O.

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
- `VOICE_REVIEW`;
- `CANCELLING`;
- `ERROR`;
- `EXITING`.

Events and actions are typed. Each background operation carries an operation
ID; late events from cancelled or replaced work are ignored. State transitions
are unit-testable without a terminal or network.

### A5 — Presentation remains semantic

`presentation.py` remains the shared semantic result model. Add an interactive
adapter that converts trusted presentation blocks and Markdown into
`prompt_toolkit` formatted text. Keep Rich for one-shot output.

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
- Browser/device authorization uses `prompt_toolkit` terminal suspension or
  `run_in_terminal()` so the browser can open without corrupting the frame.
- The app redraws on return and clearly handles expired, denied, or abandoned
  codes.
- The canonical public origin is `https://auth.hello.food`; deployment-provider
  hostnames must never appear in normal user journeys.
- Existing fail-closed scope, capability, HTTPS, token-storage, and onboarding
  contracts remain unchanged.

### A8 — Voice is a first-class composer input

Integrate the existing voice modules behind a session `VoiceController`.

- `/voice`, `Ctrl+Space`, or `F8` starts capture from the composer.
- Recording state and elapsed time are visible without scrolling the composer
  away.
- `Esc` cancels; the same voice binding or `Enter` stops capture.
- Partial/final transcription updates are tied to the initiating operation ID.
- The transcript enters an in-session review surface with Accept, Edit, Record
  again, Type instead, and Cancel.
- Accepting a transcript populates/submits through the same turn service as
  typed text and never prints a banner.
- Missing hardware, missing optional dependencies, denied permissions, lost
  scope, timeout, and transcription failure keep the session usable and offer
  typed fallback.
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

Create one `TerminalSession` lifecycle boundary responsible for:

- capability detection before raw-mode takeover;
- alternate-screen entry and exit;
- cursor visibility;
- bracketed paste and optional mouse modes;
- resize and redraw;
- browser/editor/child suspension and restoration;
- `SIGINT`, `SIGTERM`, `SIGHUP`, and suspend/resume behavior where supported;
- final restoration in `finally`, including unexpected exceptions.

Crash reporting happens only after terminal restoration and remains redacted.
No feature module may write ANSI mode sequences directly.

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

Exact modules may be consolidated during Phase 0 if responsibilities stay
separate. Do not create one monolithic `tui.py` or move network/business rules
into widgets.

Expected existing-file changes include `pyproject.toml`, `main.py`,
`commands/agent.py`, `commands/auth.py`, `presentation.py`, `render.py`,
`banner.py`, `README.md`, and release documentation.

## Deliverables

### D1 — Terminal foundation and capability contract

- Pin the supported `prompt_toolkit` range.
- Prove full-screen launch, resize, paste, suspend/resume, exception recovery,
  and fallback behavior across the supported Python matrix.
- Add explicit TTY/capability selection and `--classic`/`HEYFOOD_NO_TUI`.
- Document terminal support and reliable key fallbacks.

### D2 — Session kernel

- Implement typed state, events, actions, operation IDs, reducer, and
  demand-driven invalidation.
- Centralize terminal lifecycle and signal behavior.
- Bound scrollback and avoid redraw busy loops.

### D3 — Viewport, composer, and visual system

- Build responsive header, scrollback, activity region, composer, completion,
  and footer/help surfaces.
- Add multi-line editing, memory-only history, paste safety, and accessible
  focus behavior.
- Adapt semantic presentation blocks and the existing palette.
- Add the contained startup animation from canonical banner assets.

### D4 — Turn orchestration and streaming

- Extract `TurnService` from `_ask_agent()` without changing server contracts.
- Stream typed events to one-shot and interactive renderers.
- Add cancellation, late-event suppression, explicit failure handling, and
  connection recovery messaging.
- Preserve conversation and household target semantics.

### D5 — Command registry and discoverability

- Implement the initial slash-command table and argument parsing.
- Add fuzzy/prefix completion, usage, aliases, availability gates, and help.
- Ensure handlers call application services rather than CLI functions.

### D6 — First-run registration, authentication, and onboarding

- Move existing prompt-independent logic into reusable application flows.
- Present create-account/sign-in and onboarding within the session journey.
- Safely suspend/redraw around browser device authorization.
- Verify net-new, returning, expired-code, denied-code, offline, and partial
  profile paths against existing services.

### D7 — Voice integration

- Integrate capture, transcription, transcript review, edit, retry, and typed
  fallback into the composer lifecycle.
- Preserve `audio:transcribe`, HTTPS, privacy, timeout, and device policy.
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
2. Write the session state/event contract and renderer boundary.
3. Define classic-mode selection and supported terminal matrix.
4. Map `_ask_agent()`, auth, onboarding, voice, and presentation code into the
   shared application layer with no duplicated business behavior.
5. Record the Grok-derived patterns and source-provenance decision.

**Exit gate:** the spike proves the foundation on macOS and Linux CI, the
architecture has no network work on the UI loop, and an independent reviewer
approves the dependency, state model, privacy model, and migration seam.

### Phase 1 — Session kernel and terminal safety

1. Implement `TerminalSession`, state, reducer, events, operation IDs, task
   supervision, and demand-driven invalidation.
2. Add safe launch/exit, signals, resize, suspend/resume, and crash restoration.
3. Add bounded semantic scrollback with no sensitive disk persistence.
4. Add deterministic unit and PTY tests before visual features.

**Exit gate:** every intentional and injected abnormal exit restores the
terminal; idle CPU is effectively zero; late events cannot mutate replacement
operations; independent safety review passes.

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
3. Implement cancellation, retry guidance, explicit service failures, target
   context, `/new`, `/for`, `/household`, `/profile`, `/location`, and `/status`.
4. Preserve one-shot `ask`, `reply`, and classic chat behavior through the same
   service.

**Exit gate:** typed turns work end to end against a controlled service;
cancellation is reliable; service failures never look successful; safety
vocabulary and structured guidance match one-shot output; independent product
and code review passes.

### Phase 4 — Net-new user journey

1. Integrate registration, login, browser/device authorization, code expiry,
   profile readiness, and onboarding state.
2. Make create-account direction primary for users without an account.
3. Return from browser authorization to the same intact viewport.
4. Exercise clean-machine and partial-state journeys.

**Exit gate:** a person with no account can run only `heyfood`, register,
authenticate, onboard, and ask a useful first question without learning command
topology; returning users resume directly; independent auth/security and UX
review passes.

### Phase 5 — Voice inside the session

1. Add voice bindings, recording status, transcript review, edit/retry, and
   submission through `TurnService`.
2. Validate missing dependencies/hardware, denied permissions, scope loss,
   timeout, cancellation, and terminal restoration.
3. Qualify a real microphone on macOS and preserve classic/one-shot voice.

**Exit gate:** voice and typed prompts are two inputs to the same composer and
turn path; no terminal corruption or sensitive persistence occurs; independent
voice/privacy review passes.

### Phase 6 — Default switch and public release

1. Ship an internal/pre-release wheel with the TUI explicitly enabled and
   complete the compatibility matrix.
2. Run the full existing suite plus PTY, wheel/sdist, installer, fresh install,
   upgrade from `0.3.2`, and classic fallback tests.
3. Make the TUI the default for bare `heyfood` and `heyfood chat` only after all
   gates pass.
4. Publish `0.4.0` through protected PyPI trusted publishing.
5. Verify PyPI attestations, GitHub release, hosted `install.sh`, clean public
   installation, and real production registration/login/onboarding/typed/voice
   smoke tests.
6. Update the landing-page animated examples only from verified shipped
   behavior.

**Exit gate:** the public artifact—not a source checkout—passes the entire
journey on supported terminals; rollback to `0.3.2` is documented; monitoring
shows no auth, turn, or transcription regression; release review closes the
plan.

## Test and quality matrix

### Functional

- new user: register -> authorize -> onboard -> first typed turn;
- returning user: launch -> restored target/context -> turn;
- incomplete profile and interrupted onboarding recovery;
- email/device code expiration, denial, and browser abandonment;
- streaming success, structured result, explicit failure, timeout, disconnect,
  retry guidance, and cancellation;
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
- exception, `Ctrl+C`, `Ctrl+D`, `SIGTERM`, `SIGHUP`, suspend/resume, browser
  handoff, and child failure restoration.

### Contracts

- `--json` emits exactly one JSON value on stdout, with no ANSI, banners,
  spinners, progress, prompts, or human hints, in non-TTY and simulated TTY;
- existing schemas and exit codes remain compatible;
- existing HTTPS enforcement, credential storage, scopes, and capability gates
  remain fail closed;
- service text cannot inject terminal escapes or markup;
- no prompt/transcript content is written to local history or diagnostics;
- wheel and sdist contain canonical banner/palette resources and required
  license files;
- `pipx`, `pip`, and hosted `install.sh` resolve the same signed release.

### Performance budgets

- first visible frame within 250 ms on a warm local launch, excluding network;
- local keystroke-to-frame latency below 50 ms at p95 under normal load;
- rendering capped at 30 frames/second, with slower animation ticks and no idle
  ticks;
- composer remains responsive during HTTP, auth, onboarding, and transcription;
- scrollback is bounded by entries and rendered lines, with a documented policy
  for folding or dropping the oldest visible entries;
- no unbounded task, queue, event, or response-fragment accumulation.

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

## Risks and controls

| Risk | Control |
|---|---|
| Terminal corruption on crash or browser handoff | One terminal lifecycle boundary, `finally` restoration, PTY fault injection, no direct ANSI mode writes elsewhere |
| UI freezes during sync client or microphone work | Worker-thread adapters, typed queues, operation IDs, no blocking work on application loop |
| TUI and one-shot behavior drift | Shared application services and semantic presentation fixtures |
| Health/dietary content leaks through history or logs | Memory-only history, redacted diagnostics, explicit future review for persistence |
| Terminal escape/markup injection from service content | Control sanitization, literal service text, validated links, bounded content |
| Key chords vary by emulator | Capability matrix and documented fallback chords |
| Accessibility regression from alternate screen | `--classic`, `HEYFOOD_NO_TUI`, no-TUI detection, keyboard-only operation, review with screen-reader workflow |
| Startup animation becomes repeated branding/noise | Once-per-process lifecycle and negative tests on every command/turn path |
| New dependency expands supply-chain risk | Pin major range, dependency/license/security review, lock/test release artifacts, trusted publishing |
| Registration or auth regression | Reuse established services, retain fail-closed gates, live clean-user release smoke |
| Over-copying Grok Build | Pattern-only adaptation by default; provenance and NOTICE gate for any direct source adaptation |
| Scope expands into a coding-agent harness | Enforce non-goals and phase deliverables; separate future proposals |

## Rollout and rollback

- Develop behind an internal opt-in while the classic path remains default.
- Produce a pre-release wheel for terminal and clean-user qualification.
- Switch bare `heyfood` only after Phase 5 approval; do not partially expose
  first-run users to an unfinished shell.
- Keep `heyfood chat --classic` and `HEYFOOD_NO_TUI=1` through at least the
  first `0.4.x` minor window.
- A rollback republishes no existing version. It releases a new patch restoring
  classic default behavior while preserving user credentials and conversation
  identifiers.
- No backend rollback, auth weakening, scope widening, or data migration is
  authorized by this CLI rollback plan.

## Non-goals

- Building a general coding-agent harness, tool runner, shell executor,
  multi-agent dashboard, plugin runtime, or repository permission model.
- Copying Grok Build's Rust implementation or recreating all of its features.
- Replacing Typer or Rich for one-shot commands.
- Changing backend dietary evaluation, safety vocabulary, profile semantics,
  authentication policy, or production infrastructure.
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
