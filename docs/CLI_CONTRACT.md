# heyfood native CLI contract

This document defines the process interface for the current Rust public cut.
The supported `v0.9.0` product commands are `agent`, `register`, `login`,
`logout`, `chat`, `onboard`, `ask`, `reply`, `log`, `item`, `diet`, `grocery`,
and `watch`. An
interactive bare `heyfood`
invocation opens the same native TUI as `heyfood chat`.
Human rendering may improve between compatible releases; machine-facing changes
follow the compatibility policy below.

The supported `v0.9.0` contract includes local native household roster
management and complete declared-profile onboarding for members. Persistent Me/member/Everyone scope selection
is stored in an account-bound encrypted repository.

## Availability boundary

The following commands perform native product work:

| Command | Contract |
|---|---|
| `agent` | Describes the exact installed executable, prints its embedded integration/safety guides and public schemas, and runs bounded local diagnostics without credentials or network access. |
| `register` | Explicitly starts create-account device authorization, exchanges the approved grant, validates the response contract, and persists the complete native session. |
| `login` | Connects an existing account on a fresh machine; on a connected machine, explicitly signs in again and atomically replaces the native grant with the canonical supported scope set. Refresh is never used to change authority. |
| `logout` | Resolves and revokes the current channel link, revokes the current device, revokes the app session last, and then clears the exact account-bound local credential pair plus the encrypted household key and vault artifacts. Remote failures never prevent resumable local teardown. |
| `chat` | Opens the authenticated interactive Rust TUI, including local native-household management when the account-bound encrypted repository is enabled. |
| `onboard` | Opens the Rust TUI directly in guided owner dietary-profile onboarding. Member onboarding is available only inside the attached TUI. |
| `ask` | Runs one hosted-agent turn. |
| `reply` | Runs one hosted-agent turn and requires `--conversation-id`. |
| `log` | Sends meal-log text through the hosted-agent turn endpoint. |
| `item` | Sends a food or menu-item assessment through the hosted-agent turn endpoint. |
| `diet` | Read-only access to the hosted Diet v1 catalog and authored guide detail after exact capability and scope discovery. |
| `grocery` | Reads, prepares, exports, and explicitly confirms Grocery v1 operations after capability discovery. |
| `watch` | Creates, lists, and removes recurring Menu Watch subscriptions. |

`agent`, `agent describe`, `agent guide`, `agent schema`, and `agent doctor`
dispatch before credential discovery or network initialization. Their JSON is
ANSI-free and deterministic for an exact build. `agent schema --list` is the
authoritative allowlist: internal approval/commit schemas are deliberately not
exposed. `agent doctor` reports only bounded build/contract facts and never
prints a user-specific executable or configuration path.

Bare `agent`, `agent describe`, and `agent doctor` return schema v4. Explicit
`--schema-version 1`, `--schema-version 2`, and `--schema-version 3` retain the
frozen compatibility views. Unsupported schema versions fail during
argument parsing before credentials or network access.

`agent compatibility` is a version-invariant offline bootstrap. `household
show` and `household member` are active machine-only, disclosure-gated local
reads requiring `--json --no-input`. They call the same application controller
as the two household MCP tools, perform no hosted dispatch or mutation, reject
display-name resolution, and do not expose a self profile.

Health integrations are deferred from the supported `v0.9.0` contract.
`health` is hidden from root help and generated shell completion, `/health` is
absent from the TUI command registry, and new grants do not request
`health:read` or `integrations:manage`. The retained top-level spelling returns
`capability_deferred` before credential access or network dispatch. Existing
provider-neutral types, transports, and frozen fixtures are not a support claim
and require no additional implementation or production canary for this release.

The supported Diet surface uses `heyfood diet`, `heyfood diet list`,
`heyfood diet show DIET_ID`, and the equivalent short form `heyfood diet
DIET_ID`. It dispatches only after capability discovery reports the exact value
`application_capabilities.diet == "v1"` and the account grant contains
`knowledge:read`; missing or unknown capability values fail closed. Runtime
diet IDs are case-sensitive, are URL-segment encoded, and are sent exactly as
returned by the service. The service currently exposes 22 authored guides,
which are separate from the 26 diet options accepted by profile onboarding.

