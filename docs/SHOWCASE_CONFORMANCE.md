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

## Installed-artifact E2E design

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

## Release gate

TUI completion requires all of the following:

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
terminal restoration, and in-session Grocery confirmation/cancel/item-name
correction are present in source. Menu Watch, authoritative health context in
ordinary TUI turns, meal-memory proof, and native voice capture/review remain
implementation work. Household targeting, consent-aware dietary context, and
Grocery safety cards still require installed-artifact and production-canary
proof.
