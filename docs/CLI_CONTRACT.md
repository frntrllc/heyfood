# heyfood native CLI contract

This document defines the process interface for the current Rust public cut.
Its active product commands are `register`, `login`, `chat`, `onboard`, `ask`,
`reply`, `log`, `item`, `grocery`, and `health`. An interactive bare `heyfood`
invocation opens the same native TUI as `heyfood chat`.
Human rendering may improve between compatible releases; machine-facing changes
follow the compatibility policy below.

## Availability boundary

The following commands perform native product work:

| Command | Contract |
|---|---|
| `register` | Starts device authorization, exchanges the approved grant, validates the response contract, and persists the complete native session. |
| `login` | Explicitly signs in again and atomically expands an existing native grant; refresh is never used for scope widening. |
| `chat` | Opens the authenticated interactive Rust TUI. |
| `onboard` | Opens the Rust TUI directly in guided dietary-profile onboarding. |
| `ask` | Runs one hosted-agent turn. |
| `reply` | Runs one hosted-agent turn and requires `--conversation-id`. |
| `log` | Sends meal-log text through the hosted-agent turn endpoint. |
| `item` | Sends a food or menu-item assessment through the hosted-agent turn endpoint. |
| `grocery` | Reads, prepares, exports, and explicitly confirms Grocery v1 operations after capability discovery. |
| `health` | Reads server-held health context and manages the provider-neutral Oura integration. Disconnect requires `--yes`. |

`ask`, `reply`, `log`, and `item` accept positional UTF-8 text, an optional
`--conversation-id`, and optional paired `--latitude`/`--longitude` values. If
positional text is omitted and stdin is not a terminal, the command reads the
prompt from stdin. `reply` fails locally when `--conversation-id` is absent.

Existing credentials missing a command's required scope fail locally with
`authorization_scope_upgrade_required` and direct the user to `heyfood login`.
The old channel and app-session credentials remain authoritative through the
new browser/device grant and session exchange. A durable reconciliation marker
blocks use if the final two-store replacement cannot complete.

Grocery reads include `grocery show` (compatibility alias `list`) and
`grocery exclusions`.
`grocery never --list-id UUID --version N ITEM` prepares an exclusion addition;
`--remove` prepares its removal. Preparation never mutates server state. REST
proposals are confirmed only from a JSON proposal read on stdin. `grocery
export LIST_ID --out FILE` creates an owner-only file exclusively by default;
`--overwrite` opts into same-directory atomic replacement. Targets and direct
parent directories that are symlinks or Windows reparse points are rejected,
temporary files are removed on pre-commit failure, and export contents never
enter diagnostics. Conversational Grocery proposals use the C3 item-list card
in the TUI: `y` accepts, `n` cancels, and Ctrl+C sends a structured cancel. The
confirmation request echoes the server IDs and idempotency key and never
converts natural language into consent. The generic C3 v1 schema describes
per-member screening as top-level `item.safety_flags`, while the frozen Grocery
Phase-A production fixture carries the authoritative Grocery annotation under
`item.safety.{status,member_flags,label_hint}`. The TUI prefers and fully renders
the nested Grocery shape—including intended member, provenance, reasons, and
substitutions—while retaining top-level `safety_flags` as an additive
compatibility input.

Legacy top-level `recommend`, `location`, `search`, `household`, `profile`, and
other hidden topology are unavailable in this cut. Recognized legacy paths fail
closed with `command_not_available`; recognition is not a support or
compatibility promise. In a terminal, bare `heyfood` opens the authenticated
TUI, performs first-run device registration when necessary, and starts guided
onboarding for a missing synchronized profile. Outside a terminal, bare
`heyfood` prints network-free next steps instead of attempting an interactive
session.

## Streams

### Standard output

For one-shot commands in human mode, stdout contains a completed command
result. The TUI owns the interactive terminal until exit. In `--json` mode,
stdout contains exactly one UTF-8 JSON value followed by one newline; JSON mode
never starts the TUI.

JSON stdout never contains:

- ANSI escape sequences;
- banners, spinners, or progress events;
- continuation hints or deprecation warnings;
- human diagnostics; or
- text before or after the JSON value.

