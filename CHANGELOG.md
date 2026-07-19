# Changelog

All notable changes to heyfood will be documented in this file. The project
uses semantic versioning while the public command, machine-output, config, and
authentication contracts are stabilized.

## Unreleased

_Nothing yet._

## 0.3.2 - 2026-07-19

### Changed

- The full hey.food ASCII banner is now reserved for the interactive bare
  `heyfood` startup journey. It no longer interrupts `ask`, `reply`, `chat`,
  accepted voice input, authentication, registration, or menu progress.
- Interactive chat now follows the hey.food terminal presentation contract:
  accent-colored user prompts, bright response text, muted secondary context,
  and deliberate spacing between turns.

### Fixed

- Explicit failed-agent results render as failures and no longer receive the
  success-oriented `Continue with:` hint.

## 0.3.1 - 2026-07-18

### Fixed

- Restores sign-in compatibility with the current hello.food service; newer
  capabilities activate automatically when the service supports them. Login now
  reads the authorization server's published capabilities before signing in and
  requests only what the live service accepts, so both the browser and device
  sign-in flows work again against the deployed service. If capability discovery
  cannot be reached, sign-in falls back to its previous behavior.
- `heyfood account delete` now explains clearly when the connected hello.food
  service does not yet offer account deletion, instead of surfacing a raw
  authorization error.
- Account deletion now reconciles a lost or transient status response before
  canceling, recovers a completed deletion receipt when the original response
  was lost, and never tells users their account was retained unless a terminal
  denied or expired state was actually confirmed.
- Cancellation is still submitted at most once. If both cancellation and the
  bounded status reconciliation are inconclusive, the CLI reports an explicit
  indeterminate state and leaves local credentials unchanged.

## 0.3.0 - 2026-07-16

### Added

- `heyfood register` with a fail-closed capability preflight, loopback/device
  account-creation intent, strict profile-readiness validation, immediate
  interactive onboarding handoff, and a credential-free one-document `--json`
  contract.
- Bare `heyfood` now provides a TTY-only first-run journey from registration or
  sign-in through profile readiness and onboarding into chat; non-TTY execution
  remains side-effect-free and prints plain next steps.
- `heyfood account delete` adds explicit local acknowledgement, browser identity
  confirmation, fresh `account:delete` authority, one-attempt cancellation, and
  post-commit receipt validation before local credentials are cleared.

- A public, no-sudo macOS/Linux installer at `https://hey.food/install.sh`
  that installs the canonical PyPI package through isolated pipx, verifies the
  command, supports exact-version, keyring, and voice opt-ins, and never edits shell
  startup files.
- Native in-memory microphone voice capture for `onboard`, `ask`, and `log` via
  the optional `voice` extra (`pipx install 'heyfood-cli[voice]'`): audio is
  uploaded once to the authenticated `/v1/audio/transcriptions` endpoint,
  processed by hello.food and its configured transcription provider, and never
  written to disk. Browser Web Speech capture becomes an explicit,
  consent-gated fallback, and typed input always works.
- `heyfood voice devices`, `voice status`, `voice set`, and `voice reset` for
  inspecting microphones and managing persisted, reversible capture preferences
  (an omitted mode stays distinct from an explicit `auto`).
- A single canonical transcription contract (`schemas/v1/transcription.schema.json`)
  that drives request limits (separate audio-file and multipart-request byte
  ceilings, an 8–48 kHz sample-rate window) and runtime validation of every
  transcription response.
- A checked-in endpoint contract (`tests/fixtures/called_endpoints.json`) that
  enumerates every `(method, endpoint)` the CLI can send, kept exhaustive by a
  CI test that fails when a new call appears without a contract update.

### Changed

- The hosted installer now hands successful installs to bare `heyfood`, with
  `heyfood login` retained as the explicit returning-user recovery command.

- `auto` voice capture never crosses from native transcription to browser
  speech recognition without an explicit, default-no consent that discloses the
  browser vendor processes the audio; declining goes to typed input. An
  explicit `--voice-capture native` never opens a browser.
- Remote (non-loopback) API and auth URLs now require verified HTTPS at every
  ingress; URL userinfo, fragments, and base-URL query strings are rejected.
  Plain HTTP is allowed only for exact loopback development hosts.
- The `audio:transcribe` scope is requested at login and verified on the stored
  channel token before the microphone is opened, so an older session is asked to
  re-authorize before any audio is recorded rather than after upload.

### Fixed

- The compatibility baseline for the released `0.2.0` (household support, no
  voice) is reconstructed from the exact published commit and preserved
  immutably; the current baseline moves to `0.3.0`.

## 0.2.0 - 2026-07-15

### Added

- Local-first household roster management with member and whole-household
  conversational scopes.
- `household list/current/use/label`, agent `--for`, interactive `/for` and
  `/household`, and numbered agent-choice handling.
- iOS-compatible dietary/device context, household confirmation rendering, and
  confirmed mutation application with profile-sync repair guidance.
- Child dietary profiles remain in protected local storage and never enter
  profile sync, matching the mobile privacy boundary.
- Lossless adult profile-sync outbox merging and automatic replay on consented
  scoped agent turns.
- Account-bound local household state, vault-only confirmation previews, and
  literal rendering of user-controlled household names.
- Local-roster fallback for `household list` when profile-sync consent has not
  been granted, with an explicit machine-readable reconciliation status.

## 0.1.0 - 2026-07-11

### Added

- Apache-2.0 package metadata and FRNTR, LLC ownership notices.
- Standalone development, contribution, security, support, and conduct policies.
- Compatibility fixtures for the version 0.1.0 help and raw-output surfaces.
- Saved-location, restaurant selection, menu acquisition, and terminal
  presentation work captured by the committed baseline.
- RFC 8628-style short-code login, least-privilege scopes, secure credential
  storage, named contexts, and saved-location conversational context.
- Terminal-safe hey.food banner resources and accessibility controls.
- macOS/Linux CI across Python 3.11–3.13 plus built-artifact and pipx smoke
  verification.
- Synced-member discovery, explicit local conversation list/resume/clear
  workflows, and zsh/bash/fish shell completion.
- Stderr-only `--verbose` request correlation, timing, context, and auth
  refresh/retry diagnostics with sensitive-field filtering.
- Versioned JSON Schema for verdict, restaurant-fit, menu, recommendation, and
  recipe compatibility results, with canonical safety vocabulary.

### Changed

- The public package, CLI version display, and HTTP User-Agent now share the
  version declared in `src/heyfood_cli/__init__.py`.
- Menu acquisition now warns after 12 seconds and returns control after 30
  seconds with a resumable job reference instead of waiting up to four minutes.
- `--json` is now the canonical ANSI-free machine-output flag across data
  commands; progress and diagnostics use stderr, and `--raw` is a deprecated
  alias for the same writer.
- `--no-input` and non-TTY stdin now prevent prompts; onboarding mutations
  require explicit `--yes`, while dry-run remains network- and persistence-free.
- Client-side validation now matches service bounds for coordinates, radius,
  limits, dates, query lengths, recipe filters, and paired location flags.
- Dietary onboarding now emits schema-v5 source provenance, migrates legacy
  flattened profiles, and clears or replaces categories without erasing
  unrelated selections or retaining stale derived values.
- Recommendation scores are labeled as composite match ranks rather than safety
  verdicts; each human result includes a direct item-evaluation command.
- Typer and Click are bounded to the compatibility-tested minor lines so a
  fresh install preserves the frozen command and help grammar.

Each protected release moves relevant entries from `Unreleased` into a version
section with an ISO date. Removed or incompatible behavior must be called out
explicitly with migration guidance.
