# heyfood command grammar

This document describes the current Rust command surface. It does not document
legacy Python behavior or hidden compatibility topology.

## Active top-level commands

```text
agent         inspect the exact installed agent integration contract offline
mcp           serve the bounded local read/discovery MCP protocol
register      create and connect a hello.food account
login         connect an existing account or replace this machine's authorization
logout        revoke this device's hosted authority and clear local credentials
ask           ask the hosted agent a one-shot question
reply         continue an explicit conversation id
log           log a meal through the hosted agent
item          assess a food or menu item
grocery       read, prepare, export, and confirm Grocery operations
watch         create, list, and remove recurring Menu Watch subscriptions
completion    print shell completion syntax
```

## Offline agent discovery

```bash
heyfood agent describe
heyfood agent guide --format markdown
heyfood agent guide --format markdown --safety
heyfood agent schema --list
heyfood agent schema manifest
heyfood agent doctor
heyfood agent setup --target codex|claude|all --scope user|project \
  [--project-root /absolute/path] [--dry-run|--apply] \
  [--plan-sha256 REVIEWED_SHA256] [--replace]
heyfood agent uninstall --target codex|claude|all --scope user|project \
  [--project-root /absolute/path] [--dry-run|--apply] \
  [--plan-sha256 REVIEWED_SHA256]
```

These commands do not read credentials, contact hello.food, mutate product
state, or start the TUI. Schema lookup accepts only a public name or exact
identifier from `--list`; unknown names return a typed runtime error.

`agent setup` and `agent uninstall` are separate opt-in user configuration
operations. They default to dry-run, require `--apply` to change state,
preserve modified or unreceipted files, and never edit general `AGENTS.md`,
or `CLAUDE.md`. Apply requires the exact digest of the rechecked dry-run plan.
MCP changes use the qualified host's own `mcp add/remove` command and are
receipt-bound. They are not agent-safe command fallbacks.

## Local MCP

```bash
heyfood mcp serve
```

This long-lived stdio JSON-RPC process is the sole exception to the one-value
CLI stdout contract. It exposes exactly six typed read/discovery tools and no
mutation, generic command, shell, file, raw API, credential, or TUI-control
surface. It uses only account-bound native credentials and the compiled
production service origin. Human/one-shot modifiers and every inherited
`HEYFOOD_*` variable fail before credential access or protocol startup.

## Text input

`ask`, `reply`, `log`, and `item` accept positional UTF-8 text:

```bash
heyfood ask "What can I eat?"
heyfood item "pad thai at Pismo's"
heyfood log "I had the tofu bowl for lunch"
heyfood reply --conversation-id CONVERSATION_ID "The second option"
```

When positional text is omitted and stdin is redirected, the command reads at
most 1 MiB of UTF-8 input:

```bash
printf '%s\n' "What can I eat?" | heyfood ask --json
```

An optional location requires a complete coordinate pair. Half-specified pairs
fail during argument parsing:

```bash
heyfood ask --lat 35.28 --lng -120.66 "What can I order nearby?"
```

## Human-only mutation authority

These direct CLI routes require a fresh decision on an attached controlling
terminal before credential access or network dispatch:

| Command family | Required phrase |
|---|---|
| `log` | `LOG` |
| `grocery add/remove/state/never` | `PREPARE` |
| `grocery confirm --decision accept` | `ACCEPT` |
| `grocery confirm --decision cancel` | `CANCEL` |
| `watch add` | `CREATE` |
| `watch remove` | `REMOVE` |

The controlling terminal is opened independently from stdin and stdout.
Arguments and redirected stdin carry data only; they never count as consent.
The `log` review shows the meal, meal type, resolved canonical Household label,
and reversible identity token before `LOG` is accepted. Member identities use
`member-id-utf8-hex=<lowercase UTF-8 hex>`; self and Everyone use
`scope=_self` and `scope=__everyone__`. An omitted selector resolves the
strictly valid saved active scope exactly once from the credential-elided
native snapshot, and the reviewed stable identity is the identity dispatched.
If only an uninspected mixed legacy source is visible, or the native snapshot
reports skipped Python keyring data, only explicit self can reach review;
omitted, member, and Everyone targets fail closed before credential access.
Consequently these commands fail with `human_terminal_required` in unattended
processes even when an automation host allocates ordinary pipes. Agents must
not drive the prompt through a PTY or use these human-only commands as a
fallback. `--no-input` rejects them with `human_input_disabled`.

## Registration, login, and logout

```bash
heyfood register
heyfood register --device --no-browser
heyfood register --device --no-browser --json --timeout 600
heyfood login
heyfood logout
```