A guide's `strong`, `moderate`, or `limited` evidence level is an advisory
description of its sources, not a safety or clinical guarantee. The authored
`render_order` controls guide presentation, and dietary safety stays visible.
`diet_not_covered` is a successful bounded result; the client does not invent
guidance for an uncovered diet. Optional item-level `diet_alignment` is
rendered only as a subordinate explanation and cannot alter safety status,
badge, color, ordering, filtering, or mutation state. Profile diet set/clear is
not exposed in this release because its write contract remains pending in
[hellofood #261](https://github.com/frntrllc/hellofood/issues/261).

Native household management is a human-attached-TUI surface. In
`NativeEnabled` mode, `/household`, `/household add`,
`/onboard --for <exact member ID or exact display name>`, `/for me`,
`/for <exact member ID or exact display name>`, and `/for everyone` operate on
the live account-bound encrypted repository. Adding a member atomically saves
the roster entry, the complete version-1 declared dietary profile, and the
selected member scope. Existing active members with an incomplete or local
profile can complete the same questionnaire. Duplicate names require an
explicit stable-ID-bound choice; archived, unknown, incomplete, conflicted, or
otherwise ineligible targets fail closed.

Member profiles and member/Everyone scope remain encrypted and local to this
device. They create no member profile-sync consent, remote member profile, or
non-owner outbox entry. For an ordinary hosted turn, the client acquires an
exact revision-bound read lease and sends only the selected declared-profile
projection as transient request context. Member scope evaluates that member;
Everyone scope evaluates the owner and every eligible active member. The
client does not set the top-level server-synced `household_scope` for this
request-first path, so an unsynced local member is never misresolved as a
remote member ID. `/for me` returns to the owner context.

“Dietary graph” support in this slice means only the complete declared local
profile used for these turns. Learned preferences, history, goals,
health/fitness data, cross-device roster sync, remote member profile sync, and
remote member erasure remain deferred.

Legacy compatibility and rollback/repair modes remain read-only and never
advertise an enabled add action. Household mutations are not exposed through
one-shot JSON, agent manifests, MCP, redirected stdin, or process arguments.
Edit, archive, restore, and permanent member erasure are not part of this
slice.

`ask`, `reply`, `log`, and `item` accept positional UTF-8 text, an optional
`--conversation-id`, and optional paired `--latitude`/`--longitude` values. If
positional text is omitted and stdin is not a terminal, the command reads the
prompt from stdin. `reply` fails locally when `--conversation-id` is absent.

Direct one-shot `log`, Grocery proposal preparation/confirmation, and Menu
Watch creation/removal are human-terminal-only commands. Before a network
request or mutation, the executable opens the controlling terminal
independently of stdin/stdout, renders terminal-safe review details, and
requires the command-specific phrase `LOG`, `PREPARE`, `ACCEPT`, `CANCEL`,
`CREATE`, or `REMOVE`. Except for the local-only native Household target
qualification described below, review also precedes credential access. Missing
terminal, EOF, an I/O error, or any other response fails closed. Redirected
stdin may carry meal or Grocery proposal data, and JSON stdout remains exactly
one value; neither channel supplies semantic authority. The direct CLI routes
are not agent-safe fallbacks. `--no-input` rejects these routes before opening a
terminal.
The review is the submitted intent, not a summary: meal logging includes the
meal, type, and privacy-safe resolved canonical Household label. Stable member
IDs and reversible identity tokens remain hidden from human output. For an
`Everyone` target, the review also states that the single meal is filed to the
owner using the owner's canonical label. In native Household mode, an omitted
`--for` uses the strictly validated active scope from the exact retained native
Household revision; execution consumes that frozen identity after `LOG` and
does not resolve the selector again. Menu Watch creation includes
every schedule/source/notification field and `--confirm-menu-url`; Grocery
confirmation includes confirmation ID, operation, expiry, the complete
structured preview, and every frozen precondition. Confirmation tokens and
idempotency authority remain hidden.

Native Household target qualification may perform a local-only read of the
account-bound authorization and key material needed to unlock the encrypted
vault and retain the exact revision under a read lock. It performs no provider
or network request and no mutation before `LOG`; the credentials are dropped
before the review prompt and reloaded only after approval. The legacy
compatibility preview remains credential-free before review.

Before `LOG`, the executable may stat the known mixed Python configuration
locators but never opens, hashes, or parses their bytes. If a mixed source is
visible without a complete credential-elided native snapshot, or the snapshot
reports that Python keyring data was not read, Household state is protected:
only an explicit self selector may be reviewed. Omitted, member, and Everyone
targets fail locally until authenticated migration can reconcile that source.
Malformed or duplicate roster identity and a missing, unknown, aliased, or
archived active scope fail closed rather than being dropped, rewritten, or
changed to self.

Existing credentials missing a command's required scope fail locally with
`authorization_scope_upgrade_required` and direct the user to `heyfood login`.
The old channel and app-session credentials remain authoritative through the
new browser/device grant and session exchange. A durable reconciliation marker
blocks use if the final two-store replacement cannot complete. The replacement
may add Grocery or Menu Watch authority while removing scopes for deferred
capabilities such as Health.

`logout` is an explicit, idempotent authorization and account-local household
teardown. It performs no automatic mutation retries. A channel-link lookup
uses channel authority;
link, device, and session revocation use the current app session, with session
revocation last. HTTP 404 is success-equivalent for these identity-bound
deletes. Local account-bound credentials are cleared even if remote cleanup
fails or is canceled. The two authorization stores are committed under a
durable `account_logout_pending` marker so interruption can be resumed without
deleting a concurrently replaced account. The native teardown journal also
removes the exact account household key and encrypted vault artifacts while
preserving unrelated non-credential data. Human success is `Logged out.`; a
partial remote or local outcome is stated explicitly. JSON includes
`remote_complete`, per-step attempted/ok/uncertainty fields, and
`local_credentials_cleared`, but never tokens, household content, or raw server
errors.

Grocery reads include `grocery show` (compatibility alias `list`) and
`grocery exclusions`.
`grocery never --list-id UUID --version N ITEM` prepares an exclusion addition;
`--remove` prepares its removal. Preparation never mutates server state. REST
proposals containing commit authority are emitted only after the controlling
terminal receives `PREPARE`. Confirmation reads the complete JSON proposal on
stdin, renders that exact proposal on the controlling terminal, and requires
`ACCEPT` or `CANCEL` matching `--decision` before dispatch. `grocery export
LIST_ID --out FILE` creates an owner-only file exclusively by default;
`--overwrite` opts into same-directory atomic replacement. Targets and direct
parent directories that are symlinks or Windows reparse points are rejected,
temporary files are removed on pre-commit failure, and export contents never
enter diagnostics. Human output requires `--out`; without it the command
refuses before writing private Household annotations to the terminal. In
machine mode, text and Markdown exports are wrapped as one JSON object carrying
`format` and exact `content`, preserving the global one-value invariant.
Windows installs the protected single-owner DACL in the
creation call, publishes by the still-open file handle without delete sharing,
and verifies the final ACL and non-reparse identity before success.
Conversational Grocery proposals use the C3 item-list card in the TUI: `y`
accepts, `n` cancels, and Ctrl+C sends a structured cancel. The confirmation
request echoes the server IDs and idempotency key and never converts natural
language into consent. The generic C3 v1 schema describes per-member screening
as top-level `item.safety_flags`, while the frozen Grocery Phase-A production
fixture carries the authoritative Grocery annotation under
`item.safety.{status,member_flags,label_hint}`. The TUI prefers and fully renders
the nested Grocery shape—including intended member, provenance, reasons, and
substitutions—while retaining top-level `safety_flags` as an additive
compatibility input. Production `sources[]` provenance is rendered as bounded,
terminal-safe source type, reference, and detail lines; legacy singleton
`provenance` remains a fallback.

In artifacts built with `native-audio`, `/voice`, Ctrl+Space, and F8 start or
stop native TUI recording. The client checks the `audio:transcribe` grant before
opening the microphone, captures a mono 16-bit WAV in bounded process memory,
uploads it once with channel authority to `/v1/audio/transcriptions`, and never
retries that POST automatically. Audio is not written to disk. A validated
transcript is placed in the ordinary composer for review, editing, rerecord, or
discard; it is not sent to the agent until the user presses Enter. Esc, Ctrl+C,
exit, capture overflow, truncation, and contract failure discard the recording
without treating transcription as agent or mutation consent. Artifacts without
native audio report that limitation truthfully before capture.

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

Human-only mutation review is written directly to the controlling terminal,
not stdout or stderr. It therefore does not corrupt redirected JSON or proposal
data streams.

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
    "message": "No hello.food account is connected. Run `heyfood login` first.",
    "hint": "Run `heyfood login` to connect an account, then retry."
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