### Standard error

stderr contains progress and human diagnostics. Programs must not parse human
stderr as a data format. Registration prints its approval URL and short code to
stderr before waiting for the terminal decision.

Global `--verbose` is reserved for privacy-safe request diagnostics on stderr;
it does not change JSON stdout. Diagnostics must not expose request bodies,
query text, authorization material, keys, profile data, or phone numbers.

## JSON mode

`--json` is the machine-output flag. `--raw` is a deprecated alias that uses the
same writer and sends its deprecation warning to stderr.

Machine-readable failures use this envelope and a nonzero exit status:

```json
{
  "ok": false,
  "error": {
    "type": "login_required",
    "message": "No hello.food account is connected. Run `heyfood register` first.",
    "hint": "Run `heyfood register` and retry."
  }
}
```

`hint` is optional. If a request may have committed on the server but the
client cannot prove the result, the error includes
`"outcome_uncertain": true`. Callers must reconcile state before retrying an
uncertain operation. Consumers must tolerate additive fields.

`register --json` never launches a browser. It waits for one authorization
decision and emits one terminal result. A successful result has this shape:

```json
{
  "schema_version": 1,
  "authenticated": true,
  "account_outcome": null,
  "profile_status": "missing",
  "next_command": "heyfood"
}
```

`profile_status` is `ready`, `missing`, or `unknown`. A contract or service
failure after authentication is never guessed to mean `missing`.
`account_outcome` remains `null` because the native grant does not expose a
trustworthy created/existing distinction; browser/backend identity resolution
is authoritative.

The JSON result from `ask`, `reply`, `log`, and `item` is the validated hosted
agent result document. The human renderer prints its `message` field when one
is present and otherwise prints compact JSON.

## Registration behavior

Registration uses the device-authorization transport. `--device` is accepted
as the explicit spelling, `--no-browser` suppresses best-effort browser launch,
and `--timeout SECONDS` accepts `1..=1800` with a default of 600. JSON mode also
suppresses browser launch regardless of `--no-browser`.

After successful `heyfood register` in an interactive terminal, the client
continues into the TUI and starts guided onboarding when the service reports a
missing profile. `--no-onboard` is the explicit opt-out: it persists the
connected account and exits without opening the TUI. JSON mode and redirected
input or output also return the registration document without attempting an
interactive handoff. Global `--no-input` likewise suppresses the questionnaire
handoff while preserving the hosted device-authorization flow.

Native account state is written only after OAuth approval, application-session
exchange, and response validation succeed. A complete authorization grant and
rotating session are persisted together. Credentials are refreshed before an
agent turn when necessary; a server-rotated refresh grant is durably accepted
before the client proceeds.

## Prompt and coordinate validation

Prompt text is required. Redirected stdin must be UTF-8 and is capped at 1 MiB.
Half-specified coordinate pairs are rejected by argument parsing. Latitude and
longitude are forwarded only when both values are supplied.

`--no-input` guarantees that the client will not prompt. The active one-shot
commands do not require an interactive prompt: callers provide positional text
or redirected stdin. Registration authorization itself is completed on the
hosted approval page.

## Exit status

| Code | Meaning |
|---:|---|
| `0` | The requested operation or interactive session completed successfully, or noninteractive bare `heyfood` printed its informational next steps. |
| `1` | Authentication, authorization, service, cancellation, unavailable-command, uncertain-outcome, or other runtime failure. |
| `2` | Command-line parsing or argument validation failed before execution. |

## Compatibility and deprecation

- Additive JSON fields are compatible changes.
- Removing or renaming an active command, option, JSON field, error type, or
  exit-status meaning requires release notes and migration guidance.
- `--raw` remains a deprecated alias through the first public native minor
  release and may be removed only in a versioned breaking release.
- Human spacing, ANSI styling, and prose are not stable machine interfaces.
- Hidden legacy topology is explicitly outside the public native contract.
- Frozen contract JSON under `fixtures/contracts/` and `schemas/` is checked
  out with LF line endings on every platform. Approved hashes and semantic
  bytes must not be changed to accommodate platform line-ending conversion.
