# heyfood conversational UX correction plan

**Status:** Complete — released and publicly verified as `0.3.2`
**Release:** `0.3.2`
**Primary user:** a person using `heyfood`, `heyfood ask`, `heyfood chat`, or voice input in an interactive terminal

## Closure evidence

- Merged by PR #15 at commit
  `bc0bd5387c53db310c91d635c978e1d957042112` and tagged `v0.3.2`.
- All 601 tests and the macOS/Linux, Python 3.11–3.13, installer, pipx,
  voice-wheel, and distribution CI gates passed.
- PyPI trusted publishing produced the wheel and sdist with provenance
  attestations, and the matching GitHub release completed successfully.
- Fresh isolated installs from both public PyPI and
  `https://hey.food/install.sh` resolved to `heyfood 0.3.2`.

## Objective

Make the installed CLI look and feel like the animated sessions on
`https://hey.food`: calm, compact, colorful, readable, and conversational.
The full hey.food ASCII artwork is a startup identity treatment, not response
chrome.

## Confirmed defects

1. `banner.controller.loading()` is called from ordinary command execution.
   Although the controller is once-per-process, each one-shot CLI command is a
   new process. The result is effectively a full 44-column banner before every
   `ask`, `reply`, voice submission, login, registration, and some menu waits.
2. Interactive chat uses a neutral `you` prompt instead of the landing-page
   accent treatment and leaves too little vertical separation between turns.
3. Unstructured agent text uses Rich defaults rather than the hey.food bright
   foreground, while structured results use the shared semantic palette. The
   same session therefore looks visually inconsistent.
4. A service failure delivered as a normal agent result can be followed by the
   success-oriented continuation hint. Failure results must never look like a
   successful conversational handoff.
5. Voice review is useful but visually heavy: after transcript acceptance the
   banner displaces the actual result, making the flow feel like a new program
   launch instead of one continuous turn.

## Design decisions

### D1 — ASCII art has one lifecycle

- The full banner may render only for the interactive bare `heyfood` startup
  journey.
- `ask`, `reply`, `chat` turns, `log --voice`, `login`, `register`, and menu
  polling never print the full banner.
- The canonical `banner.txt`, palette, Unicode/width checks, `NO_COLOR`,
  `HEYFOOD_NO_BANNER`, and `--no-banner` support remain intact for the bare
  startup surface and a future contained startup animation.
- JSON, piped, CI, dumb-terminal, and non-TTY behavior remains banner-free.
- A future animation must replace the static bare-startup moment in place; it
  may not add banner frames to command responses or leave cursor-control state.

### D2 — Landing-page terminal styling is the runtime contract

Use the existing shared palette from `theme.py` and block model from
`presentation.py`:

- prompt / identity: `accent` (`#9bc53d`);
- primary response text: `bright` (`#edeae0`);
- progress / secondary context: `muted` (`#74808d`);
- informational, caution, and failure states: `info`, `warning`, and `danger`;
- no decorative boxes around ordinary assistant prose;
- one intentional blank line between chat turns, not between every output
  line;
- structured tables/cards retain responsive narrow-terminal behavior.

### D3 — Voice is one continuous conversational turn

- Keep the concise recording and transcription status on stderr.
- Keep transcript review editable and clearly separated from the result.
- Accepting a transcript proceeds directly into muted thinking/progress and
  the formatted response; it never triggers ASCII art.
- Cancellation, rerecording, typed fallback, non-microphone handling, and JSON
  isolation remain unchanged.

### D4 — Failure states do not masquerade as success

- Recognized error-result contracts render with the danger tone and one
  actionable retry/resume hint.
- Suppress `Continue with: ...` for error results and for empty results.
- Do not classify failures by matching friendly English prose. Use explicit
  response fields/events; if Production currently lacks an explicit marker,
  capture that as a backend contract defect instead of adding brittle text
  matching to the CLI.
- Preserve safe error redaction and stable JSON error envelopes.

## Implementation phases

### Phase 1 — Remove response banners

1. Remove ordinary-command `loading()` calls from agent, auth, and restaurant
   execution paths.
2. Keep `welcome()` solely in the interactive bare-command intro.
3. Replace banner tests that assert command-time rendering with negative
   lifecycle tests for ask, voice-to-agent, auth, chat turns, and menu polling.
4. Update README/help prose to state the banner appears only on bare interactive
   startup.

**Exit gate:** simulated-TTY captures contain no banner for `ask`, `reply`,
accepted voice input, `chat` responses, `login`, `register`, or menu polling;
bare interactive `heyfood` still renders it once.

### Phase 2 — Unify conversational presentation

1. Add small render helpers for the chat header, user prompt, ordinary agent
   Markdown, turn spacing, and explicit agent failure results.
2. Route `chat` prompts and initial messages through the accent prompt helper.
3. Render ordinary prose using the bright theme while preserving Markdown
   semantics and literal treatment of service-provided Rich markup.
4. Keep all structured agent responses on `presentation.py` blocks so the
   installed CLI and landing demos share tones and layout semantics.
5. Add terminal-width tests at narrow and standard widths plus `NO_COLOR` and
   non-TTY assertions.

**Exit gate:** golden/simulated-terminal captures match the landing-page
hierarchy: accent prompt, muted progress, bright response, semantic status
colors, and visible turn spacing without repeated branding.

### Phase 3 — Failure and voice polish

1. Inspect the real failed-agent payload shown by Production and confirm
   whether it exposes an explicit failure marker.
2. Render explicit failures as failures and suppress success continuation
   hints.
3. Verify the accepted native-voice path enters the same `_ask_agent` renderer
   as typed input with no extra branding.
4. Preserve every native voice safety, timeout, device, scope, and JSON test.

**Exit gate:** the exact voice scenario from the reported transcript produces
one continuous, banner-free turn; an explicit service failure is compact,
colored, actionable, and never followed by a success hint.

### Phase 4 — Release verification

1. Run the full test suite and installed-wheel smoke test.
2. Build wheel/sdist and verify packaged banner resources remain byte-for-byte
   canonical even though their lifecycle is narrower.
3. Exercise a clean installed CLI in standard TTY, narrow TTY, `NO_COLOR`,
   non-TTY, and `--json` modes.
4. Cut a patch release and verify the PyPI artifact, hosted installer, and
   `heyfood --version` resolve to it.
5. Regenerate or verify landing-page demo fixtures from the same presentation
   block data before publishing screenshots/animations.

## Acceptance criteria

- The giant ASCII banner never appears before or after an ordinary response.
- Bare interactive `heyfood` may show the banner exactly once.
- `heyfood chat` has accent prompts, bright responses, muted progress, semantic
  status colors, and consistent inter-turn spacing.
- Accepted voice input flows directly from transcript review to the same
  response presentation as typed input.
- Explicit failed-agent results have no success continuation hint.
- `--json` remains exactly one JSON value on stdout with no ANSI, banner,
  spinner, progress, or human hint on stdout.
- Narrow terminals remain readable and never require a 44-column banner.
- Canonical banner assets remain packaged and tested for the bare-startup use
  case.

## Non-goals

- Redesigning backend dietary guidance or changing safety vocabulary.
- Adding brittle prose-based error detection.
- Replacing the stable JSON schema.
- Adding terminal animation to every command.
- Recreating the website in the CLI; the shared hierarchy and palette are the
  contract, not browser-only chrome.
