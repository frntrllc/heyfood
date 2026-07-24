# Landing-page TUI conformance

The simulated terminal on [hey.food](https://hey.food/) is a product contract.
Its formatting, intermediate states, control boundaries, and end-to-end
capabilities must be reproducible by an installed `heyfood` artifact before the
Rust TUI is declared complete.

The machine-readable inventory is
`tests/showcase/showcase-contract.v1.json`. It freezes the deployed experience
observed on 2026-07-21 and the matching website source at
`847615429b5d423bc32e49ebbe1819ae6cf248a4`.

The inventory and its Rust integrity test are requirements bookkeeping only.
They do not execute a journey and are not release evidence. Only the
installed-artifact E2E suite described below can satisfy the showcase gate.

## Required journeys

| Journey | Installed-user stages | Release proof |
|---|---|---|
| Menu Watch | locate, verify, watch, diff | restaurant identity evidence, live canonical menu, explicit low-confidence confirmation, local-time schedule, baseline semantics, durable non-empty diff |
| Dinner Planner | plan, compare, shop, remember | authoritative household snapshot, consented Oura and Apple Health context, restaurant and recipe evidence, member-specific uncertainty, signed grocery proposal, separate grocery and meal commits, durable meal provenance |
| Voice Meal Log | record, transcribe, review, log | scope before capture, real native microphone, memory-only bounded WAV, authenticated transcription, editable review, non-mutation on cancel, standard meal-log path |

An agent response that merely repeats expected prose is not a pass. Every
assertion must be backed by the relevant service response, persisted version,
provenance record, resource lifecycle observation, or non-mutation check.

## Presentation contract

The installed application must reproduce the semantic hierarchy demonstrated
on the page:

- compact `hey.food` header, activity state, retained transcript, and
  bottom-anchored composer;
- `You` and `hey.food` speaker labels with accent, bright, muted, and warning
  tones used consistently;
- structured responses with an introduction, aligned label/value details, and
  an evidence or caution note;
- composer responsiveness during streaming, auth, transcription, and backend
  work;
- the documented Enter, newline, scroll, cancellation, and exit controls;
- readable compact, standard, and wide layouts without semantic loss.

Browser-only window chrome and animation timing are not runtime requirements.
The semantic layout, information density, colors, spacing, states, and keyboard
experience are.

## `0.5.0` bounded recovery-release matrix

The `0.5.0` release gate is intentionally narrower than the twelve-stage
landing-page inventory. Its machine-readable contract is
`tests/showcase/core-release-matrix.v1.json`. The installed archive must prove:

- clean-user registration, missing-profile onboarding and consent, and a first
  authenticated TUI turn;
- complete process exit followed by a second installed process that reloads
  credentials without registration and completes another authenticated turn;
- an active household Grocery list with member-specific screening,
  substitutions, provenance, proposal review/edit, non-mutating cancel, one
  accepted mutation and one list-version advance, plus independent rejection of
  stale list and household-context authority;
- no blind retry after uncertain dispatch, Ctrl-C stream cancellation, Ctrl-C
  cancellation of a pending Grocery confirmation without mutation, a typed
  failure that leaves the TUI usable, and complete presentation restoration
  after normal and application-interrupt exits. The companion internal PTY and
  terminal-guard gates remain required for native-signal canonical-mode and
  body-error/panic restoration;
- semantic output at 40, 80, and 120 columns, `NO_COLOR`, the exact archive
  digest, and the real platform credential backend on the final signed
  candidate.

Native voice and Menu Watch diff are not `0.5.0` gates. Health and Menu Watch
management require one bounded production canary or truthful deferral. Passing
the synthetic source-archive matrix does not authorize release: production
registration/Grocery canaries, protected signing, the same matrix against the
signed archives, and independent exact-SHA review remain required.

## Full post-`0.5.0` showcase E2E design

The release test starts from the exact archive intended for publication, not a
Cargo target directory:

1. Verify the archive checksum and attestation, then install into a clean,
   temporary user environment.
2. Launch the installed executable under a real PTY with a clean credential and
   state directory.
3. Register and onboard a purpose-built synthetic account through the same
   visible journey a user receives.
4. Seed only approved synthetic backend fixtures: household members, dietary
   constraints, consented Oura/Apple Health summaries, restaurant/menu
   evidence, a recipe, a versioned grocery list, and a controllable clock.
5. Drive all twelve showcase stages through keyboard input. Capture ANSI screen
   frames, semantic runtime events, HTTP method/path evidence, persisted
   versions, and privacy-safe resource-lifecycle telemetry.
6. Verify both the visible terminal state and the authoritative backend result
   after every stage.
7. Repeat negative variants: cancel, stale list, changed household context,
   consent revocation, low-confidence and mismatched menu sources, unchanged
   menu, microphone denial, truncated audio, transcription timeout, stream
   disconnect, terminal resize, and process signal.
8. Uninstall the executable and prove that no test depended on repository
   source, fixture paths, or a development-only flag.

The deterministic sandbox suite runs on every supported OS and terminal-width
class. Separately approved production canaries prove real deployment wiring,
real menu acquisition, Grocery non-mutation/commit behavior, connected-health
consent, and real microphone capture without storing prompt, transcript,
dietary, or audio content in test evidence.

## Full showcase-complete gate

Longer-term landing-page TUI completion requires all of the following. These
requirements do not broaden the bounded `0.5.0` recovery-release matrix:

- 12/12 showcase stages pass from the installed artifact;
- every negative and cancellation case passes;
- presentation goldens pass at 40, 80, and 120 columns plus `NO_COLOR` and
  reduced-motion modes;
- macOS, Linux, and Windows terminal-restoration and resource-join gates pass;
- the tested archive digest equals the proposed release artifact digest;
- no stage reports placeholder, simulated, or “not connected” success;
- landing-page fixtures are regenerated only from verified semantic documents
  produced by the qualified Rust client/backend contract.

The current Rust TUI foundation does not yet satisfy this gate. Streaming,
editable composition, bounded scrollback, responsive rendering, cancellation,
terminal restoration, in-session Grocery confirmation/cancel/proposal editing,
and the native-audio capture → authenticated transcription → editable composer
path are present in source. Voice checks authorization before opening the
microphone, keeps bounded WAV data in memory, never retries transcription, and
does not submit the transcript until the user presses Enter. Menu Watch
create/list/remove and a read-only TUI subscription panel are present against
the deployed least-privilege contract. Its showcased diff stage remains blocked
on an account-scoped backend diff-read route. Authoritative health context in
ordinary TUI turns and meal-memory proof remain implementation work. Native
voice still requires real-hardware and installed-artifact qualification,
including denial, truncation, timeout, cancellation, and resource-lifecycle
cases. Household targeting, consent-aware dietary context, and Grocery safety
cards now have bounded installed-artifact proof; production-canary proof
remains.

Native CLI CI runs the bounded `0.5.0` matrix from packaged archives on macOS,
Linux, and Windows. It verifies the checksum and one-executable archive policy,
extracts into a clean temporary user environment, drives registration and
onboarding through a real PTY, exits and starts a returning-user process,
exercises household Grocery confirmation and conflict paths, checks
cancellation and uncertain-dispatch behavior, reconstructs visible terminal
screens at 40/80/120 columns, captures privacy-safe ANSI evidence, and requires
isolated credentials and user state to be absent before PASS evidence is
written. Installed captures verify the ordered alternate-screen, bracketed
paste, and cursor restoration sequence. Rust CI's companion
`Internal PTY, signal, restoration vertical` verifies native signals and
canonical-mode restoration; terminal-guard tests verify body-error and panic
restoration. The Windows force-clean seam is available only through the
non-default `qualification-credentials` feature used by the test target; normal
product builds retain fail-closed logout and reconciliation behavior.
Source-archive evidence deliberately reports `release_gate_complete: false`;
it qualifies the bounded matrix but neither the signed candidate nor the
broader twelve-stage showcase contract.