`--json` suppresses browser launch and interactive prompts. Device authorization
still requires one human approval on `auth.hello.food`. Refresh cannot change
authority. On a fresh machine, bare `heyfood` starts the account-neutral browser
flow where the user chooses sign-in or account creation, while `heyfood login`
connects an existing account. On a connected machine, `login` atomically
replaces the grant with the canonical supported scope set.

`heyfood logout` resolves the current channel link, revokes link and device
authority before revoking the authenticating app session, and always clears
the exact local account-bound credential pair. It is idempotent and reports
uncertain remote outcomes without exposing credentials.

## Grocery

```bash
heyfood grocery list
heyfood grocery add --list-id UUID --version VERSION "red lentils" "onion"
heyfood grocery remove --list-id UUID --version VERSION ITEM_OR_INDEX
heyfood grocery state --list-id UUID --version VERSION ITEM purchased
heyfood grocery export UUID --format markdown [--out FILE [--overwrite]]
heyfood grocery confirm --decision accept --proposal-stdin < proposal.json
```

Mutation commands prepare a proposal and do not commit it. The human must type
`PREPARE` before the capability-bearing proposal is emitted. Confirmation
reads the proposal from stdin so authorization material does not enter shell
history or process arguments, renders the exact proposal on the controlling
terminal, including its confirmation ID, operation, expiry, complete
structured preview, and every frozen precondition, and requires an
`ACCEPT`/`CANCEL` decision matching `--decision`. The token and idempotency
authority are never rendered.

## Deferred Health integrations

Health integrations are not part of the supported `v0.6.3` command surface.
They are absent from root help, shell completion, and the TUI command registry.
The retained `health` spelling fails locally with `capability_deferred` before
credential access or network dispatch. Oura and Apple Health integration work
remains post-`v0.6.3`; no Health implementation or canary is required for this
release.

## Menu Watch

```bash
heyfood watch list
heyfood watch add RESTAURANT_UUID --weekday thursday --hour 9 --notify
heyfood watch add RESTAURANT_UUID --weekday thursday --hour 9 \
  --menu-url https://restaurant.example/menu --confirm-menu-url \
  --tz America/Chicago
heyfood watch remove WATCH_UUID
```

Watch creation, listing, and removal use the deployed `menu:watch` scope.
Creation freezes the restaurant-local cadence, resolved timezone, notification
preference, and activation state. The TUI renders the latest account-owned
change summary with source, freshness, and provenance. Item-level added,
removed, modified, and price-change detail remains follow-on work and is not
claimed by the current client. Direct creation requires `CREATE`; direct
removal requires `REMOVE` on the controlling terminal. The creation review
includes `--confirm-menu-url` because that flag is the human's explicit menu
identity assertion.

## Global process controls

```text
--json       one ANSI-free JSON value on stdout
--no-color   disable ANSI styling
--no-banner  disable decorative branding
--verbose    privacy-safe diagnostics on stderr
--no-input   never prompt for missing local input
```

`--raw` is a deprecated alias for `--json`.

## Interactive TUI

Bare `heyfood` launches the authenticated TUI. On a clean machine it offers
sign-in or account creation, completes device authorization, and continues into
the same TUI process.

```text
/grocery             open the capability-gated active Grocery list
/watch               open recurring Menu Watch subscriptions
/profile             read consent and synchronized dietary profile state
/household           show account-bound local household context
/for MEMBER|everyone change household scope and reset conversation continuity
/location            show account-bound local location context
/status              check service, profile, optional scopes, and voice readiness
/voice               start/stop native capture in a qualified native-audio artifact
/new                 reset conversation continuity
/clear               clear visible scrollback
/help                show the active slash-command registry
/exit                leave the TUI
```

The panels are read-only and cancellable. `/voice`, Ctrl+Space, and F8 use the
same bounded capture/transcription/review state machine when the artifact
contains native audio support; unavailable artifacts and insufficient scopes
fail before microphone access. Dietary onboarding, interactive Grocery
confirmation, and the bounded installed-artifact core matrix remain active
release work. Item-level Menu Watch diff detail, real-hardware voice
qualification, full parity, and the complete twelve-stage showcase are
post-`v0.6.3` conformance work, not release gates.

## Unavailable compatibility topology

Health integrations, profile editing, restaurant search, recommendation, menu,
recipe, household management, voice device configuration, diagnostics, and
account management are not active Rust commands. Some names remain hidden
for migration topology only. Health returns `capability_deferred`; unfinished
compatibility topology returns `command_not_available`.
